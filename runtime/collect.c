/* Conservative mark and sweep collected heap. A second managed region beside
   the generational heap: a collected block carries the same 16 byte header as a
   generational one, [i64 size][i64 gen][payload], so the generational
   dereference check reads a collected block's generation word exactly as it
   reads a generational block's, and the two layers stay byte compatible. What
   differs is reclamation. A generational block is retired by an explicit free
   that bumps its generation and parks it; a collected block is reclaimed only
   by a collection, which conservatively scans the roots, marks what is
   reachable, and sweeps the rest.

   The collector is single mutator. It runs only on the anchor thread, the main
   thread the event loop is also confined to, so a collection point is a point
   where the collecting thread's own stack is stable and no safepoint handshake
   with other threads is needed. An allocation or a collection asked for off the
   anchor thread is fatal by name. The collected free list is fully isolated
   from the generational one: a collected address never enters the generational
   free list and a generational address never enters this one.

   Soundness rests on an invariant the runtime cannot enforce alone: a collected
   reference stays confined to the anchor thread's reachable roots. The root set
   is the anchor thread stack, the anchor thread register spill, the generational
   registry, and the registered task and environment regions. A collected
   reference stored where none of those reach is invisible and can be swept while
   live. The stores outside the root set are a worker thread stack, a spawned or
   pool environment block, a channel ring buffer, and a raw untracked allocation.
   The same thread awaitable channel is the sharpest of these: a collected
   reference sent then received sits only in the malloc'd ring with no root, so a
   collection between the send and the receive sweeps it. The floor mints
   collected blocks only through the foreign boundary, so nothing yet reaches
   these stores; the surface checker that lands the collector type must ban them.
   The channel ban must be a type ban, rejecting a channel whose element is a
   collected type, since a same thread channel evades any liveness argument. */
#include <pthread.h>
#include <setjmp.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The header contract shared with the generational heap: payload sits 16 bytes
   past the malloc base, the size word at base and the generation word at base
   plus 8, so payload minus 8 is the generation the dereference check reads. */
#define COOL_GC_HDR 16

/* A gen payload snapshot the collector scans as a root region, provided by the
   generational heap under its own lock and released before the scan. */
extern int64_t cool_gen_registry_snapshot(void ***ptrs, int64_t **sizes);

static pthread_mutex_t gc_lock = PTHREAD_MUTEX_INITIALIZER;

/* One descriptor per collected block, in append order. base is the malloc base,
   not the payload. A dead descriptor keeps its block mapped and parked for
   reuse, never returned to libc, so its advanced generation survives and a
   stale managed reference to it still faults. */
typedef struct {
    char *base;
    int64_t size;
    uint8_t mark;
    uint8_t live;
} gc_desc;

static gc_desc *gc_descs = NULL;
static int64_t gc_ndesc = 0;
static int64_t gc_cap = 0;

/* Reuse list of dead descriptor indices, size matched on pop so a reused block
   keeps its own advanced generation rather than resetting it. */
static int64_t *gc_free = NULL;
static int64_t gc_free_n = 0;
static int64_t gc_free_cap = 0;

/* Live descriptor indices sorted by base, rebuilt at the start of every
   collection. The address test binary searches this array. */
static int64_t *gc_sorted = NULL;
static int64_t gc_sorted_n = 0;
static int64_t gc_sorted_cap = 0;

/* Explicit mark worklist of descriptor indices, so transitive marking never
   recurses. A block is pushed at most once because the mark bit guards it. */
static int64_t *gc_work = NULL;
static int64_t gc_work_n = 0;
static int64_t gc_work_cap = 0;

static int64_t gc_bytes_since = 0;
static int64_t gc_threshold = 256 * 1024;
static int64_t gc_live_bytes = 0;
static int64_t gc_collections = 0;

static char *gc_anchor_sp = NULL;
static pthread_t gc_anchor_thread;
static int gc_anchored = 0;

/* Set once when the first collected block is minted and never cleared, since
   descriptors are append only. The free guard reads it without the lock: a heap
   with no collected block can skip the address test entirely. */
static int gc_has_blocks = 0;

/* Extra root regions the async substrate registers: task frames and closure
   environment blocks, which live outside the generational registry. */
typedef struct gc_region {
    char *base;
    int64_t len;
    struct gc_region *next;
} gc_region;

static gc_region *gc_regions = NULL;

static void gc_oom(void) {
    fflush(stdout);
    fputs("fatal: out of memory\n", stderr);
    abort();
}

static void gc_off_thread(void) {
    fflush(stdout);
    fputs("fatal: the collector runs on the main thread only\n", stderr);
    abort();
}

/* Records the main thread stack high water, once. Emitted main calls this as
   its first instruction, so the first call names the top of the outermost frame
   on the true main thread, and the scan runs from a collection point up to it.
   The anchor is set once: main is an ordinary dusk function, so it can recurse
   or be spawned, and a later call from an inner frame or another thread would
   lower the high water and drop the outer frame out of every scan, sweeping a
   live block. A second call is a no op. */
void cool_gc_anchor(void *p) {
    pthread_mutex_lock(&gc_lock);
    if (!gc_anchored) {
        gc_anchor_sp = (char *)p;
        gc_anchor_thread = pthread_self();
        gc_anchored = 1;
    }
    pthread_mutex_unlock(&gc_lock);
}

static int gc_on_anchor(void) {
    return gc_anchored && pthread_equal(pthread_self(), gc_anchor_thread);
}

/* --- descriptor and side table growth ------------------------------------ */

static gc_desc *gc_desc_new(char *base, int64_t size) {
    if (gc_ndesc == gc_cap) {
        int64_t ncap = gc_cap ? gc_cap * 2 : 64;
        gc_desc *nd = (gc_desc *)realloc(gc_descs, (size_t)ncap * sizeof(gc_desc));
        if (!nd) {
            gc_oom();
        }
        gc_descs = nd;
        gc_cap = ncap;
    }
    gc_desc *d = &gc_descs[gc_ndesc];
    d->base = base;
    d->size = size;
    d->mark = 0;
    d->live = 1;
    __atomic_store_n(&gc_ndesc, gc_ndesc + 1, __ATOMIC_SEQ_CST);
    __atomic_store_n(&gc_has_blocks, 1, __ATOMIC_RELEASE);
    return d;
}

static void gc_free_push(int64_t di) {
    if (gc_free_n == gc_free_cap) {
        int64_t ncap = gc_free_cap ? gc_free_cap * 2 : 64;
        int64_t *nf = (int64_t *)realloc(gc_free, (size_t)ncap * sizeof(int64_t));
        if (!nf) {
            gc_oom();
        }
        gc_free = nf;
        gc_free_cap = ncap;
    }
    gc_free[gc_free_n++] = di;
}

static void gc_work_push(int64_t di) {
    if (gc_work_n == gc_work_cap) {
        int64_t ncap = gc_work_cap ? gc_work_cap * 2 : 64;
        int64_t *nw = (int64_t *)realloc(gc_work, (size_t)ncap * sizeof(int64_t));
        if (!nw) {
            gc_oom();
        }
        gc_work = nw;
        gc_work_cap = ncap;
    }
    gc_work[gc_work_n++] = di;
}

/* --- address test -------------------------------------------------------- */

/* Linear ownership test over every descriptor, used by the free guard. A word v
   is collector owned when it lands in any descriptor's payload, live or dead.
   Dead descriptors matter: a collected block is never returned to libc, only
   parked for reuse, so its address stays owned by the collector after a sweep.
   Testing liveness here would be a hole, since a stale reference to a just swept
   block would read as not collected and the generational free path would park
   it, aliasing the collected address into the generational free list. The end
   is inclusive so a slice pointer one past the last element still resolves. */
static int gc_owns_locked(void *vp) {
    uintptr_t v = (uintptr_t)vp;
    for (int64_t i = 0; i < gc_ndesc; i++) {
        uintptr_t start = (uintptr_t)gc_descs[i].base + COOL_GC_HDR;
        uintptr_t end = start + (uintptr_t)gc_descs[i].size;
        if (v >= start && v <= end) {
            return 1;
        }
    }
    return 0;
}

static int gc_cmp_idx(const void *a, const void *b) {
    char *ba = gc_descs[*(const int64_t *)a].base;
    char *bb = gc_descs[*(const int64_t *)b].base;
    if (ba < bb) {
        return -1;
    }
    if (ba > bb) {
        return 1;
    }
    return 0;
}

/* Binary search of the sorted live descriptors for the block containing v,
   returning its descriptor index or -1. Blocks do not overlap, so the largest
   payload start not above v is the only candidate. Adjacent blocks can share a
   boundary address; resolving it to either is safe, since a false hit only over
   retains. */
static int64_t gc_find(void *vp) {
    uintptr_t v = (uintptr_t)vp;
    int64_t lo = 0, hi = gc_sorted_n - 1, ans = -1;
    while (lo <= hi) {
        int64_t mid = lo + (hi - lo) / 2;
        uintptr_t start = (uintptr_t)gc_descs[gc_sorted[mid]].base + COOL_GC_HDR;
        if (start <= v) {
            ans = mid;
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    if (ans < 0) {
        return -1;
    }
    int64_t di = gc_sorted[ans];
    uintptr_t start = (uintptr_t)gc_descs[di].base + COOL_GC_HDR;
    uintptr_t end = start + (uintptr_t)gc_descs[di].size;
    if (v >= start && v <= end) {
        return di;
    }
    return -1;
}

/* Marks the block a word points into, if any, and pushes it for transitive
   marking. Idempotent: a block already marked is skipped, which bounds the
   worklist and terminates the scan. */
static void gc_mark_word(void *vp) {
    int64_t di = gc_find(vp);
    if (di < 0 || gc_descs[di].mark) {
        return;
    }
    gc_descs[di].mark = 1;
    gc_work_push(di);
}

/* Conservative word aligned scan of a byte range. lo is rounded up to the word
   size and each aligned word is read as a candidate pointer. Every stack and
   payload pointer is word aligned by the ABI, so the rounding never skips a
   real root, and reading only whole words that fit keeps the scan in bounds. */
static void gc_scan_range(char *lo, char *hi) {
    uintptr_t a = ((uintptr_t)lo + (sizeof(void *) - 1)) & ~(uintptr_t)(sizeof(void *) - 1);
    char *p = (char *)a;
    while (p + sizeof(void *) <= hi) {
        void *v;
        memcpy(&v, p, sizeof(void *));
        gc_mark_word(v);
        p += sizeof(void *);
    }
}

/* --- the collection ------------------------------------------------------ */

/* Runs one collection with the collector lock held. The caller has already
   asserted the anchor thread, so the scan reads a stable stack. */
static void gc_collect_locked(void) {
    /* Build the sorted live index and clear marks. */
    if (gc_sorted_cap < gc_ndesc) {
        int64_t ncap = gc_ndesc;
        int64_t *ns = (int64_t *)realloc(gc_sorted, (size_t)ncap * sizeof(int64_t));
        if (!ns) {
            gc_oom();
        }
        gc_sorted = ns;
        gc_sorted_cap = ncap;
    }
    gc_sorted_n = 0;
    for (int64_t i = 0; i < gc_ndesc; i++) {
        gc_descs[i].mark = 0;
        if (gc_descs[i].live) {
            gc_sorted[gc_sorted_n++] = i;
        }
    }
    qsort(gc_sorted, (size_t)gc_sorted_n, sizeof(int64_t), gc_cmp_idx);
    gc_work_n = 0;

    /* Root set. A register spill through setjmp catches a root held only in a
       callee saved register; the stack scan from a collection point up to the
       anchor catches roots on the stack; the generational registry snapshot
       and the registered regions catch roots on the managed and substrate
       heaps. Marks push the worklist for the transitive close. */
    jmp_buf env;
    memset(&env, 0, sizeof(env));
    (void)setjmp(env);
    gc_scan_range((char *)&env, (char *)&env + sizeof(env));

    char sp_marker;
    char *sp = &sp_marker;
    char *lo = sp < gc_anchor_sp ? sp : gc_anchor_sp;
    char *hi = sp < gc_anchor_sp ? gc_anchor_sp : sp;
    gc_scan_range(lo, hi);

    void **snap_ptr = NULL;
    int64_t *snap_sz = NULL;
    int64_t snap_n = cool_gen_registry_snapshot(&snap_ptr, &snap_sz);
    for (int64_t i = 0; i < snap_n; i++) {
        char *pl = (char *)snap_ptr[i];
        gc_scan_range(pl, pl + snap_sz[i]);
    }
    free(snap_ptr);
    free(snap_sz);

    for (gc_region *r = gc_regions; r; r = r->next) {
        gc_scan_range(r->base, r->base + r->len);
    }

    /* Transitive close: drain the worklist, scanning each marked block's
       payload for further collected references. */
    while (gc_work_n > 0) {
        int64_t di = gc_work[--gc_work_n];
        char *pl = gc_descs[di].base + COOL_GC_HDR;
        gc_scan_range(pl, pl + gc_descs[di].size);
    }

    /* Sweep. A live but unmarked block is unreachable: bump its generation so a
       stale managed reference faults, mark it dead, and park it for reuse. The
       generation bump is atomic because the dereference check reads that word
       without any lock from any thread. The block is never returned to libc. */
    for (int64_t i = 0; i < gc_ndesc; i++) {
        if (gc_descs[i].live && !gc_descs[i].mark) {
            int64_t *gen = (int64_t *)(gc_descs[i].base + 8);
            __atomic_fetch_add(gen, 1, __ATOMIC_SEQ_CST);
            gc_descs[i].live = 0;
            gc_live_bytes -= gc_descs[i].size;
            gc_free_push(i);
        }
    }

    int64_t twice = gc_live_bytes * 2;
    gc_threshold = twice > (256 * 1024) ? twice : (256 * 1024);
    gc_bytes_since = 0;
    gc_collections++;
}

/* --- public allocation and collection ------------------------------------ */

/* Mints a collected block. Off the anchor thread it is fatal. A collection runs
   first when the allocation debt has reached the threshold. A size matched dead
   block is reused with its advanced generation intact, otherwise a fresh block
   is minted with generation one. Returns the payload, 16 bytes past the base. */
void *cool_collect_alloc(int64_t size) {
    if (!gc_on_anchor()) {
        gc_off_thread();
    }
    if (size < 0) {
        size = 0;
    }
    pthread_mutex_lock(&gc_lock);
    if (gc_bytes_since >= gc_threshold) {
        gc_collect_locked();
    }
    char *base = NULL;
    for (int64_t i = 0; i < gc_free_n; i++) {
        int64_t di = gc_free[i];
        if (gc_descs[di].size == size) {
            base = gc_descs[di].base;
            gc_descs[di].live = 1;
            gc_descs[di].mark = 0;
            gc_free[i] = gc_free[--gc_free_n];
            break;
        }
    }
    if (!base) {
        base = (char *)malloc(COOL_GC_HDR + (size_t)size);
        if (!base) {
            pthread_mutex_unlock(&gc_lock);
            gc_oom();
        }
        int64_t *hdr = (int64_t *)base;
        hdr[0] = size;
        __atomic_store_n(&hdr[1], 1, __ATOMIC_SEQ_CST);
        gc_desc_new(base, size);
    }
    gc_bytes_since += size;
    gc_live_bytes += size;
    pthread_mutex_unlock(&gc_lock);
    return base + COOL_GC_HDR;
}

/* Forces a collection. Off the anchor thread it is fatal. */
void cool_gc_collect(void) {
    if (!gc_on_anchor()) {
        gc_off_thread();
    }
    pthread_mutex_lock(&gc_lock);
    gc_collect_locked();
    pthread_mutex_unlock(&gc_lock);
}

/* The free guard the generational heap consults before retiring a pointer. A
   collected address must never enter the generational free list, and this
   answers whether an address is owned by the collector, live or already swept,
   since the collector never returns a block to libc. The unlocked has blocks
   read skips the lock and the test on a heap that never minted a collected
   block, the common case, so the guard costs almost nothing there. */
int cool_gc_is_collected(void *p) {
    if (!__atomic_load_n(&gc_has_blocks, __ATOMIC_ACQUIRE)) {
        return 0;
    }
    pthread_mutex_lock(&gc_lock);
    int r = gc_owns_locked(p);
    pthread_mutex_unlock(&gc_lock);
    return r;
}

/* --- root regions -------------------------------------------------------- */

void cool_gc_add_region(void *base, int64_t len) {
    gc_region *r = (gc_region *)malloc(sizeof(gc_region));
    if (!r) {
        gc_oom();
    }
    r->base = (char *)base;
    r->len = len < 0 ? 0 : len;
    pthread_mutex_lock(&gc_lock);
    r->next = gc_regions;
    gc_regions = r;
    pthread_mutex_unlock(&gc_lock);
}

void cool_gc_del_region(void *base) {
    pthread_mutex_lock(&gc_lock);
    gc_region **pp = &gc_regions;
    while (*pp) {
        if ((*pp)->base == (char *)base) {
            gc_region *dead = *pp;
            *pp = dead->next;
            free(dead);
            break;
        }
        pp = &(*pp)->next;
    }
    pthread_mutex_unlock(&gc_lock);
}

/* --- statistics ---------------------------------------------------------- */

int64_t cool_gc_live_blocks(void) {
    pthread_mutex_lock(&gc_lock);
    int64_t n = 0;
    for (int64_t i = 0; i < gc_ndesc; i++) {
        if (gc_descs[i].live) {
            n++;
        }
    }
    pthread_mutex_unlock(&gc_lock);
    return n;
}

int64_t cool_gc_live_bytes(void) {
    pthread_mutex_lock(&gc_lock);
    int64_t n = gc_live_bytes;
    pthread_mutex_unlock(&gc_lock);
    return n;
}

int64_t cool_gc_collections(void) {
    pthread_mutex_lock(&gc_lock);
    int64_t n = gc_collections;
    pthread_mutex_unlock(&gc_lock);
    return n;
}
