# tokio vocabulary — what it does / who calls it

A reference of the core types and functions, grouped by layer. Format per entry:

- **Name** `kind` — *what it does* · **called by** *who reaches for it*

Line references are to `tokio/src/…` and drift over time; use them as a starting
point, not gospel. "you" = application code; everything else is internal.

---

## Layer 1 — user-facing entry points

These are the only names most applications ever type.

- **`#[tokio::main]`** `macro` (tokio-macros) — rewrites `async fn main` into
  `fn main { Runtime::new().block_on(async { … }) }`. · **called by** you, once.
- **`#[tokio::test]`** `macro` — same rewrite for a test fn. · **called by** you.
- **`Runtime`** `struct` (`runtime/runtime.rs`) — owns the worker threads +
  drivers; the thing that *is* the runtime. · **called by** `#[tokio::main]`, or
  you when you build it by hand.
- **`Builder`** `struct` (`runtime/builder.rs`) — configures a `Runtime`
  (`new_multi_thread` / `new_current_thread`, `worker_threads`, `enable_all`,
  `enable_alt_timer`). · **called by** you, and by the `#[main]` expansion.
- **`Runtime::block_on`** `fn` — runs a future to completion on the current
  thread, driving the whole runtime until it resolves. The bridge from sync to
  async. · **called by** `#[tokio::main]`; you, at the top of `main`.
- **`tokio::spawn`** `fn` (`task/spawn.rs`) — hands a future to the scheduler as a
  new independent task; returns a `JoinHandle`. · **called by** you, from inside
  any async context.
- **`spawn_blocking`** `fn` (`runtime/blocking/`) — runs a *blocking* closure on a
  dedicated blocking-thread pool so it can't freeze a worker. The escape hatch for
  `std::thread::sleep`, sync file I/O, CPU-bound work. · **called by** you.
- **`Handle`** `struct` (`runtime/handle.rs`) — a cheap clonable reference to a
  running runtime; lets code far from `main` call `spawn` / `block_on`.
  · **called by** you, libraries, and internally everywhere a task needs "the
  current runtime."
- **`JoinHandle<T>`** `struct` (`runtime/task/join.rs`) — a future that resolves to
  a spawned task's output; also lets you `abort()` it. · **called by** you, when
  you `.await` a spawned task.

---

## Layer 2 — the scheduler (dispatch core)

`runtime/scheduler/mod.rs` — everything routes through these two enums.

- **`enum Handle { CurrentThread, MultiThread }`** (`scheduler/mod.rs:32`) — the
  scheduler behind the public `Handle`; every runtime entry point matches on this
  to pick the flavor. · **called by** `spawn`, `block_on`, driver wiring.
- **`enum Context { CurrentThread, MultiThread, … }`** (`scheduler/mod.rs:47`) —
  per-thread "am I inside a worker, and which one?" state. · **called by** `spawn`
  (to find the local queue), the time driver (to pick a worker's wheel).
- **`CurrentThread`** `scheduler` (`scheduler/current_thread/`) — single-threaded
  executor; one run queue, no work-stealing. · **called by** `block_on`, the
  current-thread runtime.
- **`MultiThread`** `scheduler` (`scheduler/multi_thread/`) — the worker pool:
  per-worker run queues + a global injection queue + work-stealing. · **called by**
  the default runtime.
- **work-stealing / `steal`** `fn` — an idle worker pulls tasks from a busy
  worker's queue to balance load. · **called by** worker threads when their own
  queue empties.

---

## Layer 3 — the task system (`runtime/task/`)

One heap allocation per task, driven through a hand-rolled vtable. Read
`state.rs` before touching any of this.

- **`Task<S>`** `struct` (`task/mod.rs:233`) — an owned handle to a task; `S` is
  the scheduler type. · **called by** the scheduler's owned-task list.
- **`Notified<S>`** `struct` (`task/mod.rs:243`) — a task that has been *scheduled*
  (is runnable, sitting in a run queue). · **called by** the run queue; produced
  when a waker fires.
- **`RawTask`** `struct` (`task/raw.rs`) — type-erased pointer to the allocation +
  the vtable; how everyone refers to a task without knowing its future type.
  · **called by** the waker, the scheduler, `JoinHandle`.
- **`Header`** `struct` (`task/core.rs`) — the always-present front of the
  allocation: state atomic + vtable + owner links. · **called by** `RawTask`
  vtable calls.
- **`Cell` / `Core`** `struct` (`task/core.rs`) — the full allocation: `Header` +
  the future + the output slot. · **called by** `harness.rs`.
- **`Harness`** `struct` (`task/harness.rs`) — the logic that actually `poll`s the
  future, catches panics, and writes the output. · **called by** a worker when it
  runs a `Notified` task.
- **`state.rs`** `module` — packs refcount + lifecycle bits into one atomic;
  the invariants live here and are enforced nowhere else. · **called by** every
  transition (schedule, run, complete, drop).
- **`waker.rs` / `RawWaker`** — builds the `std::task::Waker` whose "wake" pushes
  the task back onto a run queue as `Notified`. · **called by** any resource
  (I/O, timer, channel) that becomes ready.

---

## Layer 4 — the driver stack (`runtime/driver.rs`)

Composes I/O + Signal + Process + Time into one nested park stack.

- **`Driver`** `struct` (`driver.rs:16`) — the composed park stack; parking the
  runtime parks this. · **called by** the scheduler when the run queue empties.
- **`Handle`** `struct` (`driver.rs:21`) — clonable access to all sub-drivers
  (register an fd, arm a timer). · **called by** `Sleep`, `TcpStream`, etc.
- **`Cfg`** `struct` (`driver.rs:37`) — build-time driver config, incl. `workers`
  (shard/wheel count). · **called by** `Builder::build`.
- **`enum TimeDriver { Disabled, Enabled, … }`** (`driver.rs:290`) — picks
  traditional vs. alt timer vs. off. · **called by** driver construction.
- **`park` / `park_timeout`** `fn` — ask the time driver for the nearest deadline,
  then block the I/O driver up to that timeout. The "one `epoll_wait` for both."
  · **called by** a worker with an empty run queue.

---

## Layer 5 — the I/O driver (`runtime/io/`)

mio (epoll/kqueue/IOCP) wrapped so readiness turns into wakes. No loom coverage
yet — issue #3018.

- **`Driver`** `struct` (`io/driver.rs:25`) — owns the mio `Poll` + event buffer.
  · **called by** the driver stack when parking.
- **`Driver::turn`** `fn` (`io/driver.rs:179`) — one `poll.poll(events, max_wait)`
  (= `epoll_wait`), then dispatch each event to its resource. The I/O heartbeat.
  · **called by** `park` / `park_timeout`.
- **`Handle`** `struct` (`io/driver.rs:37`) — registers fds with mio, hands back a
  registration. · **called by** `TcpStream::connect`, `TcpListener::bind`, etc.
- **`ScheduledIo`** `struct` (`io/scheduled_io.rs:101`) — per-fd state: one packed
  `AtomicUsize` (`| shutdown:1 | tick:15 | readiness:16 |`) + a `Mutex<Waiters>`.
  **Its address is the mio token.** · **called by** the driver (via token→pointer)
  and by resource futures polling readiness.
- **`ScheduledIo::set_readiness`** `fn` — records that an fd became
  readable/writable when an event arrives. · **called by** `Driver::turn`.
- **`ScheduledIo::wake`** `fn` (`io/scheduled_io.rs:236`) — wakes the tasks parked
  on this fd. · **called by** `Driver::turn`, right after `set_readiness`.
- **`Readiness`** `struct` (`io/scheduled_io.rs:147`) — the future a resource
  awaits; `Init → Waiting → Done`, inserting a pinned `Waiter` into the list.
  · **called by** `AsyncRead`/`AsyncWrite` impls, `AsyncFd::readable`.
- **`ReadyEvent`** `struct` (`io/driver.rs:74`) — "this fd is now ready for X,"
  carried from driver to resource. · **called by** the readiness plumbing.
- **`Registration`** `struct` (`io/registration.rs`) — a resource's handle to its
  `ScheduledIo`; the glue between `TcpStream` and the driver. · **called by**
  every net/fs/pipe resource.

---

## Layer 6 — time (`tokio::time` + `runtime/time/`)

Public API on top; a hierarchical hashed timing wheel underneath.

### public

- **`sleep(Duration)` / `sleep_until(Instant)`** `fn` (`time/sleep.rs`) — a future
  that completes at a deadline. Duration is converted to an absolute `Instant`
  then discarded. · **called by** you.
- **`Sleep`** `struct` (`time/sleep.rs:225`) — the future itself; delegates to a
  `TimerEntry`. · **called by** you (`.await`), and by `timeout`/`interval`.
- **`timeout(Duration, future)`** `fn` (`time/timeout.rs:86`) — races your future
  against a `Sleep`; whichever finishes first wins (`Err(Elapsed)` if the timer).
  · **called by** you.
- **`Timeout<T>`** `struct` (`time/timeout.rs:176`) — that race, as a future.
- **`interval(Duration)`** `fn` (`time/interval.rs:73`) — yields a tick on a fixed
  period. · **called by** you (loops, heartbeats).
- **`Interval`** `struct` (`time/interval.rs:386`) — the recurring-tick future.
- **`Instant`** `struct` (`time/instant.rs:34`) — tokio's clock instant; the type a
  deadline is expressed in. · **called by** all of the above.

### internal

- **`TimerEntry`** `struct` (`time/entry.rs:287`) — the owner-side handle a `Sleep`
  holds; lazily initializes its `TimerShared` on first poll (#6512). · **called by**
  `Sleep`.
- **`TimerShared`** `struct` (`time/entry.rs:329`) — the node that lives in the
  wheel; reachable via the owner **or** the driver lock, never both at once.
  · **called by** the wheel and the driver.
- **`TimerHandle`** `struct` (`time/entry.rs:321`) — driver-side pointer to a
  `TimerShared`. · **called by** the wheel when firing.
- **`StateCell`** `struct` (`time/entry.rs:91`) — one atomic holding *either* the
  true deadline *or* a sentinel (`STATE_DEREGISTERED = u64::MAX`,
  `STATE_PENDING_FIRE = MAX-1`); lets a firing driver CAS-detect a racing
  `reset()`. · **called by** `TimerEntry::poll_elapsed`, the driver.
- **`Wheel`** `struct` (`time/wheel/mod.rs:22`) — 6 levels × 64 slots; upper levels
  cascade down, only level 0 fires. · **called by** the time driver.
- **`Wheel::insert`** — slot a timer by its deadline. · **called by** the driver
  when a `Sleep` registers.
- **`Wheel::poll` / `poll_at`** (`wheel/mod.rs:141,136`) — pull the next expired
  timer / report the next deadline. · **called by** the driver on park + wake.
- **`next_expiration`** `fn` (`wheel/mod.rs:168`) — the nearest deadline across the
  wheel; feeds `max_wait` for `epoll_wait`. · **called by** `park`.
- **alt timer (`runtime/time_alt/`)** — per-worker wheels, no mutex, cross-worker
  cancellation queue. Unstable, opt-in via `Builder::enable_alt_timer` (#7467).
  · **called by** you (if you opt in) → the driver.

---

## Layer 7 — the loom shim (`runtime` uses it everywhere)

- **`crate::loom::sync::{Mutex, RwLock, atomic::*}`** — under `--cfg loom`, the
  model-checked versions; otherwise thin re-exports of `std::sync`. **Every**
  concurrency primitive in runtime code imports these, never `std::sync`.
  · **called by** all lock-free / lock-minimal runtime code.
- **`crate::loom::cell::UnsafeCell`** — has a `with`/`with_mut` API that detects
  aliasing under loom; `std::cell::UnsafeCell` does not. · **called by** intrusive
  nodes, `ScheduledIo`, timer entries.
- **`loom::model(closure)`** `fn` — runs a test once per possible interleaving.
  · **called by** loom tests under `cfg_loom!`.

---

## Cross-cutting idioms

- **`cfg_rt!`, `cfg_time!`, `cfg_io_driver!`, `cfg_unstable!`, `cfg_loom!`** `macros`
  (`macros/cfg.rs`) — encode feature *combinations* (`cfg_io_driver!` = net or
  process or signal or fs). Wrap conditional items in these, not raw
  `#[cfg(feature=…)]`. · **called by** every module gating itself.
- **`linked_list.rs` / `Pointers<T>` / `unsafe impl Link`** (`util/`) — the
  intrusive-list pattern: node lives inside a user-owned pinned struct
  (timer entries, IO registrations, `JoinSet` entries). · **called by** the timer
  wheel, `ScheduledIo` waiters, `JoinSet`.
