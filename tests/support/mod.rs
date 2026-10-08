use std::{net::TcpStream, time::Duration};

/// The listener polls for connections, but fixture request/response I/O blocks.
pub fn configure_http_stream(stream: &TcpStream) {
    // Windows accepted sockets inherit the listener's nonblocking mode.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
}
