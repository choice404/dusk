/* Threads for dusk, over pthreads. A spawned closure's environment is a heap
   block the spawner fills and the trampoline frees after the body returns, so
   the thread never touches the spawner's stack. The thread handle is a record
   in the generational heap holding the pthread_t, so a stale handle faults
   through the same dereference check every managed pointer uses, and join
   retires it, making a double join a deterministic fault. */
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <time.h>

extern void *cool_gen_alloc(int64_t size);
extern void cool_gen_free(void *p);
extern int64_t cool_gen_retire_checked(void *p, int64_t gen, void *out, int64_t n);

typedef struct {
    void (*fn)(void *);
    void *env;
} cool_task;

static void *cool_thread_tramp(void *arg) {
    cool_task t = *(cool_task *)arg;
    free(arg);
    t.fn(t.env);
    free(t.env);
    return NULL;
}

/* Spawns a thread running fn(env). Returns the handle record's data pointer,
   or NULL when the task allocation, the handle allocation, or pthread_create
   fails, in which case env is freed here since the trampoline never runs. */
void *cool_thread_spawn(void *fn, void *env) {
    cool_task *task = malloc(sizeof(cool_task));
    if (!task) {
        free(env);
        return NULL;
    }
    task->fn = (void (*)(void *))fn;
    task->env = env;
    pthread_t *rec = (pthread_t *)cool_gen_alloc((int64_t)sizeof(pthread_t));
    if (!rec) {
        free(task);
        free(env);
        return NULL;
    }
    if (pthread_create(rec, NULL, cool_thread_tramp, task) != 0) {
        free(task);
        free(env);
        cool_gen_free(rec);
        return NULL;
    }
    return rec;
}

/* Joins the thread behind a handle record. The generation check and the
   record's retirement happen in one heap critical section, so a double join,
   even from two threads holding copies of the handle, faults deterministically
   instead of double parking the block. Returns 0 on success. */
int64_t cool_thread_join(void *rec, int64_t gen) {
    pthread_t t;
    if (cool_gen_retire_checked(rec, gen, &t, (int64_t)sizeof(pthread_t))) {
        return 1;
    }
    if (pthread_join(t, NULL) != 0) {
        return 1;
    }
    return 0;
}

/* The spawn environment block. Aborts on exhaustion rather than handing back
   a null the capture stores would write through. */
void *cool_alloc_env(int64_t n) {
    void *p = malloc((size_t)n);
    if (!p) {
        fflush(stdout);
        fputs("fatal: out of memory\n", stderr);
        abort();
    }
    return p;
}

void cool_sleep_ms(int64_t ms) {
    struct timespec ts;
    ts.tv_sec = ms / 1000;
    ts.tv_nsec = (ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

/* Sequentially consistent int64 atomics, the only ordering 0.3.x offers. */
int64_t cool_atomic_load(int64_t *p) {
    return __atomic_load_n(p, __ATOMIC_SEQ_CST);
}

void cool_atomic_store(int64_t *p, int64_t v) {
    __atomic_store_n(p, v, __ATOMIC_SEQ_CST);
}

/* Adds d and returns the new value. */
int64_t cool_atomic_add(int64_t *p, int64_t d) {
    return __atomic_add_fetch(p, d, __ATOMIC_SEQ_CST);
}

/* Compare and swap. Returns 1 when the swap happened. */
int64_t cool_atomic_cas(int64_t *p, int64_t expect, int64_t desired) {
    return __atomic_compare_exchange_n(p, &expect, desired, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST) ? 1 : 0;
}

/* Bounded channels as the textbook monitor: one mutex, two condition
   variables, a ring of cap * elem_size bytes, a closed flag, and a count of
   threads blocked inside a wait. The condvars run on CLOCK_MONOTONIC so the
   timed receive planned for 0.3.3 cannot be confused by a wall clock step. */
typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t not_full;
    pthread_cond_t not_empty;
    char *buf;
    int64_t elem_size;
    int64_t cap;
    int64_t len;
    int64_t head;
    int64_t waiters;
    int closed;
} cool_chan;

static void cool_chan_fatal(const char *msg) {
    fflush(stdout);
    fputs(msg, stderr);
    abort();
}

/* Creates a bounded channel. Exhaustion and a bad capacity are fatal, the
   same contract as the allocator, because a channel that cannot exist has no
   error path a fresh program could act on. */
void *cool_chan_new(int64_t elem_size, int64_t cap) {
    if (elem_size < 1) {
        cool_chan_fatal("fatal: channel element size must be at least 1\n");
    }
    if (cap < 1) {
        cool_chan_fatal("fatal: channel capacity must be at least 1\n");
    }
    if (cap > INT64_MAX / elem_size) {
        cool_chan_fatal("fatal: out of memory\n");
    }
    cool_chan *c = malloc(sizeof(cool_chan));
    if (!c) {
        cool_chan_fatal("fatal: out of memory\n");
    }
    c->buf = malloc((size_t)(cap * elem_size));
    if (!c->buf) {
        cool_chan_fatal("fatal: out of memory\n");
    }
    c->elem_size = elem_size;
    c->cap = cap;
    c->len = 0;
    c->head = 0;
    c->waiters = 0;
    c->closed = 0;
    pthread_condattr_t ca;
    if (pthread_mutex_init(&c->mu, NULL) != 0 || pthread_condattr_init(&ca) != 0
        || pthread_condattr_setclock(&ca, CLOCK_MONOTONIC) != 0
        || pthread_cond_init(&c->not_full, &ca) != 0
        || pthread_cond_init(&c->not_empty, &ca) != 0) {
        cool_chan_fatal("fatal: channel init failed\n");
    }
    pthread_condattr_destroy(&ca);
    return c;
}

/* Copies elem_size bytes in, blocking while the ring is full. Returns 0 on
   success and 1 when the channel is closed, whether it was closed before the
   call or while this sender slept. */
int64_t cool_chan_send(void *ch, void *elem) {
    cool_chan *c = (cool_chan *)ch;
    pthread_mutex_lock(&c->mu);
    while (c->len == c->cap && !c->closed) {
        c->waiters++;
        pthread_cond_wait(&c->not_full, &c->mu);
        c->waiters--;
    }
    if (c->closed) {
        pthread_mutex_unlock(&c->mu);
        return 1;
    }
    int64_t slot = (c->head + c->len) % c->cap;
    memcpy(c->buf + slot * c->elem_size, elem, (size_t)c->elem_size);
    c->len++;
    pthread_cond_signal(&c->not_empty);
    pthread_mutex_unlock(&c->mu);
    return 0;
}

/* Copies the oldest element out, blocking while the ring is empty. Returns 0
   on success and 1 when the channel is closed and drained, in which case the
   out buffer is zeroed so the caller never reads stale bytes. */
int64_t cool_chan_recv(void *ch, void *out) {
    cool_chan *c = (cool_chan *)ch;
    pthread_mutex_lock(&c->mu);
    while (c->len == 0 && !c->closed) {
        c->waiters++;
        pthread_cond_wait(&c->not_empty, &c->mu);
        c->waiters--;
    }
    if (c->len == 0) {
        memset(out, 0, (size_t)c->elem_size);
        pthread_mutex_unlock(&c->mu);
        return 1;
    }
    memcpy(out, c->buf + c->head * c->elem_size, (size_t)c->elem_size);
    c->head = (c->head + 1) % c->cap;
    c->len--;
    pthread_cond_signal(&c->not_full);
    pthread_mutex_unlock(&c->mu);
    return 0;
}

/* Closes the channel and wakes every blocked sender and receiver. Idempotent,
   so racing closers are harmless. Buffered elements stay receivable. */
void cool_chan_close(void *ch) {
    cool_chan *c = (cool_chan *)ch;
    pthread_mutex_lock(&c->mu);
    c->closed = 1;
    pthread_cond_broadcast(&c->not_full);
    pthread_cond_broadcast(&c->not_empty);
    pthread_mutex_unlock(&c->mu);
}

/* Frees the channel. Freeing while a thread is blocked inside a send or recv
   is fatal, caught best effort under the monitor lock. The sanctioned order
   is close, then join every thread that touches the channel, then free. */
void cool_chan_free(void *ch) {
    cool_chan *c = (cool_chan *)ch;
    pthread_mutex_lock(&c->mu);
    if (c->waiters > 0) {
        cool_chan_fatal("fatal: channel freed while threads wait on it\n");
    }
    pthread_mutex_unlock(&c->mu);
    pthread_cond_destroy(&c->not_full);
    pthread_cond_destroy(&c->not_empty);
    pthread_mutex_destroy(&c->mu);
    free(c->buf);
    free(c);
}
