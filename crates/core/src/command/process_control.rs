//! Operating-system process containment for command sessions.
//!
//! A [`Child`] has exactly one owner: the command-session watcher.  Pollers and
//! interrupters send control messages to that watcher and never race it for
//! `wait(2)` or process-group signalling.

use std::io;
use std::process::{Child, Command, ExitStatus};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

/// Drop guard that makes watcher creation/panic paths fail closed.  Normal
/// completion marks the child reaped; every other path kills the group and
/// performs a best-effort reap.
#[derive(Debug)]
pub(crate) struct ManagedCommandChild {
    child: Option<Child>,
}

impl ManagedCommandChild {
    pub(crate) fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("managed command child is present until it is reaped")
    }

    pub(crate) fn mark_reaped(&mut self) {
        self.child = None;
    }
}

impl Drop for ManagedCommandChild {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            force_terminate_command_process_group(child);
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
pub(crate) fn configure_command_process_group(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn configure_command_process_group(_command: &mut Command) {}

/// Observe a completed group leader without reaping it, clean up ordinary
/// descendants, then perform the sole authoritative reap.
///
/// `Child::try_wait` reaps on Unix.  Signalling the process group afterwards
/// could then race PID/PGID reuse.  `waitid(..., WNOWAIT)` pins the leader's
/// identity until descendants have been cleaned up and `Child::wait` reaps it.
#[cfg(unix)]
pub(crate) fn try_wait_command_process_group(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    loop {
        let mut information = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        // SAFETY: `information` is writable storage for one siginfo_t.  The PID
        // belongs to this process and WNOWAIT deliberately preserves waitability.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                information.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        // SAFETY: successful waitid initializes siginfo_t.  POSIX specifies
        // si_pid == 0 when WNOHANG observes no waitable state.
        let information = unsafe { information.assume_init() };
        // SAFETY: si_pid is defined for SIGCHLD information from WEXITED.
        if unsafe { information.si_pid() } == 0 {
            return Ok(None);
        }

        force_terminate_command_process_group(child);
        return child.wait().map(Some);
    }
}

#[cfg(not(unix))]
pub(crate) fn try_wait_command_process_group(child: &mut Child) -> io::Result<Option<ExitStatus>> {
    child.try_wait()
}

#[cfg(unix)]
fn signal_process_group(child: &mut Child, signal: libc::c_int) {
    let Ok(process_group) = i32::try_from(child.id()) else {
        if signal == libc::SIGKILL {
            let _ = child.kill();
        }
        return;
    };
    // SAFETY: kill does not dereference memory.  A negative PID addresses only
    // the fresh process group created for this child.
    let signalled = unsafe { libc::kill(-process_group, signal) } == 0;
    if !signalled && signal == libc::SIGKILL {
        let _ = child.kill();
    }
}

/// Request a graceful user-style interruption of the complete process group.
#[cfg(unix)]
pub(crate) fn interrupt_command_process_group(child: &mut Child) {
    signal_process_group(child, libc::SIGINT);
}

#[cfg(not(unix))]
pub(crate) fn interrupt_command_process_group(child: &mut Child) {
    // Internal compilation fallback only.  Public command authorization remains
    // fail-closed on Windows until Job Objects provide descendant containment.
    let _ = child.kill();
}

#[cfg(unix)]
pub(crate) fn force_terminate_command_process_group(child: &mut Child) {
    signal_process_group(child, libc::SIGKILL);
}

#[cfg(not(unix))]
pub(crate) fn force_terminate_command_process_group(child: &mut Child) {
    let _ = child.kill();
}
