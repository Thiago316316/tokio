use std::{
    collections::HashMap,
    io,
    os::fd::RawFd,
    sync::Arc,
};

use super::wake_fd::WakeFd;

const WAKE_TOKEN: u64 = u64::MAX;

use super::schedule_io::ScheduleIo;

const MAX_EVENTS: usize = 128;

pub struct Driver {
    epoll_fd: RawFd,//? if fd is the index of the file descriptor index, then what is the diffrence of the RawFd?
    regsitrations: HashMap<RawFd, Arc<ScheduleIo>>,
    wake_fd: Arc<WakeFd>,
}

impl Driver {
    pub fn new() -> io::Result<Self>{
        let epoll_fd = unsafe {
            libc::epoll_create1(libc::EPOLL_CLOEXEC)
        };

        if epoll_fd == -1 {
            return Err(io::Error::last_os_error());
        }

        let wake_fd = WakeFd::new()?;

        let mut event = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: WAKE_TOKEN,
        };

        let result = unsafe {
            libc::epoll_ctl(
                epoll_fd,
                libc::EPOLL_CTL_ADD,
                wake_fd.raw_fd(),
                &mut event,
            )
        };

        if result == -1{
            return Err(io::Error::last_os_error());
        }

        Ok(Self {
            epoll_fd,
            regsitrations: HashMap::new(),
            wake_fd,
        })
    }

    pub fn register(
        &mut self,
        fd: RawFd,//? what is the difference from fd?
        io: Arc<ScheduleIo>,
        interests: u32,
    ) -> io::Result<()> {
        println!("Driver.register()");
        let mut event = libc::epoll_event {
            events: interests,
            u64: fd as u64,
        };

        let result = unsafe {
            libc::epoll_ctl(
                self.epoll_fd,
                libc::EPOLL_CTL_ADD,
                fd,
                &mut event,
            )
        };

        if result == -1 {
            return Err(io::Error::last_os_error());
        }

        self.regsitrations.insert(fd, io);
        Ok(())
    }

    pub fn wait(&self, timeout_ms: i32) -> io::Result<usize> {
        println!("Driver.wait()");
        let mut events = vec![
            libc::epoll_event {events: 0, u64: 0};
            MAX_EVENTS
        ];

        let count = unsafe {
            libc::epoll_wait(
                self.epoll_fd,
                events.as_mut_ptr(),
                MAX_EVENTS as i32,
                timeout_ms,
            )
        };

        if count == -1 {
            let error = io::Error::last_os_error();

            if error.kind() == io::ErrorKind::Interrupted{
                return Ok(0);
            }

            return Err(error)
        }

        for event in events.iter().take(count as usize) {
            if event.u64 == WAKE_TOKEN {
                self.wake_fd.drain()?;
                continue;
            }

            let fd = event.u64 as RawFd;

            let Some(io) = self.regsitrations.get(&fd) else {
                continue
            };

            let flags = event.events;

            if flags 
                & (
                    libc::EPOLLIN |
                    libc::EPOLLRDHUP |
                    libc::EPOLLHUP |
                    libc::EPOLLERR
                ) as u32 != 0
                {
                    io.set_readable();
                }

            if flags
                & (
                    libc::EPOLLOUT |
                    libc::EPOLLHUP |
                    libc::EPOLLERR
                ) as u32 != 0
                {
                    io.set_writable();
                }
        }
        Ok(count as usize)
    }
}

impl Driver{
    pub fn park(&self, timeout_ms: i32) -> io::Result<usize> {
        println!("Driver.park()");
        self.wait(timeout_ms)
    }

    pub fn wake_handle(&self) -> Arc<WakeFd> {
        Arc::clone(&self.wake_fd)
    }
}

impl Drop for Driver {
    fn drop (&mut self) {
        println!("Driver.drop(): closing epoll fd");
        unsafe {
            libc::close(self.epoll_fd);
        }
    }
}