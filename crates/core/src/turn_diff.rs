use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

pub const AGENT_TURN_DIFF_SCHEMA_VERSION: u32 = 1;
pub const MAX_AGENT_TURN_FILE_CONTENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTurnFileContent {
    Missing,
    Text(String),
    Binary,
    TooLarge,
}

impl AgentTurnFileContent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Text(_) => "text",
            Self::Binary => "binary",
            Self::TooLarge => "too_large",
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(content) => Some(content),
            Self::Missing | Self::Binary | Self::TooLarge => None,
        }
    }

    pub fn from_storage(kind: &str, text: Option<String>) -> Result<Self, String> {
        match (kind, text) {
            ("missing", None) => Ok(Self::Missing),
            ("text", Some(content)) => Ok(Self::Text(content)),
            ("binary", None) => Ok(Self::Binary),
            ("too_large", None) => Ok(Self::TooLarge),
            _ => Err("Stored agent turn file content is inconsistent.".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnFileChange {
    pub path: String,
    pub before: AgentTurnFileContent,
    pub after: AgentTurnFileContent,
}

impl AgentTurnFileChange {
    pub fn is_exact_noop(&self) -> bool {
        self.before == self.after
            && matches!(
                self.before,
                AgentTurnFileContent::Missing | AgentTurnFileContent::Text(_)
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnDiffIdentity {
    pub run_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub project_id: String,
    pub workspace_root: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnDiffRecord {
    pub identity: AgentTurnDiffIdentity,
    pub files: Vec<AgentTurnFileChange>,
    pub truncated: bool,
}

pub fn capture_agent_turn_file_content(path: &Path) -> AgentTurnFileContent {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return AgentTurnFileContent::Missing;
        }
        Err(_) => return AgentTurnFileContent::Binary,
    };

    if metadata.file_type().is_symlink() {
        return fs::read_link(path)
            .map(|target| AgentTurnFileContent::Text(target.to_string_lossy().into_owned()))
            .unwrap_or(AgentTurnFileContent::Binary);
    }
    if !metadata.is_file() {
        return AgentTurnFileContent::Binary;
    }
    if metadata.len() > MAX_AGENT_TURN_FILE_CONTENT_BYTES as u64 {
        return AgentTurnFileContent::TooLarge;
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let read_result = File::open(path).and_then(|file| {
        file.take((MAX_AGENT_TURN_FILE_CONTENT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
    });
    if read_result.is_err() {
        return AgentTurnFileContent::Binary;
    }
    if bytes.len() > MAX_AGENT_TURN_FILE_CONTENT_BYTES {
        return AgentTurnFileContent::TooLarge;
    }
    if bytes.contains(&0) {
        return AgentTurnFileContent::Binary;
    }
    String::from_utf8(bytes)
        .map(AgentTurnFileContent::Text)
        .unwrap_or(AgentTurnFileContent::Binary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_missing_text_binary_and_oversized_files() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.txt");
        assert_eq!(
            capture_agent_turn_file_content(&missing),
            AgentTurnFileContent::Missing
        );

        let text = directory.path().join("text.txt");
        fs::write(&text, "hello\n").unwrap();
        assert_eq!(
            capture_agent_turn_file_content(&text),
            AgentTurnFileContent::Text("hello\n".to_string())
        );

        let binary = directory.path().join("binary.bin");
        fs::write(&binary, [0, 1, 2]).unwrap();
        assert_eq!(
            capture_agent_turn_file_content(&binary),
            AgentTurnFileContent::Binary
        );

        let oversized = directory.path().join("oversized.txt");
        let file = File::create(&oversized).unwrap();
        file.set_len((MAX_AGENT_TURN_FILE_CONTENT_BYTES + 1) as u64)
            .unwrap();
        assert_eq!(
            capture_agent_turn_file_content(&oversized),
            AgentTurnFileContent::TooLarge
        );
    }
}
