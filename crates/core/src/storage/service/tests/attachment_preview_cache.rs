use super::*;
use base64::Engine;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use std::fs;
use std::io::Cursor;
use std::sync::Barrier;
use std::time::Instant;

fn png(seed: u8, width: u32, height: u32) -> Vec<u8> {
    let image = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([
            (x % 255) as u8 ^ seed,
            (y % 255) as u8,
            seed,
            if x < width / 2 { 0 } else { 255 },
        ])
    });
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut output, ImageFormat::Png)
        .unwrap();
    output.into_inner()
}

fn original_preview(path: &Path, stats: &CacheStats) -> String {
    let mut file = File::open(path).unwrap();
    thumbnail_data_url(BufReader::new(CountedReader {
        file: &mut file,
        stats,
    }))
    .unwrap()
    .strip_prefix("data:image/png;base64,")
    .unwrap()
    .to_string()
}

#[test]
fn preview_hot_read_does_not_read_or_decode_original_and_matches_transparent_thumbnail() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("transparent.png");
    fs::write(&path, png(19, 640, 320)).unwrap();
    let cache = AttachmentPreviewCache::default();
    let first = cache.read(&path).unwrap();
    assert_eq!(first, original_preview(&path, &CacheStats::default()));
    let reads = cache.stats.read_bytes.load(Ordering::Relaxed);
    assert!(reads > 0);
    for _ in 0..10 {
        assert_eq!(cache.read(&path).as_deref(), Some(first.as_str()));
    }
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 1);
    assert_eq!(cache.stats.read_bytes.load(Ordering::Relaxed), reads);
    let thumbnail = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(first)
            .unwrap(),
    )
    .unwrap()
    .to_rgba8();
    assert_eq!(thumbnail.dimensions(), (256, 128));
    assert_eq!(thumbnail.get_pixel(0, 0)[3], 0);
    assert_eq!(thumbnail.get_pixel(255, 127)[3], 255);
}

#[test]
fn ten_parallel_requests_share_hash_read_and_thumbnail_generation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("concurrent.png");
    let bytes = png(25, 800, 600);
    fs::write(&path, &bytes).unwrap();
    let expected_stats = CacheStats::default();
    let expected = original_preview(&path, &expected_stats);
    let cache = AttachmentPreviewCache::default();
    let barrier = Barrier::new(10);
    std::thread::scope(|scope| {
        let handles = (0..10)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    cache.read(&path)
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            assert_eq!(handle.join().unwrap().as_deref(), Some(expected.as_str()));
        }
    });
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 1);
    assert_eq!(
        cache.stats.read_bytes.load(Ordering::Relaxed),
        bytes.len() as u64 + expected_stats.read_bytes.load(Ordering::Relaxed)
    );
    assert!(cache.state.lock().unwrap().flights.is_empty());
}

#[test]
fn duplicate_content_at_different_paths_shares_one_generation() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = png(32, 640, 480);
    let paths = (0..10)
        .map(|index| {
            let path = directory.path().join(format!("copy-{index}.png"));
            fs::write(&path, &bytes).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let cache = AttachmentPreviewCache::default();
    let barrier = Barrier::new(10);
    std::thread::scope(|scope| {
        for path in &paths {
            let cache = &cache;
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                assert!(cache.read(path).is_some());
            });
        }
    });
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 1);
    assert_eq!(cache.state.lock().unwrap().previews.len(), 1);
    fs::remove_file(&paths[0]).unwrap();
    assert!(cache.read(&paths[0]).is_none());
    assert!(cache.read(&paths[1]).is_some());
}

#[test]
fn failed_generation_is_retryable_and_does_not_leave_inflight_or_cached_value() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("broken.png");
    fs::write(&path, b"not an image").unwrap();
    let cache = AttachmentPreviewCache::default();
    for _ in 0..2 {
        assert!(cache.read(&path).is_none());
        let state = cache.state.lock().unwrap();
        assert!(state.flights.is_empty());
        assert!(state.previews.is_empty());
        assert!(state.sources.is_empty());
    }
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 2);
    fs::write(&path, png(3, 64, 32)).unwrap();
    assert!(cache.read(&path).is_some());
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 3);
}

#[test]
fn dropping_generation_guard_releases_waiters_and_allows_retry() {
    let cache = AttachmentPreviewCache::default();
    let key = FlightKey::Content(ContentKey {
        digest: [9; 32],
        variant: PreviewVariant {
            version: 1,
            max_edge: 256,
        },
    });
    let flight = Arc::new(Flight::default());
    cache
        .state
        .lock()
        .unwrap()
        .flights
        .insert(key.clone(), flight.clone());
    let guard = FlightGuard::new(&cache, key, flight.clone());
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| flight.wait());
        drop(guard);
        assert!(waiter.join().unwrap().is_none());
    });
    assert!(cache.state.lock().unwrap().flights.is_empty());
}

#[test]
fn deletion_restore_replacement_and_restart_do_not_return_old_preview() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("picture.png");
    let cache = AttachmentPreviewCache::default();
    fs::write(&path, png(10, 32, 32)).unwrap();
    let original = cache.read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    assert!(cache.read(&path).is_none());
    fs::write(&path, png(11, 32, 32)).unwrap();
    let restored = cache.read(&path).unwrap();
    assert_ne!(restored, original);
    let replacement = directory.path().join("replacement.png");
    fs::write(&replacement, png(12, 32, 32)).unwrap();
    fs::rename(replacement, &path).unwrap();
    let replaced = cache.read(&path).unwrap();
    assert_ne!(replaced, restored);
    cache.invalidate(&path);
    assert!(cache.state.lock().unwrap().sources.is_empty());
    assert_eq!(cache.state.lock().unwrap().bytes, 0);
    assert_eq!(cache.read(&path).unwrap(), replaced);
    let reopened = AttachmentPreviewCache::default();
    assert_eq!(reopened.read(&path).unwrap(), replaced);
    assert_eq!(reopened.stats.generations.load(Ordering::Relaxed), 1);
}

#[cfg(unix)]
#[test]
fn in_place_edit_with_preserved_mtime_and_permission_changes_invalidate_identity() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("picture.png");
    // Uncompressed BMP is unsupported; fixed-dimension PNGs can differ in encoded length.
    // A trailing unused byte preserves size while changing content without changing pixels.
    let mut source = png(50, 32, 32);
    source.push(0);
    fs::write(&path, &source).unwrap();
    let cache = AttachmentPreviewCache::default();
    assert!(cache.read(&path).is_some());
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    *source.last_mut().unwrap() = 1;
    fs::write(&path, source).unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(cache.read(&path).is_some());
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 2);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    // A root-run test can still open mode 000, matching the OS authorization outcome.
    if File::open(&path).is_err() {
        assert!(cache.read(&path).is_none());
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(cache.read(&path).is_some());
}

#[test]
fn variant_identity_includes_algorithm_version_and_size() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("picture.png");
    fs::write(&path, png(1, 32, 32)).unwrap();
    let cache = AttachmentPreviewCache::default();
    for variant in [
        PreviewVariant {
            version: 1,
            max_edge: 256,
        },
        PreviewVariant {
            version: 2,
            max_edge: 256,
        },
        PreviewVariant {
            version: 2,
            max_edge: 128,
        },
    ] {
        assert!(cache.read_variant(&path, variant).is_some());
        assert!(cache.read_variant(&path, variant).is_some());
    }
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 3);
}

#[test]
fn lru_evicts_by_retained_encoded_bytes_and_large_values_are_not_retained() {
    let directory = tempfile::tempdir().unwrap();
    let paths = (0..3)
        .map(|index| {
            let path = directory.path().join(format!("picture-{index}.png"));
            fs::write(&path, png(index, 64, 32)).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let sizes = paths
        .iter()
        .map(|path| original_preview(path, &CacheStats::default()).len())
        .collect::<Vec<_>>();
    let budget = sizes.iter().copied().max().unwrap() * 2;
    let cache = AttachmentPreviewCache::with_budget(budget);
    cache.read(&paths[0]).unwrap();
    cache.read(&paths[1]).unwrap();
    cache.read(&paths[0]).unwrap(); // second picture is least recently used
    cache.read(&paths[2]).unwrap();
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 3);
    assert!(cache.state.lock().unwrap().bytes <= budget);
    cache.read(&paths[0]).unwrap();
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 3);
    cache.read(&paths[1]).unwrap();
    assert_eq!(cache.stats.generations.load(Ordering::Relaxed), 4);
    let tiny = AttachmentPreviewCache::with_budget(1);
    assert!(tiny.read(&paths[0]).is_some());
    assert_eq!(tiny.state.lock().unwrap().bytes, 0);
    assert!(tiny.state.lock().unwrap().sources.is_empty());
}

#[test]
fn gif_first_frame_and_small_image_semantics_are_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("animation.gif");
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        for color in [Rgba([255, 0, 0, 255]), Rgba([0, 0, 255, 255])] {
            encoder
                .encode_frame(image::Frame::new(RgbaImage::from_pixel(32, 16, color)))
                .unwrap();
        }
    }
    fs::write(&path, &bytes).unwrap();
    let cache = AttachmentPreviewCache::default();
    let preview = cache.read(&path).unwrap();
    assert_eq!(preview, original_preview(&path, &CacheStats::default()));
    assert_eq!(cache.read(&path).unwrap(), preview);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let image = image::load_from_memory(
        &base64::engine::general_purpose::STANDARD
            .decode(preview)
            .unwrap(),
    )
    .unwrap();
    assert_eq!((image.width(), image.height()), (32, 16));
    assert_eq!(image.to_rgba8().get_pixel(0, 0), &Rgba([255, 0, 0, 255]));
}

#[cfg(unix)]
fn cpu_micros() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the output when it returns zero.
    unsafe {
        assert_eq!(libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()), 0);
        let usage = usage.assume_init();
        (usage.ru_utime.tv_sec as u64 + usage.ru_stime.tv_sec as u64) * 1_000_000
            + usage.ru_utime.tv_usec as u64
            + usage.ru_stime.tv_usec as u64
    }
}

/// Opt-in synthetic benchmark; never reads the real data directory.
#[cfg(unix)]
#[test]
fn attachment_preview_cache_benchmark() {
    if std::env::var_os("MYCOPILOT_ATTACHMENT_PREVIEW_BENCH").is_none() {
        return;
    }
    for count in [20, 100] {
        let directory = tempfile::tempdir().unwrap();
        let paths = (0..count)
            .map(|index| {
                let path = directory.path().join(format!("picture-{index}.png"));
                fs::write(&path, png(index as u8, 1280, 720)).unwrap();
                path
            })
            .collect::<Vec<_>>();
        let cache = AttachmentPreviewCache::default();
        for phase in ["baseline", "cold", "hot"] {
            let stats = CacheStats::default();
            let reads_before = cache.stats.read_bytes.load(Ordering::Relaxed);
            let decodes_before = cache.stats.generations.load(Ordering::Relaxed);
            let started = Instant::now();
            let cpu_before = cpu_micros();
            for path in &paths {
                let preview = if phase == "baseline" {
                    original_preview(path, &stats)
                } else {
                    cache.read(path).unwrap()
                };
                assert!(!preview.is_empty());
            }
            let cpu = cpu_micros() - cpu_before;
            let elapsed = started.elapsed().as_micros();
            let (reads, decodes) = if phase == "baseline" {
                (stats.read_bytes.load(Ordering::Relaxed), count as u64)
            } else {
                (
                    cache.stats.read_bytes.load(Ordering::Relaxed) - reads_before,
                    cache.stats.generations.load(Ordering::Relaxed) - decodes_before,
                )
            };
            let bytes = cache.state.lock().unwrap().bytes;
            assert!(bytes <= PREVIEW_BYTE_BUDGET);
            if phase == "hot" {
                assert_eq!((reads, decodes), (0, 0));
            }
            eprintln!("preview_cache count={count} phase={phase} elapsed_us={elapsed} cpu_us={cpu} file_read_bytes={reads} generations={decodes} retained_bytes={bytes} budget_bytes={PREVIEW_BYTE_BUDGET}");
        }
    }
}
