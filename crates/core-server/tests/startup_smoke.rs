use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn production_bootstrap_serves_skill_management_and_shuts_down_cleanly() {
    let profile = tempfile::tempdir().expect("temporary profile");
    let database = profile.path().join("storage.sqlite");
    let mut child = Command::new(env!("CARGO_BIN_EXE_core-server"))
        .env("MYCOPILOT_STORAGE_DB", &database)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn production core-server binary");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let (line_tx, line_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    send_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "core.ping",
            "params": { "message": "startup-smoke" }
        }),
    );
    let ping = receive_response(&line_rx);
    assert_eq!(ping["id"], 1);
    assert_eq!(ping["result"]["message"], "pong");
    assert_eq!(ping["result"]["echo"], "startup-smoke");

    send_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "skills.listManagement",
            "params": {}
        }),
    );
    let management = receive_response(&line_rx);
    assert_eq!(management["id"], 2);
    assert_eq!(management["result"]["schemaVersion"], 1);
    assert!(management["result"]["skills"].is_array());

    send_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "core.shutdown"
        }),
    );
    let shutdown = receive_response(&line_rx);
    assert_eq!(shutdown["id"], 3);
    assert!(shutdown["result"].is_object());
    drop(stdin);

    let status = wait_for_exit(&mut child, EXIT_TIMEOUT);
    reader.join().expect("stdout reader thread");
    let stderr = read_stderr(&mut child);
    assert!(
        status.success(),
        "core-server exited with {status}; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("Cannot drop a runtime in a context where blocking is not allowed"),
        "blocking runtime lifecycle regressed:\n{stderr}"
    );
}

fn send_request(stdin: &mut impl Write, request: Value) {
    writeln!(stdin, "{request}").expect("write JSON-RPC request");
    stdin.flush().expect("flush JSON-RPC request");
}

fn receive_response(lines: &Receiver<std::io::Result<String>>) -> Value {
    let line = lines
        .recv_timeout(RESPONSE_TIMEOUT)
        .expect("core-server response before timeout")
        .expect("read core-server response");
    serde_json::from_str(&line).expect("valid JSON-RPC response")
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> ExitStatus {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("inspect child status") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("core-server did not exit within {timeout:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_stderr(child: &mut Child) -> String {
    let mut output = String::new();
    child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut output)
        .expect("read core-server stderr");
    output
}
