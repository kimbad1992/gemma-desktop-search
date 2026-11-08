//! ffmpeg/ffprobe로 영상 프레임·오디오 구간 추출

use crate::util::{hidden_command, run_capture};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

pub struct MediaInfo {
    pub duration: f64,
    pub has_video: bool,
    pub has_audio: bool,
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    format: Option<ProbeFormat>,
}
#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    #[serde(default)]
    disposition: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

pub fn probe(ffprobe: &Path, path: &Path) -> Result<MediaInfo> {
    let mut cmd = hidden_command(ffprobe);
    cmd.args([
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=codec_type:stream_disposition=attached_pic",
        "-of",
        "json",
    ])
    .arg(path);
    let out = run_capture(cmd, Duration::from_secs(30))?;
    if !out.status.success() {
        bail!(
            "ffprobe 실패: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let p: Probe = serde_json::from_slice(&out.stdout).context("ffprobe 출력 파싱 실패")?;
    let is_cover = |s: &ProbeStream| {
        s.disposition
            .as_ref()
            .and_then(|d| d.get("attached_pic"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            == 1
    };
    Ok(MediaInfo {
        duration: p
            .format
            .and_then(|f| f.duration)
            .and_then(|d| d.parse().ok())
            .unwrap_or(0.0),
        has_video: p
            .streams
            .iter()
            .any(|s| s.codec_type.as_deref() == Some("video") && !is_cover(s)),
        has_audio: p.streams.iter().any(|s| s.codec_type.as_deref() == Some("audio")),
    })
}

fn ffmpeg_stdout(ffmpeg: &Path, args: &[String], timeout: Duration) -> Result<Vec<u8>> {
    let mut cmd = hidden_command(ffmpeg);
    cmd.args(["-hide_banner", "-nostdin", "-v", "error"]).args(args);
    let out = run_capture(cmd, timeout)?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!(
            "ffmpeg 실패: {}",
            crate::util::truncate_chars(String::from_utf8_lossy(&out.stderr).trim(), 200)
        );
    }
    Ok(out.stdout)
}

/// 영상의 t초 지점 한 프레임을 JPEG로 (긴 변 896px 이하)
pub fn frame_at(ffmpeg: &Path, path: &Path, t: f64) -> Result<Vec<u8>> {
    let args = vec![
        "-ss".into(),
        format!("{t:.3}"),
        "-i".into(),
        path.display().to_string(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        "scale=896:896:force_original_aspect_ratio=decrease".into(),
        "-f".into(),
        "image2pipe".into(),
        "-c:v".into(),
        "mjpeg".into(),
        "-q:v".into(),
        "3".into(),
        "-".into(),
    ];
    ffmpeg_stdout(ffmpeg, &args, Duration::from_secs(60))
}

/// HEIC/AVIF 등 image 크레이트가 못 읽는 이미지를 JPEG로
pub fn image_to_jpeg(ffmpeg: &Path, path: &Path) -> Result<Vec<u8>> {
    let args = vec![
        "-i".into(),
        path.display().to_string(),
        "-frames:v".into(),
        "1".into(),
        "-f".into(),
        "image2pipe".into(),
        "-c:v".into(),
        "mjpeg".into(),
        "-q:v".into(),
        "2".into(),
        "-".into(),
    ];
    ffmpeg_stdout(ffmpeg, &args, Duration::from_secs(60))
}

/// start초부터 len초를 16kHz mono 16-bit WAV로. 0.5초 미만이면 None.
pub fn audio_segment(ffmpeg: &Path, path: &Path, start: f64, len: f64) -> Result<Option<Vec<u8>>> {
    let args = vec![
        "-ss".into(),
        format!("{start:.3}"),
        "-t".into(),
        format!("{len:.3}"),
        "-i".into(),
        path.display().to_string(),
        "-vn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        "16000".into(),
        "-f".into(),
        "s16le".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        "-".into(),
    ];
    let pcm = match ffmpeg_stdout(ffmpeg, &args, Duration::from_secs(120)) {
        Ok(p) => p,
        Err(e) if start > 0.0 => return Err(e),
        Err(e) => return Err(e),
    };
    if pcm.len() < 16_000 {
        return Ok(None);
    }
    Ok(Some(wav_from_s16le(&pcm, 16_000)))
}

/// raw s16le PCM → WAV (mono)
pub fn wav_from_s16le(pcm: &[u8], rate: u32) -> Vec<u8> {
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(pcm);
    w
}

/// 길이 dur인 미디어에서 n개 지점을 고르게 (구간 중앙)
pub fn spread(dur: f64, n: usize) -> Vec<f64> {
    if dur <= 0.0 || n == 0 {
        return vec![0.0];
    }
    (0..n).map(|i| dur * (i as f64 + 0.5) / n as f64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_sizes() {
        let pcm = vec![0u8; 3200];
        let w = wav_from_s16le(&pcm, 16000);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 3200);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 3200);
        assert_eq!(w.len(), 44 + 3200);
    }

    #[test]
    fn spread_points() {
        assert_eq!(spread(10.0, 2), vec![2.5, 7.5]);
        assert_eq!(spread(0.0, 5), vec![0.0]);
    }
}
