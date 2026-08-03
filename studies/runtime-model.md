# The tokio runtime — mental model

A learner's notes on how tokio actually drives async code. Written from the
outside in: what a task is, how the loop runs, and the two mistakes that are
easy to make when picturing it.

---

## 1. What a task is

- A spawned task is **one heap allocation** (`runtime/task/core.rs`): the future,
  its output slot, the state bits, and the refcount all live in that one block.
- The task's **waker is essentially a pointer to that allocation**. Anything that
  wants to wake the task calls the waker, which knows the task's address. So a
  task's identity *is* its address.
- **`Pin` is memory-safety machinery, not about results.** An `async fn` compiles
  to a struct that can hold references across `.await` points (self-referential),
  so it must not move while running — that's all `Pin` means: "this future won't
  move." The *result* is separate: when the task finishes it writes its output
  into that same allocation, the `JoinHandle` (itself a future) becomes ready, and
  whoever `.await`ed it reads the value out.

Do **not** confuse this with the I/O side: the `epoll` token being a pointer to
`ScheduledIo` is the *I/O resource's* address-as-identity, a different object from
the task. Two separate address tricks; don't blend them.

---

## 2. "Park" means two different things

Separating them makes the loop exact:

- **Task-park** = a single future returns `Pending`. Just that one task steps
  aside and leaves its waker behind.
- **Thread-park** = the whole worker calls `epoll_wait`. Happens only when there
  is *nothing left to run*.

A task **parks itself** by returning `Pending`. Parking is never something the
runtime does *to other tasks*.

---

## 3. The loop

```
1. RUN: poll runnable tasks off the queue.
        each one either finishes, or returns Pending (task-parks) and stashes its waker.
2. queue empty? (everyone task-parked)  →  compute nearest timer deadline
3. THREAD-PARK: epoll_wait(timeout = nearest deadline)   ← 0% CPU
4. WAKE: OS returns ready fds + fired timers → call their wakers → tasks go back on the queue
5. goto 1
```

Key points:

- You **drain the whole run queue first** — poll every task that can make
  progress — *then* compute one deadline for all parked timers and block **once**.
  You don't compute a deadline per future.
- That single `epoll_wait` serves **both** I/O and timers: the nearest timer
  deadline becomes its timeout, so it wakes on "an fd is ready" *or* "the soonest
  timer expired," whichever comes first.
- On a single worker thread, **one task runs at a time**. There is no "meanwhile."
  A task runs until it voluntarily returns `Pending`, then the next runnable task
  runs. The *multi*-thread runtime gives real parallelism: N workers = N tasks
  genuinely running at once, plus work-stealing to balance them.

---

## 4. Two mistakes to avoid

### It is event-driven, not turn-based

The runtime is **not** a scheduler handing out timeslices in rotation. A parked
task is **not** re-polled because it's "next in line" — it's re-polled **only when
its waker fires** (I/O ready, timer expired, channel message). After everyone
returns `Pending`, the thread parks in `epoll_wait` and sleeps until an event
says "task X is ready now"; only then does X go back on the queue.

**Finishing order follows event-completion order, not queue order.**

### `.await` doesn't always park

`poll` returns `Pending` only if the thing isn't ready. If the awaited value is
already ready when polled (data already buffered, a free lock, a channel with a
message waiting), `.await` returns `Ready` **immediately** and execution falls
straight through to the next await — no parking. And when a task *is* re-polled,
it runs forward through **as many awaits as it can** until it hits one that's
`Pending` or the function ends — not one await per visit.

---

## 5. Worked trace

Three tasks, each a sequence of awaits:

```
fn ABC() { X.await; Y.await; Z.await; }
fn 123() { 8.await; 9.await; }
fn @#$() { &.await; }

queue: ABC, 123, @#$

poll ABC → X pending → ABC parks (waker left for X's event)
poll 123 → 8 pending → 123 parks (waker left for 8's event)
poll @#$ → & pending → @#$ parks (waker left for &'s event)

queue empty → THREAD parks in epoll_wait(nearest deadline)

...event: & ready → waker → @#$ re-polled → & Ready → no more awaits → @#$ FINISHED
...event: X ready → waker → ABC re-polled → X Ready → advance → Y pending → ABC parks
...event: 8 ready → 123 re-polled → 8 Ready → 9 pending → 123 parks
...event: Y ready → ABC re-polled → Y Ready → Z pending → ABC parks
...event: 9 ready → 123 re-polled → 9 Ready → done → 123 FINISHED
...event: Z ready → ABC re-polled → Z Ready → done → ABC FINISHED
```

`@#$` finished first — not because it was last in the queue, but because *its*
event completed first.

---

## 6. Why it's efficient (and the honest limits)

Efficiency comes almost entirely from three things:

- **One allocation per task** — not an OS thread. An OS thread costs ~8 MB of
  stack + a kernel scheduling entry; a task costs bytes-to-KB and no kernel
  involvement. This is the **M:N model**: millions of tasks on a handful of
  threads.
- **Parked tasks cost 0 CPU** — they sit in a queue or wait on an `epoll` fd;
  no thread spins.
- **Cooperative switching is a function call** — return from `poll`, call the
  next `poll`. No preemption, no per-task context-switch syscall. Plus: no GC.

Honest limits and context:

- *Concurrent* millions of tasks is routine; "billions simultaneously alive" is
  memory-bound (a billion × 200 bytes ≈ 200 GB). You process billions *over
  time*, not *at once*.
- Tokio is **not** categorically "unbeatable." Go uses the same M:N model and is
  easier to write (goroutines) but has a GC and heavier per-goroutine cost.
  C++ (Seastar, ASIO) can match or beat raw throughput without memory safety.
  Erlang/BEAM wins on massive-concurrency fault isolation.
- Tokio's real edge is the **combination**: near-zero-cost async + no GC + memory
  safety + a mature ecosystem — predictable tail latency *and* you can't segfault.
  Everything fancy (work-stealing, the timer wheel, the sharding trilogy) is
  squeezing the last few percent out of that foundation.
