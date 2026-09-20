//! # M2 — TCP via epoll: readiness and the driver stack
//!
//! **New concept:** readiness (as opposed to completion), and how drivers
//! compose into a single park.
//!
//! **Status: not started.** Depends on [`crate::m0_task`] and
//! [`crate::m1_time`].
//!
//! ## The problem
//!
//! M0's `park()` sleeps until someone unparks. M1's sleeps until the nearest
//! deadline. Now a socket has to be able to wake it too — and there may be
//! 10,000 sockets and a timer, and the thread has to sleep on **all of them at
//! once**.
//!
//! ## Readiness, not completion
//!
//! This is the sentence to internalise: epoll tells you a socket *would not
//! block*, not that any bytes were moved. You still have to call `read()`
//! yourself, and it can still return `EWOULDBLOCK` because another thread beat
//! you to it, or the kernel was optimistic.
//!
//! So the flow is always:
//!
//! ```text
//! 1. try the syscall
//! 2. EWOULDBLOCK? -> clear the readiness bit, register the waker, return Pending
//! 3. epoll says ready -> set the bit, wake the waker
//! 4. goto 1
//! ```
//!
//! and step 2's ordering is load-bearing. Clear the bit **before** retrying,
//! never after: if the kernel becomes ready between your failed `read` and
//! your bit-clear, clearing after would erase a real event and the socket
//! hangs forever. This is the readiness equivalent of M0's lost wakeup, and it
//! is why `ScheduledIo` is a packed atomic rather than a bool.
//!
//! (Completion-based IO — io_uring, IOCP — inverts this: you hand the kernel a
//! buffer and it tells you when bytes actually moved. Fundamentally different
//! ownership story, which is why tokio's io_uring support is a separate
//! unstable thing and not a swap-in.)
//!
//! ## The driver stack
//!
//! Once there are two sources of wakeups, `park()` has to consult both. The
//! composition is a nesting, not a list:
//!
//! ```text
//! time driver          "the nearest deadline is in 40ms"
//!   └── io driver      epoll_wait(fds, timeout = 40ms)
//!         └── signal   (a self-pipe registered as just another fd)
//! ```
//!
//! The time driver does not sleep. It computes a timeout and hands it to the
//! thing that *can* sleep. This is why `next_deadline()` from M1 sits on the
//! hot path of every idle→busy transition, and why making it O(1) mattered.
//!
//! The trick that makes signals and cross-thread wakeups work at all: register
//! a pipe (or `eventfd`) with epoll, and to interrupt a sleeping `epoll_wait`,
//! write one byte to it. There is no other way to wake a blocked `epoll_wait`.
//!
//! ## What to build
//!
//! - A `Driver` owning an epoll fd, an `eventfd`/pipe for unparking, and a
//!   registration table `fd -> Arc<ScheduledIo>`.
//! - `ScheduledIo`: readiness bits + a reader waker slot + a writer waker slot.
//!   Start with `Mutex<Option<Waker>>` for each; the packed atomic is an
//!   optimisation, not the concept.
//! - `park(timeout)`: `epoll_wait`, then for each event set readiness and wake.
//! - `TcpStream` wrapping a non-blocking fd, with `async fn read`/`write`
//!   built from the four-step loop above.
//! - Wire M1's `next_deadline()` in as the timeout argument. **This is the
//!   milestone where M1 stops being a data structure and becomes a timer.**
//!
//! Use `libc` directly or `mio`. `libc` teaches more; `mio` gets you to M3
//! faster and is what tokio does. Either is defensible — but if you use `mio`,
//! read `runtime/io/driver.rs` first and notice how thin the wrapper is.
//!
//! ## Tests that decide it
//!
//! - Echo server: connect, write, read back, on one thread, with a task also
//!   sleeping. Both make progress.
//! - A socket that becomes readable *while the task is not polling* still
//!   wakes it (readiness was latched, not lost).
//! - A `sleep` and a socket race: whichever is nearer fires first, and the
//!   other is not lost.
//! - Spurious wakeup: force an `EWOULDBLOCK` after epoll reported ready, assert
//!   the task re-registers and completes rather than hanging.
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `runtime/driver.rs` | builds the nested park stack described above |
//! | `runtime/io/driver.rs` | the epoll loop (via mio) |
//! | `runtime/io/scheduled_io.rs` | **the file to actually study.** One `AtomicUsize` packed `\| shutdown:1 \| tick:15 \| readiness:16 \|`, plus a `Mutex<Waiters>` with an intrusive list |
//! | `runtime/io/registration.rs` | the fd ↔ `ScheduledIo` binding, and the async retry loop |
//!
//! The `tick` field in that packed word is worth a minute: it distinguishes a
//! readiness event from *this* epoll pass from a stale one, which is the bug
//! class you cannot test your way out of without it.
//!
//! Note also what does **not** exist upstream: there are no loom tests for
//! `runtime/io/` at all. That is tokio issue #3018, open since 2020 — see the
//! Loom section of the repo's `CLAUDE.md`. If you want a real contribution
//! that is not a toy, `ScheduledIo` needs zero mio mocking and is the tractable
//! half of it.
