//! Demo: I/O readiness vs. a timer, racing. Run with:
//!     cargo run --example rt_io_demo
//!
//! A server and a client run inside ONE runtime. The client sends a request
//! and then does:
//!
//!     timeout(deadline, socket.read(..))
//!
//! which races two futures against each other:
//!   * socket.read()  -> woken by the I/O driver (epoll) when bytes ARRIVE.
//!                        No duration is involved; it waits for readiness.
//!   * the timeout    -> woken by the time driver when `deadline` elapses.
//!
//! Whichever completes first wins:
//!   Scenario A: server replies in 100ms, client waits up to 300ms -> I/O wins.
//!   Scenario B: server replies in 500ms, client waits up to 200ms -> timer wins.

use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, timeout};

/// A toy server: accept one connection, read the request, "process" for
/// `processing` time, then reply. The processing delay stands in for however
/// long a real server takes — the client cannot know it in advance.
async fn run_server(listener: TcpListener, processing: Duration) {
    let (mut socket, _) = listener.accept().await.unwrap();

    let mut buf = [0u8; 64];
    let n = socket.read(&mut buf).await.unwrap(); // wait for the request bytes
    println!("    (server) got request: {:?}", &buf[..n]);

    sleep(processing).await; // simulate work; client has NO idea this is happening
    socket.write_all(b"PONG").await.unwrap();
    println!("    (server) sent reply after {}ms of work", processing.as_millis());
}

async fn scenario(name: &str, server_processing: u64, client_timeout: u64) {
    println!("\n=== {name}: server works {server_processing}ms, client waits ≤ {client_timeout}ms ===");

    // Bind to port 0 -> OS picks a free port; read it back so the client can connect.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(run_server(listener, Duration::from_millis(server_processing)));

    // --- client side ---
    let mut socket = TcpStream::connect(addr).await.unwrap();
    socket.write_all(b"PING").await.unwrap();

    let t0 = Instant::now();
    let mut buf = [0u8; 64];

    // The race: read-readiness (I/O driver) vs. the deadline (time driver).
    let result = timeout(Duration::from_millis(client_timeout), socket.read(&mut buf)).await;

    match result {
        Ok(Ok(n)) => println!(
            "  I/O WON  -> reply {:?} arrived after {}ms (before the {client_timeout}ms deadline)",
            &buf[..n],
            t0.elapsed().as_millis()
        ),
        Ok(Err(e)) => println!("  socket error: {e}"),
        Err(_elapsed) => println!(
            "  TIMER WON -> {}ms deadline hit first; the read was cancelled, server still working",
            client_timeout
        ),
    }

    let _ = server.await; // let the server finish (Scenario B: it replies into the void)
}

#[tokio::main]
async fn main() {
    scenario("Scenario A (fast server)", 100, 300).await;
    scenario("Scenario B (slow server)", 500, 200).await;

    println!("\nkey point: the read had NO duration. It was woken by bytes arriving");
    println!("(I/O driver / epoll), not by a clock. The only millisecond value the");
    println!("client chose was the timeout — an upper bound, raced against the read.");
}
