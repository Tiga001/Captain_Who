use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalTokenUsageSummaryInput {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalTokenUsageDay {
    pub date: String,
    /// Decimal string avoids JavaScript/SQLite integer precision loss.
    pub token_count: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalTokenUsageSummaryOutput {
    pub timezone: String,
    pub started_at: i64,
    pub days: Vec<LocalTokenUsageDay>,
    /// All-time totals, independent of the requested daily display window.
    pub total_tokens: String,
    pub today_tokens: String,
    pub peak_daily_tokens: String,
    /// All-time requests whose complete token usage was unavailable.
    pub unreported_request_count: u64,
}
