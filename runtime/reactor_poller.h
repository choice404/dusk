/* The readiness reactor's kernel-poller seam. The portable reactor core in
   reactor.c owns the watch records, the fd->watch registry, the armed gauge
   and the lifecycle state machine; it reaches the kernel's readiness machinery
   only through the six functions declared here. One backend translation unit
   defines them: the epoll backend on Linux (reactor_epoll.c), the kqueue
   backend on the BSDs and macOS (reactor_kqueue.c, 0.4.4 M2). The seam moves
   no lock boundary and keeps no state the core cannot see, so swapping the
   backend cannot change the reactor's observable behavior.

   struct cool_poller is opaque to the core: the core embeds one by value and
   passes its address to every seam call, but only the backend that is compiled
   in knows its fields (an epoll fd plus an eventfd stop sentinel on Linux, a
   kqueue plus a stop ident on the BSDs). Its layout is fixed per platform at
   the bottom of this header so the core can size the embedded field. */
#ifndef COOL_REACTOR_POLLER_H
#define COOL_REACTOR_POLLER_H

#include <stdint.h>

/* Backend-defined; completed per platform at the bottom of this header. */
struct cool_poller;

/* Normalized readiness mask bits, chosen so a kqueue port lands on the same
   values an epoll fire already produces. The backend translates its native
   bits into these before handing an event to the core. */
#define COOL_EV_READ 1
#define COOL_EV_WRITE 2
#define COOL_EV_HUP 4
#define COOL_EV_ERR 8

/* One readiness event as the core sees it, native bits already normalized.
   watch is the token the arm handed the kernel (a cool_watch *, opaque here);
   mask is the COOL_EV_* set; is_stop is one for the wake sentinel, in which
   case watch is NULL and mask is zero. */
typedef struct {
    void *watch;
    int64_t mask;
    int is_stop;
} cool_event;

/* Creates the poller and its wake sentinel. Returns 0 on success, 1 on any
   failure, leaving no descriptor half open on the failure path. */
int cool_poller_create(struct cool_poller *p);

/* Closes every descriptor the poller owns. Idempotent-safe: a second call over
   already-closed descriptors is harmless. */
void cool_poller_destroy(struct cool_poller *p);

/* Arms a one-shot readiness watch on fd (for_write picks write vs read
   readiness), carrying watch as the token the fire will return. Returns 0 on
   success, or a class code the backend maps from its errno: 1 for the
   already-armed case (EEXIST), 2 for a regular file that cannot report
   readiness (EPERM), 3 for anything else (a bad fd and the rest). */
int cool_poller_arm(struct cool_poller *p, int fd, int for_write, void *watch);

/* Disarms fd's watch. Result ignored: a watch the kernel already dropped
   because its fd was closed is the tolerated already-gone case. */
void cool_poller_disarm(struct cool_poller *p, int fd);

/* Waits for readiness into out[0..max), returning the event count (>= 0) or -1
   on a non-EINTR failure. block != 0 waits forever; block == 0 is a zero
   timeout poll that drains whatever is already ready. EINTR is retried inside.
   Each out entry carries a normalized mask, and the wake sentinel sets is_stop
   (draining its own sentinel state so a level-triggered wake cannot spin). */
int cool_poller_wait(struct cool_poller *p, cool_event *out, int max, int block);

/* Fires the wake sentinel so a blocked cool_poller_wait returns with an
   is_stop event. Returns 0 on success and nonzero on failure, so the shared
   stop path can fault a sentinel that could not be signalled. */
int cool_poller_wake(struct cool_poller *p);

/* Shared registry probe, defined in the portable core (reactor.c) and called
   with the reactor mutex held. Returns nonzero when fd already has a live
   watch. ONLY the kqueue backend calls it: EV_ADD silently updates an existing
   registration, so kqueue must probe to reproduce the already-armed fault that
   epoll gets from EPOLL_CTL_ADD's EEXIST. The epoll backend never calls it, so
   the Linux path is unchanged. */
int cool_reg_probe(int fd);

/* --- Per-platform poller layout ----------------------------------------- */

#if defined(__linux__)

/* The epoll backend: an epoll fd and an eventfd registered with a NULL
   data.ptr as the wake sentinel. */
struct cool_poller {
    int epfd;
    int stopfd;
};

#elif defined(__APPLE__) || defined(__FreeBSD__) || defined(__NetBSD__) || defined(__OpenBSD__)

/* The kqueue backend layout: just the kqueue descriptor. The wake sentinel is
   a fixed EVFILT_USER ident (0), a constant in reactor_kqueue.c, so it needs no
   stored field. */
struct cool_poller {
    int kq;
};

#else

#error "unsupported platform: no reactor poller backend"

#endif

#endif /* COOL_REACTOR_POLLER_H */
