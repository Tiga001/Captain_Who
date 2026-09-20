//! Bounded visual derivatives. Originals remain in their attachment/artifact store.
use crate::protocol::{AgentError, AgentResult};
use base64::Engine;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits};
use std::io::{BufRead, Seek};

pub(crate) const MODEL_IMAGE_MAX_EDGE: u32 = 2048;
const MAX_DECODE_PIXELS: u64 = 40_000_000;
const MAX_DECODE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DECODE_EDGE: u32 = 32_768;
const MAX_THUMBNAIL_DATA_URL_BYTES: usize = 192 * 1024;
// Bound aggregate decoding memory across imports, direct reads, and history hydration.
static IMAGE_DECODE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) struct PreparedModelImage {
    pub bytes: Vec<u8>,
    pub mime_type: &'static str,
    pub source_format: &'static str,
    pub source_mime_type: &'static str,
    pub original_width: u32,
    pub original_height: u32,
    pub width: u32,
    pub height: u32,
    pub thumbnail_data_url: String,
}

pub(crate) fn prepare_model_image(reader: impl BufRead + Seek) -> AgentResult<PreparedModelImage> {
    let _permit = IMAGE_DECODE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (mut image, source_format, source_mime_type, orientation) = decode(reader)?;
    let (original_width, original_height) = (image.width(), image.height());
    image.apply_orientation(orientation);
    let thumbnail_data_url = thumbnail_from_image(&image)?;
    let mut derived = if image.width().max(image.height()) > MODEL_IMAGE_MAX_EDGE {
        image.thumbnail(MODEL_IMAGE_MAX_EDGE, MODEL_IMAGE_MAX_EDGE)
    } else {
        image
    };
    // PNG preserves transparency and text. Downscale noisy images further to bound wire bytes.
    loop {
        let bytes = encode_png(&derived)?;
        if bytes.len() as u64 <= super::MAX_AGENT_VISUAL_INPUT_BYTES {
            return Ok(PreparedModelImage {
                bytes,
                mime_type: "image/png",
                source_format,
                source_mime_type,
                original_width,
                original_height,
                width: derived.width(),
                height: derived.height(),
                thumbnail_data_url,
            });
        }
        let edge = derived.width().max(derived.height()) * 3 / 4;
        if edge < 64 {
            return Err(AgentError::new("无法生成符合模型图片预算的预览。"));
        }
        derived = derived.thumbnail(edge, edge);
    }
}

pub(crate) fn thumbnail_data_url(reader: impl BufRead + Seek) -> AgentResult<String> {
    let _permit = IMAGE_DECODE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (mut image, _, _, orientation) = decode(reader)?;
    image.apply_orientation(orientation);
    thumbnail_from_image(&image)
}

fn decode(
    reader: impl BufRead + Seek,
) -> AgentResult<(DynamicImage, &'static str, &'static str, Orientation)> {
    let mut reader = ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|_| AgentError::new("无法读取图片格式。"))?;
    let (format, mime_type) = match reader.format() {
        Some(ImageFormat::Png) => ("png", "image/png"),
        Some(ImageFormat::Jpeg) => ("jpeg", "image/jpeg"),
        Some(ImageFormat::Gif) => ("gif", "image/gif"),
        Some(ImageFormat::WebP) => ("webp", "image/webp"),
        _ => {
            return Err(AgentError::new(
                "不支持的图片类型。支持：PNG、JPEG、GIF、WebP。",
            ))
        }
    };
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_EDGE);
    limits.max_image_height = Some(MAX_DECODE_EDGE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| decode_error())?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS
        || decoder.total_bytes() > MAX_DECODE_BYTES
    {
        return Err(decode_error());
    }
    let orientation = decoder.orientation().map_err(|_| decode_error())?;
    let decoded = DynamicImage::from_decoder(decoder).map_err(|_| decode_error())?;
    Ok((decoded, format, mime_type, orientation))
}

fn decode_error() -> AgentError {
    AgentError::structured("agent.image_decode_limit", "图片无法安全解码：文件可能损坏，或像素/解码内存超过上限。原文件已保留，可先裁剪或缩小图片后重试。", serde_json::json!({"maxPixels":MAX_DECODE_PIXELS,"maxDecodedBytes":MAX_DECODE_BYTES}))
}

fn encode_png(image: &DynamicImage) -> AgentResult<Vec<u8>> {
    let image = image.to_rgba8();
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|_| AgentError::new("图片预览编码失败。"))?;
    Ok(bytes)
}

fn thumbnail_from_image(image: &DynamicImage) -> AgentResult<String> {
    let mut edge = 256;
    loop {
        let bytes = if image.width().max(image.height()) > edge {
            encode_png(&image.thumbnail(edge, edge))?
        } else {
            encode_png(image)?
        };
        let value = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        if value.len() <= MAX_THUMBNAIL_DATA_URL_BYTES {
            return Ok(value);
        }
        edge = edge * 3 / 4;
        if edge < 16 {
            return Err(AgentError::new("图片缩略图超过预算。"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn model_derivative_resizes_original_and_bounds_preview() {
        let image = DynamicImage::new_rgb8(3000, 1200);
        let source = encode_png(&image).unwrap();
        let prepared = prepare_model_image(Cursor::new(&source)).unwrap();
        assert_eq!(
            (prepared.original_width, prepared.original_height),
            (3000, 1200)
        );
        assert_eq!((prepared.width, prepared.height), (2048, 819));
        assert!(prepared.bytes.len() as u64 <= super::super::MAX_AGENT_VISUAL_INPUT_BYTES);
        assert!(prepared.thumbnail_data_url.len() <= MAX_THUMBNAIL_DATA_URL_BYTES);
        assert_eq!(image::load_from_memory(&source).unwrap().width(), 3000);
    }

    #[test]
    fn jpeg_orientation_is_baked_into_derivative_without_upscaling() {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(
                &[255, 0, 0, 0, 0, 255],
                2,
                1,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut segment = vec![0xff, 0xe1];
        segment.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        segment.extend_from_slice(exif);
        jpeg.splice(2..2, segment);
        let prepared = prepare_model_image(Cursor::new(jpeg)).unwrap();
        assert_eq!((prepared.original_width, prepared.original_height), (2, 1));
        assert_eq!((prepared.width, prepared.height), (1, 2));
        let thumbnail = prepared
            .thumbnail_data_url
            .strip_prefix("data:image/png;base64,")
            .unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(thumbnail)
            .unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (1, 2));
    }

    #[test]
    fn oversized_header_is_rejected_before_image_allocation() {
        // Valid PNG header for an extreme canvas; no pixel buffer is allocated by this test.
        let mut bytes = encode_png(&DynamicImage::new_rgb8(1, 1)).unwrap();
        bytes[16..20].copy_from_slice(&10_000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&5_000u32.to_be_bytes());
        let mut crc = !0u32;
        for byte in &bytes[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
            }
        }
        bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
        assert_eq!(
            ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .unwrap()
                .into_dimensions()
                .unwrap(),
            (10_000, 5_000)
        );
        let error = prepare_model_image(Cursor::new(bytes)).err().unwrap();
        assert_eq!(error.code(), Some("agent.image_decode_limit"));
    }
}
