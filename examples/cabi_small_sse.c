#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

// A struct of two float64 fields is two SSE eightbytes, passed as a (double,
// double) register pair in xmm0 and xmm1. Since float32 is banned in a boundary
// struct, an SSE eightbyte is always exactly one double, never a packed
// <2 x float>.
struct FF { double a; double b; };
struct F1 { double a; };

static void must(int cond, const char *what) {
    if (!cond) { fprintf(stderr, "cabi_small_sse: %s\n", what); abort(); }
}

int64_t take_ff(struct FF x) {
    must(x.a > 1.49 && x.a < 1.51, "FF.a"); must(x.b > 2.49 && x.b < 2.51, "FF.b");
    return (int64_t)(x.a * 10) + (int64_t)(x.b * 10) * 1000;
}
int64_t take_f1(struct F1 x) {
    must(x.a > 6.24 && x.a < 6.26, "F1.a");
    return (int64_t)(x.a * 100);
}
