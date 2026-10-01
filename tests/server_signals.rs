//! Ctrl-C handling of the `md-render` binary while a render is in progress (LIVE-11).
//!
//! Runs the real binary on a document that takes seconds to render in a debug build, asks
//! for it, and interrupts the server while the render runs: the process must exit promptly
//! (renders run on detached threads, which do not hold up the exit), and a second Ctrl-C
//! must force the exit at once instead of being swallowed.

#![cfg(unix)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A document that takes the offline renderer a while (code blocks, tables, emoji, math).
fn big_document() -> String {
    let mut s = String::from("# Big\n\n");
    for i in 0..2500 {
        s.push_str(&format!(
            "## H{i}\n\nSome *text* {i} with `code` and a [link](x{i}.md) :smile:.\n\n\
             ```python\ndef f{i}(x):\n    return x*{i}  # comment\n```\n\n\
             | a | b |\n|---|---|\n| {i} | $x^{i}$ |\n\n"
        ));
    }
    s
}

/// Start `md-render` on `dir/big.md` (port 0) and return the child and its port.
fn start(dir: &std::path::Path) -> (Child, u16) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_md-render"))
        .arg("--styles")
        .arg(dir.join("no-bundle.tar.gz"))
        .arg(dir.join("big.md"))
        .arg("127.0.0.1:0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start md-render");
    let mut stderr = BufReader::new(child.stderr.take().expect("stderr"));
    let started = Instant::now();
    let mut line = String::new();
    let port = loop {
        line.clear();
        if stderr.read_line(&mut line).unwrap_or(0) == 0 {
            let _ = child.kill();
            panic!("md-render exited before listening");
        }
        if let Some(rest) = line.split("Running on http://127.0.0.1:").nth(1) {
            break rest
                .split('/')
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .expect("port in the banner");
        }
        assert!(started.elapsed() < Duration::from_secs(30), "no banner");
    };
    // Keep draining stderr so the child never blocks on a full pipe.
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = stderr.read_to_end(&mut sink);
    });
    (child, port)
}

/// Request `/` without waiting for the response (it waits for the render).
fn request_page(port: u16) -> TcpStream {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(
        s,
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .expect("send request");
    s
}

fn interrupt(child: &Child) {
    let ok = Command::new("kill")
        .arg("-INT")
        .arg(child.id().to_string())
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "kill -INT failed");
}

/// Wait for the child to exit; the elapsed time, or None after `limit` (the child is killed).
fn wait_exit(child: &mut Child, limit: Duration) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < limit {
        if child.try_wait().expect("try_wait").is_some() {
            return Some(started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

#[test]
fn a_second_ctrl_c_exits_immediately() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.md"), big_document()).unwrap();
    let (mut child, port) = start(dir.path());
    let _pending = request_page(port);
    std::thread::sleep(Duration::from_millis(300)); // the render is running now
    interrupt(&child);
    std::thread::sleep(Duration::from_millis(300));
    interrupt(&child);
    let took = wait_exit(&mut child, Duration::from_secs(20));
    let took = took.expect("md-render did not exit within 20 s of the second Ctrl-C");
    assert!(took < Duration::from_secs(2), "exit took {took:?}");
}

#[test]
fn one_ctrl_c_does_not_wait_for_the_render() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("big.md"), big_document()).unwrap();
    let (mut child, port) = start(dir.path());
    let _pending = request_page(port);
    std::thread::sleep(Duration::from_millis(300));
    interrupt(&child);
    // At most the connection grace period (5 s) for the page load that waits for the
    // render, not the rest of the render itself.
    let took = wait_exit(&mut child, Duration::from_secs(30));
    let took = took.expect("md-render did not exit within 30 s of Ctrl-C");
    assert!(took < Duration::from_secs(7), "exit took {took:?}");
}
