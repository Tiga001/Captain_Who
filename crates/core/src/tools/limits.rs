// Shared tool limits and byte/entry caps.
pub(super) const MAX_READ_FILE_BYTES: u64 = 512 * 1024;
pub(super) const MAX_SEARCH_FILE_BYTES: u64 = 512 * 1024;
pub(super) const DEFAULT_READ_MAX_LINES: usize = 400;
pub(super) const MAX_READ_LINES: usize = 2_000;
pub(super) const DEFAULT_SEARCH_LIMIT: usize = 40;
pub(super) const MAX_SEARCH_LIMIT: usize = 200;
pub(super) const MAX_WALK_ENTRIES: usize = 20_000;
pub(super) const MAX_GIT_DIFF_BYTES: usize = 200 * 1024;
pub(super) const MAX_DOCUMENT_FILE_BYTES: u64 = 25 * 1024 * 1024;
pub(super) const DEFAULT_DOCUMENT_MAX_CHARS: usize = 40_000;
pub(super) const MAX_DOCUMENT_TEXT_CHARS: usize = 120_000;
