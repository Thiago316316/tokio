//! # M6 — `JoinHandle`, abort, and shutdown: task lifecycle
//!
//! **New concept:** who owns a task, and how a runtime stops.
//!
//! **Status: not started.** This one is *new* — it is not in the original
//! M0–M5 roadmap, and it was added because M0–M5 can be completed without ever
//! confronting it.
//!
//! ## Why it is not part of M0
//!
//! [`crate::m0_task`]'s `spawn` returns nothing. That is a legitimate
//! simplification for a single-threaded runtime, and it hides the fact that a
//! task allocation has **three** independent owners racing over it:
//!
//! - the **scheduler**, which holds it while it is queued or running;
//! - the **`JoinHandle`**, which may be dropped, awaited, or used to abort at
//!   any moment;
//! - every **`Waker`** cloned out of it, which may outlive both and may fire
//!   from any thread.
//!
//! Single-threaded, this is a refcount. Multi-threaded, after
//! [`crate::m4_worker`], it is a state machine — which is why M6 sits here and
//! not at the start.
//!
//! ## The questions that have no obvious answer
//!
//! Each of these is a real design decision, and tokio's answer to each is
//! non-obvious enough to be worth finding out on your own first:
//!
//! - A task completes. Nobody is awaiting the `JoinHandle`. **Where does the
//!   output live, and who frees it?**
//! - `abort()` is called while the task is *mid-poll on another worker*. You
//!   cannot stop a running poll. What does `abort` mean here, and when does the
//!   `JoinHandle` observe it?
//! - The `JoinHandle` is dropped. Does the task keep running? (Tokio: yes.
//!   `async_std` made the opposite choice. Neither is wrong — but they are
//!   different, and the difference is user-visible.)
//! - The task panics. The panic must not kill the worker; it has to be caught
//!   and delivered to the `JoinHandle` as a `JoinError`. What if nobody is
//!   listening?
//! - `Runtime::drop` is called with tasks still queued. Are they dropped
//!   (running their destructors, possibly cancelling timers and closing
//!   sockets), or leaked?
//!
//! ## Shutdown is the part that bites
//!
//! Shutting down means, roughly:
//!
//! 1. stop accepting new tasks;
//! 2. wake every parked worker;
//! 3. drop every queued task — which runs user destructors, which can spawn,
//!    cancel timers, and close sockets *during shutdown*;
//! 4. shut down the drivers;
//! 5. join the worker threads.
//!
//! Step 3 is where the difficulty lives, and step 4 racing with step 3 is a
//! real, shipped tokio bug: **#2924**, *"net: possible race during I/O driver
//! shutdown"* — an IO resource created concurrently with driver shutdown could
//! miss its shutdown notification and hang forever.
//!
//! That bug is the reason tokio issue **#3018** (*"io::driver should have loom
//! tests"*) exists, and #3018 has been open with `E-help-wanted` since
//! 2020-10-21. Which is to say: this milestone is not a tidying-up exercise
//! appended to the end of the list. It is a place where the upstream project
//! has known, unfixed, five-year-old gaps.
//!
//! ## What to build
//!
//! - A shared cell for the task output + a waker slot for the joiner, with a
//!   state machine covering: running / complete-unconsumed / joined / aborted /
//!   panicked.
//! - `JoinHandle<T>: Future<Output = Result<T, JoinError>>`.
//! - `abort()`, including the "already running" and "already finished" cases.
//! - A task registry so shutdown can find tasks that are neither queued nor
//!   running (they are parked inside a driver).
//! - `Runtime::drop` implementing the five steps above.
//!
//! ## Tests that decide it
//!
//! - Output is delivered to a `JoinHandle` awaited *after* the task finished.
//! - Dropping the `JoinHandle` of a running task leaks nothing (check with a
//!   `Drop` counter, not by eye).
//! - `abort()` on a task parked in a `sleep` removes its wheel entry.
//! - A panicking task does not poison the worker; the next task still runs.
//! - Build the runtime, spawn 1000 tasks that all `sleep`, drop the runtime
//!   immediately. No hang, no leak. Loop it 1000×.
//! - The #2924 shape: create an IO resource concurrently with shutdown.
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `runtime/task/state.rs` | **the file.** Refcount + lifecycle in one atomic; its comments are the specification and are enforced nowhere else |
//! | `runtime/task/join.rs` | `JoinHandle` |
//! | `runtime/task/abort.rs` | `AbortHandle` |
//! | `runtime/task/error.rs` | `JoinError` — cancelled vs panicked |
//! | `runtime/task/list.rs` | the registry ("owned tasks") shutdown walks |
//! | `task/join_set.rs` | the ergonomic layer on top |
//!
//! `CLAUDE.md` puts it flatly: *"Read `state.rs` before touching anything in
//! this directory: the invariants are stated there and enforced nowhere else."*
