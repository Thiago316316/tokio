//! # M8 — the blocking pool, and why `tokio::fs` is a lie
//!
//! **New concept:** two pools, and the boundary between async and not.
//!
//! **Status: not started.** New — not in the original M0–M5 roadmap. Depends
//! on [`crate::m4_worker`].
//!
//! ## The problem
//!
//! [`crate::m7_coop`] handles a task that is *always ready*. It does nothing
//! for a task that is genuinely **blocked in a syscall**:
//!
//! ```text
//! std::fs::read("big.dat")          // blocks the OS thread
//! expensive_pure_computation()      // never yields
//! diesel::query(&conn)              // a blocking C library
//! ```
//!
//! A worker thread inside any of these is gone. Its run queue is frozen —
//! other workers can steal from it, so the runtime degrades rather than
//! stopping, but with `N` such calls on an `N`-worker runtime it stops dead.
//! Including the drivers, so timers stop and sockets go unread.
//!
//! ## The idea
//!
//! A second, separate thread pool with completely different properties:
//!
//! | | worker pool | blocking pool |
//! |---|---|---|
//! | size | fixed, ≈ core count | elastic, up to 512 |
//! | runs | futures | closures |
//! | work stealing | yes | no — it is a plain queue |
//! | cancellable | yes (drop the future) | **no** |
//! | idle threads | parked, kept | reaped after a timeout |
//!
//! The size difference is the design: worker threads are sized for *CPU
//! parallelism*, blocking threads for *outstanding blocked syscalls*, and those
//! are unrelated numbers. 512 threads is absurd for computation and reasonable
//! for "how many files might I have open reads on".
//!
//! **Not cancellable** is the sharp edge and the thing to sit with. Once a
//! closure is running on a blocking thread, nothing can stop it — there is no
//! poll to not-call and no `Drop` to fire. So `spawn_blocking(...)` whose
//! `JoinHandle` you drop still runs to completion, and `Runtime::drop` cannot
//! wait for it, which is why `shutdown_timeout` exists and why its docs are
//! full of warnings.
//!
//! ## The payoff: `fs/` collapses
//!
//! `tokio/src/fs/` is ~5,000 lines. There is no async file IO in it. Every
//! function is `spawn_blocking` around the `std::fs` equivalent.
//!
//! That is not laziness. Until io_uring, Linux had no usable async file IO:
//! files are *always* "ready" to epoll, so registering one is meaningless, and
//! the kernel will happily block you in `read()` on a cold page. A thread pool
//! is the only correct answer available.
//!
//! Getting this is worth 5,000 lines of reading, and it generalises: **when a
//! library says "async", ask which of the two pools it is actually using.**
//!
//! ## What to build
//!
//! - A `BlockingPool`: `Mutex<VecDeque<Box<dyn FnOnce() + Send>>>` + `Condvar`,
//!   spawn-on-demand up to a cap, reap idle threads after ~10s.
//! - `spawn_blocking(f) -> JoinHandle<T>` — reuses [`crate::m6_lifecycle`]'s
//!   handle, minus abort.
//! - One `fs::read` on top of it, to prove the point to yourself.
//! - Shutdown: the pool must be drained or abandoned deliberately, with the
//!   choice documented.
//!
//! ## Tests that decide it
//!
//! - N+1 blocking calls on an N-worker runtime: async tasks still progress.
//! - `spawn_blocking` whose handle is dropped still runs (assert via a channel).
//! - The pool grows past core count and then shrinks back.
//! - Runtime shutdown with a blocking task mid-flight terminates — and write
//!   down which of "waits" or "abandons" you chose.
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `runtime/blocking/pool.rs` | the pool, the spawn/reap policy, and `shutdown_timeout` |
//! | `runtime/blocking/schedule.rs` | how a blocking task reports back to the main runtime |
//! | `runtime/blocking/shutdown.rs` | the drain |
//! | `fs/mod.rs` and any file next to it | confirm for yourself that it is all `asyncify!` |
//! | `runtime/scheduler/block_in_place.rs` | the other door: hand your *worker* to the blocking pool and keep its queue alive. Multi-thread only, and the asymmetry is instructive |
