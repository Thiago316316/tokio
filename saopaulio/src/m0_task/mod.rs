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
    cell::RefCell, collections::VecDeque, future::Future, pin::Pin, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, task::{Context, Poll, Wake, Waker}, thread, time::Duration,
};

struct Executor{
    queue: Mutex<VecDeque<Arc<Task>>>,
    executor: Mutex<Option<thread::Thread>>,
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
}
//here we implement the struct of our Task, a place to put a future 
//and a queue to poll it from, when runing and pushing when waking


impl Task {
    fn poll(self: Arc<Self>) {
        self.notified.store(false, Ordering::Release);
        
        let waker = Waker::from(self.clone());
        let mut cx = Context::from_waker(&waker);

        let mut future_op = self.future.lock().unwrap();//get mutable access to the pined
                                                                                   // refference on the heap.

        let Some(future) = future_op.as_mut() 
            else { return; };

        match future.as_mut()/*mutable future inside the ReffCell<Pin<Box*/.poll(&mut cx) {
            Poll::Ready(()) => {
                println!("\nTask.poll(): task finished");
                *future_op = None;
            }

            Poll::Pending => {
                println!("\nTask.poll(): task returned Pending");
            }
        }
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

fn spawn<F>(future: F,)
    where F: Future<Output = ()> +Send + 'static,
{
    CURRENT_RUNTIME.with(|runtime| {
        let runtime = runtime.borrow();
        let runtime = runtime.as_ref().expect("no runtime running");

        let task = Arc::new(Task{
            future: Mutex::new(Some(Box::pin(future))),
            exec: runtime.exec.clone(),
            notified: AtomicBool::new(false),
        });

        runtime.exec.queue.lock().unwrap().push_back(task);
    });

    println!("\nspawn(): task added to queue")
}//build a new task.future and put it on the queue

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

fn run(exec: Arc<Executor>) {

    loop {
        let task = {
            exec.queue.lock().unwrap().pop_front()
        };

        match task {
            Some(task) => {
                println!("run(): Executor polling task");
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

fn block_on<F>(future: F, exec: Arc<Executor>) -> F::Output
where F: Future,
{
    let thread = thread::current();
    println!("\nblock_on(): current thread {:?}", thread.id());
    *exec.executor.lock().unwrap() = Some(thread.clone());
    

    CURRENT_RUNTIME.with(|runtime|{
        *runtime.borrow_mut() = Some(RuntimeContext { 
            exec: exec.clone(), 
        });
    });

    let waker = Waker::from(Arc::new(ThreadWaker{
        thread: thread.clone(),//cloning the rust thread handler, not the thread per se
                               //and we clone becuse block_on will be runing with one, 
                               //so it need to have its own
    }));

    let mut cx = Context::from_waker(&waker);
    let mut future_fixed_on_heap = Box::pin(future);

    loop {
        match future_fixed_on_heap.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => { println!("\nblock_on(): main future Pending") }
        }
        run(exec.clone());

        println!("\nblock_on(): Executor parking");
        thread::park();
    }
}

struct WaitOnce {
    started: Arc<AtomicBool>,
}

impl Future for WaitOnce {
    type Output = ();

    fn poll(
        mut self: Pin<&mut Self>,
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

#[derive(Clone)]
struct RuntimeContext {
    exec: Arc<Executor>,
}

thread_local! {
    static CURRENT_RUNTIME: RefCell<Option<RuntimeContext>> = RefCell::new(None);
}

pub fn main() {
    let executor = Arc::new(Executor {
        queue: Mutex::new(VecDeque::new()),
        executor: Mutex::new(None),
    });

    block_on(
        async {
            println!("\nmain()block_on()future block: main future started");

            spawn(
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

            spawn(async {
                println!("task A");

                spawn(async {
                    println!("task B");
                });
            });

            println!("\nmain()block_on()future block: inner runtime context test end");

        },
        executor.clone(),
    );
    
    
    run(executor.clone());
}
