/* Allocation sites for the generational heap, included by runtime.c after the
   header macros. Not a standalone header: it reads the header layout from the file
   that includes it, and the recording runs with the heap lock held.

   A build with --heap-sites compiles the runtime with COOL_HEAP_SITES. Every
   generational block then carries two more words in front of its header, never
   behind it, so the generation stays at payload minus eight and the size at minus
   sixteen and no offset the emitted IR reads moves:

       default            [size][gen] payload                COOL_SITE_PRE = 0
       --heap-sites       [site][chain][size][gen] payload   COOL_SITE_PRE = 16

   site is the innermost recorded location, a "path:line" constant of the program,
   or null. chain is an index into an interned table of up to eight locations,
   innermost first. The prefix is sixteen bytes, so the payload keeps the sixteen
   byte alignment malloc hands back. An AddressSanitizer build puts two words more
   in front of these, see heap_registry.h, which also writes COOL_GEN_BASE, the one
   inverse of the mint's arithmetic.

   The locations come from a stack each thread keeps. Codegen pushes the location of
   a call in the program's own files before the call and pops it after, one
   instruction wide, so nothing between the two can leave the stack unbalanced. A
   new thread starts with an empty stack. A default build has no prefix, no stack,
   and no call: the recording below is an empty inline function. */

#ifdef COOL_HEAP_SITES

#define COOL_SITE_PRE 16
#define COOL_SITE_CAP 1024
#define COOL_SITE_CHAIN 8

#define COOL_GEN_SITE_WORD(p) ((const char **)((char *)(p) - COOL_GEN_HDR - 16))
#define COOL_GEN_CHAIN_WORD(p) ((int64_t *)((char *)(p) - COOL_GEN_HDR - 8))

static __thread const char *cool_site_stack[COOL_SITE_CAP];
static __thread int64_t cool_site_depth = 0;

/* Pushes the location of a call about to run. A push past the cap is counted and
   not stored, so a recursion deeper than the stack still pops back to where it
   was, and an allocation down there is attributed to the deepest recorded frame. */
void cool_site_push(const char *loc) {
    if (cool_site_depth < COOL_SITE_CAP) {
        cool_site_stack[cool_site_depth] = loc;
    }
    cool_site_depth++;
}

/* Pops the location the matching push stored. The depth never goes below zero. */
void cool_site_pop(void) {
    if (cool_site_depth > 0) {
        cool_site_depth--;
    }
}

/* One interned chain: its hash, its length, and its locations innermost first.
   Chain 0 is the empty chain and is never hashed. */
typedef struct {
    uint64_t hash;
    int64_t n;
    const char *loc[COOL_SITE_CHAIN];
} cool_chain;

static cool_chain *cool_chains = NULL;
static int64_t cool_chain_n = 0;
static int64_t cool_chain_cap = 0;
static int64_t *cool_chain_slot = NULL;
static int64_t cool_chain_slot_cap = 0;

static void cool_site_oom(void) {
    fflush(stdout);
    fputs("fatal: out of memory\n", stderr);
    abort();
}

/* The hash of a chain over its location pointers. Locations are interned constants
   of one module, so pointer equality is location equality. */
static uint64_t cool_chain_hash(const char **locs, int64_t n) {
    uint64_t h = 1469598103934665603ULL ^ (uint64_t)n;
    for (int64_t i = 0; i < n; i++) {
        h ^= (uint64_t)(uintptr_t)locs[i];
        h *= 1099511628211ULL;
    }
    return h;
}

static int cool_chain_equal(const cool_chain *c, uint64_t h, const char **locs, int64_t n) {
    if (c->hash != h || c->n != n) {
        return 0;
    }
    for (int64_t i = 0; i < n; i++) {
        if (c->loc[i] != locs[i]) {
            return 0;
        }
    }
    return 1;
}

/* Places chain id in the open addressed slot table, which has room. */
static void cool_chain_slot_put(int64_t id) {
    uint64_t mask = (uint64_t)cool_chain_slot_cap - 1;
    uint64_t i = cool_chains[id].hash & mask;
    while (cool_chain_slot[i] != 0) {
        i = (i + 1) & mask;
    }
    cool_chain_slot[i] = id;
}

/* Doubles the slot table and places every chain again, keeping the load at most
   one half. */
static void cool_chain_rehash(void) {
    int64_t ncap = cool_chain_slot_cap ? cool_chain_slot_cap * 2 : 256;
    int64_t *ns = calloc((size_t)ncap, sizeof(int64_t));
    if (!ns) {
        cool_site_oom();
    }
    free(cool_chain_slot);
    cool_chain_slot = ns;
    cool_chain_slot_cap = ncap;
    for (int64_t id = 1; id < cool_chain_n; id++) {
        cool_chain_slot_put(id);
    }
}

/* Appends a new chain and returns its id. */
static int64_t cool_chain_add(uint64_t h, const char **locs, int64_t n) {
    if (cool_chain_n == 0) {
        cool_chain_n = 1;
    }
    if (cool_chain_n >= cool_chain_cap) {
        int64_t ncap = cool_chain_cap ? cool_chain_cap * 2 : 256;
        cool_chain *nc = realloc(cool_chains, (size_t)ncap * sizeof(cool_chain));
        if (!nc) {
            cool_site_oom();
        }
        cool_chains = nc;
        cool_chain_cap = ncap;
        cool_chains[0].hash = 0;
        cool_chains[0].n = 0;
    }
    int64_t id = cool_chain_n++;
    cool_chains[id].hash = h;
    cool_chains[id].n = n;
    for (int64_t i = 0; i < n; i++) {
        cool_chains[id].loc[i] = locs[i];
    }
    if (2 * cool_chain_n > cool_chain_slot_cap) {
        cool_chain_rehash();
    } else {
        cool_chain_slot_put(id);
    }
    return id;
}

/* The id of a chain, interning it on first sight. Held with the heap lock. */
static int64_t cool_chain_intern_locked(const char **locs, int64_t n) {
    if (n == 0) {
        return 0;
    }
    uint64_t h = cool_chain_hash(locs, n);
    if (cool_chain_slot_cap > 0) {
        uint64_t mask = (uint64_t)cool_chain_slot_cap - 1;
        uint64_t i = h & mask;
        while (cool_chain_slot[i] != 0) {
            int64_t id = cool_chain_slot[i];
            if (cool_chain_equal(&cool_chains[id], h, locs, n)) {
                return id;
            }
            i = (i + 1) & mask;
        }
    }
    return cool_chain_add(h, locs, n);
}

/* Records where a block was allocated: the top of this thread's stack, at most
   eight locations innermost first, interned as a chain. Held with the heap lock,
   on a block handed out by a mint or a reuse alike. */
static void cool_site_record_locked(void *p) {
    int64_t top = cool_site_depth < COOL_SITE_CAP ? cool_site_depth : COOL_SITE_CAP;
    int64_t k = top < COOL_SITE_CHAIN ? top : COOL_SITE_CHAIN;
    const char *locs[COOL_SITE_CHAIN];
    for (int64_t i = 0; i < k; i++) {
        locs[i] = cool_site_stack[top - 1 - i];
    }
    *COOL_GEN_SITE_WORD(p) = k > 0 ? locs[0] : NULL;
    *COOL_GEN_CHAIN_WORD(p) = cool_chain_intern_locked(locs, k);
}

#else

#define COOL_SITE_PRE 0

static inline void cool_site_record_locked(void *p) {
    (void)p;
}

#endif
