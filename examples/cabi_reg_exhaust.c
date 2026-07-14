#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

// Register exhaustion receivers. Each asserts every scalar and struct field it
// was told to expect, aborting loudly on a mismatch, then returns a position
// weighted checksum so a field that landed in the wrong register also fails the
// golden. The struct arguments are the interesting ones: the caller must have
// passed them whole on the stack (byval), not straddling a leftover register.
struct P { int64_t x; int64_t y; };
struct Q { double x; double y; };

static void must(int cond, const char *what) {
    if (!cond) { fprintf(stderr, "cabi_reg_exhaust: %s\n", what); abort(); }
}

// Five INTEGER scalars fill rdi..r8, leaving one INT register free; P needs two,
// so the whole struct rides the stack, not r9 plus memory.
int64_t f6(int64_t a, int64_t b, int64_t c, int64_t d, int64_t e, struct P p) {
    must(a == 1 && b == 1 && c == 1 && d == 1 && e == 1, "f6 scalars");
    must(p.x == 7 && p.y == 9, "f6 struct");
    return a + b + c + d + e + p.x * 10 + p.y;
}

// Seven SSE scalars fill xmm0..xmm6, leaving one SSE register free; Q needs two,
// so Q rides the stack.
int64_t g8(double a, double b, double c, double d, double e, double f, double g, struct Q q) {
    must(a == 1.0 && g == 1.0, "g8 scalars");
    must(q.x == 7.0 && q.y == 9.0, "g8 struct");
    return (int64_t)(a + b + c + d + e + f + g) + (int64_t)(q.x * 10.0) + (int64_t)q.y;
}

// Four INTEGER scalars leave two INT registers free; P needs exactly two and so
// stays in registers, proving the walk does not over-force a struct to memory.
int64_t f4(int64_t a, int64_t b, int64_t c, int64_t d, struct P p) {
    must(a == 1 && b == 1 && c == 1 && d == 1, "f4 scalars");
    must(p.x == 7 && p.y == 9, "f4 struct");
    return a + b + c + d + p.x * 100 + p.y;
}
