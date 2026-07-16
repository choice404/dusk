/* A C host that links against the dusk library libembed_lib.a and calls its
   exported entry points through the generated header. This is the proof that a
   dusk module compiled with `dusk build --lib` is callable from C, and by
   extension from any language with a C FFI. */
#include <stdio.h>
#include "embed_lib.h"

int main(void) {
    int64_t xs[4] = {1, 2, 3, 4};
    printf("add=%lld\n", (long long)embed_add(40, 2));
    printf("scale=%.2f\n", embed_scale(1.5, 3.0));
    printf("sum=%lld\n", (long long)embed_sum(xs, 4));
    return 0;
}
