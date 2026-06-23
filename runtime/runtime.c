/* Runtime linked into every coolc binary. Names are prefixed cool_ to avoid
   clashing with user symbols. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <inttypes.h>

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
