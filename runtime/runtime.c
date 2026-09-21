/* Runtime linked into every coolc binary. Names are prefixed cool_ to avoid
   clashing with user symbols. The heap is thread safe: one mutex guards the
   generational free list and the debug tables, and the generation word is
   accessed atomically on both sides of the dereference check, so the check
   machinery itself is never a C level data race. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <inttypes.h>
#include <limits.h>
#include <string.h>
#include <pthread.h>
#include <sys/stat.h>
#include <errno.h>
#include <dirent.h>
#include <time.h>

#ifdef __wasm__
/* wasi-libc has no process or shell layer, so popen and pclose are absent from
   its stdio.h. cool_popen and cool_pclose below must still compile for the wasm
   build (the browser playground), which never calls them; declare the two here
   so this file compiles, and runtime/wasm_shim.c defines them as inert stubs. */
FILE *popen(const char *command, const char *type);
int pclose(FILE *stream);
#endif

static pthread_mutex_t cool_heap_lock = PTHREAD_MUTEX_INITIALIZER;

void cool_gen_fault(void);
void *cool_gen_alloc(int64_t n);

/* The collector's free guard. A collected address must never enter the
   generational free list, so both free paths consult this first and no op on a
   collected pointer. Defined in collect.c; on a program that never mints a
   collected block it returns 0 without taking any lock. */
extern int cool_gc_is_collected(void *p);

/* print writes the value with no newline, println appends one. The builtins
   print and println in the language map to the matching pair per value type. A
   null string prints as empty rather than crashing, since the language's empty
   error carries a null message pointer. */
void cool_print_cstr(const char *s) {
    fputs(s ? s : "", stdout);
}

void cool_println_cstr(const char *s) {
    puts(s ? s : "");
}

/* Flushes buffered stdout so a line printed just before a child process runs
   reaches the shared file descriptor first, keeping the parent's output and the
   child's in order even when stdout is a pipe. dawn uses this before it runs a
   built binary, matching the line-buffered flush the Rust standard library does
   on every newline. */
void cool_flush(void) {
    fflush(stdout);
}

/* Byte counted text writers for char, char array, and char slice printing.
   The buffer is not NUL terminated; exactly n bytes go to the stream, so an
   embedded NUL or a multibyte UTF-8 sequence passes through untouched. */
void cool_print_bytes(const char *p, int64_t n) {
    if (n > 0) {
        fwrite(p, 1, (size_t)n, stdout);
    }
}

void cool_eprint_bytes(const char *p, int64_t n) {
    fflush(stdout);
    if (n > 0) {
        fwrite(p, 1, (size_t)n, stderr);
    }
}

/* Content equality for the string == and != operators. A null operand reads
   as empty, matching the printers, so the empty error message compares equal
   to "" instead of crashing. One address is equal to itself without reading a
   byte, and two nulls are the one pair that already answered equal through the
   empty reading, so the pointer test changes no answer and skips the walk on
   the interned and self compared cases the compiler asks about most. */
int64_t cool_str_eq(const char *a, const char *b) {
    if (a == b) {
        return 1;
    }
    const char *x = a ? a : "";
    const char *y = b ? b : "";
    return strcmp(x, y) == 0 ? 1 : 0;
}

/* String concatenation for the + operator. The result is a fresh string on
   the generational heap, the same allocation substring returns, so free()
   reclaims it like any other heap string. */
char *cool_str_concat(const char *a, const char *b) {
    const char *x = a ? a : "";
    const char *y = b ? b : "";
    size_t la = strlen(x);
    size_t lb = strlen(y);
    char *out = (char *)cool_gen_alloc((int64_t)(la + lb + 1));
    memcpy(out, x, la);
    memcpy(out + la, y, lb);
    out[la + lb] = 0;
    return out;
}

/* Stderr printers for the printerr builtin. None appends a newline; codegen
   emits the newline as its own segment, so one set serves every call shape.
   Each flushes stdout first, so buffered program output lands before the
   message even when the program aborts right after printing it. */
void cool_eprint_cstr(const char *s) {
    fflush(stdout);
    fputs(s ? s : "", stderr);
}

void cool_eprint_i64(int64_t v) {
    fflush(stdout);
    fprintf(stderr, "%" PRId64, v);
}

void cool_eprint_f64(double v) {
    fflush(stdout);
    fprintf(stderr, "%g", v);
}

/* The unsigned decimal stderr printer. Codegen hands it the same i64 LLVM word the
   signed one takes, already zero extended from the narrower unsigned widths, and
   only the format differs: PRIu64 reads the full 64 bit magnitude, so u64 max
   prints 18446744073709551615 rather than -1. */
void cool_eprint_u64(uint64_t v) {
    fflush(stdout);
    fprintf(stderr, "%" PRIu64, v);
}

/* PARITY: the f-string builder (cool_fsb_i64 / cool_fsb_u64 / cool_fsb_f64 below)
   must render with the same PRId64, PRIu64, and %g formats these printers use, so
   f"{v}" writes the same bytes println("{}", v) does. Change a format here and
   there together. */
void cool_print_i64(int64_t v) {
    printf("%" PRId64, v);
}

void cool_println_i64(int64_t v) {
    printf("%" PRId64 "\n", v);
}

/* The unsigned decimal pair. An unsigned dusk value of any width arrives here
   widened to 64 bits by a zext, so this one entry point serves uint8 through
   uint64 the way cool_print_i64 serves the signed widths. */
void cool_print_u64(uint64_t v) {
    printf("%" PRIu64, v);
}

void cool_println_u64(uint64_t v) {
    printf("%" PRIu64 "\n", v);
}

void cool_print_f64(double v) {
    printf("%g", v);
}

void cool_println_f64(double v) {
    printf("%g\n", v);
}

/* The interpolated string (f-string) builder. An f"..." expression opens one
   builder, appends the rendered bytes of each chunk and hole in order, then
   finishes into a single generational heap string. The scratch buffer is plain
   malloc doubled on demand and freed by cool_fsb_fin after the copy, so only the
   finished string outlives the expression.

   PARITY (keep in lockstep with the print family just above): cool_fsb_i64 formats
   with PRId64, cool_fsb_u64 with PRIu64, and cool_fsb_f64 with %g, byte identical to
   cool_print_i64, cool_print_u64, and cool_print_f64, so f"{v}" writes exactly the
   bytes println("{}", v) writes minus the newline for every printable type. Changing
   a format here means changing it there too. */
typedef struct {
    char *buf;
    size_t len;
    size_t cap;
} cool_fsb;

static void cool_fsb_oom(void) {
    fflush(stdout);
    fputs("fatal: out of memory\n", stderr);
    abort();
}

static void cool_fsb_reserve(cool_fsb *b, size_t extra) {
    /* The running length plus this append must not wrap size_t; a wrap would let a
       short reserve back a long write and corrupt the heap, so an overflow aborts on
       the OOM path rather than proceeding. */
    if (extra > SIZE_MAX - b->len) {
        cool_fsb_oom();
    }
    size_t need = b->len + extra;
    if (need <= b->cap) {
        return;
    }
    /* Double until the capacity covers need, but never past the point where doubling
       would itself wrap; there it clamps to the exact need, which the guard above
       proved is representable. */
    size_t ncap = b->cap ? b->cap : 64;
    while (ncap < need) {
        if (ncap > SIZE_MAX / 2) {
            ncap = need;
            break;
        }
        ncap *= 2;
    }
    char *nb = (char *)realloc(b->buf, ncap);
    if (!nb) {
        cool_fsb_oom();
    }
    b->buf = nb;
    b->cap = ncap;
}

void *cool_fsb_new(void) {
    cool_fsb *b = (cool_fsb *)malloc(sizeof(cool_fsb));
    if (!b) {
        cool_fsb_oom();
    }
    b->buf = NULL;
    b->len = 0;
    b->cap = 0;
    return b;
}

/* Byte counted append, the char / char array / char slice sink: exactly n bytes,
   so an embedded NUL or a multibyte UTF-8 sequence passes through, matching
   cool_print_bytes. */
void cool_fsb_bytes(void *h, const char *p, int64_t n) {
    if (n <= 0) {
        return;
    }
    cool_fsb *b = (cool_fsb *)h;
    cool_fsb_reserve(b, (size_t)n);
    memcpy(b->buf + b->len, p, (size_t)n);
    b->len += (size_t)n;
}

/* NUL terminated string sink for a chunk constant, a string hole, a rawptr, an
   error message, or a struct's toString result. A null pointer appends nothing,
   matching cool_print_cstr's empty read. */
void cool_fsb_cstr(void *h, const char *s) {
    if (!s) {
        return;
    }
    cool_fsb_bytes(h, s, (int64_t)strlen(s));
}

void cool_fsb_i64(void *h, int64_t v) {
    char tmp[32];
    int n = snprintf(tmp, sizeof(tmp), "%" PRId64, v);
    if (n > 0) {
        cool_fsb_bytes(h, tmp, n);
    }
}

/* The unsigned twin of cool_fsb_i64, PRIu64 against cool_print_u64's PRIu64, so
   f"{v}" and println("{}", v) agree byte for byte on an unsigned value. The 32
   byte scratch covers the widest unsigned decimal, 20 digits plus the NUL. */
void cool_fsb_u64(void *h, uint64_t v) {
    char tmp[32];
    int n = snprintf(tmp, sizeof(tmp), "%" PRIu64, v);
    if (n > 0) {
        cool_fsb_bytes(h, tmp, n);
    }
}

void cool_fsb_f64(void *h, double v) {
    char tmp[64];
    int n = snprintf(tmp, sizeof(tmp), "%g", v);
    if (n > 0) {
        cool_fsb_bytes(h, tmp, n);
    }
}

/* Copies the accumulated bytes into a fresh generational heap string, the same
   allocation class cool_str_concat and substring return, so free() reclaims the
   result like any other heap string. The scratch is released here; the builder
   is single use. */
char *cool_fsb_fin(void *h) {
    cool_fsb *b = (cool_fsb *)h;
    /* The NUL terminator makes the request len + 1; refuse a length that would wrap
       that byte count or overflow the signed size cool_gen_alloc takes, and abort if
       the allocation itself fails, so the terminating store below never lands on a
       null or short buffer. */
    if (b->len >= (size_t)INT64_MAX) {
        cool_fsb_oom();
    }
    char *out = (char *)cool_gen_alloc((int64_t)(b->len + 1));
    if (!out) {
        cool_fsb_oom();
    }
    if (b->len > 0) {
        memcpy(out, b->buf, b->len);
    }
    out[b->len] = 0;
    free(b->buf);
    free(b);
    return out;
}

/* The std.logging level word. Default 1 (Info). Relaxed ordering is enough:
   the gate check races with a concurrent set at worst by one message, never a
   torn read, and no other memory depends on the ordering. */
static int64_t cool_log_level = 1;

int64_t cool_log_level_get(void) {
    return __atomic_load_n(&cool_log_level, __ATOMIC_RELAXED);
}

void cool_log_level_set(int64_t l) {
    __atomic_store_n(&cool_log_level, l, __ATOMIC_RELAXED);
}

/* The program image guard, the check that keeps a free of a constant from dying
   by signal. A string literal, and every other constant the compiler parks in the
   program image, carries no allocation header, so the generational retire's read
   of the size word sixteen bytes ahead of the payload and its bump of the
   generation word eight bytes ahead of it land in read only memory and take the
   process down with no name on the fault. The linker names the whole loaded
   image, [__executable_start, _end), and neither the brk heap nor an mmap region
   ever falls inside that range, so an address in it was never handed out by any
   allocator and freeing it is a misuse the runtime can name at the free itself.
   Both symbols are weak, so a linker that does not define them leaves the guard
   inert rather than failing the link, and the guard compiles out entirely off
   ELF, on wasm and on Mach-O, where neither symbol exists and a free of a
   constant keeps its older undefined behavior. */
#if defined(__ELF__) && !defined(__wasm__) && !defined(__APPLE__) && \
    (defined(__linux__) || defined(__unix__))
#define COOL_IMAGE_GUARD 1
extern char __executable_start[] __attribute__((weak));
extern char _end[] __attribute__((weak));
#endif

/* Every free runs this, and the runtime is compiled with no optimization flag, so
   it is forced inline: a call frame per free is real time in a program that frees
   millions of strings, and the test itself is two comparisons. */
__attribute__((always_inline)) static inline int cool_in_image(const void *p) {
#ifdef COOL_IMAGE_GUARD
    /* Compared as integers rather than as pointers, since the C rule for a
       relational operator covers two pointers into one object and these name
       three unrelated ones. A bound of zero is a symbol the linker did not
       define, and either one missing reads the whole guard inert: a defined
       _end over an undefined start would otherwise call every low address a
       constant. */
    uintptr_t lo = (uintptr_t)(const void *)__executable_start;
    uintptr_t hi = (uintptr_t)(const void *)_end;
    uintptr_t q = (uintptr_t)p;
    return lo != 0 && hi > lo && q - lo < hi - lo;
#else
    (void)p;
    return 0;
#endif
}

/* Reports a free of an address inside the program image. Both free entry points
   test the range before they touch the block, so the misuse aborts by name at the
   free itself instead of faulting namelessly one word ahead of the pointer the
   program handed in. */
static void cool_const_free_fault(void) {
    fflush(stdout);
    fputs("fatal: free of a string literal or other constant; it was never allocated\n", stderr);
    abort();
}

void *cool_alloc(size_t n) {
    return malloc(n);
}

void cool_free(void *p) {
    // A constant was never allocated, so libc has no block to reclaim and the
    // misuse gets a name here rather than an allocator abort or worse.
    if (cool_in_image(p)) {
        cool_const_free_fault();
    }
    // A collected address is owned by the collector, not libc; freeing one here
    // would hand a live collected block back to the allocator.
    if (cool_gc_is_collected(p)) {
        return;
    }
    free(p);
}

/* Debug allocator. Tracks every allocation in a table so it can report leaks
   and catch a double free. A freed block is poisoned with 0xDD and kept, not
   returned to libc, so a use after free reads poison and its address is never
   handed out again. This trades memory for detection, which is the point in a
   debug build. */
#define COOL_DBG_MAX 4096
static void *cool_dbg_ptr[COOL_DBG_MAX];
static int64_t cool_dbg_size[COOL_DBG_MAX];
static int cool_dbg_freed[COOL_DBG_MAX];
static int cool_dbg_count = 0;
static int64_t cool_dbg_double = 0;

void *cool_debug_alloc(int64_t n) {
    void *p = malloc(n);
    pthread_mutex_lock(&cool_heap_lock);
    if (cool_dbg_count < COOL_DBG_MAX) {
        cool_dbg_ptr[cool_dbg_count] = p;
        cool_dbg_size[cool_dbg_count] = n;
        cool_dbg_freed[cool_dbg_count] = 0;
        cool_dbg_count++;
    }
    pthread_mutex_unlock(&cool_heap_lock);
    return p;
}

void cool_debug_free(void *p) {
    pthread_mutex_lock(&cool_heap_lock);
    for (int i = 0; i < cool_dbg_count; i++) {
        if (cool_dbg_ptr[i] == p) {
            if (cool_dbg_freed[i]) {
                cool_dbg_double++;
                pthread_mutex_unlock(&cool_heap_lock);
                return;
            }
            cool_dbg_freed[i] = 1;
            memset(p, 0xDD, (size_t)cool_dbg_size[i]);
            pthread_mutex_unlock(&cool_heap_lock);
            return;
        }
    }
    cool_dbg_double++;
    pthread_mutex_unlock(&cool_heap_lock);
}

int64_t cool_debug_leaks(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = 0;
    for (int i = 0; i < cool_dbg_count; i++) {
        if (!cool_dbg_freed[i]) {
            n++;
        }
    }
    pthread_mutex_unlock(&cool_heap_lock);
    return n;
}

int64_t cool_debug_double_frees(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = cool_dbg_double;
    pthread_mutex_unlock(&cool_heap_lock);
    return n;
}

/* Generational heap for managed pointers. Each managed allocation carries a
   header in front of the data holding the payload size and a generation, with
   the generation in the word right before the data so a check is a single load
   at p minus 8. free bumps the generation and parks the block on a size matched
   free list. A later allocation of the same size reuses the block with its now
   advanced generation, so a stale reference still holding the old generation
   mismatches and faults at its next dereference. The generation never resets
   for a block, which is what keeps reuse sound.

   The header is two words, [i64 size][i64 gen], and stays two words. The
   collected heap in collect.c mints blocks with the same shape so one
   dereference check reads either layer, emitted IR reaches the generation at
   payload minus eight, and the payload keeps the sixteen byte alignment malloc
   itself hands back. Everything the allocator learned to do faster below sits
   beside the header, never inside it. */
#define COOL_GEN_HDR 16

/* The size word carries one reserved bit. It is set while the block is parked
   on a free list and cleared when a later allocation hands the block back out,
   so a free of an already parked block is one load and one test rather than a
   walk of the list. A payload size is bounded below the bit, which puts the
   ceiling four exabytes past anything malloc can answer. */
#define COOL_GEN_FREED_TAG ((int64_t)1 << 62)
#define COOL_GEN_SIZE_MAX ((int64_t)1 << 62)

/* Header word addresses from a payload, so the offsets are written once. */
#define COOL_GEN_SIZE_WORD(p) ((int64_t *)((char *)(p) - COOL_GEN_HDR))
#define COOL_GEN_GEN_WORD(p) ((int64_t *)((char *)(p) - 8))

/* Size matched free lists. A parked block is found by its exact payload size,
   the same match the linear list made, but through a table rather than a scan:
   sizes up to COOL_GEN_BIN_MAX index a direct array, larger sizes hash into an
   open addressed table of the same head slots. Both park and unpark are a
   handful of loads.

   The list nodes live in their own arrays, never in the parked payload. A use
   after free that writes through a stale pointer therefore cannot corrupt the
   allocator; it only scribbles on a block whose generation already faults the
   next managed dereference, which is the property the previous static array
   had and the one worth keeping. Node index zero is the empty list, so the
   zeroed bin table starts out empty with no initialization pass. */
#define COOL_GEN_BIN_MAX 4096
static int64_t cool_gen_bin[COOL_GEN_BIN_MAX + 1];

typedef struct {
    int64_t size;
    int64_t head;
} cool_gen_big_bin;

static cool_gen_big_bin *cool_gen_big = NULL;
static int64_t cool_gen_big_cap = 0;
static int64_t cool_gen_big_n = 0;

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

/* Locates the head slot for a large size in the open addressed table, growing
   it when it passes half full. An entry is never removed, so a size class that
   empties keeps its slot with an empty head and costs one probe. Returns NULL
   when the size has no slot and none can be made. Held with the heap lock. */
static int64_t *cool_gen_big_slot_locked(int64_t size, int create) {
    if (cool_gen_big_cap == 0) {
        if (!create) {
            return NULL;
        }
        int64_t ncap = 256;
        cool_gen_big_bin *nb = calloc((size_t)ncap, sizeof(cool_gen_big_bin));
        if (!nb) {
            return NULL;
        }
        cool_gen_big = nb;
        cool_gen_big_cap = ncap;
        cool_gen_big_n = 0;
    }
    for (;;) {
        uint64_t h = (uint64_t)size * 0x9E3779B97F4A7C15ull;
        h ^= h >> 29;
        int64_t mask = cool_gen_big_cap - 1;
        int64_t i = (int64_t)(h & (uint64_t)mask);
        for (;;) {
            if (cool_gen_big[i].size == size) {
                return &cool_gen_big[i].head;
            }
            if (cool_gen_big[i].size == 0) {
                break;
            }
            i = (i + 1) & mask;
        }
        if (!create) {
            return NULL;
        }
        if ((cool_gen_big_n + 1) * 2 <= cool_gen_big_cap) {
            cool_gen_big[i].size = size;
            cool_gen_big[i].head = 0;
            cool_gen_big_n++;
            return &cool_gen_big[i].head;
        }
        int64_t ncap = cool_gen_big_cap * 2;
        cool_gen_big_bin *nb = calloc((size_t)ncap, sizeof(cool_gen_big_bin));
        if (!nb) {
            return NULL;
        }
        for (int64_t k = 0; k < cool_gen_big_cap; k++) {
            if (cool_gen_big[k].size == 0) {
                continue;
            }
            uint64_t g = (uint64_t)cool_gen_big[k].size * 0x9E3779B97F4A7C15ull;
            g ^= g >> 29;
            int64_t j = (int64_t)(g & (uint64_t)(ncap - 1));
            while (nb[j].size != 0) {
                j = (j + 1) & (ncap - 1);
            }
            nb[j] = cool_gen_big[k];
        }
        free(cool_gen_big);
        cool_gen_big = nb;
        cool_gen_big_cap = ncap;
        /* Retry the probe against the grown table. */
    }
}

/* The head slot for an exact payload size, or NULL when there is none. A size
   outside the representable range belongs to no class: it can only come from a
   corrupt or foreign header, and refusing it here keeps a bad size out of the
   bin index rather than letting it address the table. Held with the heap lock. */
static int64_t *cool_gen_bin_slot_locked(int64_t size, int create) {
    if (size < 0 || size >= COOL_GEN_SIZE_MAX) {
        return NULL;
    }
    if (size <= COOL_GEN_BIN_MAX) {
        return &cool_gen_bin[size];
    }
    return cool_gen_big_slot_locked(size, create);
}

/* Parks a retired block on its size class. Held with the heap lock. */
static void cool_gen_park_locked(void *p, int64_t size) {
    int64_t *head = cool_gen_bin_slot_locked(size, 1);
    if (!head) {
        return;
    }
    int64_t node = cool_fl_node_locked();
    if (node == 0) {
        return;
    }
    cool_fl_ptr[node] = p;
    cool_fl_next[node] = *head;
    *head = node;
}

/* Pops a parked block of exactly this payload size, or NULL when the class is
   empty. Held with the heap lock. */
static void *cool_gen_unpark_locked(int64_t size) {
    int64_t *head = cool_gen_bin_slot_locked(size, 0);
    if (!head || *head == 0) {
        return NULL;
    }
    int64_t node = *head;
    void *p = cool_fl_ptr[node];
    *head = cool_fl_next[node];
    cool_fl_next[node] = cool_fl_recycle;
    cool_fl_recycle = node;
    return p;
}

/* Block registry. One entry per generational block the heap has ever minted,
   with its payload size, so the collector can scan every live generational
   block as a root region.

   The table is append only. A block's address never goes back to libc, and
   reuse is size matched exactly, so a block's address and size are fixed for
   the life of the process and one entry describes it forever. Which entries are
   live is not stored twice: the parked bit in each block's own size word already
   says it, and the snapshot reads that bit. So a mint appends, a reuse touches
   nothing, and a free touches nothing, where the dense table this replaced
   walked itself on every free looking for the entry to swap out.

   The collector copies a snapshot under the lock, keeping only the live
   entries, and scans it released, so a block another thread frees mid scan is
   still mapped, parked not returned, and reads only over retain. */
static void **cool_reg_ptr = NULL;
static int64_t *cool_reg_size = NULL;
static int64_t cool_reg_n = 0;
static int64_t cool_reg_cap = 0;

/* Appends a freshly minted payload with the heap lock held. Growth failure is
   out of memory, fatal, since dropping an entry would under root the collector
   and free a still reachable block. */
static void cool_reg_add_locked(void *p, int64_t size) {
    if (cool_reg_n == cool_reg_cap) {
        int64_t ncap = cool_reg_cap ? cool_reg_cap * 2 : 128;
        void **np = realloc(cool_reg_ptr, (size_t)ncap * sizeof(void *));
        int64_t *ns = realloc(cool_reg_size, (size_t)ncap * sizeof(int64_t));
        if (!np || !ns) {
            fflush(stdout);
            fputs("fatal: out of memory\n", stderr);
            abort();
        }
        cool_reg_ptr = np;
        cool_reg_size = ns;
        cool_reg_cap = ncap;
    }
    cool_reg_ptr[cool_reg_n] = p;
    cool_reg_size[cool_reg_n] = size;
    cool_reg_n++;
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
    // bytes. The parked bit is set and cleared under this same lock, so the
    // live set the copy names is the live set at the moment of the copy.
    int64_t m = 0;
    for (int64_t i = 0; i < n; i++) {
        void *p = cool_reg_ptr[i];
        if (*COOL_GEN_SIZE_WORD(p) & COOL_GEN_FREED_TAG) {
            continue;
        }
        pp[m] = p;
        ss[m] = cool_reg_size[i];
        m++;
    }
    pthread_mutex_unlock(&cool_heap_lock);
    *ptrs = pp;
    *sizes = ss;
    return m;
}

void *cool_gen_alloc(int64_t size) {
    /* A size the header cannot carry beside its reserved bit is answered the way
       a failed malloc is, since no such allocation could succeed anyway. */
    if (size < 0 || size >= COOL_GEN_SIZE_MAX) {
        return NULL;
    }
    pthread_mutex_lock(&cool_heap_lock);
    void *p = cool_gen_unpark_locked(size);
    if (p) {
        /* Rewriting the size clears the parked bit, so the block is live again
           and a free of it is a first free rather than a double. The size itself
           is unchanged: the class matched it exactly. */
        *COOL_GEN_SIZE_WORD(p) = size;
        pthread_mutex_unlock(&cool_heap_lock);
        return p;
    }
    pthread_mutex_unlock(&cool_heap_lock);
    char *base = malloc(COOL_GEN_HDR + (size_t)size);
    if (!base) {
        return NULL;
    }
    int64_t *hdr = (int64_t *)base;
    hdr[0] = size;
    __atomic_store_n(&hdr[1], 1, __ATOMIC_SEQ_CST);
    void *q = base + COOL_GEN_HDR;
    pthread_mutex_lock(&cool_heap_lock);
    cool_reg_add_locked(q, size);
    pthread_mutex_unlock(&cool_heap_lock);
    return q;
}

/* The retire path with the heap lock already held. */
static void cool_gen_free_locked(void *p) {
    // Double free guard: a block already parked must not be parked again, or a
    // later allocation could hand the same address out twice. The parked bit in
    // the size word answers that in one load, and it answers for every parked
    // block rather than for the prefix a fixed list could hold, so a second free
    // is caught where the old walk could run past it. A managed pointer reaches
    // its own generation check first and faults there; what arrives here is the
    // untracked layer, a raw payload or a generation zero pointer, whose second
    // free this is the only guard for.
    int64_t *hdr = COOL_GEN_SIZE_WORD(p);
    int64_t size = *hdr;
    if (size & COOL_GEN_FREED_TAG) {
        fflush(stdout);
        fputs("fatal: double free\n", stderr);
        abort();
    }
    // The generation bump is atomic because the dereference check reads the
    // word without the heap lock, from any thread.
    int64_t *gen = COOL_GEN_GEN_WORD(p);
    __atomic_fetch_add(gen, 1, __ATOMIC_SEQ_CST);
    *hdr = size | COOL_GEN_FREED_TAG;
    cool_gen_park_locked(p, size);
}

void cool_gen_free(void *p) {
    if (!p) {
        return;
    }
    // A string literal or other constant has no header in front of it, so the
    // retire below would read a size and bump a generation in read only memory.
    if (cool_in_image(p)) {
        cool_const_free_fault();
    }
    // A collected address is reclaimed by the collector's sweep, never parked on
    // the generational free list, so retiring one here is a no op.
    if (cool_gc_is_collected(p)) {
        return;
    }
    pthread_mutex_lock(&cool_heap_lock);
    cool_gen_free_locked(p);
    pthread_mutex_unlock(&cool_heap_lock);
}

/* Checks the remembered generation and retires the block in one critical
   section, copying `n` payload bytes out first while the block is still live.
   join uses this so two threads joining copies of one handle cannot both pass
   the check and double retire: the loser sees the bumped generation under the
   same lock and faults. A mismatch never returns.

   This carries no collected free guard, unlike cool_gen_free and cool_free.
   Only join records, generational blocks, flow here, so a collected address
   never reaches it. It is the adjacent variant slot: if a collected block could
   ever be retired this way, the guard would have to run before the heap lock is
   taken, since the guard takes the collector lock and the lock order is
   collector then heap. Guarding it here now, with no caller, would only invite
   that inversion, so it stays unguarded until a caller needs it. */
int64_t cool_gen_retire_checked(void *p, int64_t gen, void *out, int64_t n) {
    if (!p) {
        return 1;
    }
    pthread_mutex_lock(&cool_heap_lock);
    int64_t *g = COOL_GEN_GEN_WORD(p);
    if (gen != 0 && __atomic_load_n(g, __ATOMIC_SEQ_CST) != gen) {
        pthread_mutex_unlock(&cool_heap_lock);
        cool_gen_fault();
    }
    memcpy(out, p, (size_t)n);
    cool_gen_free_locked(p);
    pthread_mutex_unlock(&cool_heap_lock);
    return 0;
}

void cool_gen_fault(void) {
    fflush(stdout);
    fputs("fatal: use of a freed or stale pointer\n", stderr);
    abort();
}

/* Reports a dereference of a null managed pointer. The untracked generation
   zero path skips the header check, so codegen tests for null there and calls
   this, keeping the named fault contract instead of dying by raw signal. */
void cool_null_fault(void) {
    fflush(stdout);
    fputs("fatal: dereference of a null pointer\n", stderr);
    abort();
}

/* Reports an array or slice index outside its bounds. Codegen emits a check
   before each array or slice index and calls this on a miss, so an out of range
   access traps instead of reading or writing past the end. */
void cool_bounds_fault(void) {
    fflush(stdout);
    fputs("fatal: index out of bounds\n", stderr);
    abort();
}

/* Location carrying fault variants. Codegen interns one "path:line" constant
   per fault site and passes it here, so a runtime fault names the statement
   that raised it instead of leaving a binary-wide search. */
void cool_bounds_fault_at(const char *loc) {
    fflush(stdout);
    fprintf(stderr, "fatal: index out of bounds at %s\n", loc);
    abort();
}

void cool_gen_fault_at(const char *loc) {
    fflush(stdout);
    fprintf(stderr, "fatal: use of a freed or stale pointer at %s\n", loc);
    abort();
}

void cool_null_fault_at(const char *loc) {
    fflush(stdout);
    fprintf(stderr, "fatal: dereference of a null pointer at %s\n", loc);
    abort();
}

void cool_shift_fault_at(const char *loc) {
    fflush(stdout);
    fprintf(stderr, "fatal: shift amount out of range at %s\n", loc);
    abort();
}

/* Reports a shift amount at or above the operand width. LLVM shifts are poison
   at such an amount, so codegen guards every dynamic shift and calls this on a
   miss, trapping loudly rather than silently masking to a wrong value. */
void cool_shift_fault(void) {
    fflush(stdout);
    fputs("fatal: shift amount out of range\n", stderr);
    abort();
}

/* Integer exponent by repeated squaring, entirely in uint64_t so the wrap is
   defined and matches the bare `mul` codegen emits. The result starts at 1, so
   `0 ** 0` is 1, the Rust pow convention. A negative exponent is meaningless for
   an integer result and faults by name rather than returning a wrong value. */
int64_t cool_pow_i64(int64_t base, int64_t exp) {
    if (exp < 0) {
        fflush(stdout);
        fputs("fatal: negative exponent in integer '**'\n", stderr);
        abort();
    }
    uint64_t result = 1;
    uint64_t b = (uint64_t)base;
    uint64_t e = (uint64_t)exp;
    while (e > 0) {
        if (e & 1) {
            result *= b;
        }
        e >>= 1;
        if (e > 0) {
            b *= b;
        }
    }
    return (int64_t)result;
}

uint64_t cool_pow_u64(uint64_t base, uint64_t exp) {
    uint64_t result = 1;
    uint64_t b = base;
    uint64_t e = exp;
    while (e > 0) {
        if (e & 1) {
            result *= b;
        }
        e >>= 1;
        if (e > 0) {
            b *= b;
        }
    }
    return result;
}

/* Copies a NUL terminated buffer into a generationally allocated buffer and
   frees the temporary, so a string handed back to the language can be freed with
   the same generational heap as every other allocation. Returns NULL, after
   freeing the temporary, when the allocation fails. */
static char *cool_gen_dup(char *tmp, size_t len) {
    char *out = (char *)cool_gen_alloc((int64_t)len + 1);
    if (out) {
        memcpy(out, tmp, len + 1);
    }
    free(tmp);
    return out;
}

/* File I/O. read slurps a whole file into a generationally allocated NUL
   terminated buffer, returning NULL on any failure so the caller's error channel
   can fire. The caller owns the buffer and frees it with the language `free`,
   which routes to the same generational heap. write truncates the file and
   writes the NUL terminated data, returning the byte count or -1 on failure. */
char *cool_read_file(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) {
        return NULL;
    }
    if (fseek(f, 0, SEEK_END) != 0) {
        fclose(f);
        return NULL;
    }
    long n = ftell(f);
    if (n < 0) {
        fclose(f);
        return NULL;
    }
    rewind(f);
    char *buf = (char *)cool_gen_alloc((int64_t)n + 1);
    if (!buf) {
        fclose(f);
        return NULL;
    }
    size_t rd = fread(buf, 1, (size_t)n, f);
    fclose(f);
    buf[rd] = '\0';
    return buf;
}

int64_t cool_write_file(const char *path, const char *data) {
    FILE *f = fopen(path, "wb");
    if (!f) {
        return -1;
    }
    size_t len = strlen(data);
    size_t wr = fwrite(data, 1, len, f);
    fclose(f);
    if (wr != len) {
        return -1;
    }
    return (int64_t)wr;
}

/* The byte size of a regular file, or -1 when it cannot be stat'd. The bootstrap
   front end reads a source into a NUL terminated buffer but a source may hold an
   embedded NUL, so the true byte length comes from the file's size rather than
   from scanning to the first NUL. */
int64_t cool_file_size(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0) {
        return -1;
    }
    return (int64_t)st.st_size;
}

int64_t cool_is_file(const char *path) {
    struct stat st;
    if (stat(path, &st) != 0) {
        return 0;
    }
    return S_ISREG(st.st_mode) ? 1 : 0;
}

/* std.fs shims. dusk never learns struct stat's, DIR's, or dirent's own
   layout: every shim below hands back plain int64/char* scalars only, the
   same discipline cool_file_size and cool_is_file already follow above.

   A second, quieter rule shapes cool_dir_open and cool_dir_next in
   particular: dusk's == rejects every pointer type outright, "pointers do
   not compare; compare the values they point to" (see check_binary in the
   sema layer), so a dusk wrapper has no way to test a returned *void or
   *raw T against NULL the way C would. Any shim that can fail hands the
   failure back through a separate int64 status instead of a nullable
   pointer: cool_dir_open's DIR* rides home as the bit pattern in an int64
   out-param, and cool_dir_next fills a caller-supplied buffer and returns a
   byte length, rather than returning a possibly-NULL char*. */

/* Fills out_size, out_mode, and out_mtime from one stat() call on path, each
   a plain int64 word so the dusk side never reads struct stat's own layout.
   st_mode carries the raw mode word, type bits and permission bits together
   (POSIX S_IFREG/S_IFDIR/S_IRWXU and friends); st_mtime is seconds since the
   epoch, UTC. Returns 0 on success or -1 on failure, leaving errno set for
   cool_errno to report. */
int64_t cool_stat(const char *path, int64_t *out_size, int64_t *out_mode, int64_t *out_mtime) {
    struct stat st;
    if (stat(path, &st) != 0) {
        return -1;
    }
    *out_size = (int64_t)st.st_size;
    *out_mode = (int64_t)st.st_mode;
    *out_mtime = (int64_t)st.st_mtime;
    return 0;
}

/* Opens a directory stream and hands the DIR* back through out_handle as its
   raw bit pattern, an opaque int64 token good only for cool_dir_next and
   cool_dir_close to cast back; see the note above for why a nullable
   pointer return will not do. Returns 0 on success or -1 on failure with
   errno set, and out_handle is only meaningful on the success path. */
int64_t cool_dir_open(const char *path, int64_t *out_handle) {
    DIR *d = opendir(path);
    if (!d) {
        return -1;
    }
    *out_handle = (int64_t)(intptr_t)d;
    return 0;
}

/* Reads the next entry of the stream named by handle_bits into buf, a
   caller supplied buffer of at least cap bytes, and returns the NUL
   excluded byte length written. A name longer than cap - 1 bytes is
   truncated to fit; cap is expected sized well past NAME_MAX so this never
   fires in practice. Returns -1 at a clean end of stream and -2 on a hard
   read error (errno set): POSIX folds both cases into one NULL return from
   readdir and tells them apart only through whether errno moved, so this
   shim clears errno before the call and checks it after, same as
   cool_dir_next's caller would have to if this were left to dusk itself. */
int64_t cool_dir_next(int64_t handle_bits, char *buf, int64_t cap) {
    DIR *d = (DIR *)(intptr_t)handle_bits;
    errno = 0;
    struct dirent *ent = readdir(d);
    if (!ent) {
        return errno != 0 ? -2 : -1;
    }
    if (cap <= 0) {
        return -2;
    }
    size_t len = strlen(ent->d_name);
    size_t room = (size_t)cap - 1;
    size_t n = len < room ? len : room;
    memcpy(buf, ent->d_name, n);
    buf[n] = '\0';
    return (int64_t)n;
}

/* Closes a directory stream opened by cool_dir_open. 0 on success, -1 on
   failure with errno set. */
int64_t cool_dir_close(int64_t handle_bits) {
    DIR *d = (DIR *)(intptr_t)handle_bits;
    if (closedir(d) != 0) {
        return -1;
    }
    return 0;
}

/* std.process shims. A popened stream rides an int64 handle the same way
   cool_dir_open's DIR* does, the bit pattern of a FILE* returned through an
   out-param: dusk's == rejects every pointer type outright, so a dusk
   wrapper has no way to null-test popen's own possibly-NULL return the way a
   C caller would; see the note above cool_dir_open for the full rule. */

/* Runs cmd through popen(cmd, mode) and hands the FILE* back through
   out_handle as its raw bit pattern. Returns 0 on success or -1 on failure
   with errno set; out_handle is only meaningful on the success path. */
int64_t cool_popen(const char *cmd, const char *mode, int64_t *out_handle) {
    FILE *fp = popen(cmd, mode);
    if (!fp) {
        return -1;
    }
    *out_handle = (int64_t)(intptr_t)fp;
    return 0;
}

/* Reads at most cap - 1 bytes, or up to and including the next newline,
   whichever comes first, from the stream named by handle into buf, a caller
   supplied buffer of at least cap bytes, NUL terminated same as fgets.
   Returns the NUL excluded byte length written, 0 at a clean end of stream,
   or -1 on a hard read error with errno set. A cap of 0 or less is reported
   as an error rather than read as a valid zero length line, the same
   defensive shape cool_dir_next's cap check follows. */
int64_t cool_fgets(int64_t handle, char *buf, int64_t cap) {
    /* A zero handle is the closed or never-opened sentinel the dusk Proc
       carries; it is never a live FILE*, so refuse it rather than dereference a
       null stream. The wrapper zeroes a Proc's handle on close, so this also
       backstops a read after close that slips past the dusk side guard. */
    if (handle == 0) {
        return -1;
    }
    FILE *fp = (FILE *)(intptr_t)handle;
    /* fgets takes an int size; a cap that is non-positive or wider than an int
       would truncate to a negative or garbage size, so refuse it up front. */
    if (cap <= 0 || cap > (int64_t)INT_MAX) {
        return -1;
    }
    errno = 0;
    if (!fgets(buf, (int)cap, fp)) {
        if (ferror(fp)) {
            return -1;
        }
        buf[0] = '\0';
        return 0;
    }
    return (int64_t)strlen(buf);
}

/* Closes a stream opened by cool_popen and returns pclose's raw wait status
   word, unmodified, the same status system() hands back to std.os's run: the
   low 7 bits carry the terminating signal and bits 8..15 carry the exit
   code, and the dusk side decodes both with run's own 128+signal convention.
   pclose itself fails only by returning -1, which no real wait status can
   equal, so the dusk wrapper tells the two apart by that exact value with
   errno set on the failure path. */
int64_t cool_pclose(int64_t handle) {
    /* A zero handle names no open stream, so there is nothing to close and
       nothing to reap: report the failure value rather than hand pclose a null
       or an already reaped FILE*, which would be a double close. */
    if (handle == 0) {
        return -1;
    }
    FILE *fp = (FILE *)(intptr_t)handle;
    return (int64_t)pclose(fp);
}

/* Nanoseconds since the Unix epoch, UTC, off CLOCK_REALTIME: whole seconds
   times one billion plus the nanosecond remainder. The one shim std.time's
   now_unix/now_ms/now_ns and its pure dusk civil calendar all derive
   from. */
int64_t cool_unix_now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

char *cool_realpath(const char *path) {
    char *tmp = realpath(path, NULL);
    if (!tmp) {
        char *empty = (char *)cool_gen_alloc(1);
        if (!empty) {
            fputs("fatal: out of memory\n", stderr);
            abort();
        }
        empty[0] = '\0';
        return empty;
    }
    return cool_gen_dup(tmp, strlen(tmp));
}

/* Reads an environment variable into a generationally allocated string. An unset
   variable comes back as the empty string, never NULL, because the language has
   no null test and treats a returned string as always valid. The caller owns the
   result and frees it with the language free, the same generational heap every
   other string uses. getenv's own buffer is copied out at once, so a later setenv
   in the same process cannot invalidate a string already handed back. */
char *cool_env(const char *name) {
    const char *v = getenv(name);
    if (!v) {
        v = "";
    }
    size_t len = strlen(v);
    char *out = (char *)cool_gen_alloc((int64_t)len + 1);
    if (!out) {
        fputs("fatal: out of memory\n", stderr);
        abort();
    }
    memcpy(out, v, len + 1);
    return out;
}

/* The C library's errno, read right after the foreign call that may have set
   it. errno is thread local in a pthreads build, so this reads the calling
   thread's own value with no lock and no race against another thread's call.
   dusk itself never sets errno; it only ever reads back what the most recent
   foreign call, strerror among them, left there. */
int64_t cool_errno(void) {
    return (int64_t)errno;
}

/* Idempotent library initialization for an exported C entry point. Every
   export's prologue calls this; pthread_once runs the body exactly once no
   matter how many entry points a C host calls or from how many threads. Today
   the body has nothing to do, since the reactor's SIGPIPE-ignore constructor
   already fires at load, but the hook exists so future load-time setup never
   changes the exported ABI. */
static pthread_once_t cool_lib_once = PTHREAD_ONCE_INIT;

static void cool_lib_init_impl(void) {
}

void cool_lib_init(void) {
    pthread_once(&cool_lib_once, cool_lib_init_impl);
}

/* Formats v as %.17g into buf, whose capacity cap includes the terminating NUL,
   and returns the byte count written without the NUL. %.17g round-trips a double
   through decimal on glibc, so the text is faithful but not the shortest form.
   A cap of zero or one, or a formatting failure, writes nothing meaningful and
   returns zero; a value that would overflow buf is truncated and the written
   length is reported. This backs json number emission. */
/* A polynomial rolling hash over a NUL terminated string's bytes, the key hash the
   generic map builds on. It reproduces the dusk hash `h = h*31 + c` exactly: each
   byte reads unsigned, the accumulator wraps in uint64_t the same way dusk's int64
   wraps in two's complement, and the final bit pattern reinterprets as int64_t, so a
   string key probes to the identical slot the old pure dusk hash reached. */
int64_t cool_hash_str(const char *s) {
    uint64_t h = 0;
    const unsigned char *p = (const unsigned char *)s;
    while (*p != 0) {
        h = h * 31u + (uint64_t)(*p);
        p++;
    }
    return (int64_t)h;
}

int64_t cool_f64_str(double v, char *buf, int64_t cap) {
    if (cap <= 0 || cap > INT_MAX) {
        return 0;
    }
    int n = snprintf(buf, (size_t)cap, "%.17g", v);
    if (n < 0) {
        buf[0] = '\0';
        return 0;
    }
    if (n >= cap) {
        return cap - 1;
    }
    return (int64_t)n;
}

/* The IEEE 754 bit pattern of a double, reinterpreted as a signed 64 bit
   integer. The bootstrap compiler emits a float constant as the hex of these
   bits, matching the host compiler which lowers a float literal as 0x{:016X} of
   f64::to_bits, so both stages produce the same textual IR for the same constant.
   The copy through memcpy is the strict aliasing safe reinterpret. */
int64_t cool_f64_bits(double x) {
    int64_t bits;
    memcpy(&bits, &x, sizeof(bits));
    return bits;
}

/* The inverse of cool_f64_bits: the double whose IEEE bits are the given
   integer, through memcpy. The float printer's test corpus builds its hard
   cases from bits, since a float literal has no exponent form. */
double cool_f64_from_bits(int64_t bits) {
    double x;
    memcpy(&x, &bits, sizeof(x));
    return x;
}

/* The std.io descriptor shims. Descriptor 1 goes through the C stdout stream
   and descriptor 2 flushes stdout and then writes the C stderr stream, so a
   Writer over either interleaves with println and printerr in program order;
   every other descriptor is a plain write(2) loop. Both paths retry EINTR and
   keep going after a short write until every byte is out, returning the count
   written; a count short of n means the OS failed with errno set (EAGAIN on a
   nonblocking descriptor, EPIPE on a closed pipe since SIGPIPE is ignored
   process wide, EBADF, ENOSPC). A write(2) that returns 0 for a positive count
   would spin, so it is reported as EIO. */
#include <unistd.h>
int64_t cool_io_write(int64_t fd, const void *buf, int64_t n) {
    if (n <= 0) {
        return 0;
    }
    if (fd == 1 || fd == 2) {
        FILE *stream = stdout;
        if (fd == 2) {
            fflush(stdout);
            stream = stderr;
        }
        const char *sp = (const char *)buf;
        size_t got = 0;
        for (;;) {
            errno = 0;
            got += fwrite(sp + got, 1, (size_t)n - got, stream);
            if (got >= (size_t)n) {
                break;
            }
            if (errno == EINTR) {
                clearerr(stream);
                continue;
            }
            if (errno == 0) {
                errno = EIO;
            }
            break;
        }
        return (int64_t)got;
    }
    const char *p = (const char *)buf;
    int64_t done = 0;
    while (done < n) {
        ssize_t w = write((int)fd, p + done, (size_t)(n - done));
        if (w < 0) {
            if (errno == EINTR) {
                continue;
            }
            return done;
        }
        if (w == 0) {
            errno = EIO;
            return done;
        }
        done += (int64_t)w;
    }
    return done;
}

/* One read(2) on fd, retried on EINTR: the count read, 0 at end of stream, or
   -1 with errno set (EAGAIN included). Descriptor 0 is read directly, not
   through the C stdin buffer, so a program picks either this path or the
   read_line builtin for its whole run. */
int64_t cool_io_read(int64_t fd, void *buf, int64_t cap) {
    if (cap <= 0) {
        return 0;
    }
    for (;;) {
        ssize_t r = read((int)fd, buf, (size_t)cap);
        if (r < 0 && errno == EINTR) {
            continue;
        }
        return (int64_t)r;
    }
}

/* Flush the C stream behind descriptor 1 or 2; every other descriptor is
   unbuffered here and flushes as a no op. 0, or -1 with errno set. */
int64_t cool_io_flush(int64_t fd) {
    FILE *stream = NULL;
    if (fd == 1) {
        stream = stdout;
    } else if (fd == 2) {
        stream = stderr;
    } else {
        return 0;
    }
    while (fflush(stream) != 0) {
        if (errno != EINTR) {
            return -1;
        }
        clearerr(stream);
    }
    return 0;
}

/* close(2); an EINTR return counts as closed, the Linux rule cool_fd_close in
   reactor.c follows. 0, or -1 with errno set. The dusk side never passes 0, 1,
   or 2. */
int64_t cool_io_close(int64_t fd) {
    if (close((int)fd) < 0 && errno != EINTR) {
        return -1;
    }
    return 0;
}

/* The bit pattern a float32 constant rounds to, returned as the double that
   equals that float, reinterpreted as a signed 64 bit integer. The host compiler
   lowers every float constant, float32 and float64 alike, as the f64 bits of the
   parsed value and narrows a float32 later with an fptrunc, so a byte for byte
   match of the emitted constant token uses cool_f64_bits. This shim rounds x to
   float first, (double)(float)x, so a consumer that needs the post rounding value
   a float32 literal actually holds, the exact double a float constant equals, has
   it directly. */
int64_t cool_f32_bits(double x) {
    float f = (float)x;
    double d = (double)f;
    int64_t bits;
    memcpy(&bits, &d, sizeof(bits));
    return bits;
}

/* Read one line from stdin into a freshly malloc'd NUL terminated buffer with
   the trailing newline stripped, returning NULL at end of input so the caller's
   error channel fires. Reads byte by byte through fgetc, so it needs no feature
   macros and behaves the same whether stdin is a terminal, a pipe, or a file. An
   empty line returns "", distinct from the NULL that marks end of input. */
char *cool_read_line(void) {
    size_t cap = 128;
    size_t len = 0;
    char *buf = malloc(cap);
    if (!buf) {
        return NULL;
    }
    int c = fgetc(stdin);
    if (c == EOF) {
        free(buf);
        return NULL;
    }
    while (c != EOF && c != '\n') {
        if (len + 1 >= cap) {
            cap *= 2;
            char *nb = realloc(buf, cap);
            if (!nb) {
                free(buf);
                return NULL;
            }
            buf = nb;
        }
        buf[len] = (char)c;
        len++;
        c = fgetc(stdin);
    }
    buf[len] = '\0';
    return cool_gen_dup(buf, len);
}

/* Read all of stdin into a freshly malloc'd NUL terminated buffer, returning
   NULL only on allocation failure. Empty stdin reads as the empty string, not an
   error, since the whole of an empty stream is nothing. */
char *cool_read_all(void) {
    size_t cap = 1024;
    size_t len = 0;
    char *buf = malloc(cap);
    if (!buf) {
        return NULL;
    }
    int c = fgetc(stdin);
    while (c != EOF) {
        if (len + 1 >= cap) {
            cap *= 2;
            char *nb = realloc(buf, cap);
            if (!nb) {
                free(buf);
                return NULL;
            }
            buf = nb;
        }
        buf[len] = (char)c;
        len++;
        c = fgetc(stdin);
    }
    buf[len] = '\0';
    return cool_gen_dup(buf, len);
}

/* Parse a base 10 floating point number, setting *ok to 1 on success and 0 when
   the string is empty or has trailing characters strtod did not consume. The
   value is returned through the result and the validity through *ok, so the
   caller can build a (float64, error) pair. */
double cool_parse_float(const char *s, int64_t *ok) {
    if (s[0] == '\0') {
        *ok = 0;
        return 0.0;
    }
    char *end;
    double v = strtod(s, &end);
    if (*end != '\0') {
        *ok = 0;
        return 0.0;
    }
    *ok = 1;
    return v;
}

/* Resolves the absolute path of the running executable into buf, returning the
   byte length written (no NUL counted) or -1 when the platform offers no way
   to ask. Linux reads the /proc/self/exe symlink; Apple asks the loader. The
   compiler uses this to find its stdlib and runtime beside a packaged install,
   the same walk the seed compiler does through current_exe. */
#if defined(__linux__)
#include <unistd.h>
int64_t cool_exe_path(char *buf, int64_t cap) {
    ssize_t n = readlink("/proc/self/exe", buf, (size_t)(cap - 1));
    if (n < 0) {
        return -1;
    }
    buf[n] = '\0';
    return (int64_t)n;
}
#elif defined(__APPLE__)
#include <mach-o/dyld.h>
int64_t cool_exe_path(char *buf, int64_t cap) {
    uint32_t size = (uint32_t)cap;
    if (_NSGetExecutablePath(buf, &size) != 0) {
        return -1;
    }
    return (int64_t)strlen(buf);
}
#else
int64_t cool_exe_path(char *buf, int64_t cap) {
    (void)buf;
    (void)cap;
    return -1;
}
#endif
