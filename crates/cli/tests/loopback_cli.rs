//! Local CLI calls must keep working on computers configured with an HTTP proxy.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

#[test]
fn local_cli_reaches_supervisor_with_a_global_proxy() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        stream.read(&mut buf).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_pockymoe"))
        .env_clear()
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .args([
            "--url",
            &url,
            "--token",
            "fixture",
            "--from",
            "00000000-0000-4000-8000-000000000003",
            "thread",
            "self",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    worker.join().unwrap();
}
