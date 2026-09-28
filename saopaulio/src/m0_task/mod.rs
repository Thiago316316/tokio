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
}

impl Wake for ThreadWaker{
    fn wake(self: Arc<Self>){
        self.thread.unpark();
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

        let waker = Waker::from(Arc::new(ThreadWaker{
            thread: thread.clone(),
        }));

        let mut cx = Context::from_waker(&waker);
        let mut future_fixed_on_heap = Box::pin(future);

        let result = loop {
            let now = self.exec.time_source.now();
            let fired = self.exec.time.lock().unwrap().expire(now);
            for waker in fired{
                waker.wake();
            }
            
            if let Poll::Ready(v) = future_fixed_on_heap.as_mut().poll(&mut cx) {break v;}

            run(self.exec.clone());

            let now = self.exec.time_source.now();
            let next = self.exec.time.lock().unwrap().next_deadline();

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