//! Fork-only staging: database snapshots end before copying files or cloning encrypted turns.
use super::*;
use crate::durable_fs::{atomic_rename_noreplace, sync_directory};
use crate::storage::conversation_fork_repository::{ConversationForkError, ConversationForkPlan};
use std::fs::{File, OpenOptions};
use std::io;

/// Versions are captured with the plan in one read transaction, then checked after BEGIN
/// IMMEDIATE. The history digest includes message/Trace epochs, not just wall-clock timestamps.
/// Other fork journals have their own immutable identities or explicit mutation revisions.
pub(super) struct ForkSourceVersions {
    sources: Vec<(String, ConversationHistoryVersion, i64, [u8; 32])>,
}

impl ForkSourceVersions {
    pub(super) fn capture(
        service: &StorageService,
        connection: &rusqlite::Connection,
        plan: &ConversationForkPlan,
    ) -> Result<Self, ConversationForkError> {
        let mut sources = Vec::new();
        for id in plan.source_conversation_ids() {
            let (history, revision) =
                super::super::history_snapshot::history_version_in_connection(
                    connection,
                    id,
                    &service.state.instance_identity,
                )?
                .ok_or_else(|| "原任务已不存在。".to_string())?;
            sources.push((
                id.to_string(),
                history,
                revision,
                side_versions(connection, id)?,
            ));
        }
        Ok(Self { sources })
    }

    pub(super) fn validate(
        &self,
        service: &StorageService,
        connection: &rusqlite::Connection,
        plan: &ConversationForkPlan,
    ) -> Result<(), ConversationForkError> {
        for (id, expected_history, expected_revision, expected_side) in &self.sources {
            let current = super::super::history_snapshot::history_version_in_connection(
                connection,
                id,
                &service.state.instance_identity,
            )?;
            if current.as_ref().is_none_or(|(history, revision)| {
                history != expected_history || revision != expected_revision
            }) || side_versions(connection, id)? != *expected_side
            {
                return Err("原任务在分叉准备期间已变化，请重试。".to_string().into());
            }
        }
        // Exact envelopes remain authoritative. A release/supersession during out-of-lock
        // decrypt/encrypt must not resurrect a continuation in the new branch.
        for mapping in &plan.provider_continuation_mappings {
            let current = provider_continuation_repository::load(
                connection,
                &mapping.source_record.continuation_id,
            )
            .map_err(storage_error)?;
            if current.as_ref() != Some(&mapping.source_record) {
                return Err("Provider continuation 在分叉准备期间已变化，请重试。"
                    .to_string()
                    .into());
            }
        }
        ensure_project_reference_exists(connection, plan.target.project_id.as_deref())?;
        Ok(())
    }
}

mod side_versions;
pub(in crate::storage::service) use side_versions::child_snapshot_side_version;
use side_versions::side_versions;

/// Owns only names allocated by this operation. Files are published before SQLite references
/// them. A rejected CAS/replay cleans only this operation's files; a successful commit disarms
/// cleanup before any fallible read-back. Abrupt process death may leave unreachable files.
#[derive(Default)]
pub(in crate::storage::service) struct ForkFiles {
    staged: Vec<PathBuf>,
    published: Vec<PathBuf>,
    committed: bool,
}

impl ForkFiles {
    pub(super) fn prepare(
        attachment_root: &Path,
        plan: &mut ConversationForkPlan,
    ) -> Result<Self, String> {
        Self::prepare_attachments(attachment_root, plan.attachments_mut())
    }

    pub(in crate::storage::service) fn prepare_attachments<'a>(
        attachment_root: &Path,
        attachments: impl Iterator<Item = &'a mut conversation_fork_repository::ForkAttachmentCopy>,
    ) -> Result<Self, String> {
        let mut files = Self::default();
        #[cfg(test)]
        PREPARE_HOOK.with(|hook| {
            let callback = hook.borrow_mut().take();
            if let Some(callback) = callback {
                callback();
            }
        });
        for attachment in attachments {
            let source_path = safe_existing_attachment_storage_path(
                attachment_root,
                &attachment.source.storage_rel_path,
            )
            .ok_or_else(|| format!("原任务附件文件不存在：{}", attachment.source.original_name))?;
            let target_rel_path = attachment_storage_rel_path(
                &attachment.target.conversation_id,
                &attachment.target.message_id,
                &attachment.target.id,
                &attachment.target.original_name,
            );
            attachment.target.storage_rel_path = slash_path(&target_rel_path);
            let target_path = attachment_root.join(&target_rel_path);
            let parent = target_path
                .parent()
                .ok_or_else(|| "新任务附件路径无效。".to_string())?;
            create_and_sync_directories(attachment_root, parent)?;
            let staging_path = parent.join(format!(
                ".{}.forking-{}",
                safe_path_component(&attachment.target.id, "attachment"),
                Uuid::new_v4()
            ));
            // create_new is mandatory: a failed allocation must never give us ownership of an
            // existing operation's file. Parent names are generated for this branch.
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staging_path)
                .map_err(|error| format!("创建新任务附件暂存文件失败：{error}"))?;
            files.staged.push(staging_path.clone());
            let source =
                File::open(&source_path).map_err(|error| format!("读取原任务附件失败：{error}"))?;
            let metadata = source
                .metadata()
                .map_err(|error| format!("读取原任务附件失败：{error}"))?;
            if !metadata.is_file() || metadata.len() != attachment.source.size_bytes {
                return Err(format!(
                    "复制附件时大小不一致：{}",
                    attachment.source.original_name
                ));
            }
            use std::io::Read;
            let copied = io::copy(
                &mut source.take(attachment.source.size_bytes.saturating_add(1)),
                &mut output,
            )
            .map_err(|error| format!("复制附件失败：{error}"))?;
            if copied != attachment.source.size_bytes {
                return Err(format!(
                    "复制附件时大小不一致：{}",
                    attachment.source.original_name
                ));
            }
            output
                .sync_all()
                .map_err(|error| format!("同步新任务附件失败：{error}"))?;
            drop(output);
            atomic_rename_noreplace(&staging_path, &target_path)
                .map_err(|error| format!("提交新任务附件失败：{error}"))?;
            files.published.push(target_path.clone());
            sync_directory(parent).map_err(|error| format!("同步新任务附件目录失败：{error}"))?;
        }
        Ok(files)
    }

    pub(in crate::storage::service) fn mark_committed(&mut self) {
        self.committed = true;
    }
}

impl Drop for ForkFiles {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in self.staged.iter().chain(&self.published) {
            if let Err(error) = fs::remove_file(path) {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!(
                        "failed to remove uncommitted fork attachment {}: {error}",
                        path.display()
                    );
                }
            }
        }
    }
}

fn create_and_sync_directories(root: &Path, parent: &Path) -> Result<(), String> {
    fs::create_dir_all(parent).map_err(|error| format!("创建新任务附件目录失败：{error}"))?;
    // Sync each new directory entry through the existing attachment root. Windows uses the
    // platform's write-through rename; sync_directory is explicitly a no-op there.
    let mut directory = Some(parent);
    while let Some(path) = directory {
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("新任务附件目录无效。".to_string());
        }
        sync_directory(path).map_err(|error| format!("同步新任务附件目录失败：{error}"))?;
        if path == root {
            break;
        }
        directory = path.parent();
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static PREPARE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}

#[cfg(test)]
pub(in crate::storage::service) fn set_prepare_hook(callback: impl FnOnce() + 'static) {
    PREPARE_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(callback)));
}

#[cfg(test)]
mod tests;
