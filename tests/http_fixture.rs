mod support;

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::Duration,
};

#[test]
fn fixture_stream_waits_for_delayed_fragmented_input() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept fixture client: {error}"),
            }
        };
        // Reproduce Windows inheritance on every platform, even where accept
        // normally returns a blocking socket. No bytes exist yet.
        stream.set_nonblocking(true).unwrap();
        support::configure_http_stream(&stream);
        assert_eq!(stream.read_timeout().unwrap(), Some(Duration::from_secs(5)));
        assert_eq!(
            stream.write_timeout().unwrap(),
            Some(Duration::from_secs(5))
        );
        ready_tx.send(()).unwrap();
        let mut bytes = [0; 2];
        result_tx
            .send(stream.read_exact(&mut bytes).map(|()| bytes))
            .unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    client.write_all(b"a").unwrap();
    assert!(matches!(
        result_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    client.write_all(b"b").unwrap();
    assert_eq!(
        result_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap(),
        *b"ab"
    );
    server.join().unwrap();
}
