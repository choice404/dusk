#include <stdint.h>

/* Calls getuid by its bare name, which the dusk file also defines. */
int getuid(void);

int64_t c_calls_getuid(void) {
    return (int64_t)getuid();
}
