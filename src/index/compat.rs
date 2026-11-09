//! 모델/서버 호환성 검사
//!
//! 인덱스에 이미 들어 있는 항목 몇 개를 지금 서버로 다시 임베딩해 저장된 벡터와 비교한다.
//! 모델 파일(BF16 ↔ Q8)이나 llama.cpp 빌드가 바뀌어도 벡터가 사실상 같으면 인덱스를 그대로 쓸 수 있다.
//! 기준 벡터를 따로 저장하지 않아도 되므로 예전 인덱스에도 그대로 쓸 수 있다.

use super::db::Db;
use super::indexer::text_chunks;
use crate::config::Config;
use crate::embed::client::{EmbedClient, EmbedError, EmbedInput};
use crate::embed::{prompt, vecmath};
use crate::extract::{self, media, Kind, Modality};
use anyhow::Result;
use std::path::Path;
use std::time::{Duration, Instant};

/// 종류별 표본 수
#[derive(Clone, Copy)]
pub struct Budget {
    pub names: usize,
    pub text: usize,
    pub images: usize,
    pub video: usize,
    pub audio: usize,
}

impl Budget {
    /// 인덱싱 시작 때 쓰는 가벼운 검사 (NAS에서도 1분 안팎)
    pub fn quick() -> Self {
        Self {
            names: 32,
            text: 8,
            images: 12,
            video: 3,
            audio: 3,
        }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            names: 48,
            text: 16,
            images: 16,
            video: 4,
            audio: 4,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Stat {
    pub label: &'static str,
    /// 비교한 표본 수
    pub n: usize,
    pub mean: f32,
    pub min: f32,
    /// 파일이 없거나 읽지 못해 건너뛴 표본 수
    pub skipped: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub stats: Vec<Stat>,
    pub elapsed: Duration,
}

impl Report {
    pub fn compared(&self) -> usize {
        self.stats.iter().map(|s| s.n).sum()
    }

    /// 기존 인덱스를 그대로 써도 되는지. BF16 ↔ Q8 실측: 평균 0.9998, 최소 0.9974 (오디오).
    /// 다른 모델이나 접두사가 바뀐 경우는 이보다 훨씬 낮다.
    pub fn compatible(&self) -> bool {
        self.stats.iter().find(|s| s.label == "이름").is_some_and(|s| s.n >= 8)
            && self.stats.iter().filter(|s| s.n > 0).all(|s| s.min >= 0.99 && s.mean >= 0.995)
    }

    pub fn summary(&self) -> String {
        self.stats
            .iter()
            .filter(|s| s.n > 0)
            .map(|s| format!("{} {}개 평균 {:.4} 최소 {:.4}", s.label, s.n, s.mean, s.min))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn stat(label: &'static str, cos: &[f32], skipped: usize) -> Stat {
    let n = cos.len();
    Stat {
        label,
        n,
        mean: if n > 0 { cos.iter().sum::<f32>() / n as f32 } else { 0.0 },
        min: cos.iter().copied().fold(f32::INFINITY, f32::min).min(if n > 0 { 1.0 } else { 0.0 }),
        skipped,
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// 서버 연결이 끊기면 검사 전체를 중단, 그 밖의 오류는 그 표본만 건너뛴다.
fn embed_one(client: &EmbedClient, input: EmbedInput) -> Result<Option<Vec<f32>>> {
    match client.embed(std::slice::from_ref(&input)) {
        Ok(mut v) => Ok(Some(v.remove(0))),
        Err(EmbedError::Transport(m)) => anyhow::bail!("서버 연결이 끊겼습니다: {m}"),
        Err(_) => Ok(None),
    }
}

pub fn check(db: &Db, cfg: &Config, client: &EmbedClient, budget: Budget) -> Result<Report> {
    let t0 = Instant::now();
    let mut stats = Vec::new();

    // 1) 파일 이름 벡터: 입력이 경로만으로 정해지므로 파일을 읽지 않는다 (텍스트 인코더 확인)
    {
        let mut cos = Vec::new();
        let mut skipped = 0;
        let mut rows = Vec::new();
        for k in [Kind::Text, Kind::Doc, Kind::Image, Kind::Video, Kind::Audio, Kind::Other] {
            rows.extend(db.sample_chunks(Modality::Name, k, budget.names.div_ceil(3))?);
        }
        rows.truncate(budget.names);
        // 이름은 한 번에 여러 개 보낸다
        for chunk in rows.chunks(16) {
            let inputs: Vec<EmbedInput> = chunk
                .iter()
                .map(|(p, _, _)| EmbedInput::Text(prompt::document(&file_name(p), p)))
                .collect();
            match client.embed(&inputs) {
                Ok(vecs) => {
                    for ((_, _, stored), v) in chunk.iter().zip(vecs) {
                        cos.push(vecmath::dot(stored, &v));
                    }
                }
                Err(EmbedError::Transport(m)) => anyhow::bail!("서버 연결이 끊겼습니다: {m}"),
                Err(_) => skipped += chunk.len(),
            }
        }
        stats.push(stat("이름", &cos, skipped));
    }

    // 2) 텍스트 청크: 인덱서와 같은 함수로 청크를 다시 만든다
    {
        let mut cos = Vec::new();
        let mut skipped = 0;
        let max_tokens = cfg.max_input_tokens();
        for (path, loc, stored) in db.sample_chunks(Modality::Text, Kind::Text, budget.text)? {
            let input = std::fs::read(&path)
                .ok()
                .and_then(|b| extract::text::decode_text(&b))
                .and_then(|t| text_chunks(cfg, client, max_tokens, &file_name(&path), &t).ok())
                .and_then(|mut c| (loc >= 0 && (loc as usize) < c.len()).then(|| c.swap_remove(loc as usize).1));
            match input {
                Some(inp) => match embed_one(client, EmbedInput::Text(inp))? {
                    Some(v) => cos.push(vecmath::dot(&stored, &v)),
                    None => skipped += 1,
                },
                None => skipped += 1,
            }
        }
        stats.push(stat("텍스트", &cos, skipped));
    }

    // 3) 이미지 (비전 인코더 확인)
    {
        let mut cos = Vec::new();
        let mut skipped = 0;
        let ffmpeg = cfg.ffmpeg();
        for (path, _, stored) in db.sample_chunks(Modality::Image, Kind::Image, budget.images)? {
            let jpeg = extract::image::load_image(Path::new(&path), ffmpeg.as_deref())
                .ok()
                .and_then(|img| extract::image::model_and_thumb(&img).ok())
                .map(|(j, _)| j);
            match jpeg {
                Some(j) => match embed_one(client, EmbedInput::Image(j))? {
                    Some(v) => cos.push(vecmath::dot(&stored, &v)),
                    None => skipped += 1,
                },
                None => skipped += 1,
            }
        }
        stats.push(stat("이미지", &cos, skipped));
    }

    // 4) 영상 구간, 5) 오디오 구간 — ffmpeg가 있을 때만
    if let (Some(ff), Some(fp)) = (cfg.ffmpeg(), cfg.ffprobe()) {
        let mut cos = Vec::new();
        let mut skipped = 0;
        let seg = cfg.video_window_secs() as f64;
        for (path, loc, stored) in db.sample_chunks(Modality::Video, Kind::Video, budget.video)? {
            let p = Path::new(&path);
            let clip = media::probe(&fp, p).ok().and_then(|info| {
                media::segments(info.duration, seg, cfg.video_max_segments)
                    .into_iter()
                    .find(|(s, _)| ((s * 1000.0) as i64 - loc).abs() <= 1)
                    .and_then(|(s, len)| media::video_segment(&ff, p, s, len).ok())
            });
            match clip {
                Some(c) => match embed_one(client, EmbedInput::Video(c))? {
                    Some(v) => cos.push(vecmath::dot(&stored, &v)),
                    None => skipped += 1,
                },
                None => skipped += 1,
            }
        }
        stats.push(stat("영상", &cos, skipped));

        let mut cos = Vec::new();
        let mut skipped = 0;
        let aseg = cfg.audio_segment_secs.max(5) as f64;
        for (path, loc, stored) in db.sample_chunks(Modality::Audio, Kind::Audio, budget.audio)? {
            let wav = media::audio_segment(&ff, Path::new(&path), loc as f64 / 1000.0, aseg)
                .ok()
                .flatten();
            match wav {
                Some(w) => match embed_one(client, EmbedInput::Audio(w))? {
                    Some(v) => cos.push(vecmath::dot(&stored, &v)),
                    None => skipped += 1,
                },
                None => skipped += 1,
            }
        }
        stats.push(stat("오디오", &cos, skipped));
    }

    Ok(Report {
        stats,
        elapsed: t0.elapsed(),
    })
}
