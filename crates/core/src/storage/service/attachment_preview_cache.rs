//! Disposable, per-storage preview cache. Model-visible frozen images use a different store.
//! Original files are immutable through the attachment API. We still open/stat every request:
//! deletion, replacement, permission changes and restore must never serve a stale thumbnail.
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

use crate::file_input::image_delivery::{
    thumbnail_data_url, THUMBNAIL_ALGORITHM_VERSION, THUMBNAIL_MAX_EDGE,
};

// Retained encoded payload is byte-bounded; both lookup maps are additionally entry-bounded.
const PREVIEW_BYTE_BUDGET: usize = 32 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 1024;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct FileVersion {
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    readonly: bool,
    #[cfg(unix)]
    unix_identity: (u64, u64, i64, i64, u32),
    #[cfg(windows)]
    windows_identity: Option<(u64, [u8; 16], i64)>,
}

impl FileVersion {
    fn from_file(file: &File) -> Option<Self> {
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(Self {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
            created: metadata.created().ok(),
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            unix_identity: {
                use std::os::unix::fs::MetadataExt;
                (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                    metadata.mode(),
                )
            },
            #[cfg(windows)]
            windows_identity: windows_file_identity(file),
        })
    }

    fn can_reuse_source(&self) -> bool {
        #[cfg(windows)]
        {
            self.windows_identity.is_some()
        }
        #[cfg(not(windows))]
        {
            true
        }
    }
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> Option<(u64, [u8; 16], i64)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_ID_INFO,
    };
    let mut basic = std::mem::MaybeUninit::<FILE_BASIC_INFO>::zeroed();
    let mut identity = std::mem::MaybeUninit::<FILE_ID_INFO>::zeroed();
    // SAFETY: each output buffer has the type/size required by its information class, and the
    // borrowed handle stays alive for both calls. Unsupported filesystems skip the source index.
    unsafe {
        if GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            basic.as_mut_ptr().cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        ) == 0
            || GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                identity.as_mut_ptr().cast(),
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            ) == 0
        {
            return None;
        }
        let basic = basic.assume_init();
        let identity = identity.assume_init();
        Some((
            identity.VolumeSerialNumber,
            identity.FileId.Identifier,
            basic.ChangeTime,
        ))
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct PreviewVariant {
    version: u32,
    max_edge: u32,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct SourceKey {
    path: PathBuf,
    file: FileVersion,
    variant: PreviewVariant,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct ContentKey {
    digest: [u8; 32],
    variant: PreviewVariant,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum FlightKey {
    Source(SourceKey),
    Content(ContentKey),
}

#[derive(Default)]
struct Flight {
    // None = in progress; Some(None) = failed. Failures are never retained in the cache.
    result: Mutex<Option<Option<Arc<String>>>>,
    ready: Condvar,
}

impl Flight {
    fn wait(&self) -> Option<Arc<String>> {
        let mut result = self
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while result.is_none() {
            result = self
                .ready
                .wait(result)
                .unwrap_or_else(|error| error.into_inner());
        }
        result.as_ref().cloned().flatten()
    }
}

struct CachedPreview {
    value: Arc<String>,
    last_used: u64,
}

struct CachedSource {
    content: ContentKey,
    last_used: u64,
}

#[derive(Default)]
struct CacheState {
    previews: HashMap<ContentKey, CachedPreview>,
    sources: HashMap<SourceKey, CachedSource>,
    flights: HashMap<FlightKey, Arc<Flight>>,
    bytes: usize,
    clock: u64,
}

impl CacheState {
    fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn preview(&mut self, key: &ContentKey) -> Option<Arc<String>> {
        let tick = self.tick();
        let entry = self.previews.get_mut(key)?;
        entry.last_used = tick;
        Some(entry.value.clone())
    }

    fn remove_preview(&mut self, key: &ContentKey) {
        if let Some(entry) = self.previews.remove(key) {
            self.bytes -= entry.value.capacity();
        }
        self.sources.retain(|_, source| &source.content != key);
    }

    fn remember_source(&mut self, source: SourceKey, content: ContentKey) {
        if !source.file.can_reuse_source() || !self.previews.contains_key(&content) {
            return;
        }
        // Replaced originals do not leave multiple fingerprints for the same path/version.
        let replaced = self
            .sources
            .iter()
            .filter(|(key, _)| key.path == source.path && key.variant == source.variant)
            .map(|(_, entry)| entry.content.clone())
            .collect::<Vec<_>>();
        self.sources
            .retain(|key, _| key.path != source.path || key.variant != source.variant);
        for previous in replaced {
            if previous != content && !self.sources.values().any(|entry| entry.content == previous)
            {
                self.remove_preview(&previous);
            }
        }
        if self.sources.len() >= MAX_CACHE_ENTRIES {
            if let Some(oldest) = self
                .sources
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            {
                self.sources.remove(&oldest);
            }
        }
        let tick = self.tick();
        self.sources.insert(
            source,
            CachedSource {
                content,
                last_used: tick,
            },
        );
    }
}

pub(super) struct AttachmentPreviewCache {
    state: Mutex<CacheState>,
    byte_budget: usize,
    #[cfg(test)]
    stats: CacheStats,
}

impl Default for AttachmentPreviewCache {
    fn default() -> Self {
        Self::with_budget(PREVIEW_BYTE_BUDGET)
    }
}

impl AttachmentPreviewCache {
    fn with_budget(byte_budget: usize) -> Self {
        Self {
            state: Mutex::new(CacheState::default()),
            byte_budget,
            #[cfg(test)]
            stats: CacheStats::default(),
        }
    }

    pub(super) fn read(&self, path: &Path) -> Option<String> {
        self.read_variant(
            path,
            PreviewVariant {
                version: THUMBNAIL_ALGORITHM_VERSION,
                max_edge: THUMBNAIL_MAX_EDGE,
            },
        )
        .map(|value| (*value).clone())
    }

    fn read_variant(&self, path: &Path, variant: PreviewVariant) -> Option<Arc<String>> {
        // The caller has authorized the attachment and resolved it inside its storage root.
        // Do not serve a memory hit unless the original is still readable.
        let path = path.canonicalize().ok()?;
        let mut file = File::open(&path).ok()?;
        let source = SourceKey {
            path,
            file: FileVersion::from_file(&file)?,
            variant,
        };
        let key = FlightKey::Source(source.clone());
        let flight = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let tick = state.tick();
            let content = state.sources.get_mut(&source).map(|entry| {
                entry.last_used = tick;
                entry.content.clone()
            });
            if let Some(value) = content.and_then(|content| state.preview(&content)) {
                return Some(value);
            }
            if let Some(flight) = state.flights.get(&key).cloned() {
                drop(state);
                return flight.wait();
            }
            let flight = Arc::new(Flight::default());
            state.flights.insert(key.clone(), flight.clone());
            flight
        };
        let guard = FlightGuard::new(self, key, flight);
        let value = (|| {
            // Hash once on a source miss; hot reads need neither file contents nor image decode.
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 64 * 1024];
            let mut total = 0u64;
            loop {
                let read = file.read(&mut buffer).ok()?;
                if read == 0 {
                    break;
                }
                #[cfg(test)]
                self.stats
                    .read_bytes
                    .fetch_add(read as u64, Ordering::Relaxed);
                total = total.checked_add(read as u64)?;
                if total > source.file.len {
                    return None;
                }
                digest.update(&buffer[..read]);
            }
            if !source.matches(&file) {
                return None;
            }
            let content = ContentKey {
                digest: digest.finalize().into(),
                variant,
            };
            let value = self.content_preview(&mut file, &source, &content)?;
            if !source.matches(&file) {
                return None;
            }
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remember_source(source, content);
            Some(value)
        })();
        guard.complete(value.clone());
        value
    }

    fn content_preview(
        &self,
        file: &mut File,
        source: &SourceKey,
        content: &ContentKey,
    ) -> Option<Arc<String>> {
        let key = FlightKey::Content(content.clone());
        let flight = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(value) = state.preview(content) {
                return Some(value);
            }
            if let Some(flight) = state.flights.get(&key).cloned() {
                drop(state);
                return flight.wait();
            }
            let flight = Arc::new(Flight::default());
            state.flights.insert(key.clone(), flight.clone());
            flight
        };
        let guard = FlightGuard::new(self, key, flight);
        let value = (|| {
            file.seek(SeekFrom::Start(0)).ok()?;
            #[cfg(test)]
            self.stats.generations.fetch_add(1, Ordering::Relaxed);
            #[cfg(test)]
            let reader = CountedReader {
                file: &mut *file,
                stats: &self.stats,
            };
            #[cfg(not(test))]
            let reader = &mut *file;
            let url = thumbnail_data_url(BufReader::new(reader)).ok()?;
            if !source.matches(file) {
                return None;
            }
            let value = Arc::new(url.strip_prefix("data:image/png;base64,")?.to_string());
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let bytes = value.capacity();
            if bytes <= self.byte_budget {
                while state.bytes + bytes > self.byte_budget
                    || state.previews.len() >= MAX_CACHE_ENTRIES
                {
                    let oldest = state
                        .previews
                        .iter()
                        .min_by_key(|(_, entry)| entry.last_used)
                        .map(|(key, _)| key.clone())?;
                    state.remove_preview(&oldest);
                }
                let tick = state.tick();
                state.bytes += bytes;
                state.previews.insert(
                    content.clone(),
                    CachedPreview {
                        value: value.clone(),
                        last_used: tick,
                    },
                );
            }
            Some(value)
        })();
        guard.complete(value.clone());
        value
    }

    pub(super) fn invalidate(&self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let removed = state
            .sources
            .iter()
            .filter(|(source, _)| source.path == path)
            .map(|(_, entry)| entry.content.clone())
            .collect::<Vec<_>>();
        state.sources.retain(|source, _| source.path != path);
        for content in removed {
            if !state.sources.values().any(|entry| entry.content == content) {
                state.remove_preview(&content);
            }
        }
    }
}

impl SourceKey {
    fn matches(&self, file: &File) -> bool {
        FileVersion::from_file(file) == Some(self.file.clone())
            && File::open(&self.path)
                .ok()
                .and_then(|file| FileVersion::from_file(&file))
                == Some(self.file.clone())
    }
}

// A panic or early exit must not leave followers blocked forever. There is no partial on-disk
// cache: the complete encoded String is published under the mutex, or no value is published.
struct FlightGuard<'a> {
    cache: &'a AttachmentPreviewCache,
    key: FlightKey,
    flight: Arc<Flight>,
    completed: bool,
}

impl<'a> FlightGuard<'a> {
    fn new(cache: &'a AttachmentPreviewCache, key: FlightKey, flight: Arc<Flight>) -> Self {
        Self {
            cache,
            key,
            flight,
            completed: false,
        }
    }

    fn complete(mut self, value: Option<Arc<String>>) {
        self.finish(value);
    }

    fn finish(&mut self, value: Option<Arc<String>>) {
        *self
            .flight
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(value);
        self.cache
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .flights
            .remove(&self.key);
        self.completed = true;
        self.flight.ready.notify_all();
    }
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.finish(None);
        }
    }
}

#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
#[derive(Default)]
struct CacheStats {
    generations: AtomicU64,
    read_bytes: AtomicU64,
}

#[cfg(test)]
struct CountedReader<'a> {
    file: &'a mut File,
    stats: &'a CacheStats,
}

#[cfg(test)]
impl Read for CountedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let bytes = self.file.read(buffer)?;
        self.stats
            .read_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        Ok(bytes)
    }
}

#[cfg(test)]
impl Seek for CountedReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}

#[cfg(test)]
#[path = "tests/attachment_preview_cache.rs"]
mod tests;
