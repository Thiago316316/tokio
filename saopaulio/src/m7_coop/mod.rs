//! # M7 — the cooperative budget
//!
//! **New concept:** a ready resource is a starvation hazard.
//!
//! **Status: not started.** New — not in the original M0–M5 roadmap. The
//! smallest milestone here by code volume (~200 lines upstream) and one of the
//! largest by behavioural consequence.
//!
//! ## The problem
//!
//! Everything so far assumed `Pending` eventually happens. Consider a socket
//! with a fast producer on the other end:
//!
//! ```text
//! loop {
//!     let n = socket.read(&mut buf).await?;   // always ready — data is always there
//!     process(&buf[..n]);
//! }
//! ```
//!
//! This future never returns `Pending`. It is not a bug, it is not blocking,
//! it is not doing anything wrong — and it holds its worker forever. Every
//! other task on that worker starves. Timers on that worker do not fire,
//! because the worker never reaches its park.
//!
//! An `mpsc` receiver with a fast sender does the same. So does a busy
//! `Notify`. Any resource that can be *persistently ready* is this hazard.
//!
//! ## The idea
//!
//! Give each task a budget — tokio uses 128 — decremented by every resource
//! operation that would have been ready. When it hits zero, the resource
//! returns `Pending` **and immediately wakes its own waker**.
//!
//! The task goes back on the run queue, everyone else gets a turn, and the
//! task resumes with a fresh budget. Nothing is lost; the wakeup is not
//! dropped, only deferred.
//!
//! What makes this elegant is where it lives: not in the scheduler, and not in
//! user code, but in the *resources*. `poll_recv`, `poll_read`, and the timer
//! all consult the same thread-local budget. A future built entirely out of
//! well-behaved resources is automatically pre-emptible without its author
//! ever thinking about it.
//!
//! ## The three things that surprise people
//!
//! - **It changes what `Poll::Pending` means.** It no longer implies "not
//!   ready". It can mean "ready, but you've had your turn". Any code that
//!   treats `Pending` as proof of non-readiness is now subtly wrong.
//! - **It is per-task, not per-poll**, and it resets when the scheduler polls
//!   the task afresh — so a task that yields voluntarily also gets a new budget.
//! - **It needs an opt-out.** `block_on` at the top of the stack must not be
//!   budget-constrained, and some code legitimately wants to drain a queue
//!   fully. Tokio has `task::unconstrained` and `task::consume_budget` for the
//!   two directions.
//!
//! ## What to build
//!
//! - A thread-local `Cell<u32>` budget, set when a worker begins polling a task.
//! - `coop::poll_proceed()` returning a guard: decrement, or return `Pending`
//!   after self-waking.
//! - Wire it into M2's socket read/write and M1's timer poll.
//! - `unconstrained()` — a wrapper that polls with the budget disabled.
//!
//! ## Tests that decide it
//!
//! - The starvation test, which fails before this milestone and passes after:
//!   one task looping on an always-ready channel, one task sleeping 10ms. The
//!   sleep must fire. Without a budget it never does.
//! - Budget resets between polls (the greedy task makes progress, it is only
//!   interleaved).
//! - `unconstrained` genuinely bypasses it.
//! - Total throughput of the greedy task drops by less than a few percent —
//!   fairness is supposed to be cheap. Measure it; this is the kind of claim
//!   the repo rule says you do not get to make for free.
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `task/coop/mod.rs` | the budget, the guard, `poll_proceed` |
//! | `task/coop/unconstrained.rs` | the opt-out |
//! | `task/coop/consume_budget.rs` | manual decrement, for CPU-bound loops with no resource to hook |
//! | `task/yield_now.rs` | the blunt, explicit version a user can call |
//!
//! Then grep for `coop::poll_proceed` across `runtime/io/` and `sync/` to see
//! how few call sites it takes to make the whole runtime pre-emptible.
