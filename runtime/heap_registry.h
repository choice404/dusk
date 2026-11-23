/* The generational heap's block registry and its counters, included by runtime.c
   after the site prefix. Not a standalone header: it reads the heap lock and the
   header layout from the file that includes it, and every function here but the
   snapshot is called with the heap lock held.

   An AddressSanitizer build puts two more words in front of a block, ahead of the
   site words of a --heap-sites build, so every word the emitted IR and the site
   report read stays where it was:

       address            [slot][flags][size][gen] payload                COOL_GEN_PRE = 16
       address + sites    [slot][flags][site][chain][size][gen] payload   COOL_GEN_PRE = 32

   slot is the block's index in the registry, which a real free needs to remove
   it, and flags marks a runtime record, see heap_asan.h. Both prefixes are
   multiples of sixteen bytes, so the payload keeps the sixteen byte alignment
   malloc hands back. The mint is the one place that steps from the address malloc
   handed back to the payload, and COOL_GEN_BASE is the one inverse of it. */

#ifdef COOL_ASAN
#define COOL_ASAN_PRE 16
#else
#define COOL_ASAN_PRE 0
#endif

#define COOL_GEN_PRE (COOL_SITE_PRE + COOL_ASAN_PRE)

/* The address malloc handed back for a block, from its payload. */
#define COOL_GEN_BASE(p) ((char *)(p) - COOL_GEN_HDR - COOL_GEN_PRE)

#ifdef COOL_ASAN
#define COOL_GEN_SLOT_WORD(p) ((int64_t *)COOL_GEN_BASE(p))
#define COOL_GEN_FLAGS_WORD(p) ((int64_t *)(COOL_GEN_BASE(p) + 8))
#endif

#ifdef COOL_ASAN
/* A break in the heap's own bookkeeping, which no dusk program can cause through
   the language, named rather than left to corrupt the heap further. */
static void cool_heap_internal(const char *what) {
    fflush(stdout);
    fprintf(stderr, "fatal: internal error: %s\n", what);
    abort();
}
#endif

/* Block registry. One entry per generational block the heap holds, with its
   payload capacity, so the collector can scan every live generational block as a
   root region.

   In every build but AddressSanitizer's the table is append only. A block's
   address never goes back to libc and its class never changes, so a block's
   address and capacity are fixed for the life of the process and one entry
   describes it forever. What changes across reuse is the size of the payload the
   current tenant asked for, and that lives in the block's own size word, as does
   liveness: the parked bit says it. So a mint appends, a reuse touches nothing,
   and a free touches nothing.

   The collector copies a snapshot under the lock, keeping only the live
   entries, and scans it released, so a block another thread frees mid scan is
   still mapped, parked not returned, and reads only over retain. Under
   AddressSanitizer the program's blocks really go back to libc and a free removes
   the entry, so there the scan holds the free gate of heap_asan.h from before the
   snapshot to after the scan, and no block the snapshot names is freed under it. */
static void **cool_reg_ptr = NULL;
static int64_t *cool_reg_room = NULL;
static int64_t cool_reg_n = 0;
static int64_t cool_reg_cap = 0;

/* The gauge's counters: live blocks, the bytes their tenants asked for, and the
   capacity of every block minted, live or parked. Written only with the heap
   lock held, on paths that already take it; heap_gauge.h reads them. */
static int64_t cool_heap_blocks = 0;
static int64_t cool_heap_bytes = 0;
static int64_t cool_heap_held = 0;

/* Appends a freshly minted payload and its capacity with the heap lock held.
   Growth failure is out of memory, fatal, since dropping an entry would under
   root the collector and free a still reachable block. */
static void cool_reg_add_locked(void *p, int64_t room) {
    if (cool_reg_n == cool_reg_cap) {
        int64_t ncap = cool_reg_cap ? cool_reg_cap * 2 : 128;
        void **np = realloc(cool_reg_ptr, (size_t)ncap * sizeof(void *));
        if (np) {
            cool_reg_ptr = np;
        }
        int64_t *nr = realloc(cool_reg_room, (size_t)ncap * sizeof(int64_t));
        if (nr) {
            cool_reg_room = nr;
        }
        if (!np || !nr) {
            fflush(stdout);
            fputs("fatal: out of memory\n", stderr);
            abort();
        }
        cool_reg_cap = ncap;
    }
    cool_reg_ptr[cool_reg_n] = p;
    cool_reg_room[cool_reg_n] = room;
#ifdef COOL_ASAN
    *COOL_GEN_SLOT_WORD(p) = cool_reg_n;
#endif
    cool_reg_n++;
}

#ifdef COOL_ASAN
/* Removes a block's entry with the heap lock held, by moving the last entry into
   its slot and rewriting the moved block's slot word, so a removal is constant
   time. An entry that does not name the block is the heap's own bookkeeping gone
   wrong, never something a program did, and is fatal by name rather than a
   registry that silently drops a live root. */
static void cool_reg_del_locked(void *p) {
    int64_t i = *COOL_GEN_SLOT_WORD(p);
    int64_t last = cool_reg_n - 1;
    if (i < 0 || i > last || cool_reg_ptr[i] != p) {
        cool_heap_internal("the heap registry does not hold a block being freed");
    }
    if (i != last) {
        cool_reg_ptr[i] = cool_reg_ptr[last];
        cool_reg_room[i] = cool_reg_room[last];
        *COOL_GEN_SLOT_WORD(cool_reg_ptr[i]) = i;
    }
    cool_reg_n = last;
}
#endif

/* The byte count the collector scans in a live block: the size its current
   tenant asked for, read from the header under the heap lock, since a block
   reused by a larger request in its class holds live words past the size it was
   minted at. The capacity bounds it, so a scribbled header can never widen the
   scan past the block, and a size the header cannot hold reads as the whole
   capacity, over retaining rather than dropping a root. */
static int64_t cool_reg_scan_len_locked(void *p, int64_t room) {
    int64_t size = *COOL_GEN_SIZE_WORD(p);
    if (size < 0 || size > room) {
        return room;
    }
    return size;
}

/* Copies the live registry for the collector. Allocates both arrays, which the
   caller frees, and returns the count. Taken under the heap lock so the copy is
   consistent, then released before the collector scans it. */
int64_t cool_gen_registry_snapshot(void ***ptrs, int64_t **sizes) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = cool_reg_n;
    void **pp = malloc((size_t)(n > 0 ? n : 1) * sizeof(void *));
    int64_t *ss = malloc((size_t)(n > 0 ? n : 1) * sizeof(int64_t));
    if (!pp || !ss) {
        // A short snapshot would drop roots and let the collector sweep a block
        // still reachable through a generational block, so an incomplete
        // snapshot is fatal rather than an empty root set the sweep proceeds on.
        free(pp);
        free(ss);
        pthread_mutex_unlock(&cool_heap_lock);
        fflush(stdout);
        fputs("fatal: out of memory\n", stderr);
        abort();
    }
    // A parked block is not a root: its payload is whatever the program left
    // there before the free, and scanning it would mint references out of dead
    // bytes. The parked bit and the size word are set and cleared under this
    // same lock, so the live set and the lengths the copy names are the ones at
    // the moment of the copy.
    int64_t m = 0;
    for (int64_t i = 0; i < n; i++) {
        void *p = cool_reg_ptr[i];
        if (*COOL_GEN_SIZE_WORD(p) & COOL_GEN_FREED_TAG) {
            continue;
        }
        pp[m] = p;
        ss[m] = cool_reg_scan_len_locked(p, cool_reg_room[i]);
        m++;
    }
    pthread_mutex_unlock(&cool_heap_lock);
    *ptrs = pp;
    *sizes = ss;
    return m;
}

