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

use crate::m1_time::{sleep::{Sleep}};
use std::{future::Future, pin::Pin, task::{Context, Poll}};
pub struct Timeout<F>{
    future: F,
    sleep: Sleep,
}

impl <F: Future> Timeout<F>{
    
    pub fn new(future: F, sleep: Sleep) -> Self{
        Timeout{future, sleep}
    }

    fn project(self: Pin<&mut Self>) -> (Pin<&mut F>, Pin<&mut Sleep>){
        unsafe {
            let this = self.get_unchecked_mut();
            (
                Pin::new_unchecked(&mut this.future),
                Pin::new_unchecked(&mut this.sleep),
            )
        }// don't ever move the children out of the reutrn of this function!!!!!
         // this is the tacit guarantee that the children never can be moved out of parent
         //that is trully pined.
         // if we move, then the address of the children will cange, wich means that 
         //ins't pined anymore, and that is UB.
    }
}

impl <F: Future> Future for Timeout<F>{
    type Output = Result<F::Output, ()>;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
        println!("\nTimeout.poll()");
        let (mut future, mut sleep) = self.project();//don't ever move the children out of the reutrn of this function!!!!!

        match Pin::new(&mut future).poll(cx) {
            Poll::Ready(value) => {
                println!("\nTimeout.poll(): Future won against sleep");
                return Poll::Ready(Ok(value));
            },
            Poll::Pending => {},
        }
        match Pin::new(&mut sleep).poll(cx){
            Poll::Ready(()) => {
                println!("\nTimeout.poll(): Sleep won against future");
                Poll::Ready(Err(()))
            },
            Poll::Pending => {
                Poll::Pending
            },
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::m0_task::{current_exec, Runtime};
    use std::future::poll_fn;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// Pending on the first poll, Ready on the second.
    ///
    /// The point is that poll #1 forces `Timeout` to reach the `Sleep` and
    /// register it in the wheel. An inner future that is Ready immediately
    /// never gets there, so `handle` stays `None` and the leak test below
    /// would pass for the wrong reason.
    struct PendingOnce {
        polled: bool,
    }

    impl Future for PendingOnce {
        type Output = u64;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
            if self.polled {
                return Poll::Ready(7);
            }
            self.polled = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    /// Sets a flag when dropped. Held across an await inside the inner future,
    /// so it only runs if that future is actually dropped.
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    /// `current_exec()` panics outside a runtime, so this only works inside a
    /// `block_on` future.
    fn live_timers() -> usize {
        current_exec().time.lock().unwrap().count()
    }

    // ------------------------------------------------------------------

    #[test]
    fn the_inner_future_wins_and_yields_its_value() {
        let rt = Runtime::new();
        let r = rt.block_on(async {
            Timeout::new(PendingOnce { polled: false }, Sleep::sleep(Duration::from_secs(60))).await
        });
        assert_eq!(r, Ok(7));
    }

    /// **The test that decides M3.**
    ///
    /// 500 timeouts that all register a wheel entry and then complete before it
    /// fires. Nothing cancels those entries except `Sleep::drop`, which runs
    /// because dropping `Timeout` drops its `sleep` field. Delete that `Drop`
    /// impl and this reports 500 corpses sitting in the wheel until their 60s
    /// deadlines pass — which, on a real server, is the steady-state leak that
    /// makes this milestone matter.
    #[test]
    fn completed_timeouts_leave_the_wheel_empty() {
        let rt = Runtime::new();
        rt.block_on(async {
            for _ in 0..500 {
                let r = Timeout::new(
                    PendingOnce { polled: false },
                    Sleep::sleep(Duration::from_secs(60)),
                )
                .await;
                assert_eq!(r, Ok(7));
            }

            assert_eq!(
                live_timers(),
                0,
                "a completed Timeout left its Sleep registered in the wheel"
            );
        });
    }

    /// When the deadline wins, the inner future is dropped where it stood — no
    /// notification, no async cleanup. That IS cancellation in Rust.
    #[test]
    fn timing_out_drops_the_inner_future() {
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();

        let rt = Runtime::new();
        let r = rt.block_on(async move {
            Timeout::new(
                async move {
                    let _guard = DropFlag(flag); // alive across the await
                    Sleep::sleep(Duration::from_secs(60)).await;
                },
                Sleep::sleep(Duration::from_millis(20)),
            )
            .await
        });

        assert_eq!(r, Err(()));
        assert!(
            dropped.load(Ordering::Acquire),
            "the inner future was not dropped when the timeout fired"
        );
    }

    /// A fired timeout must also clean up: both its own `Sleep` and the one the
    /// inner future had registered.
    #[test]
    fn a_fired_timeout_leaves_the_wheel_empty() {
        let rt = Runtime::new();
        rt.block_on(async {
            let r = Timeout::new(
                Sleep::sleep(Duration::from_secs(60)),
                Sleep::sleep(Duration::from_millis(20)),
            )
            .await;

            assert_eq!(r, Err(()));
            assert_eq!(live_timers(), 0, "the inner future's timer outlived it");
        });
    }

    /// Poll order, deterministically.
    ///
    /// The `Timeout` is built but NOT polled, then 30ms pass. Its 1ms deadline
    /// is now long gone, so on the very first poll both arms are ready at once.
    /// Polling the inner future first means the work wins. Swap the two `match`
    /// blocks in `poll` and this returns `Err(())` — i.e. a 504 for a request
    /// that actually succeeded.
    #[test]
    fn the_inner_future_wins_a_tie() {
        let rt = Runtime::new();
        let r = rt.block_on(async {
            let mut t = Box::pin(Timeout::new(
                async { 7u64 },
                Sleep::sleep(Duration::from_millis(1)),
            ));

            Sleep::sleep(Duration::from_millis(30)).await;

            poll_fn(move |cx| t.as_mut().poll(cx)).await
        });

        assert_eq!(
            r,
            Ok(7),
            "poll order is reversed — the deadline beat completed work"
        );
    }

    /// Nesting. The outer deadline fires first; dropping the outer `Timeout`
    /// drops the inner one, which drops *its* two futures. Three registered
    /// timers, all unwound by one `Drop` chain that nobody wrote by hand.
    #[test]
    fn nested_timeouts_clean_up() {
        let rt = Runtime::new();
        rt.block_on(async {
            let r = Timeout::new(
                Timeout::new(
                    Sleep::sleep(Duration::from_secs(60)),
                    Sleep::sleep(Duration::from_secs(30)),
                ),
                Sleep::sleep(Duration::from_millis(20)),
            )
            .await;

            assert_eq!(r, Err(()));
            assert_eq!(live_timers(), 0, "a nested timer survived the outer drop");
        });
    }
}
