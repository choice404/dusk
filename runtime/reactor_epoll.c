/* The epoll backend for the readiness reactor's poller seam. The whole file is
   a translation unit on Linux and compiles to nothing elsewhere, so the shared
   reactor core links against exactly one backend. Every kernel call here is
   lifted verbatim from the pre-seam reactor.c epoll code: an epoll fd, an
   eventfd wake sentinel registered with a NULL data.ptr, and EPOLLONESHOT so
   each armed token fires exactly once. The behavior is bit for bit the same;
   only the kernel now sits behind cool_poller_*. */
#if defined(__linux__)

#define _GNU_SOURCE /* eventfd, epoll_create1, EPOLLONESHOT */
#include <errno.h>
#include <stdint.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <unistd.h>

#include "reactor_poller.h"

#define COOL_EPOLL_BATCH 16 /* events per epoll_wait */

/* Fresh epoll fd and eventfd each create, so a destroyed poller recreates
   clean. The eventfd is registered with data.ptr == NULL so a fired sentinel
   is recognized by its null token. Any failure unwinds every fd it opened and
   returns 1, leaving nothing half open. */
int cool_poller_create(struct cool_poller *p) {
    int epfd = epoll_create1(EPOLL_CLOEXEC);
    if (epfd < 0) {
        return 1;
    }
    int stopfd = eventfd(0, EFD_CLOEXEC);
    if (stopfd < 0) {
        close(epfd);
        return 1;
    }
    struct epoll_event sev;
    sev.events = EPOLLIN;
    sev.data.ptr = NULL;
    if (epoll_ctl(epfd, EPOLL_CTL_ADD, stopfd, &sev) != 0) {
        close(stopfd);
        close(epfd);
        return 1;
    }
    p->epfd = epfd;
    p->stopfd = stopfd;
    return 0;
}

void cool_poller_destroy(struct cool_poller *p) {
    close(p->epfd);
    close(p->stopfd);
    p->epfd = -1;
    p->stopfd = -1;
}

/* A one-shot registration keyed on watch through data.ptr; EPOLLONESHOT means
   the token fires exactly once. Classifies epoll_ctl's errno into the seam's
   codes: EEXIST (already armed) -> 1, EPERM (a regular file) -> 2, anything
   else (a bad fd and the rest) -> 3. */
int cool_poller_arm(struct cool_poller *p, int fd, int for_write, void *watch) {
    struct epoll_event ev;
    ev.events = (uint32_t)(for_write ? EPOLLOUT : EPOLLIN) | EPOLLONESHOT;
    ev.data.ptr = watch;
    if (epoll_ctl(p->epfd, EPOLL_CTL_ADD, fd, &ev) != 0) {
        int e = errno;
        if (e == EEXIST) {
            return 1;
        }
        if (e == EPERM) {
            return 2;
        }
        return 3;
    }
    return 0;
}

/* EPOLL_CTL_DEL, result ignored: a watch the kernel already dropped because
   its fd was closed (EBADF) is the tolerated already-gone case. */
void cool_poller_disarm(struct cool_poller *p, int fd) {
    epoll_ctl(p->epfd, EPOLL_CTL_DEL, fd, NULL);
}

/* epoll_wait with a -1 (block) or 0 (poll) timeout, EINTR retried internally.
   Each event becomes a cool_event: the NULL data.ptr sentinel sets is_stop and
   drains the eventfd (EINTR retried) so a level-triggered read cannot spin the
   drain, everything else translates the epoll bits into the normalized mask.
   Returns the event count (>= 0) or -1 on a non-EINTR failure. */
int cool_poller_wait(struct cool_poller *p, cool_event *out, int max, int block) {
    int cap = max < COOL_EPOLL_BATCH ? max : COOL_EPOLL_BATCH;
    struct epoll_event evs[COOL_EPOLL_BATCH];
    int n;
    do {
        n = epoll_wait(p->epfd, evs, cap, block ? -1 : 0);
    } while (n < 0 && errno == EINTR);
    if (n < 0) {
        return -1;
    }
    for (int i = 0; i < n; i++) {
        if (evs[i].data.ptr == NULL) {
            uint64_t v;
            ssize_t rd;
            do {
                rd = read(p->stopfd, &v, sizeof v);
            } while (rd < 0 && errno == EINTR);
            out[i].watch = NULL;
            out[i].mask = 0;
            out[i].is_stop = 1;
        } else {
            int64_t mask = 0;
            if (evs[i].events & EPOLLIN) {
                mask |= COOL_EV_READ;
            }
            if (evs[i].events & EPOLLOUT) {
                mask |= COOL_EV_WRITE;
            }
            if (evs[i].events & EPOLLHUP) {
                mask |= COOL_EV_HUP;
            }
            if (evs[i].events & EPOLLERR) {
                mask |= COOL_EV_ERR;
            }
            out[i].watch = evs[i].data.ptr;
            out[i].mask = mask;
            out[i].is_stop = 0;
        }
    }
    return n;
}

/* Writes the eventfd wake sentinel, EINTR retried. Returns 0 when the full
   token landed and 1 otherwise, so the shared stop can fault a sentinel that
   could not be signalled. */
int cool_poller_wake(struct cool_poller *p) {
    uint64_t one = 1;
    ssize_t wr;
    do {
        wr = write(p->stopfd, &one, sizeof one);
    } while (wr < 0 && errno == EINTR);
    return wr == (ssize_t)sizeof one ? 0 : 1;
}

#endif /* __linux__ */
