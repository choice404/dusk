/* The C library's own rendering of a double, the oracle the float printer's goldens
   compare against. It is test material only: the two oracle goldens bind it through
   the @csource directive, and nothing in the runtime or the standard library links
   it. The dusk side owns the buffer and passes its capacity, so the wrapper never
   allocates and never formats into storage it does not own.

   kind selects the conversion, 0 for %e, 1 for %f, 2 for %g, 3 for %E, and 4 for
   %G, and prec is the precision the directive carries. The return is the byte count
   written, not counting the NUL, or -1 when the kind is unknown, the capacity is not
   positive, or the rendering did not fit. */
#include <stdint.h>
#include <stdio.h>

int64_t fmt_oracle(double x, int64_t kind, int64_t prec, char *out, int64_t cap) {
    static const char *const kinds[5] = {"%.*e", "%.*f", "%.*g", "%.*E", "%.*G"};
    if (kind < 0 || kind > 4 || cap <= 0) {
        return -1;
    }
    int n = snprintf(out, (size_t)cap, kinds[kind], (int)prec, x);
    if (n < 0 || (int64_t)n >= cap) {
        return -1;
    }
    return (int64_t)n;
}
