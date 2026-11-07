//! Process-lifetime regression for terminal shutdown during real server bootstrap.

use serde_json::Value;
use std::time::{Duration, Instant};

struct ProcessGroupGuard(std::cell::Cell<i32>);

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        let pid = self.0.get();
        if pid > 0 {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

#[tokio::test]
async fn sigterm_exits_after_a_sighup_ignoring_terminal_is_killed() {
    let root = std::env::temp_dir().join(format!(
        "kc-server-bootstrap-terminal-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let script = root.join("ignore-hup.sh");
    let ready = root.join("shell.ready");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ntrap '' HUP\nprintf 'ready\\n' > '{}'\nwhile :; do /bin/sleep 1; done\n",
            ready.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = root.join("config.toml");
    std::fs::write(
        &config,
        format!("[terminals]\nshell = '{}'\n", script.display()),
    )
    .unwrap();
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let base = format!("http://127.0.0.1:{port}");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_karucode"));
    command
        .arg("--workspace")
        .arg(&workspace)
        .arg("--data-dir")
        .arg(root.join("data"))
        .arg("--config")
        .arg(&config)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--no-open")
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("KARUCODE_LOG", "error")
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut server = command.spawn().expect("start real kc-server");
    let server_pid = server.id().unwrap() as i32;
    let shell_guard = ProcessGroupGuard(std::cell::Cell::new(0));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Ok(response) = client.get(format!("{base}/api/health")).send().await {
            if response.status().is_success() {
                break;
            }
        }
        if let Some(status) = server.try_wait().unwrap() {
            panic!("server exited before health became ready: {status}");
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "server did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let created: Value = client
        .post(format!("{base}/api/terminals"))
        .header("x-karucode-client", "web")
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let shell_pid = created["terminal"]["pid"].as_i64().unwrap() as i32;
    assert!(shell_pid > 0, "{created}");
    shell_guard.0.set(shell_pid);
    for _ in 0..200 {
        if ready.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        ready.exists(),
        "shell never installed its SIGHUP disposition"
    );
    assert_eq!(unsafe { libc::kill(-shell_pid, 0) }, 0);

    // No synthetic Drop-only check: terminate the actual server process exactly as
    // a service manager would, and inspect the process group only after its exit.
    assert_eq!(unsafe { libc::kill(server_pid, libc::SIGTERM) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(8), server.wait())
        .await
        .expect("server failed to finish terminal cleanup")
        .unwrap();
    assert!(status.success(), "server SIGTERM exit was {status}");
    assert_ne!(
        unsafe { libc::kill(-shell_pid, 0) },
        0,
        "SIGHUP-ignoring shell survived the server process"
    );
    shell_guard.0.set(0);
    std::fs::remove_dir_all(root).ok();
}
