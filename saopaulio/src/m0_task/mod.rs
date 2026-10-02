//! # M0 — `block_on` + `spawn`: task, waker, run queue
//!
//! **New concept:** what a "task" actually is, and why a `Waker` exists.
//!
//! **Status: not started.** [`crate::m1_time`] is blocked on this — a wheel
//! full of `T` cannot become a wheel full of `Waker` until there is something
//! to wake.
//!
//! ## The problem
//!
//! `Future::poll` returns `Pending` and the runtime moves on. Nothing has
//! recorded *why* it is pending or *what* would change that. So either the
//! runtime re-polls every future constantly — a spin loop with extra steps —
//! or the future tells the runtime when to come back.
//!
//! The `Waker` is that channel, and it is deliberately dumb: one method,
//! `wake()`, no arguments, no return. It does not say what happened. It says
//! only *poll me again*. Everything else — which future, what state — is the
//! future's own business. That narrowness is what lets an epoll registration,
//! a timer wheel, and an `mpsc` sender all drive the same scheduler without
//! knowing anything about each other.
//!
//! ## The idea
//!
//! A runtime is a loop:
//!
//! ```text
//! loop {
//!     while let Some(task) = run_queue.pop() {
//!         task.poll();             // its waker pushes it back here if needed
//!     }
//!     park();                      // nothing runnable — sleep until something wakes us
//! }
//! ```
//!
//! That is the whole thing. Every later milestone is a refinement of one line:
//! M1 makes `park` take a timeout, M2 makes it `epoll_wait`, M4 gives each
//! thread its own `run_queue`.
//!
//! ## What to build
//!
//! - **`Task`** — a boxed `Future<Output = ()>` plus a slot for the scheduler
//!   handle. Box it. Do not reach for a vtable yet.
//! - **A run queue** — `Rc<RefCell<VecDeque<Arc<Task>>>>` is correct for a
//!   single thread and will be thrown away at M4. That is fine.
//! - **A waker** — `wake()` pushes the task back onto the queue. Use
//!   `std::task::Wake` (the safe trait, stable since 1.51) and let `Arc`
//!   handle the refcounting. Hand-rolling `RawWakerVTable` here teaches
//!   pointer casting, not scheduling.
//! - **`block_on(fut)`** — drive one future on the current thread, parking
//!   between polls. The waker for *this* one unparks the thread instead of
//!   pushing to the queue.
//! - **`spawn(fut)`** — push onto the queue, return nothing for now.
//!
//! ## Tests that decide it
//!
//! - A future that returns `Pending` once and stores its waker, woken from
//!   another thread, gets polled again and completes.
//! - `spawn` inside a spawned task works (the queue is reachable from within
//!   a poll).
//! - A task that returns `Pending` and **drops the waker without calling it**
//!   hangs forever. Write this test, watch it hang, then delete it. Losing a
//!   wakeup is the defining bug of this entire subject and you should meet it
//!   deliberately once.
//! - A waker cloned and woken twice does not enqueue the task twice.
//!
//! ## Checkpoint
//!
//! `block_on` a future that spawns three tasks which complete in an order
//! determined by who wakes whom. No timers, no IO — just the queue.
//!
//! ## What M0 deliberately leaves out
//!
//! `spawn` returns nothing here. There is no `JoinHandle`, no output value, no
//! abort, no drop-cancellation. That is [`crate::m6_lifecycle`], and it is
//! left out because its difficulty only becomes visible once tasks are shared
//! across threads at [`crate::m4_worker`].
//!
//! ## Where tokio does this
//!
//! | | |
//! |---|---|
//! | `runtime/task/core.rs` | the single heap allocation holding future, output, and scheduler |
//! | `runtime/task/raw.rs` | the hand-rolled vtable — *why* it is hand-rolled: three types erased without boxing each separately |
//! | `runtime/task/harness.rs` | the poll loop around one task |
//! | `runtime/task/state.rs` | refcount + lifecycle bits packed into one atomic. **Read this before believing you understand M0** — it is the file whose invariants M6 is about |
//! | `runtime/task/waker.rs` | the `RawWaker` construction |
//! | `runtime/scheduler/current_thread/mod.rs` | the single-threaded version of the loop above |
//!
//! Compare sizes honestly: tokio's `runtime/task/` is ~4,900 lines. Yours
//! should be under 200. The difference is `JoinHandle`, abort, task dumps,
//! tracing, and the no-allocation vtable — not the idea.

use std::{
    cell::RefCell, collections::{HashMap, VecDeque}, future::Future, pin::Pin, sync::{
        Arc, Mutex, 
        atomic::{AtomicBool, AtomicUsize, Ordering}}, task::{Context, Poll, Wake, Waker}, thread, time::Duration,
};

use crate::m1_time::{ wheel::Wheel};
use crate::time_source::TimeSource;

pub(crate) struct Executor{
    queue: Mutex<VecDeque<Arc<Task>>>,
    executor: Mutex<Option<thread::Thread>>,
    owned: Mutex<HashMap<usize,Arc<Task>>>,// holds a strong ref to every task, and every task holds a 
                                           //strong Arc<Executor>, so keep in mind that a task that 
                                           //stays Pending forever keeps itself and the entire executor
                                           //alive forever, "silently leaked".
    next_task_id: AtomicUsize,
    pub(crate) time: Mutex<Wheel<Waker>>,
    pub(crate) time_source: TimeSource,
}
struct Task{
    future:Mutex<// lock gurad mechanism to handle data between threads.
        Option< //transform the result into an enum with two option Some or None.
            Pin< //fixate the position of the pointer, we have something on the heap, but the Task just have 
                //the pointer to it, and this is what we fixate it.
                Box< //in runtime alloc a space on the heap for the type inside it, that is dynamically sized.
                    dyn Future< //means that we don't know the concrete future type.
                        Output = () //just means that the return needs to void.
                        > + Send
                        >>>>,
    exec: Arc<Executor>,
    notified: AtomicBool,
    id: usize,// id to track the task in the scenario where it was polled but not waked, then got silently droped.
}
//here we implement the struct of our Task, a place to put a future 
//and a queue to poll it from, when runing and pushing when waking


impl Task {
    fn poll(self: Arc<Self>) {
        let _runtime_guard = RuntimeContextGurad::enter(self.exec.clone());

        self.notified.store(false, Ordering::Release);
        
        let waker = Waker::from(self.clone());
        let mut cx = Context::from_waker(&waker);

        let mut future_op = self.future.lock().unwrap();//get mutable access to the pined
                                                                                   // refference on the heap.

        let Some(future) = future_op.as_mut() /*A task panic must not be assumed 
                                    to be an isolated task failure. In your current runtime, it can terminate the executor. */
            else { return; };

        let completed = match future.as_mut()/*mutable future inside the ReffCell<Pin<Box*/.poll(&mut cx) {
            Poll::Ready(()) => {
                println!("\nTask.poll(): task finished");
                *future_op = None;
                true
            }

            Poll::Pending => {
                println!("\nTask.poll(): task returned Pending");
                false
            }
        };

        drop(future_op);

        if completed {
            self.exec.owned.lock().unwrap().remove(&self.id);
        }// if the task returned ready, then we can drop the task from the track "owned".
    }
    //poll basically receive an Arc<Task>, build a wake with it and make a context
    //then borrow as mutable to match it with the two scenary (Read and Pending)
}

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        println!("\nwake(): check notified state");
        if self.notified.swap(true,Ordering::AcqRel) {
        println!("\nwake(): stop please you are alredy wake");
            return;
        }

        println!("\nwake(): putting task back into queue");
        let arc_bump = self.clone();
        self.exec.queue.lock().unwrap().push_back(arc_bump);
        println!("\nwake(): task id {} added to queue", self.id);

        println!("\nwake(): unparking the thread: {:?}", thread::current().id());
        self
        .exec
        .executor
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .unpark();//giving the hability to unpark the thread when the time to check again come 
    }//get tthe Task and put on the queue to be polled again
}

fn run(exec: Arc<Executor>) {

    loop {
        let task = {
            exec.queue.lock().unwrap().pop_front()
        };

        match task {
            Some(task) => {
                println!("run(): Executor polling task");
                println!("!! Live owned tasks !!! {:?}", exec.owned.lock().unwrap().keys().clone());
                task.poll();
            },
            None => break,
        }
    }
}

struct  ThreadWaker{
    thread: thread::Thread,
    notified: AtomicBool,
}

impl Wake for ThreadWaker{
    fn wake(self: Arc<Self>){
        if !self.notified.swap(true, Ordering::AcqRel) {
            println!("\nThreadWaker.wake(): unparking the thread: {:?}", thread::current().id());
            self.thread.unpark();
        }
    }
}

struct RuntimeContextGurad {
    previous: Option<RuntimeContext>,
}

impl RuntimeContextGurad {
    fn enter(exec: Arc<Executor>) -> Self {
        let previous = CURRENT_RUNTIME.with(|runtime| {
            runtime
                .borrow_mut()
                .replace(RuntimeContext { exec })
        });

        Self { previous }
    }
}

impl Drop for RuntimeContextGurad {
    fn drop(&mut self){
        let previous = self.previous.take();

        CURRENT_RUNTIME.with(|runtime| {
            *runtime.borrow_mut() = previous;
        });
    }
}

/// The executor this thread is currently running inside.
///
/// `spawn` and `sleep` both need the current runtime but take no handle
/// argument, so both read it from `CURRENT_RUNTIME`, which
/// `RuntimeContextGurad::enter` sets on entry to `block_on` and to every
/// `Task::poll`.
///
/// Panics outside a runtime — the same rule tokio has. Note that `Sleep::drop`
/// must NOT call this: a destructor can run after the runtime is gone, and
/// panicking in a destructor aborts. `Sleep` holds its own `Arc<Executor>`.
pub(crate) fn current_exec() -> Arc<Executor> {
    CURRENT_RUNTIME.with(|runtime| {
        runtime
            .borrow()
            .as_ref()
            .expect("no runtime running")
            .exec
            .clone()
    })
}

#[derive(Clone)]
struct RuntimeContext {
    exec: Arc<Executor>,
}

thread_local! {
    static CURRENT_RUNTIME: RefCell<Option<RuntimeContext>> = RefCell::new(None);
}

#[derive(Clone)]
pub struct Runtime {exec: Arc<Executor>}

impl Runtime {
    pub fn new() -> Self {
        let executor = Self {
            exec:
                Arc::new(Executor {
                queue: Mutex::new(VecDeque::new()),
                executor: Mutex::new(None),
                owned: Mutex::new(HashMap::new()),
                next_task_id: AtomicUsize::new(1),
                time_source: TimeSource::new(),
                time: Mutex::new(Wheel::<Waker>::new()),
            })
        };

        executor
    }

    pub fn block_on<F: Future>(&self, future: F) -> F::Output{
        let thread = thread::current();
        println!("\nblock_on(): current thread {:?}", thread.id());
        *self.exec.executor.lock().unwrap() = Some(thread.clone());

        let _runtime_guard = RuntimeContextGurad::enter(self.exec.clone());

        let thread_waker = Arc::new(ThreadWaker{
            thread: thread.clone(),
            notified: AtomicBool::new(true),//initial poll
        });

        let waker = Waker::from(thread_waker.clone());

        let mut cx = Context::from_waker(&waker);
        let mut future_fixed_on_heap = Box::pin(future);

        let result = loop {
            let now = self.exec.time_source.now();
            let fired = self.exec.time.lock().unwrap().expire(now);

            for waker in fired{
                waker.wake();
            }
            
            if thread_waker.notified.swap(false, Ordering::AcqRel) {
                println!("\nblock_on(): thread waker notified");
                //poling the main future
                if let Poll::Ready(v) = future_fixed_on_heap.as_mut().poll(&mut cx) {break v;}
            }
            

            //running the tasks on the queue
            run(self.exec.clone());

            let now = self.exec.time_source.now();
            let next = self.exec.time.lock().unwrap().next_deadline();
println!(
    "block_on(): now={}, next_deadline={:?}",
    now,
    next
);
            match next{
                None => thread::park(),
                Some(deadline) => {
                    let wait = deadline.saturating_sub(now);
                    if wait > 0 {
                        thread::park_timeout(Duration::from_millis(wait));
                    }
                }
            }
        };

        result
    }

    pub fn spawn<F>(future: F,)
        where F: Future<Output = ()> +Send + 'static,
    {
        // the RefCell borrow ends here, before any mutex is taken
        let exec = current_exec();

        let task_id = exec.next_task_id.fetch_add(1, Ordering::Relaxed);

        let task = Arc::new(Task{
            future: Mutex::new(Some(Box::pin(future))),
            exec: exec.clone(),
            notified: AtomicBool::new(true),
            id: task_id
        });

        exec.owned.lock().unwrap().insert(task_id, task.clone());

        exec.queue.lock().unwrap().push_back(task);
        println!("\nspawn(): spauwn pushing task id {} queue", task_id);

        println!("\nspawn(): task added to queue")
    }//build a new task.future and put it on the queue

    pub fn run_until_idle(&self){
        loop {
        let task = {
            self.exec.queue.lock().unwrap().pop_front()
        };

        match task {
            Some(task) => {
                println!("run(): Executor polling task");
                println!("!! Live owned tasks !!! {:?}", self.exec.owned.lock().unwrap().keys().clone());
                task.poll();
            },
            None => break,
        }
    }
    }

}

pub fn main() {
    let executor = Runtime::new();
    executor.block_on(
        async {
            println!("\nmain()block_on()future block: main future started");

            Runtime::spawn(
            async {
                println!("\nmain()block_on()future block -> spawn()future block: task 1 started");

                Twice{poll_count:0}.await;

                println!("\nmain()block_on()future block -> spawn()future block: task one complete");
                }, 
            );

            println!("\nmain()block_on()future block: main future waiting");

            WaitOnce {started: Arc::new(AtomicBool::new(false))}.await;

            println!("\nmain()block_on()future block: main future complete");

            println!("\nmain()block_on()future block: inner runtime context test start");

            Runtime::spawn(async {
                println!("task A");

                Runtime::spawn(async {
                    println!("task B");
                });
            });

            Runtime::spawn(async {
                println!("Yield Task A: start");
                yield_now().await;
                println!("Yield Task A: resumed");
            });

            Runtime::spawn(async {
                println!("Yield Task B: start");
                yield_now().await;
                println!("Yield Task B: resumed");
            });

            Runtime::spawn(async {
                println!("Yield Task C: start");
                yield_now().await;
                println!("Yield Task C: resumed");
            });

            println!("\nmain()block_on()future block: inner runtime context test end");

        },
    );
    
    Runtime::run_until_idle(&executor);
}


struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<()>
    {
        if self.yielded{
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

async fn yield_now() {
    YieldNow {yielded: false}.await
}

struct WaitOnce {
    started: Arc<AtomicBool>,
}

impl Future for WaitOnce {
    type Output = ();

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        if self.started.load(Ordering::Acquire) {
            println!("\nWaitOnce.poll(): Ready");
            Poll::Ready(())
        } else {
            println!("\nWaitOnce.poll(): Pending");


            let waker = cx.waker().clone();
            let ready = self.started.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                ready.store(true, Ordering::Release);

                println!("\nWaitOnce.poll(): OTHER THREAD wake()");
                waker.wake();
            });

            Poll::Pending
        }
    }
}

struct Twice {
    poll_count: usize,
}

impl Future for Twice {
    type Output = ();

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        self.poll_count += 1;

        println!("\nTwice::poll() #{}", self.poll_count);

        if self.poll_count >= 2 {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }//this is like a toll, we need to pass by this two times, the first will return Pending, 
     //then the next Ready. this is a fake/semi future.
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    // ------------------------------------------------------------------
    // Fixture: the minimal "await a flag" primitive.
    //
    // Every M0 test needs one task to wait for another, and M0 has no
    // JoinHandle and no channels. `Signal` is the smallest thing that closes
    // that gap: a bool plus a waker slot. It is also, in miniature, what
    // `Notify` and `oneshot` are — which is why it belongs in tests and not in
    // the module.
    //
    // Crucially, a `Wait` polled inside a spawned task stores that TASK's
    // waker, so waking it exercises `Wake for Task`. A `Wait` awaited directly
    // in `block_on` stores the `ThreadWaker` instead. The tests below depend on
    // that difference.
    // ------------------------------------------------------------------
    struct Signal {
        done: AtomicBool,
        waker: Mutex<Option<Waker>>,
    }

    impl Signal {
        fn new() -> Arc<Self> {
            Arc::new(Signal {
                done: AtomicBool::new(false),
                waker: Mutex::new(None),
            })
        }

        fn set(&self) {
            self.done.store(true, Ordering::Release);
            if let Some(w) = self.waker.lock().unwrap().take() {
                w.wake();
            }
        }
    }

    struct Wait(Arc<Signal>);

    impl Future for Wait {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0.done.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            *self.0.waker.lock().unwrap() = Some(cx.waker().clone());
            // Re-check: `set` may have run between the load and the store, in
            // which case that wake went to a waker we just replaced.
            if self.0.done.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            Poll::Pending
        }
    }

    /// Run a runtime on its own thread and fail instead of hanging. Every bug
    /// in this subject presents as a hang, and a hung `cargo test` says nothing.
    fn with_timeout<F>(secs: u64, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            f();
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(secs))
            .expect("the runtime hung — a wakeup was lost");
    }

    // ------------------------------------------------------------------

    #[test]
    fn block_on_returns_the_futures_output() {
        let rt = Runtime::new();
        assert_eq!(rt.block_on(async { 1 + 1 }), 2);
    }

    /// The path nothing else covers: `Wake for Task` invoked from a foreign
    /// thread. `Wait` inside the spawned task holds the Task's waker, so the OS
    /// thread's `set()` goes through `Task::wake` — push to the queue, then
    /// unpark the executor. Every `Arc`/`Mutex`/`AtomicBool` in `Executor` and
    /// `Task` exists to make this sound; before the `unsafe impl Send` was
    /// removed, this was UB.
    #[test]
    fn a_spawned_task_woken_from_another_thread_completes() {
        with_timeout(5, || {
            let rt = Runtime::new();
            let from_os_thread = Signal::new();
            let task_finished = Signal::new();

            let trigger = from_os_thread.clone();
            let finished = task_finished.clone();

            rt.block_on(async move {
                Runtime::spawn(async move {
                    Wait(trigger).await; // suspends; woken across threads
                    finished.set();
                });

                let t = from_os_thread.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(50));
                    t.set();
                });

                Wait(task_finished).await;
            });
        });
    }

    /// Two wakes on one task must produce ONE queue entry.
    ///
    /// Counting polls of a task that *completes* cannot see this: the second
    /// queue entry would find `future` already taken and return without
    /// polling, so the count is the same either way. That masking is test 3's
    /// invariant, not this one.
    ///
    /// So the future here never completes, and `run_until_idle` is driven
    /// directly — one poll per queue entry, nothing absorbed. Wakes are issued
    /// only on poll #1, so every poll after that came from a queue entry:
    ///
    ///   deduped:     poll 1 (wakes x2 -> 1 entry), poll 2            => 2
    ///   not deduped: poll 1 (wakes x2 -> 2 entries), poll 2, poll 3  => 3
    #[test]
    fn two_wakes_enqueue_the_task_once() {
        struct WakeTwiceThenStall {
            polls: Arc<AtomicUsize>,
        }

        impl Future for WakeTwiceThenStall {
            type Output = ();
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                let n = self.polls.fetch_add(1, Ordering::AcqRel) + 1;
                if n == 1 {
                    let w = cx.waker().clone();
                    w.wake_by_ref();
                    w.wake_by_ref();
                }
                Poll::Pending // never completes: no `Option` guard to hide behind
            }
        }

        let polls = Arc::new(AtomicUsize::new(0));
        let counter = polls.clone();

        let rt = Runtime::new();

        // `spawn` needs CURRENT_RUNTIME, so it has to happen inside `block_on`.
        // This main future is synchronous, so `block_on` returns having queued
        // the task without ever polling it.
        rt.block_on(async move {
            Runtime::spawn(WakeTwiceThenStall { polls: counter });
        });

        rt.run_until_idle();

        assert_eq!(
            polls.load(Ordering::Acquire),
            2,
            "two wakes produced more than one queue entry — `notified` is not deduping"
        );
    }

    /// A waker that outlives its task must not resurrect it. `Task::poll` takes
    /// the future out of the `Option` on `Ready`, so a stale wake finds `None`
    /// and returns without polling.
    #[test]
    fn a_completed_task_is_not_polled_again() {
        struct StashWaker {
            stash: Arc<Mutex<Option<Waker>>>,
            polls: Arc<AtomicUsize>,
        }

        impl Future for StashWaker {
            type Output = ();
            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                self.polls.fetch_add(1, Ordering::AcqRel);
                *self.stash.lock().unwrap() = Some(cx.waker().clone());
                Poll::Ready(())
            }
        }

        let stash: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));
        let polls = Arc::new(AtomicUsize::new(0));

        let rt = Runtime::new();
        {
            let (stash, polls) = (stash.clone(), polls.clone());
            rt.block_on(async move {
                let done = Signal::new();
                let finished = done.clone();
                Runtime::spawn(async move {
                    StashWaker { stash, polls }.await;
                    finished.set();
                });
                Wait(done).await;
            });
        }

        assert_eq!(polls.load(Ordering::Acquire), 1);

        // The task is finished and off `owned`. Waking it anyway must be inert.
        stash.lock().unwrap().take().unwrap().wake();
        rt.run_until_idle();

        assert_eq!(
            polls.load(Ordering::Acquire),
            1,
            "a stale waker re-polled a completed future"
        );
    }

    /// `spawn` reads `CURRENT_RUNTIME`, which `Task::poll` enters via
    /// `RuntimeContextGurad`. So spawning works from inside a poll, with no
    /// queue argument threaded through.
    #[test]
    fn a_task_can_spawn_another_task() {
        with_timeout(5, || {
            let rt = Runtime::new();
            let inner_ran = Signal::new();
            let signal = inner_ran.clone();

            rt.block_on(async move {
                Runtime::spawn(async move {
                    Runtime::spawn(async move {
                        signal.set();
                    });
                });
                Wait(inner_ran).await;
            });
        });
    }

    /// The M0 checkpoint: three tasks whose interleaving is decided purely by
    /// the run queue. Each records itself, yields, then records itself again.
    /// FIFO ordering means all three get their first turn before any gets its
    /// second — that is the definition of the scheduler being fair.
    #[test]
    fn three_tasks_round_robin_through_the_queue() {
        with_timeout(5, || {
            let rt = Runtime::new();
            let log: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
            let done = Signal::new();
            let remaining = Arc::new(AtomicUsize::new(3));

            let outer_log = log.clone();
            let outer_done = done.clone();

            rt.block_on(async move {
                for name in ["A", "B", "C"] {
                    let log = outer_log.clone();
                    let remaining = remaining.clone();
                    let done = outer_done.clone();

                    Runtime::spawn(async move {
                        log.lock().unwrap().push(name);
                        yield_now().await;
                        log.lock().unwrap().push(name);

                        if remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                            done.set();
                        }
                    });
                }
                Wait(done).await;
            });

            assert_eq!(
                *log.lock().unwrap(),
                vec!["A", "B", "C", "A", "B", "C"],
                "tasks did not round-robin — the queue is not FIFO, or yield_now \
                 re-polls immediately instead of going back to the queue"
            );
        });
    }
}
