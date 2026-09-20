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
    cell::RefCell,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
    time::Duration,
};

struct Task{
    future:RefCell<//at run time provides interior mutability, in the Arc scenary, we have alaways have a refference 
                   //not the future, so we use this have mutable access to the future.
        Pin< //fixate the position of the pointer, we have something on the heap, but the Task just have 
             //the pointer to it, and this is what we fixate it.
            Box< //in runtime alloc a space on the heap for the type inside it, that is dynamically sized.
                dyn Future< //means that we don't know the concrete future type.
                    Output = () //just means that the return needs to void.
                    >>>>,
    queue: std::rc::Rc<RefCell<VecDeque<Arc<Task>>>>,
    executor: thread::Thread,
}
//here we implement the struct of our Task, a place to put a future 
//and a queue to poll it from, when runing and pushing when waking

unsafe impl Send for Task{} 
unsafe impl Sync for Task{} 
///! wrong, need to solve later

impl Task {
    fn poll(self: Arc<Self>) {
        let waker = Waker::from(self.clone());
        let mut cx = Context::from_waker(&waker);

        let mut future = self.future.borrow_mut();//get mutable access to the pined
                                                                                              // refference on the heap.

        match future.as_mut()/*mutable future inside the ReffCell<Pin<Box*/.poll(&mut cx) {
            Poll::Ready(()) => {
                println!("task finished");
            }

            Poll::Pending => {
                println!("task returned Pending");
            }
        }
    }
    //poll basically receive an Arc<Task>, build a wake with it and make a context
    //then borrow as mutable to match it with the two scenary (Read and Pending)
}

impl Wake for Task {
    fn wake(self: Arc<Self>) {
        println!("WAKER: putting task back into queue");
        let arc_bump = self.clone();
        self.queue.borrow_mut().push_back(arc_bump);

        self.executor.unpark();//giving the hability to unpark the thread when the time to check again come 
    }//get tthe Task and put on the queue to be polled again
}

fn spawn<F>(
    future: F, 
    queue: &std::rc::Rc<RefCell<VecDeque<Arc<Task>>>>,
    executor: thread::Thread,)
    where F: Future<Output = ()> + 'static,
{
    let task = Arc::new(Task {
        future: RefCell::new(Box::pin(future)),
        queue: queue.clone(),   //thhe clone, just clone the pointer, not the value, 
                                //so the queue is only one, tehre aren't different clones
                                //with different quantities of the same queue
        executor,

    }); //this ins't makeing other, separate, Task, becuse its an Arc there is 
        //only one value with referencing bumping, the only new thing is the future
        //even queue is only cloned, which in Arc is just a ref bump

    queue.borrow_mut().push_back(task);

    println!("spawn: task added to queue")
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

        println!("Twice::poll() #{}", self.poll_count);

        if self.poll_count >= 2 {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();///! can push to queue two times, need to solve later
            Poll::Pending
        }
    }//this is like a toll, we need to pass by this two times, the first will return Pending, 
     //then the next Ready. this is a fake/semi future.
}

fn run(queue: std::rc::Rc<RefCell<VecDeque<Arc<Task>>>>) {

    let executor = thread::current();//? where i will use this?

    loop{
        let task = queue.borrow_mut().pop_front();

        match task {
            Some(task) => {
                println!("EXECUTOR: polling task");

                task.poll()
            }

            None => {
                println!("EXECUTOR: queue empty");
                thread::park();
            }
        }
    }
}//get a queue as mutable, pop the first task, see if the task return something, then pull it.

struct  ThreadWaker{
    thread: thread::Thread,
}

impl Wake for ThreadWaker{
    fn wake(self: Arc<Self>){
        self.thread.unpark();
    }
}

fn block_on<F>(future: F) -> F::Output
where F: Future,
{
    let thread = thread::current();

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
            Poll::Pending => {thread::park();}
        }//we basically loop eternally and park the thread until the future returns Ready
    }
}

struct  WakeOnce{
    waker: Option<Waker>,
    started: bool,
    done: bool,
}
impl Future for WakeOnce {
    type Output = ();

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        if self.done {
            println!("Future: Ready");
            return Poll::Ready(());
        }

        if !self.started {
            println!("Future: first poll -> Pending");

            self.started = true;
            self.waker = Some(cx.waker()//this isn't the wake function of Task, 
                                              //Context has its own wake that return the waker
                                              //that the executor gave to this particular poll()
            .clone());

            let waker = self.waker.as_ref().unwrap().clone();

            thread::spawn(move || {
                println!("Other thread {:?}: sleeping...", thread::current().id());
                thread::sleep(Duration::from_secs(1));

                println!("Other thread {:?}: wake()",thread::current().id());
                waker.wake();
            });
        }

        self.done = true;

        Poll::Pending
    }
}

struct WaitOnce {
    started: bool,
}

impl Future for WaitOnce {
    type Output = ();

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        if self.started {
            println!("WaitOnce: Ready");
            Poll::Ready(())
        } else {
            println!("WaitOnce: Pending");

            self.started = true;

            let waker = cx.waker().clone();

            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));

                println!("OTHER THREAD: wake()");
                waker.wake();
            });

            Poll::Pending
        }
    }
}

pub fn main() {
    let queue = std::rc::Rc::new(
        RefCell::new(VecDeque::new())
    );

    let executor = thread::current();

    spawn(
        async {
            println!("task started");

            Twice {poll_count: 0}.await;

            println!("task complete");
        },
        &queue,
        executor.clone(),
        );

    block_on(WaitOnce{started: false});
        
    run(queue);
}


mod tests {
    use crate::m0_task::block_on;

    #[test]
    fn block_on_waits_until_woken() {
        let result = block_on(
            async{ 42 }
        );
        assert_eq!(result, 42);
    } 

}