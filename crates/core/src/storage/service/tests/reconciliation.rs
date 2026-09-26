//! Storage reconciliation tests, grouped by durable receipt and recovery boundary.
//! Shared fixtures reuse the parent test module's storage isolation and cleanup.

use super::*;

mod command_fixture;
mod file_effects;
mod fixtures;
mod manual_command;
mod manual_non_command;
mod mcp_fixture;
mod mcp_receipts;
mod mcp_recovery;
mod non_command_fixture;
mod startup;
mod startup_malformed;
mod startup_nested;
mod tamper;
mod transactions;
