#include <stdint.h>

/* Calls the runtime's allocator by its C name. */
void *cool_gen_alloc(int64_t n);

int64_t c_alloc(void) {
    char *p = cool_gen_alloc(8);
    p[0] = 9;
    return p[0];
}
