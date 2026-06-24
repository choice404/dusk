/* Runtime linked into every coolc binary. Names are prefixed cool_ to avoid
   clashing with user symbols. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <inttypes.h>
#include <string.h>

void cool_println_cstr(const char *s) {
    puts(s);
}

void cool_print_i64(int64_t v) {
    printf("%" PRId64 "\n", v);
}

void cool_print_f64(double v) {
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
