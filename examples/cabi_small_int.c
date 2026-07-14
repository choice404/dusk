#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

// Structs whose fields are all integers small enough to ride in general purpose
// registers. Each receiver byte-asserts every field against the value dusk was
// told to send, aborting loudly on a mismatch, then returns a position-weighted
// checksum so a field that landed in the wrong register also fails the golden.
struct II { int32_t a; int32_t b; };
struct LL { int64_t a; int64_t b; };
struct I3 { int32_t a; int32_t b; int32_t c; };
struct BB { int8_t a; int8_t b; int8_t c; };

static void must(int cond, const char *what) {
    if (!cond) { fprintf(stderr, "cabi_small_int: %s\n", what); abort(); }
}

int64_t take_ii(struct II x) {
    must(x.a == 10, "II.a"); must(x.b == 20, "II.b");
    return (int64_t)x.a + (int64_t)x.b * 1000;
}
int64_t take_ll(struct LL x) {
    must(x.a == 100, "LL.a"); must(x.b == 200, "LL.b");
    return x.a + x.b * 1000;
}
int64_t take_i3(struct I3 x) {
    must(x.a == 1, "I3.a"); must(x.b == 2, "I3.b"); must(x.c == 3, "I3.c");
    return (int64_t)x.a + (int64_t)x.b * 10 + (int64_t)x.c * 100;
}
int64_t take_bb(struct BB x) {
    must(x.a == 4, "BB.a"); must(x.b == 5, "BB.b"); must(x.c == 6, "BB.c");
    return (int64_t)x.a + (int64_t)x.b * 10 + (int64_t)x.c * 100;
}
