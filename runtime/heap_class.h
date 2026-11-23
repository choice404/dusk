/* The generational heap's size classes and parked block lists, included by
   runtime.c after the header macros. Not a standalone header: it reads the heap
   lock's discipline and the header layout from the file that includes it, and every
   function here is called with the heap lock held. */

/* Size classes. A block is minted with the capacity of its request's class and
   any later request of that class may reuse it, so retained memory is bounded by
   the peak live bytes per class rather than by the count of distinct sizes a
   program ever asked for. Up to 1016 bytes a class spans sixteen sizes and holds
   16k + 8 bytes, which with the header is exactly the chunk glibc hands an exact
   request of any size in it, so nothing is wasted that malloc was not already
   rounding away. Past that each doubling splits into eight classes, wasting at
   most an eighth of a request. Class 487 reaches the largest size the header
   carries, so the table is one static array of 488 list heads.

   The list nodes live in their own arrays, never in the parked payload. A use
   after free that writes through a stale pointer therefore cannot corrupt the
   allocator; it only scribbles on a block whose generation already faults the
   next managed dereference. Node index zero is the empty list, so the zeroed bin
   table starts out empty with no initialization pass. */
#define COOL_GEN_SMALL_MAX 1016
#define COOL_GEN_CLASSES 488
static int64_t cool_gen_bin[COOL_GEN_CLASSES];

/* The class of a payload size in [0, COOL_GEN_SIZE_MAX). Small sizes step by
   sixteen; above COOL_GEN_SMALL_MAX the top bit of size - 1 picks the doubling
   and the three bits under it pick one of its eight classes. */
static int64_t cool_gen_class(int64_t size) {
    if (size <= COOL_GEN_SMALL_MAX) {
        return (size + 7) >> 4;
    }
    uint64_t m = (uint64_t)size - 1;
    int64_t e = 63 - (int64_t)__builtin_clzll(m);
    int64_t j = (int64_t)((m >> (e - 3)) & 7);
    return 64 + 8 * (e - 9) + j;
}

/* The payload capacity of a class, at least every size the class holds. */
static int64_t cool_gen_cap(int64_t cls) {
    if (cls < 64) {
        return 16 * cls + 8;
    }
    int64_t e = 9 + (cls - 64) / 8;
    int64_t j = (cls - 64) % 8;
    return (9 + j) << (e - 3);
}

static void **cool_fl_ptr = NULL;
static int64_t *cool_fl_next = NULL;
static int64_t cool_fl_cap = 0;
static int64_t cool_fl_n = 1;
static int64_t cool_fl_recycle = 0;

/* Hands back a node index, recycled if one is free and fresh otherwise, or zero
   when the arrays cannot grow. Zero is not a failure the caller must escalate:
   a block that cannot be parked is simply dropped, which is what the old fixed
   list did past its cap, and dropping is sound because the block is already
   generation bumped and off the registry. Held with the heap lock. */
static int64_t cool_fl_node_locked(void) {
    if (cool_fl_recycle != 0) {
        int64_t n = cool_fl_recycle;
        cool_fl_recycle = cool_fl_next[n];
        return n;
    }
    if (cool_fl_n >= cool_fl_cap) {
        int64_t ncap = cool_fl_cap ? cool_fl_cap * 2 : 1024;
        void **np = realloc(cool_fl_ptr, (size_t)ncap * sizeof(void *));
        if (!np) {
            return 0;
        }
        cool_fl_ptr = np;
        int64_t *nn = realloc(cool_fl_next, (size_t)ncap * sizeof(int64_t));
        if (!nn) {
            /* The pointer array grew and the next array did not. Leaving the cap
               where it was keeps the two in step at the smaller size, so the
               slack is wasted rather than read out of bounds. */
            return 0;
        }
        cool_fl_next = nn;
        cool_fl_cap = ncap;
    }
    return cool_fl_n++;
}

/* Parks a retired block on the list of its class. Held with the heap lock. */
static void cool_gen_park_locked(void *p, int64_t cls) {
    int64_t node = cool_fl_node_locked();
    if (node == 0) {
        return;
    }
    cool_fl_ptr[node] = p;
    cool_fl_next[node] = cool_gen_bin[cls];
    cool_gen_bin[cls] = node;
}

/* Pops a parked block of this class, or NULL when the class is empty. Held with
   the heap lock. */
static void *cool_gen_unpark_locked(int64_t cls) {
    int64_t node = cool_gen_bin[cls];
    if (node == 0) {
        return NULL;
    }
    void *p = cool_fl_ptr[node];
    cool_gen_bin[cls] = cool_fl_next[node];
    cool_fl_next[node] = cool_fl_recycle;
    cool_fl_recycle = node;
    return p;
}
