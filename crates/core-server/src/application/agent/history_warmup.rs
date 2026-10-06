//! Best-effort preparation of stable history, without reserving a Turn or consuming mail.
use super::*;
use std::collections::VecDeque;

const MAX_PENDING_WARMUPS: usize = 64;
const INITIAL_YIELD: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct HistoryWarmupQueue {
    pending: VecDeque<String>,
    queued: HashSet<String>,
    running: bool,
}

impl HistoryWarmupQueue {
    fn enqueue(&mut self, conversation_id: &str) -> bool {
        if self.queued.insert(conversation_id.to_owned()) {
            if self.pending.len() == MAX_PENDING_WARMUPS {
                if let Some(oldest) = self.pending.pop_front() {
                    self.queued.remove(&oldest);
                }
            }
            self.pending.push_back(conversation_id.to_owned());
        }
        if self.running {
            false
        } else {
            self.running = true;
            true
        }
    }

    fn next(&mut self) -> Option<String> {
        let next = self.pending.pop_front();
        if let Some(id) = &next {
            // A later terminal commit may queue this owner once more while its old snapshot is
            // being prepared. The cache's authoritative revision check rejects that old result.
            self.queued.remove(id);
        } else {
            self.running = false;
        }
        next
    }
}

impl AgentService {
    pub(super) fn enqueue_history_warmup(&self, conversation_id: &str) {
        if conversation_id.is_empty() || self.workflow_dispatch_stopped.load(Ordering::Acquire) {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            // Synchronous callers and shutdown never create a runtime just for an optimization.
            return;
        };
        if !self
            .history_warmup
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .enqueue(conversation_id)
        {
            return;
        }
        let service = self.clone();
        runtime.spawn(async move {
            // Let immediate admissions and their persistence win before any speculative reads.
            // No sender or scheduler waits for this delay or for the preparation worker.
            tokio::time::sleep(INITIAL_YIELD).await;
            loop {
                let next = service
                    .history_warmup
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .next();
                let Some(conversation_id) = next else {
                    break;
                };
                let worker = service.clone();
                #[cfg(debug_assertions)]
                let started = Instant::now();
                let worker_conversation_id = conversation_id.clone();
                if tokio::task::spawn_blocking(move || {
                    worker.prepare_idle_history(&worker_conversation_id);
                })
                .await
                .is_err()
                {
                    // Do not include panic payloads: history and tool arguments may be sensitive.
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "历史上下文预热失败：会话={conversation_id}，原因=后台任务未正常结束，耗时={}ms",
                        started.elapsed().as_millis()
                    );
                }
                // One bounded worker drains and exits; it never retains AgentService in a
                // permanent notification loop or owns an execution-capacity permit.
                tokio::task::yield_now().await;
            }
        });
    }

    fn prepare_idle_history(&self, conversation_id: &str) {
        if self.workflow_dispatch_stopped.load(Ordering::Acquire)
            || self.is_conversation_deleting(Some(conversation_id))
            || self
                .has_conversation_turn_occupancy(conversation_id)
                .unwrap_or(true)
        {
            return;
        }
        #[cfg(debug_assertions)]
        let started = Instant::now();
        // This function is read-only and revalidates its snapshot before publishing. In
        // particular, do not call root admission, bind_input, or a model-request preview here.
        match self.prepare_cached_history_for(
            conversation_id,
            super::prepared_history::HistoryPreparationSource::IdleWarmup,
        ) {
            Ok(_) => {}
            Err(error) => {
                #[cfg(debug_assertions)]
                eprintln!(
                    "历史上下文预热失败：会话={conversation_id}，原因={}，耗时={}ms",
                    warmup_failure_reason(&error),
                    started.elapsed().as_millis()
                );
                #[cfg(not(debug_assertions))]
                let _ = error;
            }
        }
    }
}

#[cfg(any(debug_assertions, test))]
fn warmup_failure_reason(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.contains("database is locked")
        || error.contains("database is busy")
        || error.contains("sqlite_busy")
        || error.contains("sqlite_locked")
    {
        "数据库暂时繁忙"
    } else if error.contains("hash")
        || error.contains("fingerprint")
        || error.contains("digest")
        || error.contains("sha256")
    {
        "历史完整性校验失败"
    } else if error.contains("model context")
        && (error.contains("missing") || error.contains("incomplete"))
    {
        "模型上下文记录缺失或不完整"
    } else if error.contains("缺少可信的日志版本") {
        "历史日志版本记录缺失"
    } else if error.contains("decompress")
        || error.contains("decode")
        || error.contains("deserialize")
        || error.contains("zstd")
        || error.contains("invalid json")
    {
        "历史数据解码失败"
    } else {
        "历史读取或校验失败"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmup_queue_coalesces_bounds_and_keeps_a_new_commit_during_preparation() {
        let mut queue = HistoryWarmupQueue::default();
        assert!(queue.enqueue("same"));
        for _ in 0..100 {
            assert!(!queue.enqueue("same"));
        }
        assert_eq!(queue.next().as_deref(), Some("same"));
        assert!(!queue.enqueue("same"));
        assert_eq!(queue.next().as_deref(), Some("same"));
        assert_eq!(queue.next(), None);
        assert!(queue.enqueue("new-worker"));
        for i in 0..MAX_PENDING_WARMUPS * 2 {
            assert!(!queue.enqueue(&format!("conversation-{i}")));
        }
        assert_eq!(queue.pending.len(), MAX_PENDING_WARMUPS);
        assert_eq!(queue.queued.len(), MAX_PENDING_WARMUPS);
        assert_eq!(queue.next().as_deref(), Some("conversation-64"));
    }

    #[test]
    fn warmup_without_runtime_does_not_queue_or_reserve_work() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Arc::new(
            StorageService::open(&directory.path().join("history-warmup.sqlite")).unwrap(),
        );
        let service = AgentService::new_authorized_for_test(storage);
        service.enqueue_history_warmup("not-open-in-renderer");
        let queue = service.history_warmup.lock().unwrap();
        assert!(!queue.running);
        assert!(queue.pending.is_empty());
        assert!(service.active_conversation_turns.lock().unwrap().is_empty());
    }

    #[test]
    fn warmup_diagnostics_use_fixed_reasons_without_echoing_history_or_raw_errors() {
        for (error, reason) in [
            ("database is locked: private body", "数据库暂时繁忙"),
            (
                "model context item failed length or hash validation: secret",
                "历史完整性校验失败",
            ),
            (
                "assistant private-message has incomplete model context",
                "模型上下文记录缺失或不完整",
            ),
            (
                "cannot decompress model context item: private body",
                "历史数据解码失败",
            ),
            ("unknown error containing secret text", "历史读取或校验失败"),
        ] {
            assert_eq!(warmup_failure_reason(error), reason);
        }
    }
}
