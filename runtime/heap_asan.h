/* The generational heap under AddressSanitizer, included by runtime.c after the
   registry. Not a standalone header: it reads the heap lock, the header layout, the
   registry, and the counters from the files that include it.

   A --sanitize=address build compiles the runtime with -fsanitize=address, and
   COOL_ASAN, the compiler's own sign of that, turns the program's heap from one
   that parks a freed block for reuse into one that hands it back to libc. A parked
   block is still mapped, so AddressSanitizer never sees a use after free of one or
   a read past the size a tenant asked for inside its class. Here an allocation is
   an exact malloc and a free is a real free, so it sees both, and a second free is
   its heap use after free report on the free's first read of the block, with the
   stacks of both frees.

   Generations come from one process wide counter that starts at 1 and never
   repeats, so a block minted at an address a freed block held starts above every
   generation that address carried and a stale reference still mismatches. Once
   libc hands the address to something that is not a generational block, a stale
   dereference reads whatever word sits eight bytes in front of it; in this build
   AddressSanitizer's quarantine and its report on that load are the guard, not the
   generation.

   The runtime's own generation checked records, a future and a thread's join
   record, stay on the parking path in this build too, flagged in the prefix and
   never removed from the registry: a late complete and a second join read a
   retired record's generation as defined behavior, so a record must stay mapped.
   Each free path refuses the other kind by name.

   The collector scans the registry's blocks with no lock held, so a real free
   holds the free gate across its registry removal and its free(), and the
   collector holds the same gate from before its snapshot to after its scan. The
   lock order is collector, gate, heap: a free takes the collector's lock for its
   guard and releases it before the gate, and nothing takes them the other way.

   A build without the sanitizer compiles none of this but the two empty gate
   functions the collector calls and the first generation of a mint, which is 1. */

/* The flags bit of a runtime record. */
#define COOL_GEN_RECORD ((int64_t)1)

/* The collector's side of the free gate. Called with the collector lock held,
   around the registry snapshot and the scan that reads it. */
void cool_gen_scan_begin(void);
void cool_gen_scan_end(void);

#ifdef COOL_ASAN

static pthread_mutex_t cool_free_gate = PTHREAD_MUTEX_INITIALIZER;
static int64_t cool_gen_counter = 0;

void cool_gen_scan_begin(void) {
    pthread_mutex_lock(&cool_free_gate);
}

void cool_gen_scan_end(void) {
    pthread_mutex_unlock(&cool_free_gate);
}

/* The first generation of a minted block, the counter's next value. */
static int64_t cool_gen_first(void) {
    return __atomic_add_fetch(&cool_gen_counter, 1, __ATOMIC_SEQ_CST);
}

/* Marks a block minted on the parking path as a runtime record. Only records
   are minted there in this build. */
static void cool_gen_mark_record(void *p) {
    *COOL_GEN_FLAGS_WORD(p) = COOL_GEN_RECORD;
}

/* Refuses a block of the program's heap on a path for runtime records, with the
   heap lock held. */
static void cool_gen_record_check_locked(void *p) {
    if ((*COOL_GEN_FLAGS_WORD(p) & COOL_GEN_RECORD) == 0) {
        pthread_mutex_unlock(&cool_heap_lock);
        cool_heap_internal("the runtime retired a block the program allocated");
    }
}

/* Allocates a block of the program's heap: exactly the bytes asked for behind the
   prefix and the header, so AddressSanitizer's redzone starts at the first byte
   past the request, never parked and never taken from a class list. */
static void *cool_asan_alloc(int64_t size) {
    if (size < 0 || size >= COOL_GEN_SIZE_MAX) {
        return NULL;
    }
    char *base = malloc(COOL_GEN_PRE + COOL_GEN_HDR + (size_t)size);
    if (!base) {
        return NULL;
    }
    void *q = base + COOL_GEN_PRE + COOL_GEN_HDR;
    *COOL_GEN_FLAGS_WORD(q) = 0;
    *COOL_GEN_SIZE_WORD(q) = size;
    pthread_mutex_lock(&cool_heap_lock);
    __atomic_store_n(COOL_GEN_GEN_WORD(q), cool_gen_first(), __ATOMIC_SEQ_CST);
    cool_site_record_locked(q);
    cool_reg_add_locked(q, size);
    cool_heap_blocks += 1;
    cool_heap_bytes += size;
    cool_heap_held += size;
    pthread_mutex_unlock(&cool_heap_lock);
    return q;
}

/* Frees a block of the program's heap back to libc, after the null, constant, and
   collected guards of the caller. The flags word is the first read of the block's
   own memory, so a block freed before is reported by AddressSanitizer right here.
   The generation and the string stamp still move, so a stale reference that reads
   the word before libc reuses the address still mismatches. */
static void cool_asan_free(void *p) {
    pthread_mutex_lock(&cool_free_gate);
    pthread_mutex_lock(&cool_heap_lock);
    if (*COOL_GEN_FLAGS_WORD(p) & COOL_GEN_RECORD) {
        pthread_mutex_unlock(&cool_heap_lock);
        pthread_mutex_unlock(&cool_free_gate);
        cool_heap_internal("a free of a block the runtime owns");
    }
    int64_t size = *COOL_GEN_SIZE_WORD(p);
    __atomic_fetch_add(COOL_GEN_GEN_WORD(p), 1, __ATOMIC_SEQ_CST);
    cool_str_stamp_bump(p);
    cool_heap_blocks -= 1;
    cool_heap_bytes -= size;
    cool_heap_held -= size;
    cool_reg_del_locked(p);
    pthread_mutex_unlock(&cool_heap_lock);
    free(COOL_GEN_BASE(p));
    pthread_mutex_unlock(&cool_free_gate);
}

/* The sanitizer's defaults for a dusk program, which ASAN_OPTIONS can still
   override. Fake stacks are off because the collector's conservative scan reads
   the real stack: a root held in a frame moved to a fake stack would be missed and
   a live collected block swept. Leak detection is off because every live block is
   reachable through the registry and the report would only ever name the
   runtime's own residue; --heap-sites is the leak report. A strong definition,
   since the sanitizer runtime links ahead of every object with a weak one of its
   own that would win over a second weak one. */
__attribute__((no_sanitize("address"))) const char *__asan_default_options(void) {
    return "detect_stack_use_after_return=0:detect_leaks=0";
}

#else

void cool_gen_scan_begin(void) {
}

void cool_gen_scan_end(void) {
}

static inline int64_t cool_gen_first(void) {
    return 1;
}

static inline void cool_gen_mark_record(void *p) {
    (void)p;
}

static inline void cool_gen_record_check_locked(void *p) {
    (void)p;
}

#endif
