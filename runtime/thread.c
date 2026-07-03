/* Threads for dusk, over pthreads. A spawned closure's environment is a heap
   block the spawner fills and the trampoline frees after the body returns, so
   the thread never touches the spawner's stack. The thread handle is a record
   in the generational heap holding the pthread_t, so a stale handle faults
   through the same dereference check every managed pointer uses, and join
   retires it, making a double join a deterministic fault. */
#include <errno.h>
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

static void cool_thread_fatal(const char *msg) {
    fflush(stdout);
    fputs(msg, stderr);
    abort();
}

/* Creates a bounded channel. Exhaustion and a bad capacity are fatal, the
   same contract as the allocator, because a channel that cannot exist has no
   error path a fresh program could act on. */
void *cool_chan_new(int64_t elem_size, int64_t cap) {
    if (elem_size < 1) {
        cool_thread_fatal("fatal: channel element size must be at least 1\n");
    }
    if (cap < 1) {
        cool_thread_fatal("fatal: channel capacity must be at least 1\n");
    }
    if (cap > INT64_MAX / elem_size) {
        cool_thread_fatal("fatal: out of memory\n");
    }
    cool_chan *c = malloc(sizeof(cool_chan));
    if (!c) {
        cool_thread_fatal("fatal: out of memory\n");
    }
    c->buf = malloc((size_t)(cap * elem_size));
    if (!c->buf) {
        cool_thread_fatal("fatal: out of memory\n");
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
        cool_thread_fatal("fatal: channel init failed\n");
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

/* Mutexes and condition variables, thin wrappers over pthread with the named
   fault contract the rest of the runtime keeps. The mutex is the error
   checking kind, so relocking by the holder and unlocking by a non holder,
   both undefined in the default pthread flavor, abort with a message instead.
   Condvars run on CLOCK_MONOTONIC like the channel's, ready for timed waits. */
void *cool_mutex_new(void) {
    pthread_mutex_t *m = malloc(sizeof(pthread_mutex_t));
    if (!m) {
        cool_thread_fatal("fatal: out of memory\n");
    }
    pthread_mutexattr_t ma;
    if (pthread_mutexattr_init(&ma) != 0
        || pthread_mutexattr_settype(&ma, PTHREAD_MUTEX_ERRORCHECK) != 0
        || pthread_mutex_init(m, &ma) != 0) {
        cool_thread_fatal("fatal: mutex init failed\n");
    }
    pthread_mutexattr_destroy(&ma);
    return m;
}

/* Each fault names the actual misuse. The error checking kind reports the
   canonical codes for a relock and a foreign unlock; anything else, EINVAL
   from a destroyed mutex above all, means the handle itself is dead, which is
   the double free and use after free family, named as such. */
void cool_mutex_lock(void *m) {
    int rc = pthread_mutex_lock((pthread_mutex_t *)m);
    if (rc == EDEADLK) {
        cool_thread_fatal("fatal: mutex relocked by the thread that holds it\n");
    }
    if (rc != 0) {
        cool_thread_fatal("fatal: lock of an invalid or freed mutex\n");
    }
}

void cool_mutex_unlock(void *m) {
    int rc = pthread_mutex_unlock((pthread_mutex_t *)m);
    if (rc == EPERM) {
        cool_thread_fatal("fatal: mutex unlocked by a thread that does not hold it\n");
    }
    if (rc != 0) {
        cool_thread_fatal("fatal: unlock of an invalid or freed mutex\n");
    }
}

/* Frees the mutex. A trylock probe catches a live holder, including this
   thread, and aborts rather than destroying a lock someone sits inside. A
   probe code outside busy and deadlock means the handle is already dead,
   which is the double free, named as such. */
void cool_mutex_free(void *m) {
    pthread_mutex_t *mu = (pthread_mutex_t *)m;
    int rc = pthread_mutex_trylock(mu);
    if (rc == EBUSY || rc == EDEADLK) {
        cool_thread_fatal("fatal: mutex freed while held\n");
    }
    if (rc != 0) {
        cool_thread_fatal("fatal: free of an invalid or freed mutex\n");
    }
    pthread_mutex_unlock(mu);
    pthread_mutex_destroy(mu);
    free(mu);
}

/* The condvar record carries a waiter count beside the pthread object, so
   freeing a condvar someone waits on faults by name, the channel's contract,
   instead of hanging inside a destroy that quiesces forever. */
typedef struct {
    pthread_cond_t cv;
    int64_t waiters;
} cool_cond;

void *cool_cond_new(void) {
    cool_cond *c = malloc(sizeof(cool_cond));
    if (!c) {
        cool_thread_fatal("fatal: out of memory\n");
    }
    c->waiters = 0;
    pthread_condattr_t ca;
    if (pthread_condattr_init(&ca) != 0
        || pthread_condattr_setclock(&ca, CLOCK_MONOTONIC) != 0
        || pthread_cond_init(&c->cv, &ca) != 0) {
        cool_thread_fatal("fatal: condvar init failed\n");
    }
    pthread_condattr_destroy(&ca);
    return c;
}

/* Waits on the condvar, releasing and reacquiring the mutex around the sleep.
   The caller must hold the mutex; a violation aborts by name. The waiter
   count rises before the mutex is released inside the wait, so a thread that
   acquires the mutex afterward observes the waiter. Wakeups can be spurious,
   so callers loop on their predicate. */
void cool_cond_wait(void *cv, void *m) {
    cool_cond *c = (cool_cond *)cv;
    __atomic_fetch_add(&c->waiters, 1, __ATOMIC_SEQ_CST);
    int rc = pthread_cond_wait(&c->cv, (pthread_mutex_t *)m);
    __atomic_fetch_sub(&c->waiters, 1, __ATOMIC_SEQ_CST);
    if (rc != 0) {
        cool_thread_fatal("fatal: condvar wait without holding the mutex\n");
    }
}

void cool_cond_signal(void *cv) {
    pthread_cond_signal(&((cool_cond *)cv)->cv);
}

void cool_cond_broadcast(void *cv) {
    pthread_cond_broadcast(&((cool_cond *)cv)->cv);
}

/* Frees the condvar. Freeing while a thread waits on it is fatal, caught
   best effort through the waiter count, since glibc's destroy would block
   forever waiting for the waiter to leave and no signal is coming. */
void cool_cond_free(void *cv) {
    cool_cond *c = (cool_cond *)cv;
    if (__atomic_load_n(&c->waiters, __ATOMIC_SEQ_CST) != 0) {
        cool_thread_fatal("fatal: condvar freed while threads wait on it\n");
    }
    pthread_cond_destroy(&c->cv);
    free(c);
}

/* Frees the channel. Freeing while a thread is blocked inside a send or recv
   is fatal, caught best effort under the monitor lock. The sanctioned order
   is close, then join every thread that touches the channel, then free. */
void cool_chan_free(void *ch) {
    cool_chan *c = (cool_chan *)ch;
    pthread_mutex_lock(&c->mu);
    if (c->waiters > 0) {
        cool_thread_fatal("fatal: channel freed while threads wait on it\n");
    }
    pthread_mutex_unlock(&c->mu);
    pthread_cond_destroy(&c->not_full);
    pthread_cond_destroy(&c->not_empty);
    pthread_mutex_destroy(&c->mu);
    free(c->buf);
    free(c);
}
