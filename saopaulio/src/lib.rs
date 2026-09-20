//! # saopaulio — an async runtime, built to understand tokio
//!
//! Not a clone. The goal is to meet every load-bearing idea in tokio once, in
//! its smallest honest form, in the order that makes each one *necessary*
//! rather than merely present.
//!
//! One module per milestone. Each carries a doc explaining the problem it
//! exists to solve, what to build, the tests that decide whether it is done,
//! and where tokio does the same thing. Read the doc before the code — for the
//! unbuilt milestones the doc *is* the module.
//!
//! See `ROADMAP.md` for the index and the current state.
//!
//! ## The milestones
//!
//! | | | new concept | status |
//! |---|---|---|---|
//! | [`m0_task`] | `block_on` + `spawn` + a run queue | task, waker, run queue | not started |
//! | [`m1_time`] | `sleep(dur)` | **the wheel** | wheel done, integration pending |
//! | [`m2_io`] | TCP via epoll | readiness, the driver stack | not started |
//! | [`m3_timeout`] | `timeout(dur, fut)` | combinator, cancellation | not started |
//! | [`m4_worker`] | N threads, per-thread queues, stealing | the worker | not started |
//! | [`m5_per_worker_time`] | the wheel from shared to per-worker | the #7467 lesson, earned | not started |
//! | [`m6_lifecycle`] | `JoinHandle`, abort, shutdown | who owns a task | not started |
//! | [`m7_coop`] | the cooperative budget | a ready resource starves its worker | not started |
//! | [`m8_blocking`] | the blocking pool | two pools, and why `fs` is not async | not started |
//!
//! M0–M5 are the original roadmap. M6–M8 were added after measuring what
//! M0–M5 would leave untouched — each is a distinct idea, not more breadth.
//!
//! ## Scale check
//!
//! `tokio/src` is 105,081 lines. The territory these milestones cover — task,
//! scheduler, io driver, time, blocking — is about 16,400 of them. The other
//! ~88,700 (`sync/`, `net/`, `io/`, `fs/`, `process/`, `signal/`) is breadth:
//! the tenth channel type teaches nothing the first did not.
//!
//! What you write should be far under 16,400 too. Tokio's version of each file
//! also carries `tokio_unstable` metrics, task dumps, tracing, loom shims, and
//! six years of edge cases. None of that is the idea.

#![allow(dead_code)]

pub mod m0_task;
pub mod m1_time;
pub mod m2_io;
pub mod m3_timeout;
pub mod m4_worker;
pub mod m5_per_worker_time;
pub mod m6_lifecycle;
pub mod m7_coop;
pub mod m8_blocking;
