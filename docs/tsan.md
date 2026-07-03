# ThreadSanitizer, a local recipe

There is no CI configuration for this in the repository. This is the recipe run
by hand before a release that touches thread safety, most recently the 0.4.1
reactor. It rebuilds one golden's emitted LLVM IR alongside the runtime's C
files under `clang -fsanitize=thread`, then runs the result in a loop.

## Steps

Emit the `.ll` for a golden with `dusk build`:

```sh
cargo run --bin dusk -- build examples/reactorsum.dusk
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
