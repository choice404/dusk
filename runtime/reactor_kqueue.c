/* The kqueue backend for the readiness reactor's poller seam, the BSD/macOS
   counterpart of reactor_epoll.c. It implements the same six cool_poller_*
   functions over kqueue/kevent, with an EVFILT_USER event as the wake sentinel
   in place of the epoll eventfd. On Linux the platform guard below is false, so
   this whole translation unit compiles to nothing but the typedef, and the
   driver links it harmlessly; the epoll backend is the one that provides the
   symbols there.

   HONEST STATUS (0.4.4 M2): this body is WRITTEN but has NOT been compiled or
   run on this machine, which is Linux. The kqueue path needs a BSD or macOS
   runner to compile and to exercise the full reactor/net/stress matrix. Two
   things in particular that a runner must confirm:

     - EVFILT_USER (the NOTE_TRIGGER wake sentinel) is present on macOS and
       FreeBSD; its exact availability on NetBSD and OpenBSD must be verified on
       those platforms, and this backend swapped for a self-pipe sentinel there
       if it is missing.

     - The collision behavior diverges from epoll in one exotic case, and a
       runner must pin it. epoll's EPOLL_CTL_ADD rejects a second registration
       on the same fd with EEXIST, which the reactor turns into the "already has
       an armed watch" fault; but epoll auto-drops a registration when the fd is
       closed, so a close-while-armed-then-reused fd can be re-armed and simply
       overwrites the stale registry entry. kqueue's EV_ADD never fails on a
       duplicate, so this backend reproduces the fault by probing the registry
       (cool_reg_probe) BEFORE the EV_ADD. That makes the ordinary doublewatch
       misuse (two arms, no close in between) fault identically to epoll, but a
       re-arm after close-while-armed sees the stale entry and faults where
       epoll would have overwritten it. That divergence is intended for M2 and
       must be pinned by the BSD/macOS runner, not smoothed over here. */
typedef int cool_reactor_kqueue_translation_unit; /* keeps the Linux TU non-empty */

#if defined(__APPLE__) || defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__)

#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/event.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#include "reactor_poller.h"

#define COOL_KQ_BATCH 16 /* events per kevent wait */
#define COOL_KQ_STOP_IDENT 0 /* the EVFILT_USER wake sentinel's ident */

/* Fresh kqueue each create, so a destroyed poller recreates clean. The kqueue
   fd is made close-on-exec by hand (kqueue() takes no CLOEXEC flag), and the
   EVFILT_USER wake sentinel is registered EV_ADD | EV_CLEAR so a single
   NOTE_TRIGGER yields exactly one is_stop event and auto-resets, the kqueue
   analog of draining the epoll eventfd. Any failure closes the kqueue and
   returns 1, leaving nothing half open. */
int cool_poller_create(struct cool_poller *p) {
    int kq = kqueue();
    if (kq < 0) {
        return 1;
    }
    if (fcntl(kq, F_SETFD, FD_CLOEXEC) < 0) {
        close(kq);
        return 1;
    }
    struct kevent kev;
    EV_SET(&kev, COOL_KQ_STOP_IDENT, EVFILT_USER, EV_ADD | EV_CLEAR, 0, 0, NULL);
    if (kevent(kq, &kev, 1, NULL, 0, NULL) < 0) {
        close(kq);
        return 1;
    }
    p->kq = kq;
    return 0;
}

void cool_poller_destroy(struct cool_poller *p) {
    close(p->kq);
    p->kq = -1;
}

/* Arms a one-shot readiness watch. kqueue's EV_ADD silently updates a duplicate
   instead of failing, so the already-armed fault is reproduced by probing the
   shared registry first (see cool_reg_probe): a live entry returns 1, the
   EEXIST class the shared arm turns into "already has an armed watch".

   A regular file is then rejected up front with an fstat, returning the code 2
   the shared arm turns into "a regular file cannot report readiness". This keeps
   parity with the epoll backend, which gets that fault for free because
   EPOLL_CTL_ADD on a regular file returns EPERM. kqueue would instead accept the
   registration and report the vnode permanently ready, so without this check a
   regular file would silently complete-ready on BSD where it faults on Linux;
   the fstat closes that divergence rather than documenting it.

   Then EV_ADD | EV_ONESHOT registers the read or write filter carrying watch as
   udata, firing exactly once. With the regular-file case handled above, any
   kevent failure now signals a genuine bad descriptor or internal filter error
   (EBADF, ENOENT, EINVAL, and the rest), all mapped to code 3, the invalid-fd
   fault; a clean EV_ADD -> 0. */
int cool_poller_arm(struct cool_poller *p, int fd, int for_write, void *watch) {
    if (cool_reg_probe(fd)) {
        return 1;
    }
    struct stat st;
    if (fstat(fd, &st) == 0 && S_ISREG(st.st_mode)) {
        return 2;
    }
    struct kevent kev;
    EV_SET(&kev, fd, for_write ? EVFILT_WRITE : EVFILT_READ, EV_ADD | EV_ONESHOT, 0, 0, watch);
    if (kevent(p->kq, &kev, 1, NULL, 0, NULL) < 0) {
        return 3;
    }
    return 0;
}

/* Disarms fd's watch. The one-shot filter is already gone by the time the fire
   path runs (EV_ONESHOT auto-removes on fire, and closing an fd auto-removes
   its kevents), and the seam does not carry which filter (read vs write) was
   armed, so this deletes both filters and tolerates the ENOENT each already
   returns. The shared fire path only calls disarm when the registry still maps
   this fd to this watch, so a reused fd's successor kevent is never touched. */
void cool_poller_disarm(struct cool_poller *p, int fd) {
    struct kevent kev;
    EV_SET(&kev, fd, EVFILT_READ, EV_DELETE, 0, 0, NULL);
    kevent(p->kq, &kev, 1, NULL, 0, NULL);
    EV_SET(&kev, fd, EVFILT_WRITE, EV_DELETE, 0, 0, NULL);
    kevent(p->kq, &kev, 1, NULL, 0, NULL);
}

/* Waits for readiness into out[0..max). block != 0 passes a NULL timeout (wait
   forever); block == 0 passes a zero timespec (drain what is already ready).
   EINTR is retried internally; a non-EINTR failure returns -1. An EVFILT_USER
   event is the wake sentinel (is_stop); otherwise the filter and flags are
   translated into the normalized mask, with EV_ERROR's data errno folded into
   the ERR bit. */
int cool_poller_wait(struct cool_poller *p, cool_event *out, int max, int block) {
    int cap = max < COOL_KQ_BATCH ? max : COOL_KQ_BATCH;
    struct kevent evs[COOL_KQ_BATCH];
    struct timespec zero = {0, 0};
    struct timespec *ts = block ? NULL : &zero;
    int n;
    do {
        n = kevent(p->kq, NULL, 0, evs, cap, ts);
    } while (n < 0 && errno == EINTR);
    if (n < 0) {
        return -1;
    }
    for (int i = 0; i < n; i++) {
        if (evs[i].filter == EVFILT_USER) {
            out[i].watch = NULL;
            out[i].mask = 0;
            out[i].is_stop = 1;
        } else {
            int64_t mask = 0;
            if (evs[i].filter == EVFILT_READ) {
                mask |= COOL_EV_READ;
            }
            if (evs[i].filter == EVFILT_WRITE) {
                mask |= COOL_EV_WRITE;
            }
            if (evs[i].flags & EV_EOF) {
                mask |= COOL_EV_HUP;
            }
            if (evs[i].flags & EV_ERROR) {
                /* evs[i].data carries the errno on an EV_ERROR event; it folds
                   into the single ERR bit the normalized mask exposes. */
                mask |= COOL_EV_ERR;
            }
            out[i].watch = evs[i].udata;
            out[i].mask = mask;
            out[i].is_stop = 0;
        }
    }
    return n;
}

/* Fires the EVFILT_USER wake sentinel with NOTE_TRIGGER so a blocked
   cool_poller_wait returns an is_stop event. Returns 0 on success and 1 on
   failure, so the shared stop can fault a sentinel that could not be
   signalled, matching the epoll wake contract. */
int cool_poller_wake(struct cool_poller *p) {
    struct kevent kev;
    EV_SET(&kev, COOL_KQ_STOP_IDENT, EVFILT_USER, 0, NOTE_TRIGGER, 0, NULL);
    return kevent(p->kq, &kev, 1, NULL, 0, NULL) < 0 ? 1 : 0;
}

#endif /* BSD/macOS */
