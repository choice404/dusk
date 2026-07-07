/* The 0.4.x event loop and one shot futures. The loop is a process singleton
   like the pool, confined to the thread that initialized it, holding a timer
   min heap keyed by deadline then registration order on CLOCK_MONOTONIC. A
   future is a completion slot in the generational heap, so consuming it
   retires the record and a second await faults deterministically, the double
   join machinery. Completion is legal from any thread and wakes the loop;
   everything else asserts the owner thread and faults by name off it. An
   await that provably cannot finish, no timer pending, no spawned thread
   alive, no pool task in flight, no armed readiness watch, aborts as a
   deadlock instead of hanging. */
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

extern void *cool_gen_alloc(int64_t size);
extern int64_t cool_gen_retire_checked(void *p, int64_t gen, void *out, int64_t n);
extern int64_t cool_live_threads(void);
extern int64_t cool_pool_inflight(void);
extern int64_t cool_reactor_armed(void);

/* Task frames and closure environment blocks are malloc'd outside the
   generational registry, so the collector cannot reach a collected block they
   hold unless they are registered as root regions. They are added when built
   and removed before their backing memory is freed. */
extern void cool_gc_add_region(void *base, int64_t len);
extern void cool_gc_del_region(void *base);

static void cool_async_fatal(const char *msg) {
    fflush(stdout);
    fputs(msg, stderr);
    abort();
}

/* A future's record, the payload of a generational block. state moves 0 to 1
   exactly once, err is the error word the completer supplied with NULL as no
   error, waiter is the task parked on this future (NULL when none or when a
   pumped await holds it), task is the owning task backref a task future uses to
   reach its frame, and the element bytes follow the header fields. */
typedef struct {
    int64_t state;
    void *err;
    int64_t elem_size;
    void *waiter;
    void *task;
    char elem[];
} cool_future;

/* A task's header, exactly 48 bytes, with the dusk visible frame immediately
   after it. poll is the state machine entry, fut and fut_gen name the task's
   own future record, next threads the ready FIFO, queued is one while the task
   sits in the FIFO so a task can be enqueued at most once, and envs heads a
   singly linked list of closure environment blocks freed at completion. */
typedef struct cool_task {
    void (*poll)(void *frame);
    void *fut;
    int64_t fut_gen;
    struct cool_task *next;
    int64_t queued;
    void *envs;
} cool_task;

/* One closure environment arena block. The payload address is what the lambda
   captures; the blocks chain through next so cool_task_return frees them all. */
typedef struct cool_env_block {
    struct cool_env_block *next;
    char payload[];
} cool_env_block;

typedef struct {
    int64_t deadline;
    int64_t seq;
    void *fut;
} cool_ltimer;

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t wake;
    pthread_t owner;
    int running;
    cool_ltimer *timers;
    int64_t tlen;
    int64_t tcap;
    int64_t seq;
    cool_task *ready_head;
    cool_task *ready_tail;
    int cranking;
} cool_loop;

static cool_loop cool_the_loop = {
    PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER,
    0, 0, NULL, 0, 0, 0, NULL, NULL, 0,
};

/* Appends a task to the ready FIFO, called with the loop lock held. The queued
   flag makes a second enqueue of the same task a no-op, so a task that is both
   the completer's waiter and already scheduled cannot appear twice. Every
   enqueue broadcasts the wake condvar: a parked crank or a pumped waiter must
   re-check its state the moment runnable work appears. */
static void cool_task_enqueue(cool_loop *l, cool_task *t) {
    if (t->queued) {
        return;
    }
    t->queued = 1;
    t->next = NULL;
    if (l->ready_tail) {
        l->ready_tail->next = t;
    } else {
        l->ready_head = t;
    }
    l->ready_tail = t;
    pthread_cond_broadcast(&l->wake);
}

static int64_t cool_mono_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000 + ts.tv_nsec / 1000000L;
}

/* The owner assertion every loop touch except complete runs. Both failures
   name themselves, so a program that forgot loop_init and a thread that
   reached across the confinement line read differently. */
static void cool_loop_assert_owner(void) {
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    int running = l->running;
    pthread_t owner = l->owner;
    pthread_mutex_unlock(&l->mu);
    if (!running) {
        cool_async_fatal("fatal: the event loop is not running\n");
    }
    if (!pthread_equal(pthread_self(), owner)) {
        cool_async_fatal("fatal: the event loop was touched off its thread\n");
    }
}

/* The generation check against the record header, named for the future
   family. A retired record means the future was already consumed or freed. */
static void cool_future_check(void *f, int64_t gen) {
    if (!f) {
        cool_async_fatal("fatal: use of a dead future\n");
    }
    int64_t *g = (int64_t *)((char *)f - 8);
    if (gen != 0 && __atomic_load_n(g, __ATOMIC_SEQ_CST) != gen) {
        cool_async_fatal("fatal: use of a dead future\n");
    }
}

/* Timer heap plumbing, a binary min heap ordered by deadline then seq, so
   equal deadlines fire in registration order and goldens stay exact. */
static int cool_ltimer_lt(const cool_ltimer *a, const cool_ltimer *b) {
    if (a->deadline != b->deadline) {
        return a->deadline < b->deadline;
    }
    return a->seq < b->seq;
}

static void cool_timers_sift_up(cool_loop *l, int64_t i) {
    while (i > 0) {
        int64_t p = (i - 1) / 2;
        if (!cool_ltimer_lt(&l->timers[i], &l->timers[p])) {
            return;
        }
        cool_ltimer tmp = l->timers[i];
        l->timers[i] = l->timers[p];
        l->timers[p] = tmp;
        i = p;
    }
}

static void cool_timers_sift_down(cool_loop *l, int64_t i) {
    for (;;) {
        int64_t s = i;
        int64_t a = 2 * i + 1;
        int64_t b = 2 * i + 2;
        if (a < l->tlen && cool_ltimer_lt(&l->timers[a], &l->timers[s])) {
            s = a;
        }
        if (b < l->tlen && cool_ltimer_lt(&l->timers[b], &l->timers[s])) {
            s = b;
        }
        if (s == i) {
            return;
        }
        cool_ltimer tmp = l->timers[i];
        l->timers[i] = l->timers[s];
        l->timers[s] = tmp;
        i = s;
    }
}

static void cool_timers_push(cool_loop *l, cool_ltimer t) {
    if (l->tlen == l->tcap) {
        int64_t cap = l->tcap ? l->tcap * 2 : 16;
        cool_ltimer *grown = realloc(l->timers, (size_t)cap * sizeof(cool_ltimer));
        if (!grown) {
            cool_async_fatal("fatal: out of memory\n");
        }
        l->timers = grown;
        l->tcap = cap;
    }
    l->timers[l->tlen] = t;
    l->tlen++;
    cool_timers_sift_up(l, l->tlen - 1);
}

/* Drops every heap entry naming this future, used when a future is consumed
   or released while its timer has not fired, so a later fire cannot touch a
   retired record. */
static void cool_timers_purge(cool_loop *l, void *fut) {
    int64_t i = 0;
    while (i < l->tlen) {
        if (l->timers[i].fut == fut) {
            l->tlen--;
            l->timers[i] = l->timers[l->tlen];
        } else {
            i++;
        }
    }
    for (int64_t j = l->tlen / 2 - 1; j >= 0; j--) {
        cool_timers_sift_down(l, j);
    }
}

/* Completes every timer whose deadline has passed. Runs with the loop lock
   held, on the owner thread, so the record writes are ordered against every
   poll and take. */
static void cool_timers_fire_due(cool_loop *l) {
    int64_t now = cool_mono_ms();
    while (l->tlen > 0 && l->timers[0].deadline <= now) {
        cool_future *fut = (cool_future *)l->timers[0].fut;
        l->tlen--;
        l->timers[0] = l->timers[l->tlen];
        cool_timers_sift_down(l, 0);
        if (fut->state == 0) {
            memset(fut->elem, 0, (size_t)fut->elem_size);
            fut->state = 1;
            fut->err = NULL;
            if (fut->waiter) {
                cool_task_enqueue(l, (cool_task *)fut->waiter);
                fut->waiter = NULL;
            }
        }
    }
}

int64_t cool_loop_init(void) {
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    if (l->running) {
        pthread_mutex_unlock(&l->mu);
        return 1;
    }
    pthread_condattr_t ca;
    if (pthread_condattr_init(&ca) != 0
        || pthread_condattr_setclock(&ca, CLOCK_MONOTONIC) != 0
        || pthread_cond_init(&l->wake, &ca) != 0) {
        pthread_mutex_unlock(&l->mu);
        return 1;
    }
    pthread_condattr_destroy(&ca);
    l->owner = pthread_self();
    l->running = 1;
    l->tlen = 0;
    l->seq = 0;
    pthread_mutex_unlock(&l->mu);
    return 0;
}

/* Frees the loop. Timer futures still pending leak their records, the
   documented rule breaking shutdown cost, never corruption. Complete every
   completer and shut the pool down before this, the channel free discipline. */
void cool_loop_free(void) {
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    l->running = 0;
    free(l->timers);
    l->timers = NULL;
    l->tlen = 0;
    l->tcap = 0;
    pthread_cond_destroy(&l->wake);
    pthread_mutex_unlock(&l->mu);
}

/* Wakes a parked await so it re-evaluates its deadlock gauge. Called by the
   runtime when a spawned thread exits or a pool task finishes; harmless when
   the loop never started. */
void cool_loop_kick(void) {
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    if (l->running) {
        pthread_cond_broadcast(&l->wake);
    }
    pthread_mutex_unlock(&l->mu);
}

/* Mints a pending future with room for one element. Owner thread only; the
   handle then travels anywhere as plain words and complete works from any
   thread. Exhaustion aborts, the allocator contract. */
void *cool_future_new(int64_t elem_size) {
    cool_loop_assert_owner();
    if (elem_size < 1) {
        cool_async_fatal("fatal: future element size must be at least 1\n");
    }
    cool_future *fut = (cool_future *)cool_gen_alloc((int64_t)sizeof(cool_future) + elem_size);
    if (!fut) {
        cool_async_fatal("fatal: out of memory\n");
    }
    fut->state = 0;
    fut->err = NULL;
    fut->elem_size = elem_size;
    fut->waiter = NULL;
    fut->task = NULL;
    return fut;
}

int64_t cool_future_gen(void *f) {
    return __atomic_load_n((int64_t *)((char *)f - 8), __ATOMIC_SEQ_CST);
}

/* Completes the future with an element and an error word, from any thread.
   Returns 0 on the first completion and 1 after, so a losing completer sees
   the refusal as a value. A completion arriving after the awaiter already
   consumed the future is a loser too, not a bug: the record's generation has
   moved on, nothing is written, and the refusal comes back like any other,
   so racing completers never need to outrun the awaiter. Only a completion
   into a record whose state it can still win writes anything, and the
   consume paths take only completed records, so the write can never race a
   retirement. */
int64_t cool_future_complete(void *f, int64_t gen, void *elem, void *err_stage) {
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    if (!l->running) {
        pthread_mutex_unlock(&l->mu);
        cool_async_fatal("fatal: the event loop is not running\n");
    }
    if (!f) {
        pthread_mutex_unlock(&l->mu);
        return 1;
    }
    int64_t *g = (int64_t *)((char *)f - 8);
    if (gen != 0 && __atomic_load_n(g, __ATOMIC_SEQ_CST) != gen) {
        pthread_mutex_unlock(&l->mu);
        return 1;
    }
    cool_future *fut = (cool_future *)f;
    if (fut->state == 1) {
        pthread_mutex_unlock(&l->mu);
        return 1;
    }
    memcpy(fut->elem, elem, (size_t)fut->elem_size);
    fut->err = *(void **)err_stage;
    fut->state = 1;
    if (fut->waiter) {
        cool_task_enqueue(l, (cool_task *)fut->waiter);
        fut->waiter = NULL;
    }
    pthread_cond_broadcast(&l->wake);
    pthread_mutex_unlock(&l->mu);
    return 0;
}

/* Copies the element and error out and retires the record, with the loop
   lock already held and kept until the retire completes, then unlocks. The
   generation bump runs under the same loop lock a completer takes for its gen
   and state checks, so a completion that already passed those checks has
   written before the retire can free the block, and one that has not yet
   checked finds the bumped generation and is refused; the retire can never
   race a write into the freed record. The heap lock nests inside the loop
   lock here and nowhere takes them in the reverse order: complete never
   touches the heap, and future_new and timer_new allocate outside the loop
   lock. */
static void cool_future_take_locked(cool_loop *l, void *f, int64_t gen, void *out, void *err_out) {
    cool_future *fut = (cool_future *)f;
    memcpy(out, fut->elem, (size_t)fut->elem_size);
    *(void **)err_out = fut->err;
    cool_timers_purge(l, f);
    int64_t dummy = 0;
    int64_t bad = cool_gen_retire_checked(f, gen, &dummy, 0);
    pthread_mutex_unlock(&l->mu);
    if (bad) {
        cool_async_fatal("fatal: use of a dead future\n");
    }
}

/* Polls without parking, after firing due timers. Returns 0 and consumes the
   future when it is ready, or 2 with a zeroed element while it is pending. */
int64_t cool_future_try(void *f, int64_t gen, void *out, void *err_out) {
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    cool_future_check(f, gen);
    cool_future *fut = (cool_future *)f;
    cool_timers_fire_due(l);
    if (fut->state == 0) {
        memset(out, 0, (size_t)fut->elem_size);
        *(void **)err_out = NULL;
        pthread_mutex_unlock(&l->mu);
        return 2;
    }
    cool_future_take_locked(l, f, gen, out, err_out);
    return 0;
}

/* Turns a relative wait in milliseconds into an absolute monotonic deadline
   for the condvar, the channel timeout shape. A wait already due clamps to
   now, so the timespec stays valid and the caller re-checks immediately. */
static struct timespec cool_abs_deadline(int64_t ms) {
    if (ms < 0) {
        ms = 0;
    }
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    ts.tv_sec += ms / 1000;
    ts.tv_nsec += (ms % 1000) * 1000000L;
    if (ts.tv_nsec >= 1000000000L) {
        ts.tv_sec += 1;
        ts.tv_nsec -= 1000000000L;
    }
    return ts;
}

/* Parks until the future completes, firing timers as their deadlines pass.
   When nothing can complete it, no timer pending, no spawned thread alive,
   no pool task in flight, no armed readiness watch, the wait is a deadlock
   and aborts by name; the gauges only drop after their bodies finish, and a
   drop kicks the loop, so the gate never fires against a completion still in
   flight. */
void cool_future_wait(void *f, int64_t gen, void *out, void *err_out) {
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    cool_future *fut = (cool_future *)f;
    pthread_mutex_lock(&l->mu);
    cool_future_check(f, gen);
    for (;;) {
        cool_timers_fire_due(l);
        if (fut->state == 1) {
            cool_future_take_locked(l, f, gen, out, err_out);
            return;
        }
        if (l->tlen == 0) {
            if (cool_live_threads() == 0 && cool_pool_inflight() == 0 && cool_reactor_armed() == 0) {
                cool_async_fatal("fatal: the event loop is idle but work is still pending\n");
            }
            pthread_cond_wait(&l->wake, &l->mu);
        } else {
            struct timespec until = cool_abs_deadline(l->timers[0].deadline - cool_mono_ms());
            pthread_cond_timedwait(&l->wake, &l->mu, &until);
        }
    }
}

/* Parks at most ms milliseconds. Returns 0 and consumes the future when it
   completes in time, or 2 with a zeroed element on timeout, leaving the
   future live, the recoverable escape from a wait the forever form would
   call a deadlock. */
int64_t cool_future_await_ms(void *f, int64_t gen, int64_t ms, void *out, void *err_out) {
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    cool_future *fut = (cool_future *)f;
    if (ms < 0) {
        ms = 0;
    }
    int64_t deadline = cool_mono_ms() + ms;
    pthread_mutex_lock(&l->mu);
    cool_future_check(f, gen);
    for (;;) {
        cool_timers_fire_due(l);
        if (fut->state == 1) {
            cool_future_take_locked(l, f, gen, out, err_out);
            return 0;
        }
        int64_t now = cool_mono_ms();
        if (now >= deadline) {
            memset(out, 0, (size_t)fut->elem_size);
            *(void **)err_out = NULL;
            pthread_mutex_unlock(&l->mu);
            return 2;
        }
        int64_t until = deadline;
        if (l->tlen > 0 && l->timers[0].deadline < until) {
            until = l->timers[0].deadline;
        }
        struct timespec ts = cool_abs_deadline(until - now);
        pthread_cond_timedwait(&l->wake, &l->mu, &ts);
    }
}

/* Releases a future that will never be consumed, pending or completed. The
   retire runs under the loop lock, the same lock a completer takes for its
   gen and state checks, so a completion racing the release either lands first
   and is dropped with the record or arrives after the generation bump and is
   refused; neither writes into a freed block. */
void cool_future_release(void *f, int64_t gen) {
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    cool_future_check(f, gen);
    cool_timers_purge(l, f);
    int64_t dummy = 0;
    int64_t bad = cool_gen_retire_checked(f, gen, &dummy, 0);
    pthread_mutex_unlock(&l->mu);
    if (bad) {
        cool_async_fatal("fatal: use of a dead future\n");
    }
}

/* Mints a future the timer heap completes with a zero element at the
   deadline. A non positive wait fires on the next await or poll, the only
   places timers fire. */
void *cool_timer_new(int64_t ms) {
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    cool_future *fut = (cool_future *)cool_gen_alloc((int64_t)sizeof(cool_future) + 8);
    if (!fut) {
        cool_async_fatal("fatal: out of memory\n");
    }
    fut->state = 0;
    fut->err = NULL;
    fut->elem_size = 8;
    fut->waiter = NULL;
    fut->task = NULL;
    if (ms < 0) {
        ms = 0;
    }
    pthread_mutex_lock(&l->mu);
    cool_ltimer t = { cool_mono_ms() + ms, l->seq, fut };
    l->seq++;
    cool_timers_push(l, t);
    pthread_mutex_unlock(&l->mu);
    return fut;
}

/* --- Async task substrate ------------------------------------------------ */

_Static_assert(sizeof(cool_task) == 48, "the task header must be exactly 48 bytes");

/* Mints a task and its result future without running anything. The record
   carries the eventual return value, elem_size at least one so even a void task
   owns a byte, and its task backref reaches the header. The header and frame
   are one malloc block, outside the generational using model; only the frame's
   state word is zeroed so the first poll dispatches to the entry. The call site
   fills the parameter slots, then cool_task_start schedules it. Owner only. */
void *cool_task_new(void *poll, int64_t frame_size, int64_t result_size) {
    cool_loop_assert_owner();
    int64_t elem_size = result_size < 1 ? 1 : result_size;
    cool_future *fut = (cool_future *)cool_gen_alloc((int64_t)sizeof(cool_future) + elem_size);
    if (!fut) {
        cool_async_fatal("fatal: out of memory\n");
    }
    fut->state = 0;
    fut->err = NULL;
    fut->elem_size = elem_size;
    fut->waiter = NULL;
    cool_task *t = (cool_task *)malloc(sizeof(cool_task) + (size_t)(frame_size < 0 ? 0 : frame_size));
    if (!t) {
        cool_async_fatal("fatal: out of memory\n");
    }
    t->poll = (void (*)(void *))poll;
    t->fut = fut;
    t->fut_gen = cool_future_gen(fut);
    t->next = NULL;
    t->queued = 0;
    t->envs = NULL;
    fut->task = t;
    *(int64_t *)((char *)t + sizeof(cool_task)) = 0;
    // The dusk visible frame is a root region: it holds the task's locals, which
    // may reference collected blocks, and it lives outside the generational
    // registry. Registered after the header is filled, removed at return.
    if (frame_size > 0) {
        cool_gc_add_region((char *)t + sizeof(cool_task), frame_size);
    }
    return fut;
}

/* The dusk visible frame sits immediately after the 48 byte header. */
void *cool_task_frame(void *fut) {
    cool_task *t = (cool_task *)((cool_future *)fut)->task;
    return (char *)t + sizeof(cool_task);
}

/* Schedules a fresh task. It never runs inline; the crank picks it up. */
void cool_task_start(void *fut) {
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    cool_task_enqueue(l, (cool_task *)((cool_future *)fut)->task);
    pthread_mutex_unlock(&l->mu);
}

/* Suspends the running task on a future. The generation check faults a dead
   future by name. A future already complete enqueues the task at once, so an
   await always costs exactly one scheduler turn, even on a ready value, and
   never resumes inline. A second task parking on one future is fatal: a future
   carries a single awaiter. */
void cool_task_await(void *frame, void *f, int64_t gen) {
    cool_loop *l = &cool_the_loop;
    cool_task *self = (cool_task *)((char *)frame - sizeof(cool_task));
    pthread_mutex_lock(&l->mu);
    cool_future_check(f, gen);
    cool_future *fut = (cool_future *)f;
    if (fut->state == 1) {
        cool_task_enqueue(l, self);
    } else if (fut->waiter && fut->waiter != self) {
        pthread_mutex_unlock(&l->mu);
        cool_async_fatal("fatal: two tasks await one future\n");
    } else {
        fut->waiter = self;
    }
    pthread_mutex_unlock(&l->mu);
}

/* Completes the running task with its return value and retires the task. When
   the task's own record is still generation live and pending it takes the
   value, wakes the awaiter, and drops its task backref; a record a release
   already retired is skipped and the value drops. The env arena and the task
   block are freed after the loop lock is released, per the malloc outside the
   loop lock discipline. */
void cool_task_return(void *frame, void *result, int64_t n) {
    cool_loop *l = &cool_the_loop;
    cool_task *t = (cool_task *)((char *)frame - sizeof(cool_task));
    pthread_mutex_lock(&l->mu);
    cool_future *fut = (cool_future *)t->fut;
    int live = fut != NULL;
    if (live && t->fut_gen != 0) {
        int64_t *g = (int64_t *)((char *)fut - 8);
        if (__atomic_load_n(g, __ATOMIC_SEQ_CST) != t->fut_gen) {
            live = 0;
        }
    }
    if (live) {
        if (fut->state == 0) {
            int64_t sz = n < fut->elem_size ? n : fut->elem_size;
            if (sz > 0) {
                memcpy(fut->elem, result, (size_t)sz);
            }
            fut->err = NULL;
            fut->state = 1;
            if (fut->waiter) {
                cool_task_enqueue(l, (cool_task *)fut->waiter);
                fut->waiter = NULL;
            }
            pthread_cond_broadcast(&l->wake);
        }
        fut->task = NULL;
    }
    pthread_mutex_unlock(&l->mu);
    // Deregister each root region before its backing memory is freed, so the
    // collector never scans freed memory. A base never registered, a zero
    // length frame or env, is a harmless no op. This runs after the loop lock is
    // released, keeping the malloc side outside the loop lock.
    cool_env_block *e = (cool_env_block *)t->envs;
    while (e) {
        cool_env_block *nx = e->next;
        cool_gc_del_region(e->payload);
        free(e);
        e = nx;
    }
    cool_gc_del_region((char *)t + sizeof(cool_task));
    free(t);
}

/* Allocates one closure environment block from the running task's arena and
   links it in. Loop thread only by construction, so no lock. */
void *cool_task_env_alloc(void *frame, int64_t n) {
    cool_task *t = (cool_task *)((char *)frame - sizeof(cool_task));
    cool_env_block *b = (cool_env_block *)malloc(sizeof(cool_env_block) + (size_t)(n < 0 ? 0 : n));
    if (!b) {
        cool_async_fatal("fatal: out of memory\n");
    }
    b->next = (cool_env_block *)t->envs;
    t->envs = b;
    // The environment payload is a root region: a captured value may reference a
    // collected block, and the block lives outside the generational registry.
    if (n > 0) {
        cool_gc_add_region(b->payload, n);
    }
    return b->payload;
}

/* The exported take a resumed poll calls to consume the future it awaited. The
   pending guard catches an impossible resume on a future still in flight, then
   the shipped retire under the loop lock runs unchanged. */
void cool_future_take(void *f, int64_t gen, void *out, void *err_out) {
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    cool_future_check(f, gen);
    cool_future *fut = (cool_future *)f;
    if (fut->state == 0) {
        pthread_mutex_unlock(&l->mu);
        cool_async_fatal("fatal: a task resumed on a pending future\n");
    }
    cool_future_take_locked(l, f, gen, out, err_out);
}

/* Cranks the loop until the target future completes, the only sync to async
   bridge. Timers fire at the top of every turn so an always ready task cannot
   starve them. A ready task runs before the deadlock gate is consulted, being
   runnable work; the gate fires only when the ready FIFO, the timer heap, and
   the three external gauges are all empty, when no completion can arrive, and
   re-entry through a second async_run is fatal. */
void cool_loop_run(void *f, int64_t gen, void *out, int64_t n) {
    (void)n;
    cool_loop_assert_owner();
    cool_loop *l = &cool_the_loop;
    pthread_mutex_lock(&l->mu);
    if (l->cranking) {
        pthread_mutex_unlock(&l->mu);
        cool_async_fatal("fatal: async_run re-entered the event loop\n");
    }
    l->cranking = 1;
    for (;;) {
        cool_timers_fire_due(l);
        cool_future_check(f, gen);
        cool_future *fut = (cool_future *)f;
        if (fut->state == 1) {
            void *err_scratch = NULL;
            l->cranking = 0;
            cool_future_take_locked(l, f, gen, out, &err_scratch);
            return;
        }
        if (l->ready_head) {
            cool_task *t = l->ready_head;
            l->ready_head = t->next;
            if (!l->ready_head) {
                l->ready_tail = NULL;
            }
            t->queued = 0;
            t->next = NULL;
            pthread_mutex_unlock(&l->mu);
            t->poll((char *)t + sizeof(cool_task));
            pthread_mutex_lock(&l->mu);
            continue;
        }
        if (l->tlen > 0) {
            struct timespec until = cool_abs_deadline(l->timers[0].deadline - cool_mono_ms());
            pthread_cond_timedwait(&l->wake, &l->mu, &until);
        } else if (cool_live_threads() + cool_pool_inflight() + cool_reactor_armed() > 0) {
            pthread_cond_wait(&l->wake, &l->mu);
        } else {
            l->cranking = 0;
            pthread_mutex_unlock(&l->mu);
            cool_async_fatal("fatal: the event loop is idle but work is still pending\n");
        }
    }
}

/* The unreachable arm of a poll's entry switch. A task must never resume in a
   state its own state machine did not emit. */
void cool_task_state_fault(void) {
    cool_async_fatal("fatal: a task resumed in an invalid state\n");
}
