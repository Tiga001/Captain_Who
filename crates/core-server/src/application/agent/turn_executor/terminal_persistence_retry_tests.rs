#[cfg(test)]
mod terminal_persistence_retry_tests {
    use super::*;

    #[tokio::test]
    async fn retries_restore_staged_state_and_apply_the_terminal_segment_once() {
        let attempts = Arc::new(Mutex::new(0_usize));
        let staged_segments = Arc::new(Mutex::new(0_usize));
        let persist_attempts = Arc::clone(&attempts);
        let persist_segments = Arc::clone(&staged_segments);
        let rollback_segments = Arc::clone(&staged_segments);

        let result = persist_terminal_with_bounded_retry(
            move || {
                *persist_segments
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) += 1;
                let mut attempts = persist_attempts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                *attempts += 1;
                if *attempts < 3 {
                    Err("injected terminal transaction failure".to_string())
                } else {
                    Ok("committed")
                }
            },
            move || {
                *rollback_segments
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = 0;
            },
        )
        .await
        .unwrap();

        assert_eq!(result, "committed");
        assert_eq!(
            *attempts.lock().unwrap_or_else(|error| error.into_inner()),
            3
        );
        assert_eq!(
            *staged_segments
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            1,
            "retry must not apply the same additive Usage segment more than once"
        );
    }

    #[tokio::test]
    async fn exhausted_retry_keeps_the_last_staged_terminal_state_for_reconciliation() {
        let attempts = Arc::new(Mutex::new(0_usize));
        let staged_segments = Arc::new(Mutex::new(0_usize));
        let persist_attempts = Arc::clone(&attempts);
        let persist_segments = Arc::clone(&staged_segments);
        let rollback_segments = Arc::clone(&staged_segments);

        let error = persist_terminal_with_bounded_retry(
            move || -> Result<(), String> {
                *persist_segments
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner()) += 1;
                *persist_attempts
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner()) += 1;
                Err("persistent terminal transaction failure".to_string())
            },
            move || {
                *rollback_segments
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner()) = 0;
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error, "persistent terminal transaction failure");
        assert_eq!(
            *attempts
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner()),
            TERMINAL_PERSISTENCE_RETRY_DELAYS_MS.len() + 1
        );
        assert_eq!(
            *staged_segments
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner()),
            1,
            "the final failed attempt remains represented while the durable Turn fence stays live"
        );
    }
}
