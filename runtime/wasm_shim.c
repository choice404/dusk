/* wasm-only runtime shims for the browser playground (wasm32-wasip1).

   Compiled into the dusk.wasm build in place of the runtime pieces wasi-libc
   cannot provide. The playground runs only `check` and `build` (IR emit, no
   link), so every function here is either never reached on that path (the
   process and shell layer) or a safe generational-only degradation (the
   collected heap, which the compiler never uses and whose collect.c cannot
   compile for wasm because setjmp needs the not-yet-shipped exception-handling
   proposal). The compiler only emits IR for wasm; the wasi link that consumes
   this file happens outside it, with collect.c left out of that link's source
   list. A native build never sees this file. */

#include <stdio.h>
#include <stdint.h>

/* Process and shell layer: absent from wasi-libc. These back std.os.run and
   std.process, which the playground never invokes. system returns the int64
   std.os declares for it, so the wasm signature matches the emitted call. */
long long system(const char *command) {
    (void)command;
    return -1;
}
FILE *popen(const char *command, const char *type) {
    (void)command;
    (void)type;
    return NULL;
}
int pclose(FILE *stream) {
    (void)stream;
    return -1;
}

/* Collected heap: collect.c is excluded from the wasm build. The compiler never
   mints a collector<T>, so the anchor prologue every main carries is a no-op
   here, and every pointer reports non-collected, which leaves the generational
   heap (runtime.c) to own all memory. These mirror the signatures runtime.c and
   the emitted IR expect from collect.c. */
void cool_gc_anchor(void *sp) {
    (void)sp;
}
int cool_gc_is_collected(void *p) {
    (void)p;
    return 0;
}
