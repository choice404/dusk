# Sanitizer builds

A sanitized build is a dusk program compiled with clang's sanitizers, so the program finds its own memory and threading defects in seconds instead of in a long soak. `dusk build`, `dusk run`, and `dusk ir` take `--sanitize=<list>`, the list a comma separated subset of `address`, `undefined`, and `thread`. This page is the working guide: what each word instruments, what a report looks like, the defaults the runtime sets and why, and how the test suite uses them. The spec's [Sanitizer Builds](../spec.md#sanitizer-builds) section is the reference.

## Usage

```sh
DUSK_HOME=$PWD target/dusk-out/dusk run --sanitize=address examples/asan_string_uaf.dusk
DUSK_HOME=$PWD target/dusk-out/dusk build --sanitize=thread examples/tsan_race_reported.dusk
DUSK_HOME=$PWD target/dusk-out/dusk build --sanitize=address,undefined --heap-sites app.dusk
```

`run` reads its flags before the file, and every word after the file belongs to the program. `address` and `thread` do not combine, since clang refuses the pair, so build one binary for each. A sanitized build needs clang's sanitizer runtimes, the compiler-rt package; the build links an empty C program against them first and stops with `dusk: --sanitize=address needs clang's sanitizer runtimes (the compiler-rt package), and clang could not link against them` when it cannot. A plain build never asks for them.

## The three words

| Word | Dusk code | The runtime and every `@csource` | The generational heap |
| ---- | --------- | -------------------------------- | --------------------- |
| `address` | every function carries `sanitize_address`, so every load and store it makes is checked | compiled with `-fsanitize=address` | really frees, at the exact size asked for |
| `thread` | every function carries `sanitize_thread`, so every access it makes is seen | compiled with `-fsanitize=thread` | parks freed blocks, as in a plain build |
| `undefined` | nothing | compiled with `-fsanitize=undefined` | parks freed blocks, as in a plain build |

`undefined` reaches C only. UndefinedBehaviorSanitizer is inserted by clang's C front end, and a `.ll` file has no front end to pass through, so nothing in a dusk function is instrumented. Dusk's own checks already make the operations it would watch defined: an index out of bounds, a null dereference, an oversized shift, and a stale pointer each fault by name in every build. The word is there for the runtime and for a program's own `@csource` files.

With `address` or `thread` every function also carries a frame pointer and an unwind table, so a report's stacks walk through dusk frames. A dusk function shows under its symbol, its name with the `.d` suffix, `reader.d`, and its file as the module's `.ll`, since a dusk build carries no line tables yet. The runtime compiles at `-O1` with debug info, so a runtime frame names its C file and line.

## AddressSanitizer and the heap

A plain build never hands a freed block back to the C allocator. It bumps the block's generation and parks it for reuse at any size its class holds, which is what makes a stale managed pointer fault by name, `fatal: use of a freed or stale pointer at path:line`. A parked block is still mapped, so AddressSanitizer would see nothing wrong with a read of it, and a block rounded up to its class hides a write a few bytes past the request.

Under `address` the program's blocks take a different path. An allocation is one `malloc` of exactly the bytes asked for behind the header, so the sanitizer's redzone starts at the first byte past the request, and a free is a real `free`, so the sanitizer quarantines the block and reports the next touch of it. A string read after its free:

```text
==1744672==ERROR: AddressSanitizer: heap-use-after-free on address 0x7bd26ffe0070
READ of size 1 at 0x7bd26ffe0070 thread T0
    #0 in strnlen.part.0 asan_interceptors.cpp.o
    #1 in cool_sx_measure runtime/runtime.c:824:28
    #2 in cool_str_index_at runtime/runtime.c:847:10
    #3 in reader.d asan_string_uaf.ll
    #4 in main.d asan_string_uaf.ll
freed by thread T0 here:
    #0 in free.part.0 asan_malloc_linux.cpp.o
    #1 in cool_asan_free runtime/heap_asan.h:124:5
    #2 in cool_gen_free_at runtime/runtime.c:584:5
    #3 in main.d asan_string_uaf.ll
previously allocated by thread T0 here:
    ...
```

Each case reads the same way:

- A read or write past the end of an `alloc_bytes` block is a `heap-buffer-overflow` in the dusk function that makes it, `examples/asan_raw_overflow.dusk`.
- A second `free` is a `heap-use-after-free` raised in `cool_gen_free_at` on its first read of the block, with the stacks of both frees, `examples/asan_double_free.dusk`. A plain build names it `fatal: double free at path:line`.
- A managed pointer used after its block is freed is reported at the generation load in the dusk function, before the generation compare runs, `examples/asan_managed_uaf.dusk`.

Generations still move. Every block takes its first generation from one process wide counter that never repeats, so a block minted at an address a freed block held starts above every generation that address carried, and a stale reference still mismatches. What is weaker than a plain build: once the C allocator hands a freed address to something that is not a generational block, a stale dereference reads whatever word sits in front of it. In a sanitized build the quarantine and the report on that load are the guard, not the generation, and a quarantine can be outrun by a program that frees enough in between.

The runtime's own records stay on the parking path in this build too: the record behind a future and the join record of a spawned thread. A completion that arrives after the awaiter consumed the future, and a second `join`, are defined behavior that reads the retired record's generation, so a record must stay mapped. `examples/asan_late_complete_ok.dusk` runs clean and prints `future already completed`, and `examples/doublejoin.dusk` still faults by name.

The collector scans the heap's blocks as roots with no lock held. Under `address` a real free waits for a scan in progress, and the scan itself is left uninstrumented, since it reads stack redzones and the bytes of blocks other threads write by design. The runtime takes its locks in one order, the collector's, then the one a free waits on, then the heap's, so a free and a scan never wait on each other in a cycle.

`std.memory.heap` reads the same counters in every build. Under `address` the held bytes drop with every free, since a freed block leaves the heap.

## The defaults the runtime sets

A sanitized runtime sets two AddressSanitizer options of its own:

- `detect_stack_use_after_return=0`. With the option on, a local of an instrumented function lives on a fake stack in the heap, where the collector's conservative stack scan never looks, so a `collector<T>` value held only in a local is swept while it is live and the next read of it faults. This one is a soundness requirement for any program that uses `collector<T>`.
- `detect_leaks=0`. Every live block is reachable through the heap's registry, so LeakSanitizer would only ever report the runtime's own residue. The leak tool is `--heap-sites`, below.

`ASAN_OPTIONS` still applies on top of these, one option at a time, so `ASAN_OPTIONS=hard_rss_limit_mb=4096` keeps both defaults. Setting `detect_stack_use_after_return=1` there is unsupported in a program that uses `collector<T>`. The defaults are one strong `__asan_default_options` in the runtime, so a `@csource` that defines its own fails to link; set `ASAN_OPTIONS` instead.

## ThreadSanitizer

Under `thread` every access a dusk function makes is instrumented, and the heap parks its blocks as in a plain build. A race on any memory is reported in the dusk function that makes the access, a heap block's included:

```text
WARNING: ThreadSanitizer: data race (pid=1746229)
  Write of size 8 at 0x7b0c00000110 by thread T2:
    #0 bump.d tsan_heap_race.ll
    #1 lambda.1.d tsan_heap_race.ll
    #2 cool_thread_tramp runtime/thread.c:48:5
```

Races are found by scheduling, so one clean run proves little. Run a threaded program in a loop:

```sh
DUSK_HOME=$PWD target/dusk-out/dusk build --sanitize=thread examples/reactorsum.dusk
for i in $(seq 1 20); do
    target/dusk-out/reactorsum || { echo "failed on iteration $i"; break; }
done
```

A clean pass prints the program's output twenty times with no `WARNING: ThreadSanitizer` and exit code 0 every time.

Before 2.0.0 this page was a hand recipe that compiled a program's emitted IR beside the runtime's C files under `clang -fsanitize=thread`. The IR carried no `sanitize_thread` attribute, so clang instrumented only function entry and exit in it, and every ThreadSanitizer run on record before 2.0.0, the reactor, the pool, the stress programs, and the collector among them, checked the C runtime's accesses and never one a dusk function made.

## The address space

AddressSanitizer and ThreadSanitizer reserve terabytes of address space for their shadow memory, so a sanitized binary cannot run under a `ulimit -v` cage. Bound one with a CPU limit, a timeout, and the sanitizer's own resident limit instead:

```sh
(ulimit -t 600; ASAN_OPTIONS=hard_rss_limit_mb=4096 timeout 300 target/dusk-out/app)
```

## With heap sites

`--heap-sites` composes with every word. It records the line of the program's own code behind each allocation and prints the blocks still live at exit, grouped by that line and the calls that led there, so a leak names its line where a sanitizer's stack names only functions. Build with both while hunting a leak and a use after free in one program; the report of `examples/sites_basic.dusk` is the same under `address` as without it. The spec's [Heap Sites](../spec.md#heap-sites) section covers the report.

## The suite

`tests/sanitize.manifest` holds the records that build with `--sanitize`. It runs without a `ulimit -v` cage:

```sh
DUSK_HOME=$PWD DUSK_BIN=target/dusk-out/dusk target/dusk-out/testrun tests/sanitize.manifest
```

For a hunt across the whole suite, build every `run` record of `tests/goldens.manifest` with `--sanitize=address`, run it the way the runner does, and compare its exit status and stdout with the plain build's. A record whose plain build faults by name, a use after free or a double free the suite pins on purpose, reports there as an AddressSanitizer error at the access instead, which is the sanitizer doing its job. For 2.0.0 every other report was read: two examples read a received datagram as a string past the end of its buffer, `examples/anet_udp.dusk` and `examples/udp_round6.dusk`, and now terminate it inside the buffer, and `examples/heapclass_grow.dusk` caps its own address space at one gibibyte, below what the sanitizer reserves, so it cannot run sanitized by design.<!--TODO: the tally once the release gate reruns the sweep-->

## The collector and one benign read

The collected heap has one deliberate cross thread read. A collection scans the live generational blocks as a root region: it copies the list of them under the heap lock, releases the lock, and scans the copied payloads, so an allocation or a free on another thread does not wait for the whole scan. While it scans, another thread may be writing one of those payloads, a future record a pool worker is completing being the plain case. That read races the write in the C11 sense.

It is benign by construction. In every build but `address`, a freed generational block is parked and stays mapped, so the scan reads live memory; under `address` a free waits for the scan to finish. The copy records each block's size from its own header, bounded by the block's capacity, so the scan never reads past a block. The bytes it reads are only tested as candidate pointers, so a stale or half written word at worst names a collected block that is then kept one more cycle, which is safe. The scan is left uninstrumented under `address` and `thread`, so it raises no report. A collected value is confined to the main thread, so a collected block is only ever written there, and the cross thread read races only a block that is not the collector's.

`gcasync`, three collectors held across one suspension in a task frame, and `gcstress`, two hundred async tasks each holding a collector across a timer, run clean under both words and print their plain output.
