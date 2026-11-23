#include <stdint.h>

struct big { int64_t a[64]; };
static struct big kept;

/* Never spells memcpy: the struct copy is clang's, which calls the C library's. */
int64_t c_copy(struct big *src) {
    struct big tmp = *src;
    kept = tmp;
    return kept.a[3];
}
