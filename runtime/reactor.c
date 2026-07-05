/* The 0.4.1 readiness reactor: one C thread that turns file descriptor
   readiness into one shot readiness futures on the event loop, behind
   std.async.io. The reactor runs NO user code and touches NO user memory; it
   trades only in file descriptors, its own watch records and epoll event
   buffers, and the exported future and loop entry points (cool_future_new,
   cool_future_gen, cool_future_complete, cool_loop_kick), which cross from
   async.c as externs. One epoll fd, one eventfd sentinel, EPOLLONESHOT so
   each armed token fires exactly once by construction.

   Three rules the whole file rests on:

   - The reactor mutex and the loop mutex are NEVER held at the same time.
     cool_fd_ready mints the future (the loop mutex is taken and released
     inside cool_future_new / cool_future_gen) BEFORE it takes the reactor
     mutex; the reactor thread does its EPOLL_CTL_DEL and registry lookup
     under the reactor mutex, then releases it BEFORE cool_future_complete and
     cool_loop_kick (the loop mutex) so neither pair overlaps.

   - The armed gauge is raised BEFORE EPOLL_CTL_ADD, so a fire's drop can
     never precede its raise, and dropped strictly AFTER cool_future_complete
     returns, so the completion is visible under the loop mutex before the
     count can reach zero. Every drop is followed by cool_loop_kick, even on a
     refused complete, so a parked awaiter re evaluates its deadlock gate the
     moment a watch can no longer complete anything.

   - The fd->watch registry, guarded by the reactor mutex, keys the fire
     path's DEL on watch identity, not the fd number alone. A close-while-armed
     misuse can let an fd number be reused and re-armed before the stale
     watch's event is handled; DELeting by fd number would then tear down the
     innocent successor's registration and hang its awaiter. The fire path
     DELs only when the registry still maps this fd to THIS watch record; an
     arm overwrites the entry for a reused fd, so a stale fire skips its DEL. */
#define _GNU_SOURCE /* pipe2, eventfd, epoll_create1, EPOLLONESHOT, accept4 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/socket.h>
#include <unistd.h>

extern void *cool_future_new(int64_t elem_size);
extern int64_t cool_future_gen(void *f);
extern int64_t cool_future_complete(void *f, int64_t gen, void *elem, void *err_stage);
extern void cool_loop_kick(void);

static void cool_reactor_fatal(const char *msg) {
    fflush(stdout);
    fputs(msg, stderr);
    abort();
}

#define COOL_REACTOR_BATCH 16 /* events per epoll_wait */
#define COOL_EV_READ 1        /* normalized mask bits, kqueue-portable */
#define COOL_EV_WRITE 2
#define COOL_EV_HUP 4
#define COOL_EV_ERR 8

/* A watch record: born on the loop thread, handed to the kernel through
   epoll_event.data.ptr, freed by the reactor thread after the fire. It
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
    int stopped;  /* the stop finished: thread joined, fds closed, registry freed */
    int epfd;
    int stopfd; /* eventfd; registered with data.ptr == NULL */
} cool_reactor;

static cool_reactor cool_the_reactor = {
    PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER, 0, 0, 0, 0, -1, -1,
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
   misuse (a still registered fd would have failed the ADD with EEXIST). */
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
   still maps to THIS watch (the caller should DEL); returns 0 when the fd no
   longer maps to w (a stale fire whose entry an arm overwrote), so the caller
   skips the DEL and leaves the successor's registration intact. */
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

/* The gauge the deadlock gate in async.c reads: an armed watch is a possible
   completer, so a nonzero count keeps an otherwise idle await parked. */
int64_t cool_reactor_armed(void) {
    return __atomic_load_n(&cool_reactor_armed_n, __ATOMIC_SEQ_CST);
}

/* Fire: one readiness event, on the reactor thread. Under the reactor mutex,
   consult the registry: DEL and drop the entry only when the fd still maps to
   THIS watch (armed set equals registered set, and the kqueue port that auto
   deletes on EV_ONESHOT lands on the same seam), else skip the DEL so a stale
   watch cannot tear down an innocent successor's registration. Release the
   reactor mutex BEFORE completing. The mask is normalized to the portable
   bits; the completion is refusal tolerant (a gen mismatch after future_free,
   or an already complete record, loses quietly per the racing completer
   rule); the gauge drops strictly after the completion and the loop is kicked
   even on a refusal. */
static void cool_reactor_handle(struct epoll_event *ev) {
    cool_watch *w = (cool_watch *)ev->data.ptr;
    cool_reactor *r = &cool_the_reactor;
    pthread_mutex_lock(&r->mu);
    if (cool_reg_take(w->fd, w)) {
        epoll_ctl(r->epfd, EPOLL_CTL_DEL, (int)w->fd, NULL);
    }
    pthread_mutex_unlock(&r->mu);
    int64_t mask = 0;
    if (ev->events & EPOLLIN) {
        mask |= COOL_EV_READ;
    }
    if (ev->events & EPOLLOUT) {
        mask |= COOL_EV_WRITE;
    }
    if (ev->events & EPOLLHUP) {
        mask |= COOL_EV_HUP;
    }
    if (ev->events & EPOLLERR) {
        mask |= COOL_EV_ERR;
    }
    void *err = NULL;
    cool_future_complete(w->fut, w->gen, &mask, &err);
    free(w);
    __atomic_fetch_sub(&cool_reactor_armed_n, 1, __ATOMIC_SEQ_CST);
    cool_loop_kick();
}

/* The reactor thread. Blocks on epoll_wait; a NULL data.ptr is the stop
   sentinel (drain the eventfd so a level triggered fd cannot spin the drain,
   finish the batch, then drain everything already ready with timeout 0 so an
   fd made ready before stop still completes), everything else is a fire. */
static void *cool_reactor_main(void *arg) {
    (void)arg;
    cool_reactor *r = &cool_the_reactor;
    struct epoll_event evs[COOL_REACTOR_BATCH];
    for (;;) {
        int n = epoll_wait(r->epfd, evs, COOL_REACTOR_BATCH, -1);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            cool_reactor_fatal("fatal: the reactor could not wait for readiness\n");
        }
        int stopping = 0;
        for (int i = 0; i < n; i++) {
            if (evs[i].data.ptr == NULL) {
                uint64_t v;
                ssize_t rd;
                do {
                    rd = read(r->stopfd, &v, sizeof v);
                } while (rd < 0 && errno == EINTR);
                stopping = 1;
            } else {
                cool_reactor_handle(&evs[i]);
            }
        }
        if (!stopping) {
            continue;
        }
        for (;;) {
            int m = epoll_wait(r->epfd, evs, COOL_REACTOR_BATCH, 0);
            if (m < 0) {
                if (errno == EINTR) {
                    continue;
                }
                cool_reactor_fatal("fatal: the reactor could not wait for readiness\n");
            }
            if (m == 0) {
                return NULL;
            }
            for (int i = 0; i < m; i++) {
                if (evs[i].data.ptr != NULL) {
                    cool_reactor_handle(&evs[i]);
                }
            }
        }
    }
}

/* Starts the reactor: a fresh epoll fd and eventfd each start, so a stopped
   reactor restarts clean. Refuse while already running OR while a stop is in
   flight: a start landing in the winner's unlock->join->teardown window would
   otherwise build a fresh epoch's fds and thread that the resuming stop then
   closes and frees out from under, stranding the new reactor thread on a
   closed epfd. The refusal (returned as 1) is the pool_start precedent. Any
   failure unwinds fds to pristine and returns 1, the pool_start unwind shape.
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
    int epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) {
        pthread_mutex_unlock(&r->mu);
        return 1;
    }
    int stopfd = eventfd(0, EFD_CLOEXEC);
    if (stopfd < 0) {
        close(epfd);
        pthread_mutex_unlock(&r->mu);
        return 1;
    }
    struct epoll_event sev;
    sev.events = EPOLLIN;
    sev.data.ptr = NULL;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, stopfd, &sev) != 0) {
        close(stopfd);
        close(epfd);
        pthread_mutex_unlock(&r->mu);
        return 1;
    }
    r->epfd = epfd;
    r->stopfd = stopfd;
    r->running = 1;
    r->stopping = 0;
    r->stopped = 0;
    if (pthread_create(&r->thread, NULL, cool_reactor_main, NULL) != 0) {
        close(stopfd);
        close(epfd);
        r->epfd = -1;
        r->stopfd = -1;
        r->running = 0;
        pthread_mutex_unlock(&r->mu);
        return 1;
    }
    pthread_mutex_unlock(&r->mu);
    return 0;
}

/* Stops the reactor: flip running under the mutex so no new arm lands, signal
   the eventfd (EINTR retried), unlock, join. Only after the join is the armed
   count checked, race free because the reactor drained with timeout 0 before
   exiting, so no thread can still drop the count; a nonzero count is the
   watchleak fault. Then close both fds, free the registry, and reset, leaving
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
    uint64_t one = 1;
    ssize_t wr;
    do {
        wr = write(r->stopfd, &one, sizeof one);
    } while (wr < 0 && errno == EINTR);
    if (wr != (ssize_t)sizeof one) {
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
    close(r->epfd);
    close(r->stopfd);
    r->epfd = -1;
    r->stopfd = -1;
    free(cool_reg);
    cool_reg = NULL;
    cool_reg_len = 0;
    cool_reg_cap = 0;
    r->stopped = 1;
    pthread_cond_broadcast(&r->done_cv);
    pthread_mutex_unlock(&r->mu);
}

/* Arm: mint the readiness future and register a one shot watch, on the loop
   thread. The reactor mutex spans the running check through the ADD, so an
   arm cannot interleave with stop's teardown; the gauge is raised before the
   ADD. Every refusal is a fault by design (the watch signatures carry no
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
    struct epoll_event ev;
    ev.events = (uint32_t)(for_write ? EPOLLOUT : EPOLLIN) | EPOLLONESHOT;
    ev.data.ptr = w;
    if (epoll_ctl(r->epfd, EPOLL_CTL_ADD, (int)fd, &ev) != 0) {
        int e = errno;
        pthread_mutex_unlock(&r->mu);
        if (e == EEXIST) {
            cool_reactor_fatal("fatal: the file descriptor already has an armed watch\n");
        }
        if (e == EPERM) {
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

/* Creates a pipe, widening both fds into out2[0..1]; 0 ok, 1 on failure. The
   fds are close on exec and blocking by default. */
int64_t cool_pipe2(void *out2) {
    int fds[2];
    if (pipe2(fds, O_CLOEXEC) != 0) {
        return 1;
    }
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

/* Writes up to n bytes; same contract as cool_read_nb. */
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

   Note cool_tcp_listen and cool_tcp_local_port are one-shot setup/probe calls
   with no would-block state, so they use only >= 0 (success) and -1 (error).

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
   clamped to a sane default of 16. Returns the listening fd, or -1 on any
   failure (the half-open fd is closed before returning). */
int64_t cool_tcp_listen(int64_t port, int64_t backlog) {
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return -1;
    }
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
   success, -1 on EAGAIN / EWOULDBLOCK (arm readable(fd) and retry), -2 on any
   other error. */
int64_t cool_tcp_accept_nb(int64_t fd) {
    int c;
    do {
        c = accept4((int)fd, NULL, NULL, SOCK_NONBLOCK | SOCK_CLOEXEC);
    } while (c < 0 && errno == EINTR);
    if (c >= 0) {
        return (int64_t)c;
    }
    if (errno == EAGAIN || errno == EWOULDBLOCK) {
        return -1;
    }
    return -2;
}

/* Opens a nonblocking IPv4 client and starts connecting to ip:port. ip is a
   NUL-terminated dotted-quad literal string (a dusk string's raw bytes); DNS
   is out of scope, so an inet_pton parse failure closes the fd and returns -2.
   A nonblocking connect that returns EINPROGRESS is SUCCESS: the caller awaits
   writable(fd) then checks SO_ERROR, so return the fd. An immediate connect
   (loopback) is also success. ECONNREFUSED returns -1 (clean refused); any
   other error closes the fd and returns -2. */
int64_t cool_tcp_connect_ip(void *ip, int64_t port) {
    int fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return -2;
    }
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
