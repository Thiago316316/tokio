# saopaulio

An async runtime built to understand tokio. Not a clone — the aim is to meet
every load-bearing idea once, in its smallest honest form, in the order that
makes each one *necessary*.

One directory per milestone under `src/`. Each has a module doc (`mod.rs`,
`//!`) covering: the problem, the idea, what to build, the tests that decide
it, and where tokio does the same thing. For unbuilt milestones the doc *is*
the module.

```bash
cargo test                 # 38 passing, all M1
cargo doc --open           # the roadmap as rustdoc — this is the intended way to read it
```

## State

| | | new concept | status |
|---|---|---|---|
| **M0** `m0_task` | `block_on` + `spawn` + a run queue | task, waker, run queue | not started |
| **M1** `m1_time` | `sleep(dur)` | the wheel | **wheel done**, integration pending |
| **M2** `m2_io` | TCP via epoll | readiness, the driver stack | not started |
| **M3** `m3_timeout` | `timeout(dur, fut)` | combinator, cancellation | not started |
| **M4** `m4_worker` | N threads, per-thread queues, stealing | the worker | not started |
| **M5** `m5_per_worker_time` | the wheel from shared to per-worker | the #7467 lesson, earned | not started |
| **M6** `m6_lifecycle` | `JoinHandle`, abort, shutdown | who owns a task | not started |
| **M7** `m7_coop` | the cooperative budget | a ready resource starves its worker | not started |
| **M8** `m8_blocking` | the blocking pool | two pools; why `fs` is not async | not started |

M0–M5 are the original roadmap. M6–M8 were added after checking what M0–M5
would leave untouched. Each is a distinct concept, not more breadth.

## Dependency order

```
M0 ─┬─> M1 ──> M3
    │    │
    └─> M2 ──┘
         │
         v
        M4 ─┬─> M5
            ├─> M6
            ├─> M7
            └─> M8
```

M1's data structure is done ahead of M0, which is fine — it is a data
structure. It becomes a *timer* only when it holds `Waker`s (M0) and drives a
park timeout (M2).

**M4 is the largest milestone here.** Expect it to take longer than M0–M3
combined; three things stop being true at once (the queue becomes lock-free,
the waker becomes concurrent, the wheel goes behind a mutex).

## Done so far — M1

- `m1_time/heap.rs` — `BinaryHeap<Reverse<(deadline, id)>>`. O(n) `cancel`,
  written first so the wheel answers a problem that was actually felt. Kept
  permanently as the **oracle** for differential tests.
- `m1_time/wheel.rs` — hierarchical hashed timing wheel, 6 levels × 64 slots,
  per-level `occupied` bitmask, cascading expire, address-computed cancel.

38 tests pass. The three that matter:

- `the_wheel_agrees_with_the_heap` — 10k random deadlines through both, 1ms
  steps, identical output.
- `..._under_jumpy_expire` — same, but each `expire` cascades several levels
  and fires several deadlines in one call.
- `next_deadline_never_overshoots_the_oracle` — 5000 random
  insert/cancel/expire steps, checked after every one against a brute-force
  scan of all 384 slots.

## Immediate next step

`m1_time::tests::cancel_cost` still has its wheel half commented out, deferred
back when one level held at most 64 timers. Step 3 removed that excuse.
Uncomment it, fix the path to `super::wheel::Wheel`, record the number. It is
ten minutes and it is the payoff for the whole module.

After that, M0 — M1 cannot finish without something to wake.

## House rules, borrowed from upstream

- **No performance claim without a reproducer.** PR #6512 shipped
  `benches/time_timeout.rs` in the same commit as the optimisation. Build M4's
  benchmark harness *before* M5, while the shared mutex still exists to measure.
- **Write the wrong version too.** M5 asks for the sharded timer (#6534) even
  though it was reverted (#7226). Reproducing a documented regression by hand
  is worth more than skipping straight to the answer.
- **Meet the bug deliberately.** Several milestones ask for a test that hangs
  or deadlocks, to be run once and then deleted. Lost wakeups are the defining
  failure of this whole subject.

## Reading tokio alongside

The parent repo is upstream tokio, so every citation in the docs resolves:

```bash
cd ..
git show f6eb1ee1   # #6512 time: lazily init timers on first poll
git show 1914e1e4   # #6534 time: use sharding for timer implementation
git show 1ae9434e   # #7226 time: REVERT the sharding work
git show 73d733a3   # #7467 time: alternative per-worker timer
gh issue view 7384  # the per-worker timer RFC
gh issue view 3018  # io::driver should have loom tests — open since 2020
```

See `../CLAUDE.md` for the timer-contention case study in full.
