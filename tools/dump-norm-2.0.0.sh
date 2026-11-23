#!/bin/sh
# dump-norm-2.0.0.sh: the IR normalizer for dump differentials across the 2.0.0
# line, read through DUSK_DUMP_NORM by tools/dump-differential.sh. It undoes the
# changes 2.0.0 makes to every function head, every function symbol, and every
# heap free, so a compare against a 1.17.3 compiler shows what else moved rather
# than burying it under a difference in nearly every module:
#   - every define but the C entry and an export "C" function gained internal
#     linkage and a .d suffix on its symbol, together, so a head that has both is
#     read as it was and a head with only one of the two still differs; main's
#     body, which was dusk__main, is main.d;
#   - every define gained the nobuiltin attribute;
#   - a reference to a function by its suffixed symbol, a call, a function value,
#     or a table slot, is read under the old name;
#   - a heap free calls cool_gen_free_at with a location constant from its own
#     @.loc.N family, so the call is read as the old cool_gen_free and the
#     location constants and their declare are dropped;
#   - an interface table's symbol gained a .vt suffix, so it can never meet the
#     symbol of a method of a struct named vtable, and is read without it.
# A reference rewritten here cannot hide a wrong target: a suffixed symbol that
# names no define fails clang's parse of the module, and the suite runs every
# program. A dropped line can leave two blank lines where a section ended, so
# blank runs are squeezed on both sides alike.
#
# usage: tools/dump-norm-2.0.0.sh < ir > normalized
sed -e 's/^define internal \([^@]*\) @main\.d(/define \1 @dusk__main(/' \
    -e 's/^define internal \([^@]*\) @\([-A-Za-z0-9_$.]*\)\.d(/define \1 @\2(/' \
    -e 's/) nobuiltin {$/) {/' \
    -e 's/@\(vtable\.[-A-Za-z0-9_$]*\.[-A-Za-z0-9_$]*\)\.vt\([^-A-Za-z0-9_$.]\)/@\1\2/g' \
    -e 's/@main\.d\([^-A-Za-z0-9_$.]\)/@dusk__main\1/g' \
    -e 's/@\([-A-Za-z0-9_$.]*[-A-Za-z0-9_$]\)\.d\([^-A-Za-z0-9_$.]\)/@\1\2/g' \
    -e 's/^\(  call void @cool_gen_free\)_at(ptr \(.*\), ptr @\.loc\.[0-9]*)$/\1(ptr \2)/' \
    -e '/^@\.loc\.[0-9]* = /d' \
    -e '/^declare void @cool_gen_free_at(ptr, ptr)$/d' | cat -s
