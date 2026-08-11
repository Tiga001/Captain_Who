//! Cross-platform durability primitives shared by managed application stores.
//!
//! These helpers intentionally expose only commit operations. Callers remain responsible for
//! creating private same-directory staging paths, validating their content, and freezing any
//! higher-level preconditions before publication.

use std::fs;
use std::io;
use std::path::Path;

/// Atomically publishes `source` at an absent `target` without replacing an existing name.
pub fn atomic_rename_noreplace(source: &Path, target: &Path) -> io::Result<()> {
    atomic_rename_noreplace_impl(source, target)
}

/// Atomically replaces `target`. Both paths must be on the same filesystem.
pub fn atomic_replace(source: &Path, target: &Path) -> io::Result<()> {
    atomic_replace_impl(source, target)
}

/// Persists directory-entry changes where the platform exposes that primitive.
///
/// Windows publication uses `MOVEFILE_WRITE_THROUGH`; Windows has no direct equivalent of a
/// Unix directory `fsync`, so this is intentionally a documented no-op there.
pub fn sync_directory(path: &Path) -> io::Result<()> {
    sync_directory_impl(path)
}

#[cfg(unix)]
fn sync_directory_impl(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory_impl(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_directory_impl(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(unix)]
fn atomic_replace_impl(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}

#[cfg(target_vendor = "apple")]
fn atomic_rename_noreplace_impl(source: &Path, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source_c = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let target_c = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path contains NUL"))?;
    // SAFETY: both C strings are NUL-terminated and live for the duration of the call.
    let result =
        unsafe { libc::renamex_np(source_c.as_ptr(), target_c.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn atomic_rename_noreplace_impl(source: &Path, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source_c = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let target_c = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path contains NUL"))?;
    // SAFETY: both C strings are NUL-terminated and live for the duration of the syscall.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source_c.as_ptr(),
            libc::AT_FDCWD,
            target_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS) | Some(libc::EINVAL)
    ) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "filesystem does not support atomic no-replace rename",
        ));
    }
    Err(error)
}

#[cfg(all(
    unix,
    not(target_vendor = "apple"),
    not(any(target_os = "linux", target_os = "android"))
))]
fn atomic_rename_noreplace_impl(source: &Path, target: &Path) -> io::Result<()> {
    let _ = (source, target);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "platform does not support atomic no-replace rename",
    ))
}

#[cfg(windows)]
fn atomic_replace_impl(source: &Path, target: &Path) -> io::Result<()> {
    move_file(source, target, true)
}

#[cfg(windows)]
fn atomic_rename_noreplace_impl(source: &Path, target: &Path) -> io::Result<()> {
    move_file(source, target, false)
}

#[cfg(windows)]
fn move_file(source: &Path, target: &Path, replace: bool) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    // SAFETY: both UTF-16 buffers are NUL-terminated and live for the call.
    let result = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), flags) };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn atomic_replace_impl(source: &Path, target: &Path) -> io::Result<()> {
    fs::rename(source, target)
}

#[cfg(not(any(unix, windows)))]
fn atomic_rename_noreplace_impl(source: &Path, target: &Path) -> io::Result<()> {
    if fs::symlink_metadata(target).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "atomic rename target already exists",
        ));
    }
    fs::rename(source, target)
}
