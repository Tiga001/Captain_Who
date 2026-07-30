use std::sync::{Arc, Mutex};

use mycopilot_mcp_client::{BoxMcpFuture, McpConnector, McpPeer};
use tokio::sync::{oneshot, Notify};

use super::*;

struct GatedRejectingConnector {
    entered: Mutex<Option<oneshot::Sender<()>>>,
    release: Arc<Notify>,
}

impl McpConnector for GatedRejectingConnector {
    fn connect<'a>(&'a self, _: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        let entered = self
            .entered
            .lock()
            .expect("gated connector entry lock")
            .take();
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            if let Some(entered) = entered {
                let _ = entered.send(());
            }
            release.notified().await;
            Err(McpError::spawn(
                "the gated management test connector never starts a process",
            ))
        })
    }
}

#[tokio::test]
async fn one_server_mutation_is_exclusive_across_await_but_other_servers_continue() {
    let (entered_sender, entered_receiver) = oneshot::channel();
    let release = Arc::new(Notify::new());
    let harness = TestHarness::with_connector(Arc::new(GatedRejectingConnector {
        entered: Mutex::new(Some(entered_sender)),
        release: Arc::clone(&release),
    }));
    let added = harness
        .service
        .add_server(create_input("gated mutation fixture"))
        .expect("add gated server");
    let authorization_preview = harness
        .service
        .prepare_launch_authorization(mutation_input(&added.server))
        .expect("prepare launch authorization");
    let authorized = harness
        .service
        .commit_launch_authorization(commit_input(&authorization_preview))
        .expect("authorize launch");
    let second_preview = harness
        .service
        .prepare_launch_authorization(mutation_input(&authorized.server))
        .expect("prepare a second one-shot preview before enabling");
    let enabled = harness
        .service
        .enable_server(mutation_input(&authorized.server))
        .expect("enable authorized server");

    let start_service = Arc::clone(&harness.service);
    let start_input = mutation_input(&enabled.server);
    let start = tokio::spawn(async move {
        start_service
            .start_server(start_input, McpManagementOperationDto::Start)
            .await
    });
    entered_receiver
        .await
        .expect("start reaches the gated connector");

    let update_error = harness
        .service
        .update_server(update_input(
            &enabled.server,
            "blocked rename",
            enabled.server.arguments.clone(),
        ))
        .await
        .expect_err("update cannot overlap start for the same Server");
    assert_eq!(
        failure_code(update_error),
        McpManagementErrorCodeDto::Conflict
    );
    let delete_error = harness
        .service
        .delete_server(mutation_input(&enabled.server))
        .await
        .expect_err("delete cannot overlap start for the same Server");
    assert_eq!(
        failure_code(delete_error),
        McpManagementErrorCodeDto::Conflict
    );
    let enable_error = harness
        .service
        .enable_server(mutation_input(&enabled.server))
        .expect_err("enable cannot overlap start for the same Server");
    assert_eq!(
        failure_code(enable_error),
        McpManagementErrorCodeDto::Conflict
    );
    let disable_error = harness
        .service
        .disable_server(mutation_input(&enabled.server))
        .await
        .expect_err("disable cannot overlap start for the same Server");
    assert_eq!(
        failure_code(disable_error),
        McpManagementErrorCodeDto::Conflict
    );
    for operation in [
        McpManagementOperationDto::Start,
        McpManagementOperationDto::Restart,
    ] {
        let error = harness
            .service
            .start_server(mutation_input(&enabled.server), operation)
            .await
            .expect_err("start/restart cannot overlap another start");
        assert_eq!(failure_code(error), McpManagementErrorCodeDto::Conflict);
    }
    let stop_error = harness
        .service
        .stop_server(mutation_input(&enabled.server))
        .await
        .expect_err("stop cannot overlap start for the same Server");
    assert_eq!(
        failure_code(stop_error),
        McpManagementErrorCodeDto::Conflict
    );
    let refresh_error = harness
        .service
        .refresh_catalog(mutation_input(&enabled.server))
        .await
        .expect_err("refresh cannot overlap start for the same Server");
    assert_eq!(
        failure_code(refresh_error),
        McpManagementErrorCodeDto::Conflict
    );
    let commit_error = harness
        .service
        .commit_launch_authorization(commit_input(&second_preview))
        .expect_err("authorization commit cannot overlap start for the same Server");
    assert_eq!(
        failure_code(commit_error),
        McpManagementErrorCodeDto::Conflict
    );

    let other = harness
        .service
        .add_server(create_input("independent mutation fixture"))
        .expect("add independent server");
    harness
        .service
        .stop_server(mutation_input(&other.server))
        .await
        .expect("a different Server is not blocked by the gated mutation");

    release.notify_one();
    let start_error = start
        .await
        .expect("join gated start")
        .expect_err("gated connector deliberately rejects without spawning");
    assert_eq!(
        failure_code(start_error),
        McpManagementErrorCodeDto::ServerError
    );
    harness
        .service
        .stop_server(mutation_input(&enabled.server))
        .await
        .expect("RAII admission is released after the awaited start fails");
    assert!(harness
        .service
        .active_server_mutations
        .lock()
        .expect("active mutation lock")
        .is_empty());

    harness.shutdown().await;
}

#[tokio::test]
async fn start_and_restart_require_enabled_state_before_launch_authorization() {
    let harness = TestHarness::new();
    let added = harness
        .service
        .add_server(create_input("disabled start fixture"))
        .expect("add server");
    let authorized = authorize(&harness.service, &added.server);
    assert!(!authorized.server.summary.enabled);
    assert_eq!(
        authorized.server.summary.launch_authorization_state,
        McpLaunchAuthorizationStateDto::Authorized
    );

    for operation in [
        McpManagementOperationDto::Start,
        McpManagementOperationDto::Restart,
    ] {
        let failure = harness
            .service
            .start_server(mutation_input(&authorized.server), operation)
            .await
            .expect_err("an authorized but disabled Server must be enabled first")
            .into_data();
        assert_eq!(failure.code, McpManagementErrorCodeDto::InvalidState);
        assert_eq!(failure.recovery, McpManagementRecoveryDto::FixInput);
        assert!(failure.message.contains("Enable"));
    }

    harness.shutdown().await;
}
