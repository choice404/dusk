# The kqueue backend, a bring-up runbook

The reactor reaches the kernel's readiness machinery through a six function
poller seam declared in `runtime/reactor_poller.h`. Linux uses the epoll backend
in `runtime/reactor_epoll.c`, which is the path exercised on every release. The
BSDs and macOS use the kqueue backend in `runtime/reactor_kqueue.c`. That
backend is written and statically reviewed against the same seam, but it has
never been compiled or run, because the machine every release is cut on is
Linux. This runbook is for the person with a BSD or macOS host who brings it up.

Nothing here is confirmed. Every step below produces a result that belongs in
the release notes of the release that first claims BSD or macOS support.

## Toolchain

The same toolchain the compiler needs everywhere.

- `clang` and LLVM 22.x on the path. The textual IR targets one LLVM major
  version, so the major must match exactly.
- A working `dusk` binary. `tools/bootstrap.sh` stands one up from a release
  artifact, but the `dusk.ll.xz` IR pins x86_64 Linux, so a BSD or macOS host
  starts from a binary already built for it, or walks the release tags forward
  from the archived Rust seed as the README's audit path describes.

On macOS the system `clang` may not be LLVM 22.x. Install the matching LLVM,
for example through Homebrew, and put its `clang` first on the path before
building. On FreeBSD install the `llvm22` package and confirm `clang --version`
reports the right major.

## Build

Build the compiler and the runtime the ordinary way. The build compiles the
kqueue translation unit for the first time, so a compile error here is the
first thing to find and fix.

```sh
DUSK_HOME=$PWD target/dusk-out/dusk build compiler/dusk.dusk
```

The platform guard in `reactor_kqueue.c` selects the kqueue body on
`__APPLE__`, `__FreeBSD__`, `__NetBSD__`, and `__OpenBSD__`, and the guard in
`reactor_poller.h` selects the one field kqueue poller layout on the same
platforms. On a platform none of those name the header emits a hard
`#error`, so a new BSD needs its define added to both guards before anything
compiles.

## Verify against the golden suite

Each golden below compiles and runs a real program through the built `dusk`
binary. Run them one at a time so a failure names itself.

```sh
DUSK_HOME=$PWD DUSK_BIN=target/dusk-out/dusk target/dusk-out/testrun tests/goldens.manifest --filter <name>
```

Run the reactor family first. These are the arm, fire, gate, and lifecycle
paths of the reactor itself.

- `reactorsum`
- `reactorlife`
- `pipewake`
- `timerinterleave`
- `awaittimeout`

Then the net family, the TCP surface over the reactor's readiness futures.

- `tcplocal`
- `acceptloop`

Then the stress family, the long runs that hold the runtime under load.

- `stress_timers`
- `stress_tasks`
- `stress_accept`
- `stress_pool`

Then the fault family, the syscall hardening paths, each of which must produce
its handled error and keep running rather than killing the process.

- `sigpipe`
- `fdexhaust_pipe`
- `fdexhaust_connect`
- `fdexhaust_accept`

A run that runs the whole suite at once is the final check once the families
above pass individually.

```sh
DUSK_HOME=$PWD DUSK_BIN=target/dusk-out/dusk target/dusk-out/testrun tests/goldens.manifest
```

## ThreadSanitizer

The reactor is a thread, so a clean golden run is not enough. Run the
ThreadSanitizer recipe in [tsan.md](tsan.md) with `reactor_kqueue.c` on the
link line in place of `reactor_epoll.c`. The kqueue backend moves no lock
boundary and keeps no state the portable core cannot see, so it should run as
clean as epoll does, twenty iterations with no `WARNING: ThreadSanitizer` and
exit code 0 every time. If it does not, the backend has introduced a race the
seam was designed to make impossible, and that is the finding.

## The one pinned divergence

The kqueue backend diverges from epoll in exactly one exotic case, and the
runner must pin it rather than smooth it over. The case is a watch armed on a
descriptor, that descriptor closed while the watch is still armed, and then a
fresh descriptor that reuses the same integer armed again.

epoll drops a registration automatically when its descriptor closes, so on
Linux the re-arm of a reused descriptor succeeds and overwrites the stale
registry entry. kqueue's `EV_ADD` never fails on a duplicate, so the backend
reproduces the already-armed fault by probing the shared registry with
`cool_reg_probe` before the `EV_ADD`. That probe makes the ordinary double arm
misuse, two arms with no close between them, fault identically to epoll. But a
re-arm after a close-while-armed sees the stale registry entry the closed
descriptor left behind and faults where epoll would have overwritten it.

That divergence is intended. The runner's job is to confirm it behaves as
described, that the ordinary double arm faults the same on both backends and
the close-while-armed-then-reused case faults on kqueue where it does not on
epoll, and to record the observed behavior. If the divergence turns out to
matter for a real program rather than the misuse path, the fix is to clear the
registry entry on close so the reused descriptor arms clean, but that is a
change to make only once a host has shown the case is reachable.

## Two more things to confirm

- `EVFILT_USER`, the wake sentinel the backend fires with `NOTE_TRIGGER`, is
  present on macOS and FreeBSD. Its availability on NetBSD and OpenBSD is not
  confirmed. If it is missing there, the backend needs a self-pipe sentinel in
  its place on those platforms.
- A regular file is rejected up front with an `fstat`, because kqueue would
  otherwise accept the registration and report a regular file permanently
  ready, where epoll faults it. Confirm a watch on a regular file faults with
  the same `a regular file cannot report readiness` message on both backends.

## Where the results go

There is no CI for this and there is no BSD or macOS host in the loop. The
results of a bring-up run, which goldens pass, whether ThreadSanitizer is
clean, and how the pinned divergence actually behaves, belong in the release
notes of the release that first claims BSD or macOS support. Until then the
kqueue backend is written and statically reviewed, not run.
