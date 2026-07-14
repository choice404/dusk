#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

// Structs that split into one INTEGER eightbyte and one SSE eightbyte, so the
// integer field rides a general purpose register and the double rides an xmm
// register. The receiver asserts both fields and folds the double's tenths into
// the checksum so a value routed to the wrong register class is caught.
struct LF { int64_t a; double b; };
struct FL { double a; int64_t b; };

static void must(int cond, const char *what) {
    if (!cond) { fprintf(stderr, "cabi_small_mixed: %s\n", what); abort(); }
}

int64_t take_lf(struct LF x) {
    must(x.a == 5, "LF.a"); must(x.b > 3.49 && x.b < 3.51, "LF.b");
    return x.a * 1000 + (int64_t)(x.b * 10);
}
int64_t take_fl(struct FL x) {
    must(x.a > 2.49 && x.a < 2.51, "FL.a"); must(x.b == 7, "FL.b");
    return x.b * 1000 + (int64_t)(x.a * 10);
}
