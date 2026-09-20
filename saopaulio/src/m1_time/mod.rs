//! # M1 — `sleep(dur)`: the wheel
//!
//! **New concept:** a deadline store, and park-with-timeout.
//!
//! **Status: the data structure is done. The runtime integration is not.**
//!
//! ## The problem
//!
//! A task calls `sleep(100ms)`. The runtime has nothing else to run. It must
//! not spin, so it parks — but it has to wake up in 100ms. So parking needs an
//! answer to one question: *what is the earliest deadline anyone is waiting
//! for?* Call it `next_deadline()`, and note that it is asked **before every
//! park**, which is to say constantly. That is the operation the whole
//! structure is designed around.
//!
//! Three operations, and they pull in different directions:
//!
//! | | asked | must be |
//! |---|---|---|
//! | `insert` | once per `sleep`/`timeout` created | cheap |
//! | `next_deadline` | before every park | **very** cheap |
//! | `cancel` | once per `timeout` that *didn't* fire — the common case | cheap |
//!
//! That last row is the one people miss. In a server, almost every `timeout`
//! is cancelled rather than fired, because the request usually succeeds. The
//! timer system's hot path is *creating and destroying timers that never go
//! off*.
//!
//! ## Step 1 — the heap ([`heap`])
//!
//! `BinaryHeap<Reverse<(deadline, id)>>`. `next_deadline` is O(1), `insert` is
//! O(log n) — and `cancel` is O(n), with no way around it. Written first, on
//! purpose, so the wheel is an answer to a problem you have actually felt.
//!
//! Kept afterwards as the **oracle** for differential testing.
//!
//! ## Steps 2–3 — the wheel ([`wheel`])
//!
//! A hierarchical hashed timing wheel (Varghese & Lauck, 1997): 6 levels × 64
//! slots. Level 0 slots are 1ms, each level up multiplies by 64, level 5 slots
//! are ~12 days, `MAX_DURATION = (1 << 36) - 1` ms.
//!
//! The trick, and the thing worth working out on paper before reading the code:
//! the level for a deadline comes from `elapsed ^ deadline`. Not the *distance*
//! to the deadline — **which base-64 digit first disagrees with now**. A timer
//! 1ms away can be a level up (see `one_millisecond_apart_can_still_be_a_level_apart`);
//! that is not a bug, it costs exactly one cascade.
//!
//! Cascading is not a separate mechanism. Draining a level-`n>0` slot
//! *re-inserts* its entries, and because `elapsed` has grown, they land lower.
//!
//! `next_deadline` is allowed to be **early, never late**. An upper-level slot
//! only knows "somewhere in [4096, 8192)", so it reports 4096. Early costs one
//! wasted wakeup; late is a broken timer.
//!
//! ## The tests that decided it
//!
//! - `the_wheel_agrees_with_the_heap` — 10k random deadlines through both,
//!   stepping 1ms at a time, output must be identical.
//! - `..._under_jumpy_expire` — same, but each `expire` jumps far enough to
//!   cascade several levels and fire several deadlines in one call.
//! - `next_deadline_never_overshoots_the_oracle` — 5000 random
//!   insert/cancel/expire steps, checking after *every* one that the bitmask
//!   fast path never reports later than a brute-force scan of all 384 slots.
//!
//! ## What is left in M1
//!
//! 1. **The `cancel_cost` benchmark below is still half-commented.** It was
//!    deferred in step 2 because one level holds at most 64 timers and there
//!    was nothing to measure. Step 3 removed that excuse: 10k timers now
//!    spread across levels. Uncommenting it produces the number that justifies
//!    this entire module over [`heap`] — do that first, it is ten minutes.
//!
//! 2. **Slot removal is O(len).** `Vec::remove` plus a shift. Tokio's entries
//!    carry their own `prev`/`next` and unlink themselves in O(1). That means
//!    intrusive linked lists, which means `unsafe` + `PhantomPinned`, which is
//!    a real step up — see `tokio/src/util/linked_list.rs`. Optional for M1;
//!    unavoidable by M5.
//!
//! 3. **The payload is `T`, and it needs to become `Waker`.** That is the
//!    actual M1 finish line and it depends on [`crate::m0_task`]:
//!    - `sleep()` is a future: first poll inserts into the wheel and returns
//!      `Pending`; later polls check whether it fired.
//!    - `next_deadline()` becomes the argument to `thread::park_timeout`.
//!    - after `expire`, the drained wakers are woken.
//!
//! 4. **Ticks are bare `u64`.** Real deadlines are `Instant`s. Tokio converts
//!    at the boundary in `runtime/time/source.rs` and keeps the wheel in
//!    integer ticks — which is also what makes `time::pause()` possible.
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `runtime/time/wheel/mod.rs` | the same 6×64 wheel — `level_for` at :275 is the same XOR trick |
//! | `runtime/time/wheel/level.rs` | the per-level `occupied` bitmask |
//! | `runtime/time/entry.rs` | what an entry becomes once it is shared with a driver — read its module doc |
//! | `runtime/time/mod.rs` | the driver that owns the wheel and fires the wakers |
//! | `runtime/time/source.rs` | `Instant` ↔ `u64` ticks |
//!
//! Note the shape difference: tokio's `poll()` hands back **one** entry at a
//! time rather than a `Vec`, so the driver can drop the lock before invoking
//! each waker. A waker can do anything, including re-entering the timer — so
//! calling one while holding the wheel is a deadlock waiting to happen. Our
//! `expire() -> Vec<T>` sidesteps this by being single-threaded and lock-free,
//! and will have to change at M4.

pub mod heap;
pub mod wheel;

#[cfg(test)]
mod tests {
    use super::heap::Timers;

    #[test]
    fn cancel_cost(){
        let n = 10_000;
        let mut t: Timers<u64> = Timers::new();
        let ids: Vec<u64> = (0..n).map(|i| t.insert(i, i)).collect();

        let start = std::time::Instant::now();
        for id in ids { t.cancel(id); }
        eprintln!("n={n} took {:?} on BinaryHeap", start.elapsed());

        /*let n_wheel = 10_000;
        let mut wheel: wheel::Wheel<u64> = wheel::Wheel::new();
        let ids_double: Vec<u64> = (0..n_wheel).map(|i| wheel.insert(i, i).unwrap()).collect();
        let start = std::time::Instant::now();
        for id in ids_double { wheel.cancel(id); }
        eprintln!("n={n_wheel} took {:?} on Wheel", start.elapsed());*/
        // Wheel cancel benchmark deferred to Step 3 — a single level
        // holds at most 64 timers, so there's nothing to measure yet.
        //
        // Step 3 is now done. This is the loose end named in the module doc:
        // uncomment, fix the paths (`super::wheel::Wheel`), and record the
        // number. It is the payoff for the whole module.
    }
}
