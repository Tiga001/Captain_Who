//! Run: cargo run --locked -p mycopilot-core --example benchmark_usage_dashboard
//!
//! Synthetic, in-memory SQLite benchmark using the production schema and repositories.
//! Includes retrieval and Rust projection, excludes IPC, UI, disk sync and other agents' locks.
//! Never opens a user database. Seeding runs through the real incremental billing triggers.

use mycopilot_core::storage::{migrations, usage_repository};
use mycopilot_core::{
    AgentUsageDashboardInput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    AgentUsageSummaryRange, AgentUsageWindow,
};
use rusqlite::{params, Connection};
use std::time::Instant;

const DAY: i64 = 86_400_000;
const END: i64 = 20_730 * DAY;
const SAMPLES: usize = 5;

fn main() {
    println!("In-memory SQLite + Rust projection; {SAMPLES} warm samples; 8 models over 30 days.");
    println!("records,windows,old_rpcs,new_rpcs,old_p50_ms,new_p50_ms,seed_ms");
    for count in [1_000, 10_000, 100_000] {
        let mut connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        let started = Instant::now();
        seed(&mut connection, count);
        let seed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        for (window_count, width) in [(7, DAY), (30, DAY), (12, 30 * DAY)] {
            let input = AgentUsageDashboardInput {
                windows: (0..window_count)
                    .map(|index| AgentUsageWindow {
                        from: END - (window_count - index) * width,
                        to: END - (window_count - index - 1) * width - 1,
                    })
                    .collect(),
            };
            let expected = legacy(&connection, &input);
            let actual = usage_repository::usage_dashboard(&connection, &input).unwrap();
            assert_summary(&expected.0, &actual.summary);
            for (expected, actual) in expected.1.iter().zip(&actual.buckets) {
                assert_summary(expected, actual);
            }
            let old_ms = median_ms(|| {
                std::hint::black_box(legacy(&connection, &input));
            });
            let new_ms = median_ms(|| {
                std::hint::black_box(
                    usage_repository::usage_dashboard(&connection, &input).unwrap(),
                );
            });
            println!(
                "{count},{window_count},{},1,{old_ms:.3},{new_ms:.3},{seed_ms:.3}",
                window_count + 1
            );
        }
    }
}

fn median_ms(mut operation: impl FnMut()) -> f64 {
    operation();
    let mut samples: Vec<_> = (0..SAMPLES)
        .map(|_| {
            let start = Instant::now();
            operation();
            start.elapsed().as_secs_f64() * 1_000.0
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    samples[SAMPLES / 2]
}

fn legacy(
    connection: &Connection,
    input: &AgentUsageDashboardInput,
) -> (AgentUsageSummaryOutput, Vec<AgentUsageSummaryOutput>) {
    let query = |from, to| {
        usage_repository::usage_summary(
            connection,
            &AgentUsageSummaryInput {
                range: AgentUsageSummaryRange::Custom,
                from: Some(from),
                to: Some(to),
            },
            END,
        )
        .unwrap()
    };
    let buckets = input
        .windows
        .iter()
        .map(|window| query(window.from, window.to))
        .collect();
    (
        query(input.windows[0].from, input.windows.last().unwrap().to),
        buckets,
    )
}

fn assert_summary(expected: &AgentUsageSummaryOutput, actual: &AgentUsageSummaryOutput) {
    // Binary-exact fixture prices make full JSON equality meaningful, including all models.
    assert_eq!(
        serde_json::to_value(expected).unwrap(),
        serde_json::to_value(actual).unwrap()
    );
}

fn seed(connection: &mut Connection, count: i64) {
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO conversations(id,title,created_at,updated_at) VALUES('bench','',1,1)",
            [],
        )
        .unwrap();
    {
        let mut message = transaction
            .prepare_cached(
                "INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
             VALUES(?1,'bench','assistant','','sent',?2,?3)",
            )
            .unwrap();
        let mut usage = transaction
            .prepare_cached(
                "INSERT INTO agent_usage_records(id,conversation_id,message_id,run_id,model_id,
                 model_name,created_at,billable_request_count,input_tokens,output_tokens,
                 total_tokens,cached_input_tokens,estimated_cost)
             VALUES(?1,'bench',?1,?1,?2,?2,?3,1,1000,100,1100,400,0.125)",
            )
            .unwrap();
        for index in 0..count {
            let id = format!("message-{index}");
            let timestamp = END - 30 * DAY + index * 30 * DAY / count;
            message.execute(params![id, timestamp, index]).unwrap();
            usage
                .execute(params![id, format!("model-{}", index % 8), timestamp])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
}
