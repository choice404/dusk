/* Runtime linked into every coolc binary. Names are prefixed cool_ to avoid
   clashing with user symbols. The heap is thread safe: one mutex guards the
   generational free list and the debug tables, and the generation word is
   accessed atomically on both sides of the dereference check, so the check
   machinery itself is never a C level data race. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <inttypes.h>
#include <string.h>
#include <pthread.h>

static pthread_mutex_t cool_heap_lock = PTHREAD_MUTEX_INITIALIZER;

void cool_gen_fault(void);

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

void cool_print_i64(int64_t v) {
    printf("%" PRId64, v);
}

void cool_println_i64(int64_t v) {
    printf("%" PRId64 "\n", v);
}

void cool_print_f64(double v) {
    printf("%g", v);
}

void cool_println_f64(double v) {
    printf("%g\n", v);
}

void *cool_alloc(size_t n) {
    return malloc(n);
}

void cool_free(void *p) {
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

/* Generational heap for managed pointers. Each managed allocation carries a 16
   byte header in front of the data, holding the payload size and a generation,
   with the generation in the word right before the data so a check is a single
   load at p minus 8. free bumps the generation and parks the block on a size
   matched free list. A later allocation of the same size reuses the block with
   its now advanced generation, so a stale reference still holding the old
   generation mismatches and faults at its next dereference. The generation
   never resets for a block, which is what keeps reuse sound. */
#define COOL_GEN_HDR 16
#define COOL_GEN_FREE_MAX 4096
static void *cool_gen_free_ptr[COOL_GEN_FREE_MAX];
static int64_t cool_gen_free_sz[COOL_GEN_FREE_MAX];
static int cool_gen_free_n = 0;

/* Live block registry. One entry per currently handed out generational payload,
   with its payload size, so the collector can scan every live generational
   block as a root region. Every alloc inserts and every free removes, both
   under the heap lock, keeping exactly one entry per payload. The collector
   copies a snapshot under the lock and scans it released, so a block another
   thread frees mid scan is still mapped, parked not returned, and reads only
   over retain. */
static void **cool_reg_ptr = NULL;
static int64_t *cool_reg_size = NULL;
static int64_t cool_reg_n = 0;
static int64_t cool_reg_cap = 0;

/* Inserts a payload with the heap lock held. Growth failure is out of memory,
   fatal, since dropping an entry would under root the collector and free a
   still reachable block. */
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

/* Removes a payload with the heap lock held, scanning from the end so a last in
   first out free, the common defer pattern, resolves in one step. A swap remove
   keeps the table dense. A payload not present is a no op. */
static void cool_reg_remove_locked(void *p) {
    for (int64_t i = cool_reg_n - 1; i >= 0; i--) {
        if (cool_reg_ptr[i] == p) {
            cool_reg_n--;
            cool_reg_ptr[i] = cool_reg_ptr[cool_reg_n];
            cool_reg_size[i] = cool_reg_size[cool_reg_n];
            return;
        }
    }
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
    memcpy(pp, cool_reg_ptr, (size_t)n * sizeof(void *));
    memcpy(ss, cool_reg_size, (size_t)n * sizeof(int64_t));
    pthread_mutex_unlock(&cool_heap_lock);
    *ptrs = pp;
    *sizes = ss;
    return n;
}

void *cool_gen_alloc(int64_t size) {
    pthread_mutex_lock(&cool_heap_lock);
    for (int i = 0; i < cool_gen_free_n; i++) {
        if (cool_gen_free_sz[i] == size) {
            void *p = cool_gen_free_ptr[i];
            cool_gen_free_n--;
            cool_gen_free_ptr[i] = cool_gen_free_ptr[cool_gen_free_n];
            cool_gen_free_sz[i] = cool_gen_free_sz[cool_gen_free_n];
            cool_reg_add_locked(p, size);
            pthread_mutex_unlock(&cool_heap_lock);
            return p;
        }
    }
    pthread_mutex_unlock(&cool_heap_lock);
    char *base = malloc(COOL_GEN_HDR + (size_t)size);
    if (!base) {
        return NULL;
    }
    int64_t *hdr = (int64_t *)base;
    hdr[0] = size;
    __atomic_store_n(&hdr[1], 1, __ATOMIC_SEQ_CST);
    void *p = base + COOL_GEN_HDR;
    pthread_mutex_lock(&cool_heap_lock);
    cool_reg_add_locked(p, size);
    pthread_mutex_unlock(&cool_heap_lock);
    return p;
}

/* The retire path with the heap lock already held. */
static void cool_gen_free_locked(void *p) {
    // Double free guard: a block already parked on the free list must not be
    // parked again, or a later allocation could hand the same address out twice.
    for (int i = 0; i < cool_gen_free_n; i++) {
        if (cool_gen_free_ptr[i] == p) {
            fflush(stdout);
            fputs("fatal: double free\n", stderr);
            abort();
        }
    }
    // The generation bump is atomic because the dereference check reads the
    // word without the heap lock, from any thread.
    int64_t *gen = (int64_t *)((char *)p - 8);
    __atomic_fetch_add(gen, 1, __ATOMIC_SEQ_CST);
    int64_t *size = (int64_t *)((char *)p - COOL_GEN_HDR);
    cool_reg_remove_locked(p);
    if (cool_gen_free_n < COOL_GEN_FREE_MAX) {
        cool_gen_free_ptr[cool_gen_free_n] = p;
        cool_gen_free_sz[cool_gen_free_n] = *size;
        cool_gen_free_n++;
    }
}

void cool_gen_free(void *p) {
    if (!p) {
        return;
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
    int64_t *g = (int64_t *)((char *)p - 8);
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
