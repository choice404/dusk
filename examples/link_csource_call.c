#include <stdint.h>

/* Calls a dusk function by its bare name. The dusk side must declare it
   export "C" for the link to find it: every other dusk function has internal
   linkage and no C file can name it. */
int64_t dusk_twice(int64_t x);

int64_t c_calls_dusk(int64_t x) {
    return dusk_twice(x) + 1;
}
