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

/// 영상의 start초부터 len초 구간을 작은 mp4로 (1fps, 긴 변 768px, 소리 없음).
/// 서버가 이 구간을 영상 그대로 받아 한 벡터로 임베딩한다.
pub fn video_segment(ffmpeg: &Path, path: &Path, start: f64, len: f64) -> Result<Vec<u8>> {
    let args = vec![
        "-ss".into(),
        format!("{start:.3}"),
        "-t".into(),
        format!("{len:.3}"),
        "-i".into(),
        path.display().to_string(),
        "-an".into(),
        "-vf".into(),
        "fps=1,scale=768:768:force_original_aspect_ratio=decrease:force_divisible_by=2".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "26".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        // 파이프로 내보내려면 조각난(fragmented) mp4여야 한다
        "-movflags".into(),
        "frag_keyframe+empty_moov".into(),
        "-f".into(),
        "mp4".into(),
        "-".into(),
    ];
    ffmpeg_stdout(ffmpeg, &args, Duration::from_secs(180))
}

/// 서버의 영상 입력 지원 확인용 2초짜리 시험 영상
pub fn test_clip(ffmpeg: &Path) -> Result<Vec<u8>> {
    let args: Vec<String> = [
        "-f", "lavfi", "-i", "testsrc=s=320x240:d=2:r=5", "-vf", "fps=1", "-c:v", "libx264",
        "-pix_fmt", "yuv420p", "-movflags", "frag_keyframe+empty_moov", "-f", "mp4", "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    ffmpeg_stdout(ffmpeg, &args, Duration::from_secs(30))
}

/// 길이 dur인 영상을 최대 max개의 seg초 구간으로: (시작, 길이) 목록.
/// 전체를 덮을 수 있으면 이어 붙이고, 너무 길면 고르게 분산한다.
pub fn segments(dur: f64, seg: f64, max: usize) -> Vec<(f64, f64)> {
    let seg = seg.max(1.0);
    if dur <= 0.0 {
        return vec![(0.0, seg)];
    }
    let n = ((dur / seg).ceil() as usize).clamp(1, max.max(1));
    if dur <= n as f64 * seg {
        (0..n)
            .map(|i| {
                let s = i as f64 * seg;
                (s, seg.min(dur - s))
            })
            .filter(|(_, l)| *l >= 1.0)
            .collect()
    } else {
        (0..n).map(|i| (dur * i as f64 / n as f64, seg)).collect()
    }
}

/// 오디오 창 시작 위치: seg초 창을 stride초마다 겹쳐서(문장이 두 창 사이에 끊기지 않게) 전체를 덮는다.
/// 창이 max개를 넘으면 전체에 고르게 분산.
pub fn audio_windows(dur: f64, seg: f64, stride: f64, max: usize) -> Vec<f64> {
    let (seg, stride) = (seg.max(1.0), stride.clamp(1.0, seg.max(1.0)));
    if dur <= seg {
        return vec![0.0];
    }
    let n = ((dur - seg) / stride).ceil() as usize + 1;
    let max = max.max(1);
    if n <= max {
        (0..n).map(|i| (i as f64 * stride).min(dur - seg)).collect()
    } else {
        (0..max).map(|i| (dur - seg) * i as f64 / (max - 1).max(1) as f64).collect()
    }
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
    fn video_segments_cover_or_spread() {
        // 70초 영상, 30초 구간 → 0~30, 30~60, 60~70
        assert_eq!(segments(70.0, 30.0, 20), vec![(0.0, 30.0), (30.0, 30.0), (60.0, 10.0)]);
        // 짧은 영상 → 전체 한 구간
        assert_eq!(segments(8.0, 30.0, 20), vec![(0.0, 8.0)]);
        // 긴 영상(1시간) → 최대 4구간을 고르게
        let s = segments(3600.0, 30.0, 4);
        assert_eq!(s.len(), 4);
        assert_eq!(s[1].0, 900.0);
        // 끝에 1초 미만 자투리는 버림
        assert_eq!(segments(60.5, 30.0, 20).len(), 2);
    }

    #[test]
    fn audio_windows_overlap_and_cover() {
        // 70초, 30초 창을 25초마다 → 0, 25, 40(마지막 창은 끝에 맞춤)
        assert_eq!(audio_windows(70.0, 30.0, 25.0, 100), vec![0.0, 25.0, 40.0]);
        assert_eq!(audio_windows(20.0, 30.0, 25.0, 100), vec![0.0]);
        // 창이 너무 많으면 처음~끝을 고르게
        let w = audio_windows(3600.0, 30.0, 25.0, 5);
        assert_eq!(w.len(), 5);
        assert_eq!((w[0], w[4]), (0.0, 3570.0));
    }

    #[test]
    fn spread_points() {
        assert_eq!(spread(10.0, 2), vec![2.5, 7.5]);
        assert_eq!(spread(0.0, 5), vec![0.0]);
    }
}
