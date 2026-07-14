#include <stdint.h>

// A foreign function returning a struct larger than sixteen bytes writes it
// through a caller-provided sret pointer and returns void. dusk allocates the
// slot, passes it as the hidden first argument, and reads the struct back out of
// it after the call.
struct M24 { int64_t a; int64_t b; int64_t c; };

struct M24 ret_m24(void) { struct M24 r = {7, 8, 9}; return r; }
int64_t take_m24(struct M24 x) { return x.a + x.b * 10 + x.c * 100; }
