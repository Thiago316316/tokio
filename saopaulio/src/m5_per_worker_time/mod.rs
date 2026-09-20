//! # M5 — moving the wheel from shared to per-worker
//!
//! **New concept:** the #7467 lesson, earned rather than read.
//!
//! **Status: not started.** Depends on [`crate::m4_worker`], including its
//! benchmark harness. Without a "before" number this milestone teaches nothing.
//!
//! ## The situation you are in after M4
//!
//! One `Mutex<Wheel>` for the whole runtime. Every `sleep` creation, every
//! `timeout` cancellation, and every park's `next_deadline()` on every worker
//! serialises through it. At 8 workers this is the runtime's hottest lock.
//!
//! ## The obvious fix, which is wrong
//!
//! Shard it: `Box<[Mutex<Wheel>]>`, one per worker, assign each timer a
//! `shard_id` at creation so cancellation knows which lock to take.
//!
//! Tokio did exactly this — PR #6534, commit `1914e1e4`, May 2024. It is a
//! careful piece of work: `next_wake` had to move out of the mutex into an
//! `AtomicU64` so parking could find the minimum without holding every lock;
//! `process()` picks a random starting shard so shard 0 is not systematically
//! drained first.
//!
//! **It was reverted a year later** — PR #7226, commit `1ae9434e`, May 2025:
//!
//! > The work on sharding the timer implementation has caused a measurable
//! > performance regression due to increased contention. This patch reverts
//! > the current work on sharding. The next step will be to work on a
//! > per-worker timer wheel.
//!
//! ## Why it was wrong, which is the entire point of this milestone
//!
//! Sharding removed contention on **insert** but made **park** O(shards) in
//! lock acquisitions — and park happens constantly. A worker that inserts one
//! timer forces every other worker's park to touch its shard's mutex. The
//! cache-line traffic grew faster than the contention shrank.
//!
//! > Sharding reduces *collisions*. It does not reduce *coupling*.
//!
//! Only removing the cross-worker sharing does that. Write that sentence on
//! something before you start, because the sharded design will feel obviously
//! correct while you are writing it. It felt obviously correct to tokio's
//! maintainers too, and they are better at this than either of us.
//!
//! ## The actual design (#7467)
//!
//! Each worker **owns** its wheel outright. No mutex on the wheel at all.
//! Timers go into the creating worker's wheel. The one hard case: a timer can
//! be dropped on a different worker than created it, and reaching into a
//! foreign wheel would reintroduce exactly the sharing being removed. So
//! cancellation is *forwarded* through a dedicated cross-worker queue — the
//! one genuinely shared structure — and applied by the owning worker.
//!
//! That is the trade: cancellation becomes asynchronous and slightly later, in
//! exchange for insert, expire, and park becoming entirely uncontended.
//!
//! ## What to build
//!
//! - Move the wheel into the worker's local state. Delete the mutex.
//! - A registration queue: worker-local, drained into the wheel before parking.
//! - A cancellation queue: `Arc<Mutex<..>>`, cross-thread, drained by the owner.
//! - Park now uses only *this* worker's `next_deadline()` — note this is a
//!   behaviour change, not just a refactor: a worker no longer knows about
//!   other workers' deadlines, and does not need to, because each wakes for
//!   its own.
//!
//! ## The measurement, which is the deliverable
//!
//! Run M4's harness against: shared mutex / sharded / per-worker, at 1, 2, 4,
//! 8 threads. **Implement the sharded version even though you know it loses.**
//! Reproducing a documented regression with your own hands is worth more than
//! the working version, and it is a thing you will essentially never be given
//! permission to do on a real project.
//!
//! Then write down what you measured. If your sharded version *wins*, your
//! benchmark is not modelling park frequency correctly — which is itself the
//! most valuable possible outcome, because it means you have discovered what
//! the benchmark has to capture.
//!
//! ## Where tokio does this
//!
//! ```bash
//! git show 1914e1e4   # #6534 sharding — the plausible wrong answer
//! git show 1ae9434e   # #7226 the revert — read the diff alongside the above
//! git show 73d733a3   # #7467 per-worker wheels — the real answer
//! gh issue view 7384  # the RFC
//! ```
//!
//! and the resulting code, upstream, in `runtime/time_alt/`:
//!
//! | | |
//! |---|---|
//! | `time_alt/entry.rs` | two intrusive pointer sets; `extra_pointers` is time-multiplexed across registration → wheel → wake queue |
//! | `time_alt/registration_queue.rs` | worker-local, drained before parking |
//! | `time_alt/cancellation_queue.rs` | the one genuinely cross-thread structure |
//! | `time_alt/wheel/` | the worker-local wheel |
//! | `time_alt/context.rs` | how a `sleep()` finds "my worker's queue" |
//!
//! Note how it shipped: behind `Builder::enable_alt_timer()`, requiring
//! `--cfg tokio_unstable`, multi-thread only, with a feedback issue (#7745) and
//! the traditional timer still present as `enum Inner { Traditional, Alternative }`.
//! Two reverts in one subsystem in one year is not dysfunction — it is what
//! being willing to merge plausible optimisations looks like when you are also
//! willing to back them out.
//!
//! ## The transferable lesson
//!
//! It is not about timers. It is: **a contended lock has two dimensions —
//! collision rate and coupling — and the standard fix only addresses one of
//! them.** Any time you are about to shard something, ask what the *readers*
//! do, and how often.
