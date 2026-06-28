/* Runtime linked into every coolc binary. Names are prefixed cool_ to avoid
   clashing with user symbols. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <inttypes.h>
#include <string.h>

/* print writes the value with no newline, println appends one. The builtins
   print and println in the language map to the matching pair per value type. */
void cool_print_cstr(const char *s) {
    fputs(s, stdout);
}

void cool_println_cstr(const char *s) {
    puts(s);
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
    if (cool_dbg_count < COOL_DBG_MAX) {
        cool_dbg_ptr[cool_dbg_count] = p;
        cool_dbg_size[cool_dbg_count] = n;
        cool_dbg_freed[cool_dbg_count] = 0;
        cool_dbg_count++;
    }
    return p;
}

void cool_debug_free(void *p) {
    for (int i = 0; i < cool_dbg_count; i++) {
        if (cool_dbg_ptr[i] == p) {
            if (cool_dbg_freed[i]) {
                cool_dbg_double++;
                return;
            }
            cool_dbg_freed[i] = 1;
            memset(p, 0xDD, (size_t)cool_dbg_size[i]);
            return;
        }
    }
    cool_dbg_double++;
}

int64_t cool_debug_leaks(void) {
    int64_t n = 0;
    for (int i = 0; i < cool_dbg_count; i++) {
        if (!cool_dbg_freed[i]) {
            n++;
        }
    }
    return n;
}

int64_t cool_debug_double_frees(void) {
    return cool_dbg_double;
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

void *cool_gen_alloc(int64_t size) {
    for (int i = 0; i < cool_gen_free_n; i++) {
        if (cool_gen_free_sz[i] == size) {
            void *p = cool_gen_free_ptr[i];
            cool_gen_free_n--;
            cool_gen_free_ptr[i] = cool_gen_free_ptr[cool_gen_free_n];
            cool_gen_free_sz[i] = cool_gen_free_sz[cool_gen_free_n];
            return p;
        }
    }
    char *base = malloc(COOL_GEN_HDR + (size_t)size);
    if (!base) {
        return NULL;
    }
    int64_t *hdr = (int64_t *)base;
    hdr[0] = size;
    hdr[1] = 1;
    return base + COOL_GEN_HDR;
}

void cool_gen_free(void *p) {
    if (!p) {
        return;
    }
    // Double free guard: a block already parked on the free list must not be
    // parked again, or a later allocation could hand the same address out twice.
    for (int i = 0; i < cool_gen_free_n; i++) {
        if (cool_gen_free_ptr[i] == p) {
            fflush(stdout);
            fputs("fatal: double free\n", stderr);
            abort();
        }
    }
    int64_t *gen = (int64_t *)((char *)p - 8);
    *gen += 1;
    int64_t *size = (int64_t *)((char *)p - COOL_GEN_HDR);
    if (cool_gen_free_n < COOL_GEN_FREE_MAX) {
        cool_gen_free_ptr[cool_gen_free_n] = p;
        cool_gen_free_sz[cool_gen_free_n] = *size;
        cool_gen_free_n++;
    }
}

void cool_gen_fault(void) {
    fflush(stdout);
    fputs("fatal: use of a freed or stale pointer\n", stderr);
    abort();
}

/* File I/O. read slurps a whole file into a freshly malloc'd NUL terminated
   buffer, returning NULL on any failure so the caller's error channel can fire.
   The caller owns the buffer and may free it. write truncates the file and
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
    char *buf = malloc((size_t)n + 1);
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
    return buf;
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
    return buf;
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
