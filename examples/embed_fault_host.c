/* A C host that calls a faulting dusk export. The first argument selects which:
   "boom" drives an out of bounds index, "collect" drives a collector mint in a
   library with no main anchor. Either aborts the whole process by name, which the
   test harness asserts. */
#include <stdio.h>
#include "embed_fault_lib.h"

int main(int argc, char** argv) {
    if (argc > 1 && argv[1][0] == 'c') {
        printf("calling collect\n");
        fflush(stdout);
        printf("%lld\n", (long long)lib_collect(5));
    } else {
        printf("calling boom\n");
        fflush(stdout);
        printf("%lld\n", (long long)lib_boom(99));
    }
    return 0;
}
