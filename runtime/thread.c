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
