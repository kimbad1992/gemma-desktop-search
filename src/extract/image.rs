//! 이미지 로드(EXIF 회전 반영), 모델 입력용 JPEG 및 썸네일 생성

use anyhow::{Context, Result};
use image::{DynamicImage, ImageDecoder};
use std::path::Path;

/// 모델에 보내는 이미지의 긴 변 최대 크기
pub const MODEL_MAX_SIDE: u32 = 896;
pub const THUMB_SIDE: u32 = 160;

pub fn load_image(path: &Path, ffmpeg: Option<&Path>) -> Result<DynamicImage> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if matches!(ext.as_str(), "heic" | "heif" | "avif") {
        let ff = ffmpeg.context("HEIC/AVIF는 ffmpeg가 필요합니다")?;
        let jpeg = super::media::image_to_jpeg(ff, path)?;
        return Ok(image::load_from_memory(&jpeg)?);
    }
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut decoder = reader.into_decoder().context("이미지 디코더 생성 실패")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).context("이미지 디코딩 실패")?;
    img.apply_orientation(orientation);
    Ok(img)
}

pub fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut buf = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    rgb.write_with_encoder(enc)?;
    Ok(buf)
}

fn fit(img: &DynamicImage, side: u32) -> DynamicImage {
    if img.width().max(img.height()) > side {
        img.resize(side, side, image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    }
}

/// (모델 입력 JPEG, 썸네일 JPEG)
pub fn model_and_thumb(img: &DynamicImage) -> Result<(Vec<u8>, Vec<u8>)> {
    let model = fit(img, MODEL_MAX_SIDE);
    let thumb = model.thumbnail(THUMB_SIDE, THUMB_SIDE);
    Ok((encode_jpeg(&model, 90)?, encode_jpeg(&thumb, 80)?))
}
