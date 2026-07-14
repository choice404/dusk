#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>

// Variadic constructors that return a struct by value. C allows this: the
// variadic tail constrains only the arguments, never the return. Each reads its
// varargs, asserts them, and returns a struct so a wrong return ABI corrupts the
// caller's read and fails the golden.
struct P { int64_t x; int64_t y; };
struct Big { int64_t a; int64_t b; int64_t c; };

static void must(int cond, const char *what) {
    if (!cond) { fprintf(stderr, "cabi_variadic_struct_return: %s\n", what); abort(); }
}

// Returns a sixteen byte struct in the rax:rdx register pair.
struct P mkp(int64_t base, ...) {
    va_list ap; va_start(ap, base);
    int64_t a = va_arg(ap, int64_t);
    int64_t b = va_arg(ap, int64_t);
    va_end(ap);
    must(base == 5 && a == 10 && b == 20, "mkp args");
    struct P r = { base + a, base + b };
    return r;
}

// Returns a twenty four byte struct through the hidden sret pointer.
struct Big mkbig(int64_t base, ...) {
    va_list ap; va_start(ap, base);
    int64_t x = va_arg(ap, int64_t);
    va_end(ap);
    must(base == 100 && x == 7, "mkbig args");
    struct Big r = { base, x, base + x };
    return r;
}
