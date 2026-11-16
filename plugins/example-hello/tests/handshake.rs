//! A plugin must answer while its stdin is still open.
//!
//! That is the whole contract of a plugin process: the host writes one
//! `initialize` line and waits on stdout, with stdin open for the rest of the
//! session. A run loop that only flushes replies at EOF passes every unit test in
//! the SDK and still makes the host wait ten seconds per plugin and then report
//! `the initialize handshake failed: timed out after 10s` - which is exactly what
//! the e2e smoke job saw, and what `example-hello` did before this test existed.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn the_plugin_answers_initialize_while_stdin_stays_open() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_example-hello"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the example plugin");

    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocol":"1"}}"#;
    writeln!(stdin, "{request}").expect("write the initialize request");
    stdin.flush().expect("flush the initialize request");

    // Read on a thread so the test can time out instead of hanging the suite; the
    // host's own budget is 10s, and a plugin that cannot answer in five is broken,
    // not slow.
    let (lines, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while reader
            .read_line(&mut line)
            .map(|read| read > 0)
            .unwrap_or(false)
        {
            let _ = lines.send(line.clone());
            line.clear();
        }
    });
    let reply = received
        .recv_timeout(Duration::from_secs(5))
        .expect("the plugin never answered `initialize` while its stdin was open");
    assert!(
        reply.contains(r#""id":1"#),
        "reply for the wrong request: {reply}"
    );
    assert!(
        reply.contains("\"result\""),
        "the handshake must answer with a manifest, not an error: {reply}"
    );
    assert!(reply.contains("example-hello"), "{reply}");

    // stdin stays open through the handshake above; closing it must end the
    // process (the host relies on that when it stops a plugin).
    drop(stdin);
    // Wait with a deadline, not `child.wait()`. A plugin that ignores EOF hangs
    // here for ever, which is how this failed once in CI: the job sat until its
    // 30-minute timeout and every other target in the run looked hung too. The
    // cause was a `Sender` clone parked in the SDK's global host client that the
    // shutdown never dropped, so the writer thread's channel never closed.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().expect("poll the plugin") {
            Some(status) => {
                assert!(status.success(), "the plugin exited with {status}");
                break;
            }
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                panic!(
                    "the plugin did not exit within 10s of its stdin closing: it hung on \
                     shutdown instead of ending with EOF"
                );
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}
