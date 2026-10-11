use std::{
    io,
    os::fd::RawFd,
    sync::Arc,
};

pub struct WakeFd {
    fd: RawFd,
}

impl WakeFd {
    pub fn new() -> io::Result<Arc<Self>> {
        let fd = unsafe {
            libc::eventfd(
                0,
                libc::EFD_NONBLOCK | libc::EFD_CLOEXEC,
            )
        };

        if fd == -1 {
            return Err(io::Error::last_os_error());
        }

        Ok(Arc::new(Self {fd}))
    }

    pub fn raw_fd(&self) -> RawFd{
        self.fd
    }

    pub fn wake(&self) {
        let value: u64 = 1;

        let result = unsafe {
            libc::write(
                self.fd, 
                &value as *const u64 as *const libc::c_void, 
                std::mem::size_of::<u64>(),
            )
        };

        if result == -1 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                eprintln!("eventfd wake failed: {error}");
            }
        }
    }

    pub fn drain(&self) -> io::Result<()> {
        let mut value: u64 = 0;

        let result = unsafe {
            libc::read(
                self.fd,
                &mut value as *mut u64 as *mut libc::c_void,
                std::mem::size_of::<u64>()
            )
        };

        if result == -1 {
            let error = io::Error::last_os_error();

            if  error.kind() == io::ErrorKind::WouldBlock {
                return Ok(())
            }

            return Err(error)
        }

        Ok(())
    }
}

impl Drop for WakeFd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}