#include <stdint.h>

/* Calls the helper the sibling C source defines. */
int64_t helper(void);

int64_t c_entry(void) { return helper() + 1; }
