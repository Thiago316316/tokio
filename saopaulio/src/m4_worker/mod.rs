//! # M4 — N threads, per-thread queues, stealing: the worker
//!
//! **New concept:** the worker, and every concurrency problem you have so far
//! been allowed to ignore.
//!
//! **Status: not started. This is the largest milestone in the list — expect
//! it to take longer than M0–M3 combined.**
//!
//! ## What actually changes
//!
//! Not "add threads". Three separate things stop being true at once:
//!
//! 1. `Rc<RefCell<VecDeque>>` becomes a lock-free deque. The run queue is now
//!    touched by its owner (push/pop, one end) and by thieves (steal, other
//!    end) concurrently.
//! 2. Your `Waker` from M0 becomes genuinely concurrent — `wake()` can be
//!    called from any thread, at any time, including while the task is being
//!    polled on another thread. This is where `runtime/task/state.rs`'s packed
//!    refcount+lifecycle atomic stops looking like over-engineering.
//! 3. M1's wheel is behind a `Mutex`, shared by every worker. Deliberately.
//!    [`crate::m5_per_worker_time`] is about removing it, and that only means
//!    something if the contention was real first.
//!
//! ## The three sub-problems, in the order they bite
//!
//! ### Stealing
//!
//! The easy one, and the one everybody starts with. A worker with an empty
//! queue takes work from a victim. Details that matter: steal **half**, not
//! one (or the thief is back immediately); pick the victim at random (or
//! worker 0 is permanently robbed); bound the retries (or idle workers burn
//! CPU spinning on each other).
//!
//! ### Notification — the actually hard one
//!
//! A task is spawned. Eight workers are parked. Who wakes up?
//!
//! - Wake all eight: a thundering herd, seven wake, find nothing, park again.
//!   Burns CPU and cache in proportion to core count.
//! - Wake exactly one: now you need to know *that* there is a parked worker
//!   and pick it, atomically, against other threads doing the same, without a
//!   lock on the spawn path.
//! - Wake none because you raced with the last worker parking: **the runtime
//!   deadlocks with work in the queue.** This is the lost wakeup from M0,
//!   returning at a scale where a test will not reliably find it.
//!
//! Tokio's answer is a single atomic packing "number of searching workers" and
//! "number of unparked workers", with the rule that a worker must announce
//! itself as searching *before* it checks the queues, and re-check *after* it
//! announces it is parking. Read `multi_thread/idle.rs`. Then read it again.
//!
//! ### The LIFO slot
//!
//! A one-task slot that bypasses the queue. When task A wakes task B and
//! immediately yields, B is hot in A's cache — running it next is a large,
//! measurable win for message-passing workloads (an `mpsc` ping-pong).
//!
//! And it is a starvation bug: two tasks waking each other occupy the slot
//! forever while the rest of the queue never runs. So the slot needs a
//! throttle — after N consecutive LIFO hits, go to the queue. Tokio has this,
//! and the constant was chosen by benchmark.
//!
//! This is the milestone's lesson in miniature: **every scheduler optimisation
//! is a fairness trade, and you cannot evaluate it by reading the code.**
//!
//! ## Build the benchmark *here*, not at M5
//!
//! [`crate::m5_per_worker_time`] claims that removing the shared timer mutex
//! is a win. That claim is only worth anything with a before-and-after number,
//! and "before" is only measurable while the shared mutex still exists — i.e.
//! now.
//!
//! So M4 ships a harness: N workers, each creating and cancelling timers in a
//! loop, reporting throughput at 1/2/4/8 threads. Tokio's equivalent is
//! `benches/time_timeout.rs`, added by PR #6512 for exactly this purpose.
//!
//! The repo rule, from `CLAUDE.md`: *"Tokio does not merge performance claims
//! without a reproducer."* Adopt it here or M5 is a vibe.
//!
//! ## Tests that decide it
//!
//! - N workers, M tasks, every task runs exactly once. Run it 10,000 times in
//!   a loop — concurrency bugs at this layer are probabilistic.
//! - Spawn from inside a task, from outside the runtime, and from a task on a
//!   different worker.
//! - The deadlock test: spawn one task while all workers are parked, with a
//!   sleep placed to make the park/spawn race likely. Loop it under
//!   `--release`.
//! - A blocking task on one worker does not stop the others (it *will* stop
//!   its own — that is [`crate::m8_blocking`]).
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `runtime/scheduler/multi_thread/queue.rs` | the bounded lock-free ring, and `steal_half` |
//! | `runtime/scheduler/multi_thread/idle.rs` | **the notification protocol.** The subtlest file in the scheduler |
//! | `runtime/scheduler/multi_thread/worker.rs` | the worker loop, the LIFO slot, and the throttle |
//! | `runtime/scheduler/multi_thread/park.rs` | worker park/unpark over the driver |
//! | `runtime/scheduler/inject.rs` | the global queue for tasks spawned from outside a worker |
//! | `runtime/scheduler/multi_thread/overflow.rs` | what happens when a worker's bounded queue is full |
//!
//! ## And this is where loom starts to matter
//!
//! Everything above is written against `crate::loom::*` in tokio, not
//! `std::sync`, so that a model checker can enumerate the interleavings
//! exhaustively rather than hoping a stress test trips them. `loom_multi_thread`
//! is sharded into four CI jobs because one unsharded run exceeds the timeout.
//!
//! You do not have to adopt loom here. But if you write the idle protocol
//! yourself and want to know whether it is *correct* rather than *not yet
//! observed to fail*, loom is the only tool that answers that question, and
//! `tokio/src/sync/tests/loom_*.rs` are the models to copy.
