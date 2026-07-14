#include <stdint.h>

// Foreign functions that return a small struct in registers: an {i64,i64} pair,
// a {double,double} SSE pair, and a mixed {i64,double} pair. dusk reassembles
// the returned register pair into the struct value its fields read from. The
// take_* twins accept the same structs back so the round trip covers both
// directions of the register-pair ABI.
struct LL { int64_t a; int64_t b; };
struct FF { double a; double b; };
struct LF { int64_t a; double b; };

struct LL ret_ll(void) { struct LL r = {11, 22}; return r; }
struct FF ret_ff(void) { struct FF r = {1.5, 2.5}; return r; }
struct LF ret_lf(void) { struct LF r = {7, 3.5}; return r; }

int64_t take_ff(struct FF x) {
    return (int64_t)(x.a * 10) + (int64_t)(x.b * 10) * 1000;
}
int64_t take_lf(struct LF x) {
    return x.a * 1000 + (int64_t)(x.b * 10);
}
