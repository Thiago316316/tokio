//! Throwaway demo to *see* the tokio runtime behave. Run with:
//!     cargo run --example rt_demo
//!
//! It shows three things:
//!   1. Concurrency: three tasks with different sleep durations interleave
//!      instead of running one-after-another (proof the time driver parks the
//!      thread and wakes each task exactly when its deadline fires).
//!   2. spawn: tasks run "in the background" on the worker pool.
//!   3. Total wall-clock time ≈ the *longest* task, not the *sum* — the tell
//!      that they overlapped.

use std::time::{Duration, Instant};
use tokio::time::sleep;
//use std::thread::sleep;
async fn worker(name: &str, ms: u64) {
    println!("  [{name}] start, will sleep {ms}ms");
    sleep(Duration::from_millis(ms)).await; // <- yields to the runtime; thread is freed
    println!("  [{name}] woke up after {ms}ms");
}
#[tokio::main(flavor = "current_thread")]
//#[tokio::main] // expands to: build a multi-thread runtime, then block_on(this body)
async fn main() {
    println!("runtime started on thread {:?}\n", std::thread::current().id());
tokio::spawn(async { std::thread::sleep(Duration::from_secs(1)); });
tokio::spawn(async { std::thread::sleep(Duration::from_secs(1)); });

    // --- Part 1: run three futures concurrently on THIS task ---
    println!("join! three sleeps (300 / 100 / 200 ms) concurrently:");
    let t0 = Instant::now();
    tokio::join!(worker("A", 300), worker("B", 100), worker("C", 200));
    println!(
        "join! finished in {}ms (≈ the longest task, not the sum of 600)\n",
        t0.elapsed().as_millis()
    );

    // --- Part 2: spawn tasks onto the worker pool and await their handles ---
    println!("spawn 3 tasks onto the worker threads:");
    let t1 = Instant::now();
    let handles: Vec<_> = (0..3)
        .map(|i| {
            tokio::spawn(async move {
                sleep(Duration::from_millis(150)).await;
                let tid = std::thread::current().id();
                println!("  [task {i}] done on thread {tid:?}");
                i * 10
            })
        })
        .collect();

    let mut sum = 0;
    for h in handles {
        sum += h.await.unwrap(); // JoinHandle is itself a Future
    }
    println!(
        "all spawned tasks done in {}ms, summed results = {sum}\n",
        t1.elapsed().as_millis()
    );
    println!("done. notice total time was tiny — the threads were PARKED while sleeping,");
    println!("not spinning. that parking is the I/O + time driver at work.");
}
