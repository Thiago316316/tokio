//! # M3 — `timeout(dur, fut)`: combinators and cancellation
//!
//! **New concept:** racing two futures, and what dropping one means.
//!
//! **Status: not started.** Depends on [`crate::m1_time`].
//!
//! ## The problem
//!
//! `timeout` is the smallest interesting combinator: poll the inner future,
//! poll a `Sleep`, return whichever finishes. Twenty lines. The interesting
//! part is the twenty-first.
//!
//! ## Cancellation in Rust is `Drop`
//!
//! There is no `cancel()` method and no cancellation token in the language.
//! A future is cancelled by **being dropped**, which means:
//!
//! - it can happen at any await point, with no warning to the future;
//! - the future gets no chance to run async cleanup — `Drop` is not `async`;
//! - anything it registered with a driver must be unregistered *in `Drop`*, or
//!   it leaks.
//!
//! So M1's `cancel` was not an optional nicety. It is the mechanism by which
//! `Sleep::drop` removes its entry from the wheel, and without it every
//! `timeout` that completes normally leaves a corpse in the wheel until its
//! deadline passes. On a server doing 50k req/s with a 30s timeout, that is
//! 1.5M dead entries resident at steady state.
//!
//! **This is the milestone that retroactively explains M1.** The reason to
//! care that the heap's `cancel` was O(n) is that this is how often it is
//! called.
//!
//! ## What to build
//!
//! - `Sleep` as a real future with a `Drop` impl that cancels its wheel entry.
//!   Hold the `TimerHandle` from M1; cancel on drop; make cancelling an
//!   already-fired timer a no-op rather than a panic.
//! - `timeout(dur, fut)` — poll `fut` first, then the `Sleep`. Order matters:
//!   if both are ready in the same poll, the work winning over the timeout is
//!   the less surprising behaviour.
//! - `select!`-shaped racing, even if only as a two-future function. The
//!   macro is sugar.
//!
//! ## Tests that decide it
//!
//! - A `timeout` whose inner future completes immediately leaves the wheel
//!   **empty**. Assert `wheel.count() == 0`. This is the test that catches the
//!   leak, and it is the one most homemade runtimes do not have.
//! - Timing out drops the inner future (use a payload with a `Drop` that sets
//!   a flag).
//! - `timeout(0, ready_future)` — the both-ready-at-once race resolves to the
//!   work, deterministically, every run.
//! - Nested: `timeout(1s, timeout(10s, f))` — the outer firing must clean up
//!   the inner's wheel entry too.
//!
//! ## Checkpoint
//!
//! You have M1's finish line and M3 together: run 10k `timeout`s that all
//! complete before their deadline, and assert the wheel is empty afterwards.
//! If it is not, you have found the exact bug this milestone exists to teach.
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `time/timeout.rs` | ~250 lines, and most of it is docs about cancellation |
//! | `time/sleep.rs` | the future; see its `Drop` |
//! | `runtime/time/entry.rs` | `TimerEntry::drop` → `cancel`, and the lock-free protocol that makes a racing `reset` safe |
//! | `time/interval.rs` | the harder cousin — a repeating timer has to decide what to do about missed ticks (`MissedTickBehavior`) |
//!
//! Worth reading alongside: commit `f6eb1ee1` (tokio PR #6512, "time: lazily
//! init timers on first poll"). It exists precisely because of the arithmetic
//! above — in a hot path where timeouts almost never fire, even *allocating*
//! the timer state is measurable overhead. It also ships its own benchmark in
//! the same commit, which is the house rule.
