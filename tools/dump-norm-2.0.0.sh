#!/bin/sh
# dump-norm-2.0.0.sh: the IR normalizer for dump differentials across the 2.0.0
# line, read through DUSK_DUMP_NORM by tools/dump-differential.sh. It undoes the
# changes 2.0.0 makes to every function head and every heap free, so a compare
# against a 1.17.3 compiler shows what else moved rather than burying it under a
# difference in nearly every module:
#   - every define but the C entry gained internal linkage, and every define the
#     nobuiltin attribute, so a head is read as it was;
#   - a heap free calls cool_gen_free_at with a location constant from its own
#     @.loc.N family, so the call is read as the old cool_gen_free and the
#     location constants and their declare are dropped.
# A dropped line can leave two blank lines where a section ended, so blank runs
# are squeezed on both sides alike.
#
# usage: tools/dump-norm-2.0.0.sh < ir > normalized
sed -e 's/^define internal /define /' \
    -e 's/) nobuiltin {$/) {/' \
    -e 's/^\(  call void @cool_gen_free\)_at(ptr \(.*\), ptr @\.loc\.[0-9]*)$/\1(ptr \2)/' \
    -e '/^@\.loc\.[0-9]* = /d' \
    -e '/^declare void @cool_gen_free_at(ptr, ptr)$/d' | cat -s
