//! Construction of the already-authorized OS spawn request.

use super::{configure_command_process_group, relative_cwd};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct CommandSpawnPlan {
    command: String,
    cwd: PathBuf,
    cwd_projection: String,
    hard_timeout: Option<Duration>,
    launch: CommandLaunchPlan,
}

#[derive(Debug, Clone)]
enum CommandLaunchPlan {
    Shell,
    Direct {
        executable: PathBuf,
        arguments: Vec<OsString>,
        environment: Vec<(OsString, OsString)>,
        clear_environment: bool,
    },
}

/// Direct executable launch authority prepared by a trusted runtime adapter.
///
/// Grouping these fields prevents the generic Session spawn plan from growing one positional
/// parameter for each future launch policy knob.
#[derive(Debug, Clone)]
pub(crate) struct CommandDirectLaunchPlan {
    executable: PathBuf,
    arguments: Vec<OsString>,
    environment: Vec<(OsString, OsString)>,
    clear_environment: bool,
}

impl CommandDirectLaunchPlan {
    /// Builds an isolated managed-runtime launch which never inherits the ambient environment.
    pub(crate) fn isolated(
        executable: PathBuf,
        arguments: Vec<OsString>,
        environment: Vec<(OsString, OsString)>,
    ) -> Self {
        Self {
            executable,
            arguments,
            environment,
            clear_environment: true,
        }
    }
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
            launch: CommandLaunchPlan::Shell,
        }
    }

    pub(crate) fn direct(
        command: String,
        cwd: PathBuf,
        root: Option<&Path>,
        hard_timeout: Option<Duration>,
        launch: CommandDirectLaunchPlan,
    ) -> Self {
        let cwd_projection = relative_cwd(root, &cwd);
        let CommandDirectLaunchPlan {
            executable,
            arguments,
            environment,
            clear_environment,
        } = launch;
        Self {
            command,
            cwd,
            cwd_projection,
            hard_timeout,
            launch: CommandLaunchPlan::Direct {
                executable,
                arguments,
                environment,
                clear_environment,
            },
        }
    }

    pub(crate) fn build(&self) -> Command {
        let mut command = match &self.launch {
            CommandLaunchPlan::Shell => shell_command(&self.command),
            CommandLaunchPlan::Direct {
                executable,
                arguments,
                environment,
                clear_environment,
            } => {
                let mut command = Command::new(executable);
                if *clear_environment {
                    command.env_clear();
                }
                command.args(arguments).envs(environment.iter().cloned());
                command
            }
        };
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
