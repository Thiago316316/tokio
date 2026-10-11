use std::{
    future::Future,
    io::{self, Read},
    net::TcpStream,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use crate::m2_io::schedule_io::ScheduleIo;

pub struct AsyncRead<'a> {
    stream: &'a mut TcpStream,
    io: Arc<ScheduleIo>,
    buf: &'a mut [u8],
}

impl<'a> AsyncRead<'a> {
    pub fn new(
        stream: &'a mut TcpStream,
        io: Arc<ScheduleIo>,
        buf: &'a mut [u8],
    ) -> Self {
        Self {stream, io, buf}
    }
}

impl Future for AsyncRead<'_> {
    type Output = io::Result<usize>;

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Self::Output> {
        loop {
            match self.stream.read(self.buf) {
                Ok(n) => return Poll::Ready(Ok(n)),
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.io.clear_readable();

                    if self.io.poll_read_ready(cx.waker()) {
                        continue;
                    }

                    return Poll::Pending;
                }

                Err(e) => return Poll::Ready(Err(e))
            }
        }
    }
}