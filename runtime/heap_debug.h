/* The Debug allocator's table, included by runtime.c where the allocator sits.
   Not a standalone header: it reads cool_heap_lock and cool_str_stamp_bump from
   the file that includes it.

   The allocator tracks every block it hands out so a program can count its leaks
   and its double frees. A freed block is poisoned with 0xDD and kept, never
   returned to libc, so a use after free reads poison and the address is never
   handed out again. That is what lets the table be insert only: an address in
   it can never come back from malloc, so no entry is ever removed and a probe
   chain never breaks.

   The table is open addressed, keyed by the block address, probed linearly, and
   doubled when it reaches half load, so it has no cap and a free is one probe
   rather than a walk. A table that cannot grow is fatal out of memory; tracking
   never silently stops, since a block it stopped tracking would read as a double
   free when the program freed it. */

typedef struct {
    void *ptr;
    int64_t size;
    int freed;
} cool_dbg_entry;

static cool_dbg_entry *cool_dbg_tab = NULL;
static int64_t cool_dbg_cap = 0;
static int64_t cool_dbg_used = 0;
static int64_t cool_dbg_live = 0;
static int64_t cool_dbg_double = 0;

#define COOL_DBG_FIRST_CAP 1024

static void cool_dbg_oom(void) {
    fflush(stdout);
    fputs("fatal: out of memory\n", stderr);
    abort();
}

/* The first slot an address probes from. Blocks are sixteen byte aligned, so the
   low bits carry nothing and the multiply spreads the rest across the table. */
static int64_t cool_dbg_home(const void *p, int64_t cap) {
    uint64_t h = (uint64_t)((uintptr_t)p >> 4) * 0x9E3779B97F4A7C15ull;
    return (int64_t)((h ^ (h >> 29)) & (uint64_t)(cap - 1));
}

/* The slot holding p, or the empty slot where p would go. The table is never
   full, since it doubles at half load, so a probe always ends. Held with the
   heap lock. */
static cool_dbg_entry *cool_dbg_slot_locked(cool_dbg_entry *tab, int64_t cap, const void *p) {
    int64_t i = cool_dbg_home(p, cap);
    while (tab[i].ptr != NULL && tab[i].ptr != p) {
        i = (i + 1) & (cap - 1);
    }
    return &tab[i];
}

/* Doubles the table and reinserts every entry. Held with the heap lock. */
static void cool_dbg_grow_locked(void) {
    int64_t ncap = cool_dbg_cap ? cool_dbg_cap * 2 : COOL_DBG_FIRST_CAP;
    cool_dbg_entry *ntab = calloc((size_t)ncap, sizeof(cool_dbg_entry));
    if (!ntab) {
        pthread_mutex_unlock(&cool_heap_lock);
        cool_dbg_oom();
    }
    for (int64_t i = 0; i < cool_dbg_cap; i++) {
        if (cool_dbg_tab[i].ptr != NULL) {
            *cool_dbg_slot_locked(ntab, ncap, cool_dbg_tab[i].ptr) = cool_dbg_tab[i];
        }
    }
    free(cool_dbg_tab);
    cool_dbg_tab = ntab;
    cool_dbg_cap = ncap;
}

/* A failed malloc hands back null untracked, so the program sees the failure
   rather than a table entry for a block that does not exist. */
void *cool_debug_alloc(int64_t n) {
    void *p = malloc((size_t)n);
    if (!p) {
        return NULL;
    }
    pthread_mutex_lock(&cool_heap_lock);
    if ((cool_dbg_used + 1) * 2 > cool_dbg_cap) {
        cool_dbg_grow_locked();
    }
    cool_dbg_entry *e = cool_dbg_slot_locked(cool_dbg_tab, cool_dbg_cap, p);
    if (e->ptr == NULL) {
        cool_dbg_used++;
    }
    e->ptr = p;
    e->size = n;
    e->freed = 0;
    cool_dbg_live++;
    pthread_mutex_unlock(&cool_heap_lock);
    return p;
}

/* A free of an address the table never saw, or of one already freed, is counted
   as a double free and touches nothing. */
void cool_debug_free(void *p) {
    pthread_mutex_lock(&cool_heap_lock);
    if (cool_dbg_cap == 0 || p == NULL) {
        cool_dbg_double++;
        pthread_mutex_unlock(&cool_heap_lock);
        return;
    }
    cool_dbg_entry *e = cool_dbg_slot_locked(cool_dbg_tab, cool_dbg_cap, p);
    if (e->ptr == NULL || e->freed) {
        cool_dbg_double++;
        pthread_mutex_unlock(&cool_heap_lock);
        return;
    }
    e->freed = 1;
    cool_dbg_live--;
    cool_str_stamp_bump(p);
    memset(p, 0xDD, (size_t)e->size);
    pthread_mutex_unlock(&cool_heap_lock);
}

int64_t cool_debug_leaks(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = cool_dbg_live;
    pthread_mutex_unlock(&cool_heap_lock);
    return n;
}

int64_t cool_debug_double_frees(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = cool_dbg_double;
    pthread_mutex_unlock(&cool_heap_lock);
    return n;
}
