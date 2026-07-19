use crate::system_paths::expand_system_path;
use crate::{
    AgentCancellationToken, AgentCommandRequest, AgentCommandRiskLevel, AgentCommandSafetyPolicy,
    AgentPermissions, AgentReadPermission, AgentWritePermission,
};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

mod allowlist;
mod execution;
mod lexer;
mod policy;
mod risk;
mod segment;
mod types;

use allowlist::*;
pub use execution::*;
use lexer::*;
pub use policy::*;
use risk::*;
use segment::*;
pub use types::*;

#[cfg(all(test, not(windows)))]
mod tests;

#[cfg(all(test, windows))]
mod windows_tests;
