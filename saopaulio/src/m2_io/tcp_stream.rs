
use std::{
    io, mem, net::{Ipv4Addr, SocketAddrV4}, os::fd::RawFd, sync::Arc, thread, time::Duration,
};

use crate::m2_io::schedule_io::ScheduleIo;
pub struct TcpStream {
    pub fd: RawFd,// index to the kernel's socket object.
    io: Arc<ScheduleIo>,
}

impl TcpStream {
    pub fn connect(addr: SocketAddrV4) -> io::Result<Self> {
        unsafe/*becuse we use libc, which is a wrapper around the C standard library */ {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);

            if fd < 0 {
                return Err(io::Error::last_os_error());
            }

            // 2. Make the scket non_blocking
            let flags = libc::fcntl(fd, libc::F_GETFL);

            if flags < 0{
                libc::close(fd);
                return Err(io::Error::last_os_error());
            }

            if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                libc::close(fd);
                return Err(io::Error::last_os_error());
            }

            // 3. Convert Rust's SocketAddrV4 into the C representation
            //    expected by connect().
            let sockaddr = libc::sockaddr_in/*struct representing the AddresPv4 for 
                                                         different families need different information */ {
                sin_family: libc::AF_INET as u16,
                sin_port: addr.port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(addr.ip().octets()),
                },
                sin_zero: [0; 8],
            };

            // 4. Ask the kernel to establish the TCP connection.
            let result = libc::connect(
                fd, 
                &sockaddr as *const _ as *const libc::sockaddr,// Here is the address structure I constructed. Treat its memory
                                                                                     //as the generic sockaddr representation that connect() expects.
                mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                );
            
            if result < 0 {
                let error = io::Error::last_os_error();

                // With a non-blocking socket, connect() commonly returns
                // EINPROGRESS because the connection is not established yet.
                if error.raw_os_error() != Some(libc::EINPROGRESS) {
                    libc::close(fd);
                    return Err(error);
                }

                println!("Connection in progress...");
            } else {
                println!("Connection established immediately.");
            }

            let io = Arc::new(ScheduleIo::new());

            Ok(Self {fd,io})
        }
    }

    pub fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        unsafe {
            let result = libc::read(
                self.fd,
                buffer.as_mut_ptr()/*raw memory of the first position of the buffer*/ as *mut libc::c_void,
                buffer.len(),/*size of the buffer */
            );// try to write bytes on the memory from the raw pointer a buffer of this size.

            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(result as usize)
            }
        }
    }// kernel write on my rust buffer

    pub fn write(&self, buffer: &[u8]) -> io::Result<usize> {
        unsafe {
            let result: isize = libc::write(
                self.fd,
                buffer.as_ptr() as *const libc::c_void,
                buffer.len(),
            );//rust buffer

            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(result as usize)
            }
        }
    }// kernel read from my rust buffer
}

impl Drop for TcpStream {
    fn drop(&mut self){
        unsafe {
            libc::close(self.fd);
        }

        println!("TcpStream: closed fd {}", self.fd);
    }
}

fn start_server(){
    thread::spawn(||{
        unsafe{
            let server_fd = libc::socket(
                libc::AF_INET,
                libc::SOCK_STREAM,
                0,
            );

            assert!(server_fd >= 0);

            let addr = libc::sockaddr_in {
                sin_family: libc::AF_INET as u16,
                sin_port: 9000u16.to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(
                        Ipv4Addr::LOCALHOST.octets(),
                    ),
                },
                sin_zero: [0; 8],
            };

            // Bind socket to 127.0.0.1:9000.
            let result = libc::bind(
                server_fd,
                &addr as *const _ as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            );

            assert_eq!(
                result,
                0,
                "Failed to bind socket: {}",
                io::Error::last_os_error()
            );

            // Put socket into listening mode.
            assert_eq!(
                libc::listen(server_fd, 16),
                0,
                "Failed to listen on socket: {}",
                io::Error::last_os_error()
            );

            println!("server: listening on 127.0.0.1:9000");

            // Wait for a client to connect.
            let client_fd = libc::accept(
                server_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );

            assert!(
                client_fd >= 0,
                "Failed to accept connection: {}",
                io::Error::last_os_error()
            );

            println!("server: accepted connection");

            // Wait a little so that the client can demonstrate
            // what happens when it tries to read before data exists.
            thread::sleep(Duration::from_secs(1));

            let message = b"hello from server";

            let written = libc::write(
                client_fd,
                message.as_ptr() as *const libc::c_void,
                message.len(),
            );

            assert!(written >=0);

            println!("server: wrote {} bytes to client", written);

            libc::close(client_fd);
            libc::close(server_fd);

            println!("server: closed connection and socket");
        }
    });// this is basically raising a server in one thread and trying to write a message back, then close when done or error.
}

pub fn test_tcp_stream(){
     start_server();

    // Give the server enough time to execute socket/bind/listen.
    thread::sleep(Duration::from_millis(100));

    // ------------------------------------------------------------
    // CLIENT
    // ------------------------------------------------------------

    let address = SocketAddrV4::new(
        Ipv4Addr::LOCALHOST,
        9000,
    );

    let stream = TcpStream::connect(address).unwrap();

    println!("client: TcpStream owns fd {}", stream.fd);

    // Give connect() time to finish.
    thread::sleep(Duration::from_millis(100));

    // ------------------------------------------------------------
    // TRY TO READ BEFORE SERVER SENDS DATA
    // ------------------------------------------------------------

    let mut buffer = [0u8; 1024];

    println!("\nclient: trying read()");

    match stream.read(&mut buffer) {
        Ok(n) => {
            println!(
                "client: read {} bytes: {:?}",
                n,
                String::from_utf8_lossy(&buffer[..n])
            );
        }

        Err(error) => {
            println!(
                "client: read failed: {:?}",
                error
            );
        }
    }

    // ------------------------------------------------------------
    // WAIT FOR SERVER DATA
    // ------------------------------------------------------------

    thread::sleep(Duration::from_secs(2));

    println!("\nclient: trying read() again");

    match stream.read(&mut buffer) {
        Ok(n) => {
            println!(
                "client: read {} bytes: {:?}",
                n,
                String::from_utf8_lossy(&buffer[..n])
            );
        }

        Err(error) => {
            println!(
                "client: read failed: {:?}",
                error
            );
        }
    }

    // ------------------------------------------------------------
    // WRITE
    // ------------------------------------------------------------

    let message = b"hello from client";

    let n = stream.write(message).unwrap();

    println!("\nclient: wrote {} bytes", n);
}