use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(target_os = "macos")]
#[test]
fn unsigned_development_bootstrap_persists_credentials_without_keychain_access() {
    use std::os::unix::fs::PermissionsExt;

    let profile = tempfile::tempdir().expect("temporary profile");
    let database = profile.path().join("storage.sqlite");
    let credential_root = profile
        .path()
        .join("image-generation-development-credentials-v1");
    let mut child = spawn_core_server(&database);
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
            "method": "imageGeneration.updateConfiguration",
            "params": {
                "schemaVersion": 1,
                "expectedRevision": "image-generation:v1:0",
                "adapterId": "smartmlSeedream",
                "endpointUrl": "https://example.com/v1/images/generations",
                "modelId": "development-smoke-model",
                "capabilities": {
                    "textToImage": true,
                    "imageToImage": false
                },
                "defaults": {
                    "sizePreset": "2K",
                    "watermark": true
                },
                "credentialMutation": {
                    "type": "replace",
                    "value": "development-smoke-credential"
                }
            }
        }),
    );
    let update = receive_response(&line_rx);
    assert_eq!(update["id"], 1);
    assert_eq!(
        update["result"]["configuration"]["credentialStatus"],
        "configured"
    );

    send_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "core.shutdown"
        }),
    );
    assert_eq!(receive_response(&line_rx)["id"], 2);
    drop(stdin);
    let status = wait_for_exit(&mut child, EXIT_TIMEOUT);
    reader.join().expect("stdout reader thread");
    let stderr = read_stderr(&mut child);
    assert!(
        status.success(),
        "core-server exited with {status}; stderr:\n{stderr}"
    );

    let root_metadata = std::fs::symlink_metadata(&credential_root).expect("credential root");
    assert!(root_metadata.is_dir());
    assert_eq!(root_metadata.permissions().mode() & 0o777, 0o700);
    let entries = std::fs::read_dir(&credential_root)
        .expect("read credential root")
        .collect::<Result<Vec<_>, _>>()
        .expect("credential entries");
    assert_eq!(entries.len(), 1);
    let credential_metadata = entries[0].metadata().expect("credential metadata");
    assert!(credential_metadata.is_file());
    assert_eq!(credential_metadata.permissions().mode() & 0o777, 0o600);

    let mut restarted = spawn_core_server(&database);
    let mut restarted_stdin = restarted.stdin.take().expect("piped stdin");
    let restarted_stdout = restarted.stdout.take().expect("piped stdout");
    let (restarted_tx, restarted_rx) = mpsc::channel();
    let restarted_reader = thread::spawn(move || {
        for line in BufReader::new(restarted_stdout).lines() {
            if restarted_tx.send(line).is_err() {
                break;
            }
        }
    });
    send_request(
        &mut restarted_stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "imageGeneration.getConfiguration"
        }),
    );
    let restored = receive_response(&restarted_rx);
    assert_eq!(restored["id"], 3);
    assert_eq!(
        restored["result"]["configuration"]["credentialStatus"],
        "configured"
    );
    send_request(
        &mut restarted_stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "core.shutdown"
        }),
    );
    assert_eq!(receive_response(&restarted_rx)["id"], 4);
    drop(restarted_stdin);
    let restarted_status = wait_for_exit(&mut restarted, EXIT_TIMEOUT);
    restarted_reader.join().expect("stdout reader thread");
    let restarted_stderr = read_stderr(&mut restarted);
    assert!(
        restarted_status.success(),
        "restarted core-server exited with {restarted_status}; stderr:\n{restarted_stderr}"
    );
}

#[test]
fn production_bootstrap_serves_management_configuration_and_shuts_down_cleanly() {
    let profile = tempfile::tempdir().expect("temporary profile");
    let database = profile.path().join("storage.sqlite");
    let mut child = spawn_core_server(&database);
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

    let mut contender = Command::new(env!("CARGO_BIN_EXE_core-server"))
        .env("MYCOPILOT_STORAGE_DB", &database)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn competing core-server binary");
    let contender_status = wait_for_exit(&mut contender, EXIT_TIMEOUT);
    let contender_stderr = read_stderr(&mut contender);
    assert!(
        !contender_status.success(),
        "a competing core-server unexpectedly acquired the live database"
    );
    assert!(
        contender_stderr.contains("another core-server already owns storage"),
        "competing core-server did not report the instance lock: {contender_stderr}"
    );

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
            "method": "imageGeneration.getConfiguration"
        }),
    );
    let image_generation = receive_response(&line_rx);
    assert_eq!(image_generation["id"], 3);
    assert_eq!(image_generation["result"]["schemaVersion"], 1);
    assert_eq!(
        image_generation["result"]["configuration"]["adapterId"],
        "smartmlSeedream"
    );
    assert_eq!(
        image_generation["result"]["configuration"]["readiness"],
        "disabled"
    );
    assert!(image_generation["result"]["configuration"]
        .get("credential")
        .is_none());

    send_request(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "core.shutdown"
        }),
    );
    let shutdown = receive_response(&line_rx);
    assert_eq!(shutdown["id"], 4);
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

fn spawn_core_server(database: &std::path::Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_core-server"))
        .env("MYCOPILOT_STORAGE_DB", database)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn production core-server binary")
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
