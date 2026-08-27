use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeErrorCategory {
    Wire,
    Semantic,
    Path,
    Authorization,
    Precondition,
    Execution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeRecovery {
    CorrectArguments,
    UseStagedChange,
    UseAbsolutePath,
    CreateParent,
    RequestPermission,
    ChooseRegularFile,
    UseDedicatedTool,
    RereadFile,
    UseUpdateOrChooseAnotherPath,
    SkipOrReviseEdit,
    InspectAuthoritativeState,
    DoNotRetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeErrorCode {
    InvalidArguments,
    UnknownField,
    IllegalFieldCombination,
    ContentTooLarge,
    WorkspaceRequired,
    PermissionDenied,
    ParentMissing,
    SymlinkForbidden,
    UnsupportedFileType,
    NotRegularFile,
    HardLinkForbidden,
    FileExists,
    FileMissing,
    RevisionConflict,
    MatchNotFound,
    AmbiguousMatch,
    NoChange,
    Failed,
    Conflict,
    OutcomeUnknown,
}

impl FileChangeErrorCode {
    pub const fn category(self) -> FileChangeErrorCategory {
        match self {
            Self::InvalidArguments | Self::UnknownField => FileChangeErrorCategory::Wire,
            Self::IllegalFieldCombination | Self::ContentTooLarge => {
                FileChangeErrorCategory::Semantic
            }
            Self::WorkspaceRequired
            | Self::ParentMissing
            | Self::SymlinkForbidden
            | Self::UnsupportedFileType
            | Self::NotRegularFile
            | Self::HardLinkForbidden => FileChangeErrorCategory::Path,
            Self::PermissionDenied => FileChangeErrorCategory::Authorization,
            Self::FileExists
            | Self::FileMissing
            | Self::RevisionConflict
            | Self::MatchNotFound
            | Self::AmbiguousMatch
            | Self::NoChange => FileChangeErrorCategory::Precondition,
            Self::Failed | Self::Conflict | Self::OutcomeUnknown => {
                FileChangeErrorCategory::Execution
            }
        }
    }

    pub const fn recovery(self) -> FileChangeRecovery {
        match self {
            Self::InvalidArguments | Self::UnknownField | Self::IllegalFieldCombination => {
                FileChangeRecovery::CorrectArguments
            }
            Self::ContentTooLarge => FileChangeRecovery::UseStagedChange,
            Self::WorkspaceRequired => FileChangeRecovery::UseAbsolutePath,
            Self::PermissionDenied => FileChangeRecovery::RequestPermission,
            Self::ParentMissing => FileChangeRecovery::CreateParent,
            Self::SymlinkForbidden | Self::NotRegularFile | Self::HardLinkForbidden => {
                FileChangeRecovery::ChooseRegularFile
            }
            Self::UnsupportedFileType => FileChangeRecovery::UseDedicatedTool,
            Self::FileExists => FileChangeRecovery::UseUpdateOrChooseAnotherPath,
            Self::FileMissing
            | Self::RevisionConflict
            | Self::MatchNotFound
            | Self::AmbiguousMatch => FileChangeRecovery::RereadFile,
            Self::NoChange => FileChangeRecovery::SkipOrReviseEdit,
            Self::Failed | Self::Conflict => FileChangeRecovery::InspectAuthoritativeState,
            Self::OutcomeUnknown => FileChangeRecovery::DoNotRetry,
        }
    }

    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::InvalidArguments | Self::UnknownField | Self::IllegalFieldCombination => {
                "文件修改参数无效。"
            }
            Self::ContentTooLarge => "文件内容过大。",
            Self::WorkspaceRequired => "当前没有 workspace；请使用绝对路径或系统路径别名。",
            Self::PermissionDenied => "当前权限不允许修改此文件。",
            Self::ParentMissing => "目标文件的父目录不存在。",
            Self::SymlinkForbidden => "不允许通过符号链接修改文件。",
            Self::UnsupportedFileType => "此文件类型需要使用专用工具。",
            Self::NotRegularFile | Self::HardLinkForbidden => "目标必须是安全的普通文件。",
            Self::FileExists => "文件已存在。",
            Self::FileMissing => "文件不存在。",
            Self::RevisionConflict => "文件已发生变化，请重新读取后再修改。",
            Self::MatchNotFound => "未找到要修改的准确内容。",
            Self::AmbiguousMatch => "要修改的内容不唯一，请提供更精确的上下文。",
            Self::NoChange => "修改后的内容与当前文件相同。",
            Self::Failed => "文件修改失败。",
            Self::Conflict => "文件修改发生冲突。",
            Self::OutcomeUnknown => "无法确认文件修改结果，请先检查文件当前状态。",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeFailure {
    pub code: FileChangeErrorCode,
    pub category: FileChangeErrorCategory,
    pub message: String,
    pub recovery: FileChangeRecovery,
}

#[derive(Debug, Clone)]
pub struct FileChangeError {
    failure: FileChangeFailure,
    diagnostic: Option<Box<str>>,
}

impl FileChangeError {
    pub fn new(code: FileChangeErrorCode) -> Self {
        Self {
            failure: FileChangeFailure {
                code,
                category: code.category(),
                message: code.safe_message().to_string(),
                recovery: code.recovery(),
            },
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(code: FileChangeErrorCode, diagnostic: impl Into<Box<str>>) -> Self {
        Self {
            diagnostic: Some(diagnostic.into()),
            ..Self::new(code)
        }
    }

    pub fn failure(&self) -> &FileChangeFailure {
        &self.failure
    }

    pub fn code(&self) -> FileChangeErrorCode {
        self.failure.code
    }

    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

impl fmt::Display for FileChangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.failure.message)
    }
}

impl std::error::Error for FileChangeError {}

pub(crate) type FileChangeResultValue<T> = Result<T, FileChangeError>;
