/* The 0.4.1 readiness reactor: one C thread that turns file descriptor
   readiness into one shot readiness futures on the event loop, behind
   std.async.io. The reactor runs NO user code and touches NO user memory; it
   trades only in file descriptors, its own watch records and cool_event
   buffers, and the exported future and loop entry points (cool_future_new,
   cool_future_gen, cool_future_complete, cool_loop_kick), which cross from
   async.c as externs. The kernel poller lives behind the reactor_poller.h
   seam (the epoll backend on Linux, kqueue on the BSDs): one poller with a
   wake sentinel, one-shot arms so each armed token fires exactly once by
   construction. This file is the portable core; the seam moves no lock
   boundary.

   Three rules the whole file rests on:

   - The reactor mutex and the loop mutex are NEVER held at the same time.
     cool_fd_ready mints the future (the loop mutex is taken and released
     inside cool_future_new / cool_future_gen) BEFORE it takes the reactor
     mutex; the reactor thread does its poller disarm and registry lookup
     under the reactor mutex, then releases it BEFORE cool_future_complete and
     cool_loop_kick (the loop mutex) so neither pair overlaps.

   - The armed gauge is raised BEFORE the poller arm, so a fire's drop can
     never precede its raise, and dropped strictly AFTER cool_future_complete
     returns, so the completion is visible under the loop mutex before the
     count can reach zero. Every drop is followed by cool_loop_kick, even on a
     refused complete, so a parked awaiter re evaluates its deadlock gate the
     moment a watch can no longer complete anything.

   - The fd->watch registry, guarded by the reactor mutex, keys the fire
     path's disarm on watch identity, not the fd number alone. A
     close-while-armed misuse can let an fd number be reused and re-armed
     before the stale watch's event is handled; disarming by fd number would
     then tear down the innocent successor's registration and hang its
     awaiter. The fire path disarms only when the registry still maps this fd
     to THIS watch record; an arm overwrites the entry for a reused fd, so a
     stale fire skips its disarm. */
#define _GNU_SOURCE /* pipe2, accept4 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <unistd.h>

#include "reactor_poller.h"

extern void *cool_future_new(int64_t elem_size);
extern int64_t cool_future_gen(void *f);
extern int64_t cool_future_complete(void *f, int64_t gen, void *elem, void *err_stage);
extern void cool_loop_kick(void);

static void cool_reactor_fatal(const char *msg) {
    fflush(stdout);
    fputs(msg, stderr);
    abort();
}

/* Ignores SIGPIPE process-wide before main runs and before any thread is
   spawned (the constructor attribute schedules it at load time). A write() to a
   pipe whose read end is closed, or to a reset socket, then returns EPIPE
   instead of killing the process with SIGPIPE, and cool_write_nb turns that
   EPIPE into a "broken pipe" error value. This is the only mechanism that
   covers write() uniformly on every target: macOS has no MSG_NOSIGNAL send
   flag, so per-call suppression cannot be portable, but SIG_IGN is. No other
   runtime translation unit installs a signal handler, so nothing is
   overridden. */
__attribute__((constructor)) static void cool_rt_signal_init(void) {
    signal(SIGPIPE, SIG_IGN);
}

#define COOL_REACTOR_BATCH 16 /* events per poller wait
                                 (COOL_EV_* mask bits live in reactor_poller.h) */

/* A watch record: born on the loop thread, handed to the kernel as the poller
   arm's watch token, freed by the reactor thread after the fire. It
   carries the future's generation, not the fd's identity, so a reused fd can
   at worst produce a wrong reason completion, never a wrong memory write. */
typedef struct {
    void *fut;
    int64_t gen;
    int64_t fd;
} cool_watch;

static int64_t cool_reactor_armed_n = 0; /* the gauge, seq_cst */

typedef struct {
    pthread_mutex_t mu;      /* lifecycle + registry; never held with the loop mutex */
    pthread_cond_t done_cv;  /* a racing stopper waits here for the winner */
    pthread_t thread;
    int running;
    int stopping; /* a stop is signalled and in progress */
    int stopped;  /* the stop finished: thread joined, poller destroyed, registry freed */
    struct cool_poller poller; /* the kernel poller behind the reactor_poller.h seam */
} cool_reactor;

static cool_reactor cool_the_reactor = {
    PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER, 0, 0, 0, 0, {0},
};

/* The fd->current-watch registry, guarded by cool_the_reactor.mu. One entry
   per fd that has a live registration; an arm on a reused fd overwrites the
   entry so a stale fire keys its DEL on watch identity, not the fd number. */
typedef struct {
    int64_t fd;
    void *w;
} cool_reg_entry;
static cool_reg_entry *cool_reg = NULL;
static int64_t cool_reg_len = 0;
static int64_t cool_reg_cap = 0;

/* Records fd->w, called with the reactor mutex held. Overwrites any existing
   entry for the fd, which can only be a stale watch left by a close-while-armed
   misuse (a still registered fd would have failed the arm with EEXIST). */
static void cool_reg_put(int64_t fd, void *w) {
    for (int64_t i = 0; i < cool_reg_len; i++) {
        if (cool_reg[i].fd == fd) {
            cool_reg[i].w = w;
            return;
        }
    }
    if (cool_reg_len == cool_reg_cap) {
        int64_t cap = cool_reg_cap ? cool_reg_cap * 2 : 8;
        cool_reg_entry *grown = realloc(cool_reg, (size_t)cap * sizeof(cool_reg_entry));
        if (!grown) {
            cool_reactor_fatal("fatal: out of memory\n");
        }
        cool_reg = grown;
        cool_reg_cap = cap;
    }
    cool_reg[cool_reg_len].fd = fd;
    cool_reg[cool_reg_len].w = w;
    cool_reg_len++;
}

/* Called with the reactor mutex held. Returns 1 and removes the entry when fd
   still maps to THIS watch (the caller should disarm); returns 0 when the fd no
   longer maps to w (a stale fire whose entry an arm overwrote), so the caller
   skips the disarm and leaves the successor's registration intact. */
static int cool_reg_take(int64_t fd, void *w) {
    for (int64_t i = 0; i < cool_reg_len; i++) {
        if (cool_reg[i].fd == fd) {
            if (cool_reg[i].w != w) {
                return 0;
            }
            cool_reg_len--;
            cool_reg[i] = cool_reg[cool_reg_len];
            return 1;
        }
    }
    return 0;
}

/* Probes the registry for a live entry on fd, called with the reactor mutex
   held. Returns nonzero when fd already has a watch registered. ONLY the kqueue
   arm calls this: kqueue's EV_ADD has no fail-if-exists, so the kqueue backend
   rejects a second watch on an already-armed fd by probing the registry,
   reproducing the EEXIST fatal the epoll backend gets for free from
   EPOLL_CTL_ADD. The epoll arm never calls it, so on Linux this is dead code
   and the epoll path is byte-for-byte unchanged. Arm runs only on the loop
   thread, so a probe races nothing but the reactor thread's take, which the
   reactor mutex already serializes, exactly as it does for put and take. */
int cool_reg_probe(int fd) {
    for (int64_t i = 0; i < cool_reg_len; i++) {
        if (cool_reg[i].fd == fd) {
            return 1;
        }
    }
    return 0;
}

/* The gauge the deadlock gate in async.c reads: an armed watch is a possible
   completer, so a nonzero count keeps an otherwise idle await parked. */
int64_t cool_reactor_armed(void) {
    return __atomic_load_n(&cool_reactor_armed_n, __ATOMIC_SEQ_CST);
}

/* Fire: one readiness event, on the reactor thread. Under the reactor mutex,
   consult the registry: disarm and drop the entry only when the fd still maps
   to THIS watch (armed set equals registered set, and the kqueue port that
   auto deletes on EV_ONESHOT lands on the same seam), else skip the disarm so a
   stale watch cannot tear down an innocent successor's registration. Release
   the reactor mutex BEFORE completing. The mask arrives already normalized to
   the portable bits; the completion is refusal tolerant (a gen mismatch after future_free,
   or an already complete record, loses quietly per the racing completer
   rule); the gauge drops strictly after the completion and the loop is kicked
   even on a refusal. */
static void cool_reactor_handle(void *watch, int64_t mask) {
    cool_watch *w = (cool_watch *)watch;
    cool_reactor *r = &cool_the_reactor;
    pthread_mutex_lock(&r->mu);
    if (cool_reg_take(w->fd, w)) {
        cool_poller_disarm(&r->poller, (int)w->fd);
    }
    pthread_mutex_unlock(&r->mu);
    void *err = NULL;
    cool_future_complete(w->fut, w->gen, &mask, &err);
    free(w);
    __atomic_fetch_sub(&cool_reactor_armed_n, 1, __ATOMIC_SEQ_CST);
    cool_loop_kick();
}

/* The reactor thread. Blocks on cool_poller_wait; an is_stop event is the wake
   sentinel (the backend drains its own sentinel so a level triggered wake
   cannot spin the drain, finish the batch, then drain everything already ready
   with a non-blocking wait so an fd made ready before stop still completes),
   everything else is a fire. */
static void *cool_reactor_main(void *arg) {
    (void)arg;
    cool_reactor *r = &cool_the_reactor;
    cool_event evs[COOL_REACTOR_BATCH];
    for (;;) {
        int n = cool_poller_wait(&r->poller, evs, COOL_REACTOR_BATCH, 1);
        if (n < 0) {
            cool_reactor_fatal("fatal: the reactor could not wait for readiness\n");
        }
        int stopping = 0;
        for (int i = 0; i < n; i++) {
            if (evs[i].is_stop) {
                stopping = 1;
            } else {
                cool_reactor_handle(evs[i].watch, evs[i].mask);
            }
        }
        if (!stopping) {
            continue;
        }
        for (;;) {
            int m = cool_poller_wait(&r->poller, evs, COOL_REACTOR_BATCH, 0);
            if (m < 0) {
                cool_reactor_fatal("fatal: the reactor could not wait for readiness\n");
            }
            if (m == 0) {
                return NULL;
            }
            for (int i = 0; i < m; i++) {
                if (!evs[i].is_stop) {
                    cool_reactor_handle(evs[i].watch, evs[i].mask);
                }
            }
        }
    }
}

/* Starts the reactor: a fresh poller each start, so a stopped reactor restarts
   clean. Refuse while already running OR while a stop is in flight: a start
   landing in the winner's unlock->join->teardown window would otherwise build
   a fresh epoch's poller and thread that the resuming stop then destroys and
   frees out from under, stranding the new reactor thread on a closed poller.
   The refusal (returned as 1) is the pool_start precedent. Any failure unwinds
   the poller to pristine and returns 1, the pool_start unwind shape.
   The thread is created with bare pthread_create, NOT cool_thread_spawn, so it
   never raises the live-thread gauge; otherwise the deadlock gate could never
   fire while the reactor idles. */
int64_t cool_reactor_start(void) {
    cool_reactor *r = &cool_the_reactor;
    pthread_mutex_lock(&r->mu);
    if (r->running || (r->stopping && !r->stopped)) {
        pthread_mutex_unlock(&r->mu);
        return 1;
    }
    if (cool_poller_create(&r->poller) != 0) {
        pthread_mutex_unlock(&r->mu);
        return 1;
    }
    r->running = 1;
    r->stopping = 0;
    r->stopped = 0;
    if (pthread_create(&r->thread, NULL, cool_reactor_main, NULL) != 0) {
        cool_poller_destroy(&r->poller);
        r->running = 0;
        pthread_mutex_unlock(&r->mu);
        return 1;
    }
    pthread_mutex_unlock(&r->mu);
    return 0;
}

/* Stops the reactor: flip running under the mutex so no new arm lands, wake the
   poller sentinel, unlock, join. Only after the join is the armed count
   checked, race free because the reactor drained with a non-blocking wait
   before exiting, so no thread can still drop the count; a nonzero count is the
   watchleak fault. Then destroy the poller, free the registry, and reset, leaving
   a restart legal. When two threads race here the loser waits on done_cv until
   the winner has finished draining, joining, and closing, so every caller
   returns with the stop guarantee, the pool_shutdown shape. Since armed == 0
   by then, the registry is empty (an entry always outlives an un-dropped
   gauge), so the free reclaims only the backing array. */
void cool_reactor_stop(void) {
    cool_reactor *r = &cool_the_reactor;
    pthread_mutex_lock(&r->mu);
    if (!r->running) {
        while (r->stopping && !r->stopped) {
            pthread_cond_wait(&r->done_cv, &r->mu);
        }
        pthread_mutex_unlock(&r->mu);
        return;
    }
    r->running = 0;
    r->stopping = 1;
    r->stopped = 0;
    if (cool_poller_wake(&r->poller) != 0) {
        pthread_mutex_unlock(&r->mu);
        cool_reactor_fatal("fatal: the reactor could not be signalled to stop\n");
    }
    pthread_t t = r->thread;
    pthread_mutex_unlock(&r->mu);
    pthread_join(t, NULL);
    if (cool_reactor_armed() != 0) {
        cool_reactor_fatal("fatal: the reactor stopped while a watch is still armed\n");
    }
    pthread_mutex_lock(&r->mu);
    cool_poller_destroy(&r->poller);
    free(cool_reg);
    cool_reg = NULL;
    cool_reg_len = 0;
    cool_reg_cap = 0;
    r->stopped = 1;
    pthread_cond_broadcast(&r->done_cv);
    pthread_mutex_unlock(&r->mu);
}

/* Arm: mint the readiness future and register a one shot watch, on the loop
   thread. The reactor mutex spans the running check through the arm, so an
   arm cannot interleave with stop's teardown; the gauge is raised before the
   poller arm. Every refusal is a fault by design (the watch signatures carry no
   error channel), so a refused arm aborts with the count left high, which the
   abort makes moot; no non-fatal path leaks a raise. */
void *cool_fd_ready(int64_t fd, int64_t for_write) {
    void *fut = cool_future_new(8);
    int64_t gen = cool_future_gen(fut);
    cool_watch *w = (cool_watch *)malloc(sizeof(cool_watch));
    if (!w) {
        cool_reactor_fatal("fatal: out of memory\n");
    }
    w->fut = fut;
    w->gen = gen;
    w->fd = fd;
    cool_reactor *r = &cool_the_reactor;
    pthread_mutex_lock(&r->mu);
    if (!r->running) {
        pthread_mutex_unlock(&r->mu);
        cool_reactor_fatal("fatal: the reactor is not running\n");
    }
    __atomic_fetch_add(&cool_reactor_armed_n, 1, __ATOMIC_SEQ_CST);
    int armed = cool_poller_arm(&r->poller, (int)fd, (int)for_write, w);
    if (armed != 0) {
        pthread_mutex_unlock(&r->mu);
        if (armed == 1) {
            cool_reactor_fatal("fatal: the file descriptor already has an armed watch\n");
        }
        if (armed == 2) {
            cool_reactor_fatal("fatal: a regular file cannot report readiness\n");
        }
        cool_reactor_fatal("fatal: a readiness watch was armed on an invalid file descriptor\n");
    }
    cool_reg_put(fd, w);
    pthread_mutex_unlock(&r->mu);
    return fut;
}

/* Sets fd non blocking; 0 ok, 1 on any fcntl failure. */
int64_t cool_fd_nonblock(int64_t fd) {
    int flags = fcntl((int)fd, F_GETFL, 0);
    if (flags < 0) {
        return 1;
    }
    if (fcntl((int)fd, F_SETFL, flags | O_NONBLOCK) < 0) {
        return 1;
    }
    return 0;
}

#if defined(__APPLE__)
/* macOS has neither pipe2/accept4 nor the SOCK_NONBLOCK|SOCK_CLOEXEC socket()
   type flags the Linux/FreeBSD fast paths use to make a descriptor nonblocking
   and close-on-exec atomically. This helper reproduces that state with fcntl so
   the whole net/pipe surface is portable, not just the poller; every fast path
   below keeps its atomic form in the #else arm, untouched. Returns 0 on
   success, 1 on any fcntl failure. */
static int cool_apple_set_nb_cloexec(int fd) {
    int fl = fcntl(fd, F_GETFD, 0);
    if (fl < 0 || fcntl(fd, F_SETFD, fl | FD_CLOEXEC) < 0) {
        return 1;
    }
    fl = fcntl(fd, F_GETFL, 0);
    if (fl < 0 || fcntl(fd, F_SETFL, fl | O_NONBLOCK) < 0) {
        return 1;
    }
    return 0;
}
#endif

/* Creates a pipe, widening both fds into out2[0..1]; 0 ok, 1 on a generic
   failure, -3 when the fd table is exhausted (EMFILE/ENFILE) so the dusk
   surface can name that apart from other refusals. The mint is atomic: on any
   failure no descriptor is opened, so there is nothing to close and nothing
   leaks. The fds are close on exec and blocking by default. */
int64_t cool_pipe2(void *out2) {
    int fds[2];
#if defined(__APPLE__)
    /* No pipe2 on macOS: make a blocking pipe, then set close-on-exec on both
       ends by hand. Matches the O_CLOEXEC-only, blocking-by-default contract of
       the pipe2(O_CLOEXEC) fast path exactly (no O_NONBLOCK here). */
    if (pipe(fds) != 0) {
        return (errno == EMFILE || errno == ENFILE) ? -3 : 1;
    }
    if (fcntl(fds[0], F_SETFD, FD_CLOEXEC) < 0 || fcntl(fds[1], F_SETFD, FD_CLOEXEC) < 0) {
        close(fds[0]);
        close(fds[1]);
        return 1;
    }
#else
    if (pipe2(fds, O_CLOEXEC) != 0) {
        return (errno == EMFILE || errno == ENFILE) ? -3 : 1;
    }
#endif
    ((int64_t *)out2)[0] = (int64_t)fds[0];
    ((int64_t *)out2)[1] = (int64_t)fds[1];
    return 0;
}

/* Reads up to n bytes; returns the count (0 is EOF), -1 on EAGAIN/EWOULDBLOCK,
   -2 on any other failure. EINTR is retried. */
int64_t cool_read_nb(int64_t fd, void *buf, int64_t n) {
    ssize_t r;
    do {
        r = read((int)fd, buf, (size_t)n);
    } while (r < 0 && errno == EINTR);
    if (r >= 0) {
        return (int64_t)r;
    }
    if (errno == EAGAIN || errno == EWOULDBLOCK) {
        return -1;
    }
    return -2;
}

/* Writes up to n bytes; like cool_read_nb (the count, -1 on EAGAIN/EWOULDBLOCK,
   -2 on a hard failure, EINTR retried), with one extra code: EPIPE, a write to
   a pipe whose read end is fully closed or to a socket shut down for writing,
   returns -3 so the dusk surface can name it "broken pipe" apart from the
   generic write failure. A socket reset by its peer fails the write with
   ECONNRESET, not EPIPE, so that is a plain -2 "the write failed", not -3.
   SIGPIPE is ignored process-wide (see cool_rt_signal_init), so the EPIPE that
   would otherwise be a signal surfaces as this -3 return value instead of
   killing the process. */
int64_t cool_write_nb(int64_t fd, void *buf, int64_t n) {
    ssize_t r;
    do {
        r = write((int)fd, buf, (size_t)n);
    } while (r < 0 && errno == EINTR);
    if (r >= 0) {
        return (int64_t)r;
    }
    if (errno == EAGAIN || errno == EWOULDBLOCK) {
        return -1;
    }
    if (errno == EPIPE) {
        return -3;
    }
    return -2;
}

/* Closes fd; 0 on success or EINTR (Linux closes the fd either way), 1
   otherwise. */
int64_t cool_fd_close(int64_t fd) {
    if (close((int)fd) == 0 || errno == EINTR) {
        return 0;
    }
    return 1;
}

/* TCP shims for std.async.net. These are ONLY socket syscalls that mint or
   probe file descriptors; they touch NO event machinery. Every socket is born
   O_NONBLOCK | O_CLOEXEC (SOCK_NONBLOCK | SOCK_CLOEXEC in the type), so it
   drops straight into the reactor's readiness model and never leaks across an
   exec. A returned fd is an ordinary fd the dusk surface hands to
   readable(fd) / writable(fd), which arm the watch through the paths above.
   No fd->watch registry entry and no armed gauge is touched here.

   Return contract, chosen to mirror cool_read_nb / cool_write_nb so the dusk
   readiness loop treats sockets and pipes uniformly:

     >= 0  success: a fresh fd, an accepted client fd, or a host-order port.
     -1    "would block" OR a clean refused: EAGAIN / EWOULDBLOCK on accept,
           ECONNREFUSED on connect. The dusk surface maps -1 to "arm the watch
           and retry" (accept -> readable(fd), connect step -> writable(fd))
           or to a clean "connection refused" error value.
     -2    hard error: any other errno. The dusk surface maps -2 to a hard
           error value; there is nothing to await.
     -3    resource exhausted: EMFILE (this process is out of descriptors) or
           ENFILE (the system is) on an fd mint. This is TERMINAL, never "would
           block": an EMFILE on accept is not -1, or the accept loop would hot
           spin re-arming readable(fd) on a listener that stays ready. The dusk
           surface maps -3 to a "too many open files" error value. The exhausted
           mint opens no descriptor, so nothing is half open and nothing leaks.

   Note cool_tcp_listen and cool_tcp_local_port are one-shot setup/probe calls
   with no would-block state, so listen uses >= 0 (success), -3 (exhausted), and
   -1 (any other error); local_port uses only >= 0 and -1.

   The nonblocking connect handshake is four steps: cool_tcp_connect_ip returns
   a pending fd on EINPROGRESS; the caller awaits writable(fd); it then calls
   cool_tcp_connect_error(fd) to read the pending SO_ERROR. 0 means the connect
   completed, > 0 is the deferred refusal/error errno (ECONNREFUSED etc). So a
   refusal that did not surface synchronously at connect time surfaces here, and
   the dusk tcp_connect returns a clean error instead of handing back a broken
   fd. cool_tcp_connect_error uses 0 (connected), > 0 (deferred errno), and -1
   (getsockopt itself failed). */

/* Opens a nonblocking loopback listener bound to INADDR_LOOPBACK:port. port 0
   asks the OS for an ephemeral port (read it back with cool_tcp_local_port),
   which is what makes a self-connecting test deterministic. A backlog <= 0 is
   clamped to a sane default of 16. Returns the listening fd, -3 when the fd
   table is exhausted at the socket mint, or -1 on any other failure (the
   half-open fd is closed before returning). Only the socket() mint can exhaust
   the fd table; setsockopt/bind/listen never open a descriptor. */
int64_t cool_tcp_listen(int64_t port, int64_t backlog) {
#if defined(__APPLE__)
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        return (errno == EMFILE || errno == ENFILE) ? -3 : -1;
    }
    if (cool_apple_set_nb_cloexec(fd) != 0) {
        close(fd);
        return -1;
    }
#else
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return (errno == EMFILE || errno == ENFILE) ? -3 : -1;
    }
#endif
    int one = 1;
    if (setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one)) != 0) {
        close(fd);
        return -1;
    }
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    addr.sin_port = htons((uint16_t)port);
    if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
        close(fd);
        return -1;
    }
    int bl = (backlog <= 0) ? 16 : (int)backlog;
    if (listen(fd, bl) != 0) {
        close(fd);
        return -1;
    }
    return (int64_t)fd;
}

/* Accepts one pending connection on a listening fd, minting the client fd
   O_NONBLOCK | O_CLOEXEC. EINTR is retried. Returns the client fd (>= 0) on
   success, -1 on EAGAIN / EWOULDBLOCK (arm readable(fd) and retry), -3 when the
   fd table is exhausted (EMFILE/ENFILE), and -2 on any other error. The -3 is
   TERMINAL, deliberately not -1: on EMFILE the pending connection stays in the
   accept queue and the listener stays readable, so mapping it to "would block"
   would spin the accept loop re-arming readable(fd) forever; the client fd is
   never minted, so nothing leaks. */
int64_t cool_tcp_accept_nb(int64_t fd) {
    int c;
    do {
#if defined(__APPLE__)
        c = accept((int)fd, NULL, NULL);
#else
        c = accept4((int)fd, NULL, NULL, SOCK_NONBLOCK | SOCK_CLOEXEC);
#endif
    } while (c < 0 && errno == EINTR);
    if (c >= 0) {
#if defined(__APPLE__)
        /* accept() gives a blocking, inheritable client fd on macOS; set the
           flags accept4 would have set atomically. A set failure is a hard
           error, mapped to -2 like any other. The fcntl runs after the errno
           checks below never see it (c >= 0 short-circuits here). */
        if (cool_apple_set_nb_cloexec(c) != 0) {
            close(c);
            return -2;
        }
#endif
        return (int64_t)c;
    }
    if (errno == EAGAIN || errno == EWOULDBLOCK) {
        return -1;
    }
    if (errno == EMFILE || errno == ENFILE) {
        return -3;
    }
    return -2;
}

/* Opens a nonblocking IPv4 client and starts connecting to ip:port. ip is a
   NUL-terminated dotted-quad literal string (a dusk string's raw bytes); DNS
   is out of scope, so an inet_pton parse failure closes the fd and returns -2.
   A nonblocking connect that returns EINPROGRESS is SUCCESS: the caller awaits
   writable(fd) then checks SO_ERROR, so return the fd. An immediate connect
   (loopback) is also success. ECONNREFUSED returns -1 (clean refused); an
   fd-table exhaustion at the socket mint (EMFILE/ENFILE) returns -3; any other
   error closes the fd and returns -2. The socket() mint is the only fd-opening
   step, so an exhaustion opens no descriptor and nothing leaks. */
int64_t cool_tcp_connect_ip(void *ip, int64_t port) {
#if defined(__APPLE__)
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        return (errno == EMFILE || errno == ENFILE) ? -3 : -2;
    }
    if (cool_apple_set_nb_cloexec(fd) != 0) {
        close(fd);
        return -2;
    }
#else
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return (errno == EMFILE || errno == ENFILE) ? -3 : -2;
    }
#endif
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((uint16_t)port);
    if (inet_pton(AF_INET, (char *)ip, &addr.sin_addr) != 1) {
        close(fd);
        return -2;
    }
    int rc;
    do {
        rc = connect(fd, (struct sockaddr *)&addr, sizeof(addr));
    } while (rc != 0 && errno == EINTR);
    if (rc == 0 || errno == EINPROGRESS) {
        return (int64_t)fd;
    }
    if (errno == ECONNREFUSED) {
        close(fd);
        return -1;
    }
    close(fd);
    return -2;
}

/* Reads the host-order local port bound to fd via getsockname; needed so a
   listener opened on port 0 can report the OS-assigned ephemeral port. Returns
   the port (>= 0) or -1 on error. */
int64_t cool_tcp_local_port(int64_t fd) {
    struct sockaddr_in addr;
    socklen_t len = sizeof(addr);
    if (getsockname((int)fd, (struct sockaddr *)&addr, &len) != 0) {
        return -1;
    }
    return (int64_t)ntohs(addr.sin_port);
}

/* Fourth step of the nonblocking connect handshake: after cool_tcp_connect_ip
   returned a pending fd and the caller awaited writable(fd), read the pending
   socket error. 0 = connected OK; > 0 = the connect failed with this errno
   (ECONNREFUSED etc); -1 = getsockopt itself failed. */
int64_t cool_tcp_connect_error(int64_t fd) {
    int err = 0;
    socklen_t len = sizeof(err);
    if (getsockopt((int)fd, SOL_SOCKET, SO_ERROR, &err, &len) < 0) {
        return -1;
    }
    return (int64_t)err;
}

/* Sets the soft RLIMIT_NOFILE (open descriptor) limit to n, preserving the hard
   limit, and returns 0 on success or 1 on any failure. This is a deterministic
   test hook: a program lowers the limit to starve the fd-minting shims into
   EMFILE, checks that the exhaustion surfaces as a named error value rather than
   a crash, then raises the limit back (any value up to the untouched hard limit)
   to confirm the reactor and loop still work. Lowering the soft limit below the
   current usage is always permitted; existing descriptors stay valid, only new
   mints fail. No dusk standard-library surface depends on this. */
int64_t cool_rlimit_nofile(int64_t n) {
    struct rlimit rl;
    if (getrlimit(RLIMIT_NOFILE, &rl) != 0) {
        return 1;
    }
    rl.rlim_cur = (rlim_t)n;
    if (setrlimit(RLIMIT_NOFILE, &rl) != 0) {
        return 1;
    }
    return 0;
}
