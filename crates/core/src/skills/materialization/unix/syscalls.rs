use std::ffi::CStr;
use std::io;
use std::os::fd::RawFd;

pub(super) fn matches_errno(error: &io::Error, values: &[i32]) -> bool {
    error
        .raw_os_error()
        .is_some_and(|actual| values.contains(&actual))
}

pub(super) fn is_already_exists(error: &io::Error) -> bool {
    matches_errno(error, &[libc::EEXIST, libc::ENOTEMPTY])
}

pub(super) fn is_unsupported(error: &io::Error) -> bool {
    matches_errno(error, &[libc::ENOSYS, libc::EINVAL, libc::ENOTSUP])
}

#[cfg(target_vendor = "apple")]
pub(super) fn rename_noreplace(parent_fd: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
    // SAFETY: both names and the parent fd are live for the call.
    let result = unsafe {
        libc::renameatx_np(
            parent_fd,
            source.as_ptr(),
            parent_fd,
            target.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn rename_noreplace(parent_fd: RawFd, source: &CStr, target: &CStr) -> io::Result<()> {
    // SAFETY: both names and the parent fd are live for the syscall.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            parent_fd,
            source.as_ptr(),
            parent_fd,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
