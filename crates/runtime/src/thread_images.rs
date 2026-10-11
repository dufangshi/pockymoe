//! Downscaled previews of thread photo attachments.
//!
//! Chat chips show photos at 6x4.5 rem; full phone screenshots are 0.2-0.8 MB
//! each. A thumbnail's longest side is [`THUMBNAIL_MAX_SIDE`] pixels, which
//! covers a chip at 3x density with headroom (the chip needs at most 288 device
//! pixels), and is encoded as JPEG, or PNG when the image is transparent. The
//! `image` crate only encodes lossless WebP, which is larger than JPEG here.
//!
//! Thumbnails are cached under the Supervisor data directory, never in the
//! workspace, keyed by the canonical source path, its size and mtime. When a
//! source cannot be decoded (GIF, HEIC, corrupt data) or the thumbnail would not
//! be smaller, a marker is cached instead and the caller serves the original.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::UNIX_EPOCH;

use anyhow::Result;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, Limits};
use sha2::{Digest, Sha256};

use crate::files::ThreadImageFile;

pub const THUMBNAIL_MAX_SIDE: u32 = 480;
const JPEG_QUALITY: u8 = 75;
/// Bump when the output changes, so stale cache entries are not reused.
const CACHE_VERSION: u32 = 1;
const CACHE_MAX_ENTRIES: usize = 4_000;
const CACHE_PRUNE_TO: usize = 3_000;
/// Large enough for a 48 MP photo's RGB buffer; bigger images fall back.
const DECODE_MAX_ALLOC: u64 = 192 * 1024 * 1024;
const DECODE_MAX_SIDE: u32 = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

pub fn cache_dir(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .join("cache")
        .join("thread-thumbnails")
}

/// A validator for one representation of one version of the source file.
pub fn etag(image: &ThreadImageFile, variant: &str) -> String {
    let modified = image
        .modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("W/\"{variant}-{:x}-{modified:x}\"", image.len)
}

fn cache_key(image: &ThreadImageFile) -> String {
    let mut hash = Sha256::new();
    hash.update(format!("v{CACHE_VERSION}\0{THUMBNAIL_MAX_SIDE}\0").as_bytes());
    hash.update(image.path.as_os_str().as_encoded_bytes());
    hash.update(etag(image, "").as_bytes());
    hex::encode(&hash.finalize()[..16])
}

fn cache_names(key: &str) -> [(String, Option<&'static str>); 3] {
    [
        (format!("{key}.orig"), None),
        (format!("{key}.jpg"), Some("image/jpeg")),
        (format!("{key}.png"), Some("image/png")),
    ]
}

/// Whether a thumbnail (or the decision to serve the original) is cached.
pub fn is_cached(image: &ThreadImageFile, dir: &Path) -> bool {
    cache_names(&cache_key(image))
        .iter()
        .any(|(name, _)| dir.join(name).is_file())
}

fn cached(image: &ThreadImageFile, dir: &Path) -> Option<CacheEntry> {
    for (name, mime) in cache_names(&cache_key(image)) {
        match mime {
            None if dir.join(&name).is_file() => return Some(CacheEntry::Original),
            None => {}
            Some(mime) => {
                if let Ok(bytes) = std::fs::read(dir.join(&name)) {
                    return Some(CacheEntry::Thumbnail(Thumbnail { bytes, mime }));
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CacheEntry {
    Thumbnail(Thumbnail),
    /// Serve the original: not decodable, or already small.
    Original,
}

/// The cached or newly built thumbnail of `image`, or the original bytes when
/// no smaller thumbnail can be made. Cache write failures only cost a rebuild.
pub fn thumbnail_or_original(
    image: &mut ThreadImageFile,
    dir: &Path,
) -> Result<(Vec<u8>, &'static str)> {
    match cached(image, dir) {
        Some(CacheEntry::Thumbnail(thumbnail)) => return Ok((thumbnail.bytes, thumbnail.mime)),
        Some(CacheEntry::Original) => return Ok((image.read_all()?, image.mime)),
        None => {}
    }
    let original = image.read_all()?;
    Ok(match build_and_cache(image, &original, dir) {
        Some(thumbnail) => (thumbnail.bytes, thumbnail.mime),
        None => (original, image.mime),
    })
}

fn build_and_cache(image: &ThreadImageFile, original: &[u8], dir: &Path) -> Option<Thumbnail> {
    let generated = match generate(image.mime, original) {
        Ok(generated) => generated.filter(|thumbnail| thumbnail.bytes.len() < original.len()),
        Err(error) => {
            tracing::debug!(path = %image.path.display(), %error, "thread image thumbnail failed");
            None
        }
    };
    let key = cache_key(image);
    let (name, bytes) = match &generated {
        Some(thumbnail) if thumbnail.mime == "image/png" => {
            (format!("{key}.png"), &thumbnail.bytes[..])
        }
        Some(thumbnail) => (format!("{key}.jpg"), &thumbnail.bytes[..]),
        None => (format!("{key}.orig"), &b""[..]),
    };
    if let Err(error) = std::fs::create_dir_all(dir)
        .map_err(anyhow::Error::from)
        .and_then(|()| write_atomic(dir, &name, bytes))
    {
        tracing::warn!(dir = %dir.display(), %error, "could not cache a thread image thumbnail");
    }
    static WRITES: AtomicU32 = AtomicU32::new(0);
    if WRITES.fetch_add(1, Ordering::Relaxed) % 64 == 0 {
        prune(dir, CACHE_MAX_ENTRIES, CACHE_PRUNE_TO);
    }
    generated
}

fn generate(mime: &str, original: &[u8]) -> Result<Option<Thumbnail>> {
    if !matches!(mime, "image/jpeg" | "image/png" | "image/webp") {
        return Ok(None);
    }
    let mut decoder = ImageReader::new(Cursor::new(original))
        .with_guessed_format()?
        .into_decoder()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(DECODE_MAX_ALLOC);
    limits.max_image_width = Some(DECODE_MAX_SIDE);
    limits.max_image_height = Some(DECODE_MAX_SIDE);
    decoder.set_limits(limits)?;
    let (width, height) = decoder.dimensions();
    if width.max(height) <= THUMBNAIL_MAX_SIDE {
        return Ok(None);
    }
    let orientation = decoder.orientation()?;
    let icc = decoder.icc_profile().ok().flatten();
    let mut decoded = DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);
    let small = decoded.thumbnail(THUMBNAIL_MAX_SIDE, THUMBNAIL_MAX_SIDE);
    drop(decoded);
    let mut bytes = Vec::new();
    let transparent =
        small.color().has_alpha() && small.to_rgba8().pixels().any(|pixel| pixel.0[3] != u8::MAX);
    if transparent {
        let rgba = small.to_rgba8();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        if let Some(icc) = icc {
            let _ = encoder.set_icc_profile(icc);
        }
        encoder.write_image(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        return Ok(Some(Thumbnail {
            bytes,
            mime: "image/png",
        }));
    }
    let rgb = small.to_rgb8();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY);
    if let Some(icc) = icc {
        let _ = encoder.set_icc_profile(icc);
    }
    encoder.write_image(
        rgb.as_raw(),
        rgb.width(),
        rgb.height(),
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(Some(Thumbnail {
        bytes,
        mime: "image/jpeg",
    }))
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.persist(dir.join(name))?;
    Ok(())
}

/// Drop the oldest entries once the cache holds more than `max` files.
fn prune(dir: &Path, max: usize, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| (metadata.modified().unwrap_or(UNIX_EPOCH), entry.path()))
        })
        .collect();
    if files.len() <= max {
        return;
    }
    files.sort();
    for (_, path) in files.iter().take(files.len() - keep) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::open_thread_image;
    use image::{ImageBuffer, Rgb, Rgba};

    fn workspace() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let attachments = dir.path().join("ws/.temp/threads/t1");
        std::fs::create_dir_all(&attachments).unwrap();
        (dir, attachments)
    }

    fn noisy_jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x * 7 ^ y * 13) as u8, (x * y) as u8, (x + y * 3) as u8])
        });
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95)
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    fn png(image: &DynamicImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn open(root: &Path, name: &str) -> (ThreadImageFile, Vec<u8>) {
        let rel = format!(".temp/threads/t1/{name}");
        let original = std::fs::read(root.join("ws").join(&rel)).unwrap();
        (
            open_thread_image(&root.join("ws"), "t1", &rel).unwrap(),
            original,
        )
    }

    #[test]
    fn thumbnail_downscales_a_large_photo_and_caches_it() {
        let (dir, attachments) = workspace();
        std::fs::write(
            attachments.join("shot-0a1b2c3d.jpg"),
            noisy_jpeg(1264, 2780),
        )
        .unwrap();
        let cache = dir.path().join("data/cache/thread-thumbnails");
        let (mut image, original) = open(dir.path(), "shot-0a1b2c3d.jpg");
        assert!(image.immutable_name);
        assert!(!is_cached(&image, &cache));
        let (bytes, mime) = thumbnail_or_original(&mut image, &cache).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(
            bytes.len() * 5 < original.len(),
            "{} vs {}",
            bytes.len(),
            original.len()
        );
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (218, 480));
        assert!(is_cached(&image, &cache));
        assert_eq!(
            cached(&image, &cache),
            Some(CacheEntry::Thumbnail(Thumbnail {
                bytes: bytes.clone(),
                mime
            }))
        );
        // A hit serves the cached bytes, even if the cache entry is the only copy.
        let (mut again, _) = open(dir.path(), "shot-0a1b2c3d.jpg");
        assert_eq!(thumbnail_or_original(&mut again, &cache).unwrap().0, bytes);
        // The workspace itself gains no files.
        assert_eq!(std::fs::read_dir(&attachments).unwrap().count(), 1);
        // Changing the source invalidates the entry and the validator.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(attachments.join("shot-0a1b2c3d.jpg"), noisy_jpeg(1000, 600)).unwrap();
        let (mut changed, _) = open(dir.path(), "shot-0a1b2c3d.jpg");
        assert!(!is_cached(&changed, &cache));
        assert_ne!(etag(&changed, "thumbnail"), etag(&image, "thumbnail"));
        let (bytes, _) = thumbnail_or_original(&mut changed, &cache).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (480, 288));
    }

    #[test]
    fn exif_orientation_is_applied_to_the_thumbnail() {
        let (dir, attachments) = workspace();
        let (width, height) = (1600, 900);
        let pixels = ImageBuffer::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, 7])
        });
        // Big-endian TIFF with one IFD entry: Orientation (0x0112) = 6, "rotate 90 CW".
        let exif = b"MM\x00\x2a\x00\x00\x00\x08\x00\x01\x01\x12\x00\x03\x00\x00\x00\x01\x00\x06\x00\x00\x00\x00\x00\x00".to_vec();
        let mut bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90);
        encoder.set_exif_metadata(exif).unwrap();
        encoder
            .write_image(
                pixels.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        std::fs::write(attachments.join("rotated.jpg"), bytes).unwrap();
        let (mut file, _) = open(dir.path(), "rotated.jpg");
        let (bytes, _) = thumbnail_or_original(&mut file, &dir.path().join("cache")).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (270, 480));
    }

    #[test]
    fn transparent_images_stay_png() {
        let (dir, attachments) = workspace();
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(1200, 600, |x, y| {
            Rgba([
                (x % 251) as u8,
                (y % 241) as u8,
                ((x * y) % 239) as u8,
                if x < 600 { 0 } else { 255 },
            ])
        }));
        std::fs::write(attachments.join("alpha.png"), png(&image)).unwrap();
        let (mut file, _) = open(dir.path(), "alpha.png");
        assert!(!file.immutable_name);
        let (bytes, mime) = thumbnail_or_original(&mut file, &dir.path().join("cache")).unwrap();
        assert_eq!(mime, "image/png");
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (480, 240));
        assert_eq!(decoded.to_rgba8().get_pixel(10, 10).0[3], 0);
    }

    #[test]
    fn small_undecodable_and_unsupported_images_fall_back_to_the_original() {
        let (dir, attachments) = workspace();
        let cache = dir.path().join("cache");
        std::fs::write(attachments.join("small.jpg"), noisy_jpeg(320, 200)).unwrap();
        std::fs::write(
            attachments.join("broken.jpg"),
            b"\xff\xd8\xffnot really a jpeg",
        )
        .unwrap();
        std::fs::write(attachments.join("anim.gif"), b"GIF89a\x01\x00\x01\x00").unwrap();
        for name in ["small.jpg", "broken.jpg", "anim.gif"] {
            let (mut file, original) = open(dir.path(), name);
            let mime = file.mime;
            assert_eq!(
                thumbnail_or_original(&mut file, &cache).unwrap(),
                (original.clone(), mime),
                "{name}"
            );
            assert_eq!(cached(&file, &cache), Some(CacheEntry::Original), "{name}");
            assert_eq!(
                thumbnail_or_original(&mut file, &cache).unwrap(),
                (original, mime),
                "{name}"
            );
        }
    }

    #[test]
    fn an_unwritable_cache_still_serves_the_thumbnail() {
        let (dir, attachments) = workspace();
        std::fs::write(attachments.join("big.jpg"), noisy_jpeg(1600, 900)).unwrap();
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"file").unwrap();
        let (mut file, _) = open(dir.path(), "big.jpg");
        let (bytes, mime) = thumbnail_or_original(&mut file, &blocker.join("cache")).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 480);
    }

    #[test]
    fn prune_keeps_the_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..6 {
            std::fs::write(dir.path().join(format!("{index}.jpg")), b"x").unwrap();
            let file = std::fs::File::options()
                .write(true)
                .open(dir.path().join(format!("{index}.jpg")))
                .unwrap();
            file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_000 + index))
                .unwrap();
        }
        prune(dir.path(), 10, 3);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 6);
        prune(dir.path(), 5, 3);
        let mut left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["3.jpg", "4.jpg", "5.jpg"]);
    }

    #[test]
    fn upload_names_are_recognised_as_immutable() {
        let (dir, attachments) = workspace();
        std::fs::create_dir_all(attachments.join("downloads")).unwrap();
        let bytes = noisy_jpeg(16, 16);
        for name in [
            "IMG_1-0a1b2c3d.jpg",
            "photo-ffffffff",
            "plain.jpg",
            "x-0A1B2C3D.jpg",
            "downloads/a-0a1b2c3d.jpg",
        ] {
            std::fs::write(attachments.join(name), &bytes).unwrap();
        }
        let immutable = |name: &str| open(dir.path(), name).0.immutable_name;
        assert!(immutable("IMG_1-0a1b2c3d.jpg"));
        assert!(immutable("photo-ffffffff"));
        assert!(!immutable("plain.jpg"));
        assert!(!immutable("x-0A1B2C3D.jpg"));
        assert!(!immutable("downloads/a-0a1b2c3d.jpg"));
    }
}
