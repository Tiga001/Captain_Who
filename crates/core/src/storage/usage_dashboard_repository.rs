//! Rebuildable hourly billing projection plus exact boundary-hour reads.
//! Reads own their snapshot; the potentially larger Rust projection runs after releasing SQLite.
use crate::{
    AgentUsageDashboardInput, AgentUsageDashboardOutput, AgentUsageModelSummary,
    AgentUsageSummaryOutput, AgentUsageWindow,
};
use rusqlite::{params, Connection, Row};
use std::collections::{BTreeMap, BTreeSet};

const HOUR_MS: i64 = 3_600_000;
const MAX_SPAN_MS: i64 = 370 * 86_400_000;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
const HOUR_SQL: &str = "(created_at - ((created_at % 3600000 + 3600000) % 3600000))";
const TOKEN_COLUMNS: [&str; 6] = [
    "input_tokens",
    "output_tokens",
    "output_thinking_tokens",
    "total_tokens",
    "cached_input_tokens",
    "cache_creation_input_tokens",
];

pub(crate) fn validate(input: &AgentUsageDashboardInput) -> Result<(), String> {
    if !(1..=31).contains(&input.windows.len()) {
        return Err("Usage dashboard requires between 1 and 31 windows".into());
    }
    for (index, window) in input.windows.iter().enumerate() {
        if window.from < 0
            || window.to < window.from
            || window.to > MAX_SAFE_INTEGER
            || (index > 0 && input.windows[index - 1].to.checked_add(1) != Some(window.from))
        {
            return Err(
                "Usage dashboard windows must be contiguous ordered safe timestamps".into(),
            );
        }
    }
    if input.windows.last().unwrap().to - input.windows[0].from + 1 > MAX_SPAN_MS {
        return Err("Usage dashboard range cannot exceed 370 days".into());
    }
    Ok(())
}

#[derive(Clone, Default)]
struct Totals {
    counts: [i64; 3],
    tokens: [Option<i64>; 6],
    cost: Option<f64>,
}

struct Contribution {
    observed_at: i64,
    model_id: String,
    model_name: String,
    totals: Totals,
}

pub(crate) struct Snapshot {
    rows: Vec<Contribution>,
    configured_names: BTreeMap<String, Option<String>>,
}

fn decode(row: &Row<'_>) -> rusqlite::Result<Contribution> {
    Ok(Contribution {
        observed_at: row.get(0)?,
        model_id: row.get(1)?,
        model_name: row.get(2)?,
        totals: Totals {
            counts: [row.get(3)?, row.get(4)?, row.get(5)?],
            tokens: [
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
            ],
            cost: row.get(12)?,
        },
    })
}

/// One read transaction covers buckets, labels and the independently computed total.
/// Every partial/overflow hour is fetched once even if several windows cut through it.
pub(crate) fn read_snapshot(
    connection: &Connection,
    input: &AgentUsageDashboardInput,
) -> Result<Snapshot, String> {
    validate(input)?;
    let _timing = crate::performance::Span::new("usage.dashboard", "read");
    let from = input.windows[0].from;
    let to = input.windows.last().unwrap().to;
    let from_hour = from.div_euclid(HOUR_MS) * HOUR_MS;
    let to_hour = to.div_euclid(HOUR_MS) * HOUR_MS;
    // Repository callers may already own a write transaction. Its snapshot is sufficient;
    // otherwise establish one read transaction for every source and the current model labels.
    let transaction = connection
        .is_autocommit()
        .then(|| connection.unchecked_transaction())
        .transpose()
        .map_err(super::storage_error)?;
    let reader: &Connection = transaction.as_deref().unwrap_or(connection);
    let mut raw_hours = BTreeSet::new();
    for window in &input.windows {
        if window.from % HOUR_MS != 0 {
            raw_hours.insert(window.from.div_euclid(HOUR_MS) * HOUR_MS);
        }
        if window.to % HOUR_MS != HOUR_MS - 1 {
            raw_hours.insert(window.to.div_euclid(HOUR_MS) * HOUR_MS);
        }
    }
    {
        let mut query = reader
            .prepare(
                "SELECT DISTINCT usage_hour FROM agent_usage_hourly_rollups
             WHERE needs_raw = 1 AND usage_hour BETWEEN ?1 AND ?2",
            )
            .map_err(super::storage_error)?;
        for hour in query
            .query_map(params![from_hour, to_hour], |row| row.get::<_, i64>(0))
            .map_err(super::storage_error)?
        {
            raw_hours.insert(hour.map_err(super::storage_error)?);
        }
    }
    let raw_hours = serde_json::to_string(&raw_hours).map_err(|error| error.to_string())?;
    let nullable = TOKEN_COLUMNS
        .iter()
        .chain(std::iter::once(&"estimated_cost"))
        .map(|column| format!("CASE WHEN {column}_count > 0 THEN {column}_sum ELSE NULL END"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut rows = {
        let mut query = reader
            .prepare(&format!(
                "SELECT last_observed_at, model_id, model_name, request_count, message_count,
                    unpriced_message_count, {nullable}
             FROM agent_usage_hourly_rollups
             WHERE usage_hour BETWEEN ?1 AND ?2 AND needs_raw = 0
               AND usage_hour NOT IN (SELECT value FROM json_each(?3))"
            ))
            .map_err(super::storage_error)?;
        let owned = query
            .query_map(params![from_hour, to_hour, raw_hours], decode)
            .map_err(super::storage_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(super::storage_error)?;
        owned
    };
    if raw_hours != "[]" {
        let tokens = TOKEN_COLUMNS.join(", ");
        let mut query = reader.prepare(&format!(
            "SELECT created_at, model_id, model_name, billable_request_count, 1,
                    CASE WHEN estimated_cost IS NULL AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL) THEN 1 ELSE 0 END,
                    {tokens}, estimated_cost
             FROM agent_usage_records
             WHERE {HOUR_SQL} IN (SELECT value FROM json_each(?1))
               AND created_at BETWEEN ?2 AND ?3
             UNION ALL
             SELECT created_at, model_id, model_name, billable_request_count, 0, 0,
                    {tokens}, estimated_cost
             FROM manual_context_compaction_usage_records
             WHERE cleared_at IS NULL AND {HOUR_SQL} IN (SELECT value FROM json_each(?1))
               AND created_at BETWEEN ?2 AND ?3"
        )).map_err(super::storage_error)?;
        rows.extend(
            query
                .query_map(params![raw_hours, from, to], decode)
                .map_err(super::storage_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(super::storage_error)?,
        );
    }
    {
        // Existing deleted history is anchored to a UTC day. Never expand a window to include
        // an anchor before its exact start, even when the UI uses another calendar timezone.
        let mut query = reader
            .prepare(
                "SELECT usage_day, model_id, model_name, request_count, message_count,
                    unpriced_message_count, input_tokens, output_tokens, output_thinking_tokens,
                    total_tokens, cached_input_tokens, cache_creation_input_tokens, estimated_cost
             FROM agent_deleted_usage_daily_rollups WHERE usage_day BETWEEN ?1 AND ?2",
            )
            .map_err(super::storage_error)?;
        rows.extend(
            query
                .query_map(params![from, to], decode)
                .map_err(super::storage_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(super::storage_error)?,
        );
    }
    let configured_names = {
        let mut query = reader
            .prepare("SELECT id, display_name FROM models")
            .map_err(super::storage_error)?;
        let owned = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(super::storage_error)?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()
            .map_err(super::storage_error)?;
        owned
    };
    if let Some(transaction) = transaction {
        transaction.commit().map_err(super::storage_error)?;
    }
    Ok(Snapshot {
        rows,
        configured_names,
    })
}

impl Totals {
    fn add(&mut self, other: &Self) -> Result<(), String> {
        for (sum, next) in self.counts.iter_mut().zip(other.counts) {
            *sum = sum
                .checked_add(next)
                .ok_or("Usage aggregate integer overflow")?;
        }
        for (sum, next) in self.tokens.iter_mut().zip(other.tokens) {
            if let Some(next) = next {
                *sum = Some(
                    sum.unwrap_or(0)
                        .checked_add(next)
                        .ok_or("Usage aggregate integer overflow")?,
                );
            }
        }
        if let Some(cost) = other.cost {
            self.cost = Some(self.cost.unwrap_or(0.0) + cost);
        }
        Ok(())
    }

    fn model(&self, id: &str, name: String, configured: bool) -> AgentUsageModelSummary {
        let tokens = self
            .tokens
            .map(|value| value.map(|value| value.max(0) as u64));
        AgentUsageModelSummary {
            model_id: id.into(),
            model_name: name,
            is_configured: configured,
            request_count: self.counts[0].max(0) as u64,
            message_count: self.counts[1].max(0) as u64,
            unpriced_message_count: self.counts[2].max(0) as u64,
            input_tokens: tokens[0],
            output_tokens: tokens[1],
            output_thinking_tokens: tokens[2],
            total_tokens: tokens[3],
            cached_input_tokens: tokens[4],
            cache_creation_input_tokens: tokens[5],
            estimated_cost: self.cost,
        }
    }
}

#[derive(Default)]
struct Aggregate {
    totals: Totals,
    models: BTreeMap<String, (Totals, i64, String)>,
}

impl Aggregate {
    fn add(&mut self, row: &Contribution) -> Result<(), String> {
        self.totals.add(&row.totals)?;
        let model = self
            .models
            .entry(row.model_id.clone())
            .or_insert_with(|| (Totals::default(), row.observed_at, row.model_name.clone()));
        model.0.add(&row.totals)?;
        if (row.observed_at, row.model_name.as_str()) > (model.1, model.2.as_str()) {
            model.1 = row.observed_at;
            model.2.clone_from(&row.model_name);
        }
        Ok(())
    }

    fn finish(self, names: &BTreeMap<String, Option<String>>) -> AgentUsageSummaryOutput {
        let mut models: Vec<_> = self
            .models
            .into_iter()
            .map(|(id, (totals, _, historical))| {
                let name = names
                    .get(&id)
                    .and_then(Option::as_deref)
                    .filter(|name| !name.is_empty())
                    .unwrap_or(&historical)
                    .to_owned();
                totals.model(&id, name, names.contains_key(&id))
            })
            .collect();
        models.sort_by(|left, right| {
            right
                .total_tokens
                .unwrap_or(0)
                .cmp(&left.total_tokens.unwrap_or(0))
                .then_with(|| left.model_name.cmp(&right.model_name))
        });
        let total = self.totals.model("", String::new(), false);
        AgentUsageSummaryOutput {
            request_count: total.request_count,
            message_count: total.message_count,
            unpriced_message_count: total.unpriced_message_count,
            input_tokens: total.input_tokens,
            output_tokens: total.output_tokens,
            output_thinking_tokens: total.output_thinking_tokens,
            total_tokens: total.total_tokens,
            cached_input_tokens: total.cached_input_tokens,
            cache_creation_input_tokens: total.cache_creation_input_tokens,
            estimated_cost: total.estimated_cost,
            models,
        }
    }
}

pub(crate) fn project(
    snapshot: Snapshot,
    windows: &[AgentUsageWindow],
) -> Result<AgentUsageDashboardOutput, String> {
    let _timing = crate::performance::Span::new("usage.dashboard", "project");
    let mut total = Aggregate::default();
    let mut buckets: Vec<_> = windows.iter().map(|_| Aggregate::default()).collect();
    for row in &snapshot.rows {
        total.add(row)?;
        let index = windows.partition_point(|window| window.to < row.observed_at);
        if let Some(bucket) = buckets.get_mut(index) {
            bucket.add(row)?;
        }
    }
    Ok(AgentUsageDashboardOutput {
        summary: total.finish(&snapshot.configured_names),
        buckets: buckets
            .into_iter()
            .map(|bucket| bucket.finish(&snapshot.configured_names))
            .collect(),
    })
}

#[cfg(test)]
mod tests;
