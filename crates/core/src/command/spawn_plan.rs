//! Construction of the already-authorized OS spawn request.

use super::{configure_command_process_group, relative_cwd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct CommandSpawnPlan {
    command: String,
    cwd: PathBuf,
    cwd_projection: String,
    hard_timeout: Option<Duration>,
}

impl CommandSpawnPlan {
    pub(crate) fn shell(
        command: String,
        cwd: PathBuf,
        root: Option<&Path>,
        hard_timeout: Option<Duration>,
    ) -> Self {
        let cwd_projection = relative_cwd(root, &cwd);
        Self {
            command,
            cwd,
            cwd_projection,
            hard_timeout,
        }
    }

    pub(crate) fn build(&self) -> Command {
        let mut command = shell_command(&self.command);
        configure_command_process_group(&mut command);
        command
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("TERM", "dumb")
            .env("CI", "1");
        command
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    pub(crate) fn cwd_projection(&self) -> &str {
        &self.cwd_projection
    }

    pub(crate) fn hard_timeout(&self) -> Option<Duration> {
        self.hard_timeout
    }
}

#[cfg(target_os = "windows")]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("cmd.exe");
    shell.arg("/C").arg(command);
    shell
}

#[cfg(not(target_os = "windows"))]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("/bin/sh");
    // Login profiles can redefine an inspected command between policy and spawn.
    shell.arg("-c").arg(command);
    shell
}
