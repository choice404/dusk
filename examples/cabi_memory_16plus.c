#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

// Structs larger than two eightbytes travel in memory: the caller copies the
// value onto the stack and hands the callee a byval pointer. The receiver reads
// its fields straight out of that copy and asserts each one.
struct M24 { int64_t a; int64_t b; int64_t c; };
struct M20 { int32_t a; int32_t b; int32_t c; int32_t d; int32_t e; };

static void must(int cond, const char *what) {
    if (!cond) { fprintf(stderr, "cabi_memory_16plus: %s\n", what); abort(); }
}

int64_t take_m24(struct M24 x) {
    must(x.a == 1, "M24.a"); must(x.b == 2, "M24.b"); must(x.c == 3, "M24.c");
    return x.a + x.b * 10 + x.c * 100;
}
int64_t take_m20(struct M20 x) {
    must(x.a == 1, "M20.a"); must(x.b == 2, "M20.b"); must(x.c == 3, "M20.c");
    must(x.d == 4, "M20.d"); must(x.e == 5, "M20.e");
    return (int64_t)x.a + x.b * 10 + x.c * 100 + x.d * 1000 + (int64_t)x.e * 10000;
}
