//! Synthetic occupancy lookup and admission-lock contention benchmark.
//!
//! Run with: cargo run -p mycopilot-core --example benchmark_turn_occupancy
//! Uses an in-memory database with the current production schema. It measures only the
//! storage work within an admission-shaped mutex, not end-to-end Agent Turn preparation.

use mycopilot_core::storage::{conversation_trace_repository, migrations};
use mycopilot_core::{
    ConversationTurnTrace, ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
    CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use rusqlite::{params, Connection};
use std::sync::{mpsc, Mutex};
use std::time::Instant;

const SAMPLES: usize = 21;

fn main() {
    println!("In-memory SQLite; {SAMPLES} samples; one 2 KiB narration per completed Turn.");
    println!("history,lookup,p50_us,p95_us,short_admission_wait_p95_us,short_storage_phase_p95_us");
    for count in [100, 1_000, 10_000] {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        seed_conversation(&mut connection, "long", count);
        seed_conversation(&mut connection, "short", 1);
        let database = Mutex::new(connection);
        let admission = Mutex::new(());
        for lightweight in [false, true] {
            lookup(&database.lock().unwrap(), "long", lightweight);
            let mut durations = Vec::with_capacity(SAMPLES);
            let mut waits = Vec::with_capacity(SAMPLES);
            let mut short_phases = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                let started = Instant::now();
                lookup(&database.lock().unwrap(), "long", lightweight);
                durations.push(started.elapsed().as_secs_f64() * 1_000_000.0);

                // Long history owns the same admission fence before the short request arrives.
                // Admission stays held across the DB lookup in both implementations.
                std::thread::scope(|scope| {
                    let (locked, ready) = mpsc::sync_channel(0);
                    let admission = &admission;
                    let database = &database;
                    let long = scope.spawn(move || {
                        let _guard = admission.lock().unwrap();
                        locked.send(()).unwrap();
                        lookup(&database.lock().unwrap(), "long", lightweight);
                    });
                    ready.recv().unwrap();
                    let started = Instant::now();
                    let _guard = admission.lock().unwrap();
                    waits.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                    lookup(&database.lock().unwrap(), "short", lightweight);
                    short_phases.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                    long.join().unwrap();
                });
            }
            let mode = if lightweight {
                "identity"
            } else {
                "full_history"
            };
            println!(
                "{count},{mode},{:.3},{:.3},{:.3},{:.3}",
                percentile(&mut durations, 50),
                percentile(&mut durations, 95),
                percentile(&mut waits, 95),
                percentile(&mut short_phases, 95),
            );
        }
    }
}

fn lookup(connection: &Connection, conversation_id: &str, lightweight: bool) {
    let occupied = if lightweight {
        conversation_trace_repository::get_in_progress_turn_identity(connection, conversation_id)
            .unwrap()
            .is_some()
    } else {
        conversation_trace_repository::list_traces_for_conversation(connection, conversation_id)
            .unwrap()
            .into_iter()
            .any(|trace| trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress)
    };
    assert!(!occupied);
}

fn percentile(samples: &mut [f64], percentile: usize) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() * percentile).div_ceil(100) - 1).min(samples.len() - 1)]
}

fn seed_conversation(connection: &mut Connection, conversation_id: &str, count: i64) {
    connection
        .execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?1, 1, 1)",
            [conversation_id],
        )
        .unwrap();
    for index in 0..count {
        let assistant_message_id = format!("{conversation_id}-assistant-{index}");
        connection
            .execute(
                "INSERT INTO messages (id, conversation_id, role, content, created_at, position)
             VALUES (?1, ?2, 'assistant', '', ?3, ?3)",
                params![assistant_message_id, conversation_id, index],
            )
            .unwrap();
        let trace = ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: format!("{conversation_id}-run-{index}"),
            conversation_id: conversation_id.into(),
            assistant_message_id,
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![ConversationTurnTraceItem::AssistantNarration {
                sequence: 0,
                content: "synthetic trace ".repeat(128),
                provider_turn_id: None,
                first_tool_call_id: None,
                truncated: false,
            }],
        };
        conversation_trace_repository::replace_trace(connection, &trace, index, index + 1).unwrap();
    }
}
