
use std::{
    sync::{Mutex},
    task::Waker,
};

const READABLE: usize = 1 << 0;//?this two are binary operation? if yes why?
const WRITABLE: usize = 1 << 1;


pub struct ScheduleIo {
    state: Mutex<IoState>,
}

struct IoState {
    readiness: usize,
    reader: Option<Waker>,
    writer: Option<Waker>,
}

impl ScheduleIo {
    pub fn new() -> Self {
        Self {
            state : Mutex::new(IoState {
                readiness: 0,
                reader: None,
                writer: None,
            }),
        }
    }

    // Called by TcpStream::read()
    pub(crate) fn poll_read_ready(&self, waker: &Waker) -> bool {
        let mut state = self.state.lock().unwrap();

        if state.readiness & READABLE != 0 {
            return true;
        }

        // Socket isn't ready, remember who is waiting.
        state.reader = Some(waker.clone());

        false
    }//? this ones aren't race safe, later will need improvment, set_readable() can happen in bettwen.

    // Called by TcpStream::write()
    pub fn poll_write_ready(&self, waker: &Waker) -> bool {
        let mut state = self.state.lock().unwrap();

        if state.readiness & WRITABLE != 0 {
            return true;
        }

        state.writer = Some(waker.clone());

        false
    }

    // Called by the epoll driver.
    pub(crate) fn set_readable(&self) {
        let waker = {
            let mut state = self.state.lock().unwrap();
            state.readiness |= READABLE;
            state.reader.take()
        };

        if let Some(waker) = waker {
            waker.wake();
        }
    }

    // Called by the epoll driver.
    pub fn set_writable(&self) {
        let waker = {
            let mut state = self.state.lock().unwrap();
            state.readiness |= WRITABLE;
            state.writer.take()
        };

        if let Some(waker) = waker {
            waker.wake();
        }
    }

    // Called when read() actually gets EWOULDBLOCK.
    pub(crate) fn clear_readable(&self){
        self.state.lock().unwrap().readiness &= !READABLE;
    }

    pub fn clear_writable(&self){
        self.state.lock().unwrap().readiness &= !WRITABLE;
    }
}