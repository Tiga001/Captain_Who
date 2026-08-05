use std::ffi::CStr;
use std::os::fd::RawFd;

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::skills::materialization) enum MaterializationTestHookPoint {
    FileBeforePublish,
    TreeBeforePublish,
}

#[cfg(test)]
type MaterializationTestHook = dyn FnMut(MaterializationTestHookPoint, RawFd, &CStr);

#[cfg(test)]
thread_local! {
    static MATERIALIZATION_TEST_HOOK: std::cell::RefCell<Option<Box<MaterializationTestHook>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(in crate::skills::materialization) struct MaterializationTestHookGuard;

#[cfg(test)]
impl Drop for MaterializationTestHookGuard {
    fn drop(&mut self) {
        MATERIALIZATION_TEST_HOOK.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

#[cfg(test)]
pub(in crate::skills::materialization) fn install_materialization_test_hook(
    hook: impl FnMut(MaterializationTestHookPoint, RawFd, &CStr) + 'static,
) -> MaterializationTestHookGuard {
    MATERIALIZATION_TEST_HOOK.with(|slot| {
        assert!(slot.borrow_mut().replace(Box::new(hook)).is_none());
    });
    MaterializationTestHookGuard
}

#[cfg(test)]
pub(super) fn invoke_materialization_test_hook(
    point: MaterializationTestHookPoint,
    parent_fd: RawFd,
    staging_name: &CStr,
) {
    MATERIALIZATION_TEST_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(point, parent_fd, staging_name);
        }
    });
}
