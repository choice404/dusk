# ThreadSanitizer, a local recipe

There is no CI configuration for this in the repository. This is the recipe run
by hand before a release that touches thread safety, most recently the 0.4.1
reactor. It rebuilds one golden's emitted LLVM IR alongside the runtime's C
files under `clang -fsanitize=thread`, then runs the result in a loop.

## Steps

Emit the `.ll` for a golden with `dusk build`:

```sh
DUSK_HOME=$PWD target/dusk-out/dusk build examples/reactorsum.dusk
```

This writes `target/dusk-out/reactorsum.ll` beside the native binary.

Compile that IR together with the four runtime `.c` files under TSan:

```sh
clang target/dusk-out/reactorsum.ll \
    runtime/runtime.c runtime/thread.c runtime/async.c runtime/reactor.c \
    -pthread -fsanitize=thread -O1 -g \
    -o target/dusk-out/reactorsum.tsan
```

Run it in a loop. TSan reports are non-deterministic under scheduling, so one
clean run proves nothing; twenty does.

```sh
for i in $(seq 1 20); do
    target/dusk-out/reactorsum.tsan || { echo "FAILED on iteration $i"; break; }
done
```

A clean pass prints the golden's expected stdout twenty times with no
`WARNING: ThreadSanitizer` output and exit code 0 on every iteration.

## Coverage for the 0.4.1 release

Run before release against the three goldens most likely to expose a reactor
race: the arm/fire/gate path, the cross thread wake racing the reactor's own
gauge drop, and two completers racing one future.

- `reactorsum`, four pool workers, four armed watches, a batch of epoll
  deliveries funnelling to one await sequence.
- `pipewake`, a spawned thread's exit gauge dropping while the reactor still
  holds the armed gauge for a watch on the same await.
- `racingcomplete`, two completers racing one future, unrelated to the
  reactor but sharing the same completion path the reactor's fire step reuses.

Swap the example name in the two commands above to cover a different golden.

## Coverage for the 0.4.4 release

Two stress goldens join the list, both async and both likely to expose a race
only a long run surfaces.

- `stress_pool`, a pool saturation golden: many more tasks than workers
  submitted at once, so the pool's queue, its worker wakeups, and every
  completion's channel handoff all run under sustained contention rather than
  the light load the earlier goldens exercise.
- `stress_accept`, an accept storm golden: many clients connecting against one
  listener at once, so `tcp_accept`'s await and retry loop, the reactor's watch
  churn on the listening descriptor, and the fd exhaustion path all run
  back to back under load instead of one connection at a time.

Run both through the same rebuild, link, and twenty iteration loop as
`reactorsum` above; `stress_accept` touches `std.async.net`, whose TCP shims
live in `runtime/reactor.c` already, so no extra runtime file joins the four
already listed.

## The collector and one benign read

`runtime/collect.c` joins the link line for every program. Add it to the
`clang` command above beside `runtime.c`, and add `runtime/reactor_epoll.c`,
which the four file list here predates.

The collected heap has one deliberate cross thread read that a strict tool will
flag. A collection scans the live generational block registry as a root region.
It copies that registry under the heap lock, releases the lock, then scans the
copied payloads with the lock released, so an allocation or a free on another
thread does not block behind the whole scan. While it scans, another thread may
be writing one of those payloads, a future record a completer thread is filling
being the plain case. That read races the write in the C11 sense.

It is benign by construction. A generational block is never returned to libc; a
freed block is parked on the size matched free list and stays mapped, so the
scan reads live memory, never a fault. The bytes it reads are only tested as
candidate pointers, so a stale or half written word at worst names a collected
block that is then over retained for one cycle, which is safe. This is not
suppressed. `gcprobe`, the floor's smoke probe, is single threaded and runs TSan
clean with no report.

## Coverage for the 0.5.1 collector

The confinement rule keeps a collected value on the main thread: it may not
cross a channel or a spawn or submit capture to another thread. A collected
block is therefore only ever written on the main thread, so the cross thread
scan read above races only a non collector payload, a future record a pool
worker completes being the plain case, never a collector's block. That is why
the two async collector goldens run TSan clean rather than surfacing the read.

- `gcasync`, three collectors held across one suspension in a task frame, a
  collection forced on each side of the await.
- `gcstress`, two hundred async tasks each holding a collector across a timer,
  with collections interleaved through the read back.

Build these with `-O0`, not the `-O1` the recipe above uses. The conservative
root scan brackets the stack between a collection point and the anchor, and the
native driver passes no optimization flag for exactly this reason, so a collector
golden must match it or the scan may miss a spilled root. Add `runtime/collect.c`
and `runtime/reactor_epoll.c` to the link line. Twenty iterations of each were
clean, expected stdout and exit code 0 every time, no `WARNING: ThreadSanitizer`.
