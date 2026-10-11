use super::*;
use mycopilot_core::storage::notification_repository::NewNotificationEventRecord;
use std::cell::Cell;
use std::sync::Arc;

fn fixture() -> (tempfile::TempDir, StorageService) {
    let directory = tempfile::tempdir().unwrap();
    let storage = StorageService::open_with_model_credentials(
        &directory.path().join("notification-response.sqlite"),
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    let now = now_ms();
    storage
        .enqueue_notification_event(&NewNotificationEventRecord {
            notification_kind: "task_completed".into(),
            source_kind: "human_root".into(),
            source_id: "synthetic-run".into(),
            run_id: Some("synthetic-run".into()),
            automation_id: None,
            conversation_id: Some("synthetic-conversation".into()),
            user_message_id: Some("synthetic-user".into()),
            assistant_message_id: Some("synthetic-assistant".into()),
            approval_action_id: None,
            subject_kind: "prompt_excerpt".into(),
            subject_text: "Synthetic notification response".into(),
            dedupe_key: "synthetic-completed".into(),
            supersession_key: "synthetic-run".into(),
            resource_revision: Some(1),
            occurred_at: now,
            expires_at: now + 3_600_000,
        })
        .unwrap();
    (directory, storage)
}

fn claim_input() -> NotificationBatchesClaimInputDto {
    NotificationBatchesClaimInputDto {
        schema_version: NOTIFICATION_SCHEMA_VERSION,
        claim_token: "response-claim".into(),
        lease_duration_ms: 60_000,
        limit: 10,
    }
}

fn seed_claim(storage: &StorageService) -> NotificationBatchRecord {
    storage
        .claim_pending_notification_batches("response-claim", 60_000, 10)
        .unwrap()
        .remove(0)
}

fn validation_input(record: &NotificationBatchRecord) -> NotificationBatchValidateInputDto {
    NotificationBatchValidateInputDto {
        schema_version: NOTIFICATION_SCHEMA_VERSION,
        batch_id: record.id.clone(),
        claim_token: "response-claim".into(),
    }
}

// The storage claim remains valid in real time. Advance only the response clock across the
// real settings/item reads to isolate this boundary from the writer's separate lease checks.
fn expires_during_response(deadline: i64) -> impl Fn() -> i64 {
    let calls = Cell::new(0);
    move || {
        let previous = calls.replace(calls.get() + 1);
        if previous == 0 {
            deadline - 1
        } else {
            deadline
        }
    }
}

#[test]
fn claim_response_omits_a_lease_expired_during_display_reads_without_extending_it() {
    let (_directory, storage) = fixture();
    let record = seed_claim(&storage);
    let response = NotificationService::new(&storage)
        .claim_with_clock(
            claim_input(),
            expires_during_response(record.claim_expires_at.unwrap()),
        )
        .unwrap();
    assert_eq!(response.claim_token, "response-claim");
    assert!(response.batches.is_empty());
    assert_eq!(
        storage
            .validate_claimed_notification_batch(&record.id, "response-claim")
            .unwrap(),
        Some(record),
        "response filtering must not renew or discard the durable delivery claim"
    );
}

#[test]
fn validation_response_omits_a_lease_expired_during_display_reads() {
    let (_directory, storage) = fixture();
    let record = seed_claim(&storage);
    let response = NotificationService::new(&storage)
        .validate_with_clock(
            validation_input(&record),
            expires_during_response(record.claim_expires_at.unwrap()),
        )
        .unwrap();
    assert_eq!(response.batch_id, record.id);
    assert!(response.batch.is_none());
    assert_eq!(
        storage
            .validate_claimed_notification_batch(&record.id, "response-claim")
            .unwrap(),
        Some(record)
    );
}

#[test]
fn response_keeps_the_exact_live_claim_and_all_display_items_before_expiry() {
    let (_directory, storage) = fixture();
    let record = seed_claim(&storage);
    let service = NotificationService::new(&storage);
    let before_deadline = || record.claim_expires_at.unwrap() - 1;
    let claimed = service
        .claim_with_clock(claim_input(), before_deadline)
        .unwrap();
    assert_eq!(claimed.batches.len(), 1);
    assert_eq!(claimed.batches[0].item_count, 1);
    assert_eq!(claimed.batches[0].items.len(), 1);
    let validated = service
        .validate_with_clock(validation_input(&record), before_deadline)
        .unwrap();
    assert_eq!(validated.batch.as_ref(), claimed.batches.first());
    assert_eq!(
        storage
            .validate_claimed_notification_batch(&record.id, "response-claim")
            .unwrap(),
        Some(record)
    );
}
