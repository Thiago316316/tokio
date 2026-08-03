# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository

This is the upstream [tokio-rs/tokio](https://github.com/tokio-rs/tokio) monorepo — the async runtime for Rust. It is a Cargo workspace (`resolver = "2"`) with `[patch.crates-io]` pointing every tokio crate at its local path, so intra-workspace changes are always picked up.

Published crates: `tokio` (runtime + core), `tokio-macros` (`#[tokio::main]`, `#[tokio::test]` proc macros), `tokio-util` (codecs, `CancellationToken`, compat shims), `tokio-stream` (`Stream` adapters), `tokio-test` (`assert_pending!`, mock IO, `time::pause` helpers).

Internal-only members: `benches` (Criterion), `examples`, `stress-test`, `tests-build` (compile-pass/fail matrix over feature combos), `tests-integration` (cross-crate behavior).

Toolchain pins (from `.github/workflows/ci.yml` `env:`):

| Purpose | Version |
| --- | --- |
| MSRV (`rust_min`) | 1.71 |
| Clippy (`rust_clippy`) | 1.88 |
| Nightly (`rust_nightly`) | nightly-2025-10-12 |
| Miri (`rust_miri_nightly`) | nightly-2026-06-29 |

MSRV is documented in five places that must be updated together: `README.md`, `tokio/README.md`, `CONTRIBUTING.md`, `.github/workflows/ci.yml`, and each crate's `Cargo.toml` `rust-version`.

## Commands

Tokio's behavior is controlled by *both* Cargo features and `--cfg` flags. Getting either wrong silently compiles a different codebase, so most commands need explicit arguments.

### Build / check

```bash
cargo check --all-features            # `--all-features` fails on non-Linux (io-uring); use --features=full there
cargo build --features full
cargo +1.88 clippy --all --tests --all-features
```

### Test

CI uses `cargo-nextest` for everything except doctests (nextest doesn't support them).

```bash
cd tokio
cargo nextest run --features full          # or: cargo test --features full
cargo test --doc --features full           # doctests, separately

# Workspace-wide, matching CI's TOKIO_STABLE_FEATURES:
cargo nextest run --workspace --features full,test-util
```

**Single test / single file.** Integration tests live in `tokio/tests/*.rs`, one binary per file:

```bash
cargo test --features full --test time_rt                    # one file
cargo test --features full --test time_rt -- test_name       # one test
cargo test --features full --lib -- runtime::time::tests     # in-crate module tests
```

**Unstable features.** Anything behind `--cfg tokio_unstable` (task dumps, metrics, `enable_alt_timer`, io-uring) needs the cfg passed to *both* rustc and rustdoc, and files gated with `#![cfg(tokio_unstable)]` silently compile to nothing without it:

```bash
RUSTFLAGS="--cfg tokio_unstable" RUSTDOCFLAGS="--cfg tokio_unstable" \
  cargo test -p tokio --features full,test-util --test time_alt
```

### Loom (concurrency model checking)

Loom exhaustively explores thread interleavings for code written against `crate::loom::*` shims. Slow — always scope it to a module.

```bash
cd tokio
LOOM_MAX_PREEMPTIONS=1 LOOM_MAX_BRANCHES=10000 \
  RUSTFLAGS="--cfg loom --cfg tokio_unstable -C debug_assertions" \
  cargo test --lib --release --features full -- --test-threads=1 --nocapture runtime::time
```

CI (`.github/workflows/loom.yml`) uses `LOOM_MAX_PREEMPTIONS=2` and shards `loom_multi_thread` into `group_a`..`group_d` because a single unsharded run exceeds the job timeout. Loom scopes live in `tokio/src/runtime/tests/`.

### Miri (UB detection)

The intrusive-linked-list and `UnsafeCell` code in the runtime is Miri's main job here.

```bash
MIRIFLAGS="-Zmiri-disable-isolation -Zmiri-strict-provenance" \
  cargo +nightly miri test --features full --lib --tests
```

### Benchmarks

```bash
cd benches
cargo bench --bench time_timeout          # one file
cargo bench multi_thread_timeout-8        # one benchmark by name
# results land in target/criterion/
```

`benches/time_timeout.rs` is the harness added by #6512 specifically to measure the timer work described below — it is the standard reproducer for timer contention.

### Formatting, docs, spellcheck

```bash
# `cargo fmt` does NOT work on this repo (workspace layout); use:
rustfmt --check --edition 2021 $(git ls-files '*.rs')

RUSTDOCFLAGS="--cfg docsrs --cfg tokio_unstable" RUSTFLAGS="--cfg docsrs --cfg tokio_unstable" \
  cargo +nightly doc --all-features --open
# or: cargo install --locked cargo-docs-rs && cargo +nightly docs-rs --open

cargo spellcheck check    # new non-code words go in spellcheck.dic — and bump its first-line count
```

## Architecture

### Layering

```
tokio::time / net / fs / process / signal   ← user-facing resources
        │
tokio::runtime::scheduler                   ← current_thread | multi_thread
        │   (Handle enum in scheduler/mod.rs dispatches everything)
tokio::runtime::driver                      ← composes Io + Signal + Process + Time into one park stack
        │
tokio::runtime::{io, time, time_alt}        ← the drivers themselves
        │
tokio::loom                                 ← std / loom shim layer (EVERY sync primitive goes through here)
```

`runtime/driver.rs` builds a nested "park stack": the time driver wraps the IO driver, which wraps the signal driver. Parking the runtime means asking the time driver for the next deadline, then parking the IO driver with that timeout. This is why the time driver's `next_expiration` computation sits on the hot path of every idle→busy transition.

`runtime/scheduler/mod.rs` defines `enum Handle { CurrentThread(..), MultiThread(..) }` and `enum Context { .. }`. Almost every runtime entry point matches on these — when adding a scheduler-aware feature, expect to touch both arms plus the `cfg(not(feature = "rt"))` `Disabled` arm.

### Task system (`runtime/task/`)

A spawned task is a single heap allocation (`core.rs`) accessed through a hand-rolled vtable (`raw.rs`) and driven by `harness.rs`. `state.rs` packs refcount + lifecycle bits into one atomic; `waker.rs` builds the `RawWaker`. There are no trait objects here — the vtable is manual so the future, output, and scheduler are erased without boxing each separately. Read `state.rs` before touching anything in this directory: the invariants are stated there and enforced nowhere else.

### Feature gating idiom

`tokio/src/macros/cfg.rs` defines ~60 macros (`cfg_rt!`, `cfg_time!`, `cfg_net!`, `cfg_unstable!`, `cfg_loom!`, …). Wrap conditional items in these rather than writing `#[cfg(feature = "...")]` inline — they encode the *combinations* (e.g. `cfg_io_driver!` means "net or process or signal or fs"), and the codebase relies on them staying the single source of truth.

### Unsafe conventions

Intrusive linked lists (`util/linked_list.rs`) are used everywhere a node must live inside a user-owned, pinned struct (timer entries, IO registrations, `JoinSet` entries). The pattern: a `Pointers<T>` field inside the node, an `unsafe impl Link` giving pointer↔handle conversions, and `PhantomPinned` to suppress LLVM's `noalias` on `&mut` (see the comment in `runtime/time_alt/entry.rs` citing rust-lang/rust#82834). Every `unsafe fn` in this area carries a `# Safety` block naming which lock must be held or which other list the node must *not* be in — those comments are the actual specification.

---

## Case study: the timer contention trilogy

This is the recommended entry point for learning how tokio is engineered. The narrative is **#6512 → #6534 → #7467**, but the real arc includes a revert, and that revert is the most instructive part.

All commits are in local history — read them directly:

```bash
git show f6eb1ee1   # #6512  time: lazily init timers on first poll        (2024-05-03)
git show 1914e1e4   # #6534  time: use sharding for timer implementation   (2024-05-22)
git show 1ae9434e   # #7226  time: REVERT the sharding work               (2025-05-05)
git show 73d733a3   # #7467  time: alternative per-worker timer            (2025-11-27)
```

### Background: what the timer actually is

`runtime/time/mod.rs` implements a **hierarchical hashed timing wheel** (Varghese & Lauck, 1997): 6 levels × 64 slots. Level 0 slots are 1ms; each level up multiplies by 64 (level 5 slots are ~12 days). Timers in upper levels cascade down as time advances; only level 0 fires wakers. `MAX_DURATION = (1 << 36) - 1` ms.

`runtime/time/entry.rs` is the concurrency core and its module doc is the best-written explanation of a lock-free protocol in the repo. Read it before the diffs. Key ideas:

- `TimerShared` is reachable either through `&mut TimerEntry` (owner) **or** the driver lock — never both concurrently, which is what makes the memory ordering work.
- One `StateCell` atomic holds *either* the true deadline *or* a sentinel (`STATE_DEREGISTERED = u64::MAX`, `STATE_PENDING_FIRE = MAX-1`). Encoding state and deadline in one word lets a firing driver CAS-detect a racing `reset()` and back off.
- Resetting a timer *later* is lock-free: the owner writes the new "true when" and the driver lazily re-slots it when it next walks that list. `registered_when` is kept separately so cancellation can still find the right list.

### #6512 — lazy timer initialization

**Problem.** `timeout(dur, fut)` allocates and initializes a `TimerShared` even when `fut` completes immediately. In a hot request path where timeouts almost never fire, that is pure overhead.

**Change.** `TimerEntry::inner` becomes `StdUnsafeCell<Option<TimerShared>>`, initialized on first `inner()` call — i.e. first poll, which is also the first moment the entry is pinned. `cancel()` and `is_elapsed()` gain early-outs for the uninitialized case.

**What to learn.** Notice what the diff *also* changed: `TimerEntry::new` went from taking `&scheduler::Handle` to taking it by value, and `sleep.rs` re-fetches the handle inside the `tracing` cfg block. That is the reviewer-driven part — avoiding one `Arc` clone on a path that now does almost nothing. Also notice the PR ships `benches/time_timeout.rs` in the same commit. Tokio does not merge performance claims without a reproducer.

### #6534 — sharded wheels (and why it looked right)

**Problem.** One `Mutex<Wheel>` for the whole runtime. Every `sleep`/`timeout` creation, cancellation, and reset on every worker serializes on it.

**Change.** `Inner.state: Mutex<InnerState>` → `Inner.wheels: Box<[Mutex<Wheel>]>`. Shard count is derived from the worker count, not user-configurable: `Builder::get_cfg` gained a `workers` parameter (`1` for current-thread, `core_threads` for multi-thread) plumbed through `driver::Cfg` into `time::Driver::new`. Each `TimerShared` gets an immutable `shard_id` assigned at lazy-init time by `generate_shard_id()`: the current worker's index if called from a worker, else `thread_rng_n()`. Because the id never changes, cancellation always knows which lock to take.

Three details worth studying:

1. `next_wake` had to move *out* of the mutex into an `AtomicOptionNonZeroU64` (a `NonZeroU64` packed into an `AtomicU64` with 0 as the niche), because parking now needs the min across all shards without holding all locks.
2. `park_internal` and `process` changed from one lock acquisition to a `filter_map(...).min()` over every shard.
3. `process()` picks a *random* starting shard so shard 0 isn't systematically drained first. Fairness fix, no correctness impact.

**Outcome: this was reverted a year later.** From #7226's commit message: *"The work on sharding the timer implementation has caused a measurable performance regression due to increased contention. This patch reverts the current work on sharding. The next step will be to work on a per-worker timer wheel."*

The lesson is the whole point of reading this pair. Sharding removed contention on *insert* but made *park* O(shards) in lock acquisitions — and park happens constantly. A worker that inserts one timer forces every other worker's park to touch its shard's mutex. The cache-line traffic grew faster than the contention shrank. Sharding reduces *collisions*; it does not reduce *coupling*. Only removing the cross-worker sharing does that.

Read the revert diff alongside the original to see how cleanly it came out — the original was structured so it *could* be reverted, which is why the maintainers took the risk in the first place.

### #7467 — per-worker timer wheels (`time_alt`)

This is the "next step" #7226 promised. RFC: tokio-rs/tokio#7384.

**Design.** Each worker owns a `Wheel` outright — no mutex on the wheel at all. Timers are inserted into the *creating* worker's wheel. Since a timer can be dropped on a different worker than created it, cancellation is forwarded through a dedicated cross-worker queue rather than reaching into a foreign wheel.

`runtime/time_alt/` structure:

| File | Role |
| --- | --- |
| `entry.rs` | `Entry` with **two** intrusive pointer sets: `cancel_pointers` (cancellation queue only) and `extra_pointers`, time-multiplexed across the registration queue → wheel → wake queue lifecycle |
| `registration_queue.rs` | worker-local, drained into the wheel before parking |
| `wheel/` | worker-local wheel (own `level.rs`/`mod.rs`, not shared with `time/wheel/`) |
| `wake_queue.rs` | entries pulled from the wheel, woken *after* releasing state |
| `cancellation_queue.rs` | the one genuinely cross-thread structure (`Arc<Mutex<..>>`) |
| `context.rs` | `LocalContext` / `TempLocalContext` — how a `sleep()` finds "my worker's queue" |
| `timer.rs` | the `Timer` type that `tokio::time::Sleep` delegates to |

The `extra_pointers` comment in `entry.rs` is the key invariant: one pointer field is reused by three different lists because an entry is provably in at most one of them at a time. Understanding *why* that's provable is the exercise.

**Integration.** `Inner` in `runtime/time/mod.rs` became `enum Inner { Traditional { .. }, Alternative { .. } }`, so both timers coexist behind one driver. Opt-in and unstable:

```rust
runtime::Builder::new_multi_thread()
    .enable_alt_timer()   // requires --cfg tokio_unstable, multi_thread only
    .build()
```

Feedback issue: tokio-rs/tokio#7745. Tests: `tokio/tests/time_alt.rs`, plus `runtime/time_alt/tests.rs` and per-queue test modules. Note #7467 also widened the loom CI scope from `runtime::time::tests` to `runtime::time` — new lock-free structures ship with loom coverage as a condition of merge.

### Adjacent commits worth reading

The trilogy sits inside a longer sequence of timer work; these show the same review culture:

```bash
git log --oneline -- tokio/src/runtime/time/
git show 24344dfe   # #6683 fix race condition leading to lost timers — a bug the above churn exposed
git show 8480a180   # #6584 avoid traversing entries twice...
git show 47210a8e   # #6715 ...reverted
git show 479a56a0   # #6779 eliminate timer wheel allocations
```

Two reverts in one subsystem in one year is not dysfunction — it's the maintainers being willing to merge plausible optimizations and equally willing to back them out when benchmarks disagree.

### How to read the review threads

The commit messages here are deliberately short; the reasoning lives on GitHub. Use:

```bash
gh pr view 6512 --comments
gh pr view 6534 --comments
gh pr view 7226 --comments
gh pr view 7467 --comments
gh issue view 7384    # the per-worker timer RFC
```

What to watch for in those threads: reviewers asking "where's the benchmark?", asking for loom coverage of any new `unsafe`, pushing back on `Ordering::Relaxed` without a justifying comment, and insisting that unstable features ship behind `tokio_unstable` + a feedback issue rather than changing default behavior.

## Loom

Loom is the highest-leverage thing to learn in this repo. Tokio's hot paths are lock-free or lock-minimal, and the review culture treats "add loom coverage" as a merge condition for new `unsafe` concurrency (see #7467, which widened the loom CI scope in the same commit that added new queues). Being fluent with loom is what lets you make claims about concurrent code that reviewers will accept.

### How it works

Loom is a model checker, not a fuzzer. It runs your test *once per possible thread interleaving*, exhaustively, under the C11 memory model — including permuting which value a `Relaxed` load is allowed to observe. If any schedule violates an assertion or leaks, it reports the exact interleaving. A passing loom test is a proof over the modeled state space, not a probabilistic argument.

The mechanism is `tokio/src/loom/`:

```
loom/mod.rs      → #[cfg(all(test, loom))] picks mocked.rs, otherwise std/
loom/std/*.rs    → thin re-exports of std::sync (production)
loom/mocked.rs   → re-exports loom::sync, wrapping Mutex/RwLock to panic-on-poison
```

**This is why every concurrency primitive in the codebase imports `crate::loom::sync::...` and never `std::sync::...`.** A direct `std::sync::Mutex` or `std::sync::atomic` in runtime code is invisible to loom and defeats the entire model — treat it as a bug in review. Same for `UnsafeCell`: `crate::loom::cell::UnsafeCell` has a tracking `with`/`with_mut` API that detects aliasing violations, while `std::cell::UnsafeCell` does not.

Note the asymmetry in `scheduled_io.rs`: `readiness`/`waiters` use `crate::loom` shims, but `linked_list_pointers` uses `std::cell::UnsafeCell`. Understanding why some fields are modeled and some aren't is a good way in.

### Where loom tests live

Gated by `cfg_loom!` / `cfg_not_loom!`, so they don't exist as compilation units without `--cfg loom`:

- `tokio/src/sync/tests/loom_*.rs` — 11 modules (mpsc, notify, semaphore, watch, broadcast, oneshot, rwlock, atomic_waker, list, set_once). **Read these first — they are the models to copy.**
- `tokio/src/runtime/tests/loom_*.rs` — 6 modules, registered in `runtime/tests/mod.rs`: `loom_blocking`, `loom_current_thread`, `loom_join_set`, `loom_local`, `loom_multi_thread`, `loom_oneshot`.
- `tokio/src/runtime/time/tests/` — reachable via the `runtime::time` loom scope.

Note what is **not** in that list: `runtime/io/`. There is no loom coverage of the I/O driver at all. That's issue #3018 below.

### Writing one

```rust
#[test]
fn my_invariant_holds() {
    loom::model(|| {
        let shared = Arc::new(Thing::new());
        let t = {
            let shared = shared.clone();
            thread::spawn(move || { shared.do_something(); })
        };
        shared.do_other_thing();
        t.join().unwrap();
        assert!(shared.invariant());
    });
}
```

Rules that will bite you:

- **Two threads, sometimes three. Never four.** The state space is roughly factorial in operations-per-thread. A 4-thread test will not finish.
- **`debug_assertions` is mandatory** — `runtime/tests/mod.rs` has a `compile_error!` enforcing it, because the model relies on debug-only checks.
- `LOOM_MAX_PREEMPTIONS` bounds preemption points (CI uses 2, the docs suggest 1 for fast iteration). Lower = faster and less thorough. Start at 1, raise to 2 before submitting.
- Loom's `thread::spawn` / `Arc` / `Notify` come from `loom::`, not `std::` — mixing them silently breaks the model.
- Loom mocks `num_cpus() == 2` and `rand::seed() == 1` (see `mocked.rs`), so any code branching on CPU count behaves differently under loom than in production.
- Iterate with `--release` (as CI does); debug-mode loom runs are painfully slow, but you still need `-C debug_assertions` explicitly.

```bash
cd tokio
LOOM_MAX_PREEMPTIONS=1 LOOM_MAX_BRANCHES=10000 \
  RUSTFLAGS="--cfg loom --cfg tokio_unstable -C debug_assertions" \
  cargo test --lib --release --features full -- --test-threads=1 --nocapture <scope>
```

### Issue #3018 — loom tests for the I/O driver

`io::driver should have loom tests`. Open since **2020-10-21**, labels `A-tokio`, `C-maintenance`, `E-help-wanted`, `M-io`, `M-runtime`. It was filed because bug **#2924** — *"net: possible race during I/O driver shutdown"*, where an I/O resource created concurrently with driver shutdown could miss its shutdown notification and hang forever — would have been much easier to catch with loom.

**Read the issue body before committing to this.** It says the work "would necessitate internal refactoring and mocking to isolate the IO driver from mio and the underlying OS." That is *not* a bounded test-writing task — it's a refactor-first task, and that is exactly why it has sat unclaimed for five years with `E-help-wanted` on it. Anyone who picks it up as "just write some tests" will stall on the mio boundary.

The tractable move is to **split it**, because the I/O driver is not uniformly mio-coupled:

| Component | mio? | loom shims already? | Verdict |
| --- | --- | --- | --- |
| `io/scheduled_io.rs` | **no** | yes (`loom::sync::{AtomicUsize, Mutex}`) | **start here** |
| `io/registration_set.rs` | no | yes (`loom::sync::atomic::AtomicUsize`) | good second step |
| `io/driver.rs` | **yes** — `mio::{Poll, Events, Registry, Waker}` | partial | needs the refactor; discuss first |

`ScheduledIo` is a self-contained concurrency puzzle needing zero mio mocking:

- One `AtomicUsize` bit-packed as `| shutdown: 1 | tick: 15 | readiness: 16 |` (`bit::Pack` chain at the bottom of the file).
- A `Mutex<Waiters>` holding an intrusive `LinkedList<Waiter>` plus dedicated `reader`/`writer` waker slots.
- A `Readiness` future with an `Init → Waiting → Done` state machine that inserts a pinned `Waiter` into that list.

Invariants worth encoding as loom tests: a `set_readiness` concurrent with a `readiness()` poll never loses a wakeup; the shutdown bit racing with waiter registration always wakes the waiter (this is #2924's scenario, and the direct payoff); tick wraparound doesn't misclassify a stale readiness event.

Suggested approach: copy the structure of `sync/tests/loom_semaphore_batch.rs` — `batch_semaphore.rs` is the closest structural analog in the tree (a packed `AtomicUsize` + `Mutex<Waitlist>` + intrusive `LinkedList<Waiter>`, i.e. the same shape as `ScheduledIo`). Add a `loom_scheduled_io` module under `cfg_loom!` in `runtime/tests/mod.rs`, then add a matching `loom-io` job to `.github/workflows/loom.yml`. Land that first as a self-contained PR.

Note how loom CI is gated: each job runs only if the PR carries the corresponding `R-loom-*` label (`R-loom-sync`, `R-loom-time-driver`, `R-loom-multi-thread`, …) *or* `github.base_ref == null`, i.e. a push to master. Loom jobs are expensive, so they are opt-in per PR — a new loom job needs a matching label, and you should expect to ask a maintainer to apply it to your PR. Treat full `driver.rs` coverage as a follow-up that should start with a comment on #3018 proposing the mio abstraction *before* writing code — a five-year-old refactor-shaped issue is one where maintainers will want to agree on the seam first.

Before starting, check current state — this file may be stale:

```bash
gh issue view 3018 --comments      # requires `gh auth login` first
git log --oneline -- tokio/src/runtime/io/
grep -rn "loom" tokio/src/runtime/io/
```

## Contribution conventions

- Commit subject: `module: imperative lowercase description`, ≤72 chars, no trailing period. The module prefix matches the `M-*` label (`time:`, `sync:`, `net:`, `ci:`, `chore:`). Body wrapped at 72. Footers: `Fixes: #1337`, `Refs: #453, #154`.
- Never commit to `master`; branch per change. Once a PR is open, **do not rebase** — push follow-up commits; maintainers squash on merge.
- Tokio avoids unit tests. Prefer integration tests in `<crate>/tests/` and doctests. Doctests are written from a *user's* perspective against the `tokio` facade, not internal paths.
- Behavior changes to a public API generally need an entry in the relevant `CHANGELOG.md` and, if experimental, gating behind `tokio_unstable` plus a feedback issue.
- Versioning: patch releases are bug fixes only; MSRV bumps only in minor releases; LTS releases get ≥1 year of backported fixes.
