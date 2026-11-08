//! 질의 실행 (백그라운드 스레드): 질의 임베딩 → 저장소 검색 → 스니펫/썸네일 정보

use super::store::{Filter, Hit, Section, VectorStore};
use crate::embed::client::{EmbedClient, EmbedInput};
use crate::embed::prompt;
use crate::extract::Kind;
use crate::index::db::Db;
use parking_lot::{Mutex, RwLock};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub enum QueryKind {
    Text(String),
    Image(PathBuf),
}

#[derive(Clone)]
pub struct HitView {
    pub hit: Hit,
    pub file_name: String,
    pub snippet: String,
    pub thumb_chunk: Option<i64>,
}

#[derive(Clone)]
pub struct Outcome {
    pub label: String,
    pub sections: Vec<(Section, Vec<HitView>)>,
    pub elapsed: Duration,
    pub searched_chunks: usize,
    pub error: Option<String>,
}

impl Outcome {
    pub fn total_files(&self) -> usize {
        self.sections.iter().map(|(_, v)| v.len()).sum()
    }
}

pub fn run(
    port: u16,
    store: &RwLock<VectorStore>,
    db: &Mutex<Db>,
    q: QueryKind,
    filter: Filter,
    top_k: usize,
    ffmpeg: Option<PathBuf>,
) -> Outcome {
    let t0 = Instant::now();
    let fail = |label: String, msg: String| Outcome {
        label,
        sections: Vec::new(),
        elapsed: t0.elapsed(),
        searched_chunks: 0,
        error: Some(msg),
    };
    let client = EmbedClient::new(port);
    let (label, input) = match &q {
        QueryKind::Text(s) => (format!("“{}”", s.trim()), EmbedInput::Text(prompt::query(s))),
        QueryKind::Image(p) => {
            let label = format!(
                "이미지: {}",
                p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            );
            let img = match crate::extract::image::load_image(p, ffmpeg.as_deref()) {
                Ok(i) => i,
                Err(e) => return fail(label, format!("이미지를 열 수 없습니다: {e}")),
            };
            match crate::extract::image::model_and_thumb(&img) {
                Ok((jpeg, _)) => (label, EmbedInput::Image(jpeg)),
                Err(e) => return fail(label, e.to_string()),
            }
        }
    };
    let qv = match client.embed(std::slice::from_ref(&input)) {
        Ok(mut v) => v.remove(0),
        Err(e) => return fail(label, e.to_string()),
    };
    let (sections, searched) = {
        let st = store.read();
        (st.search(&qv, &filter, top_k), st.live_chunks())
    };
    let db = db.lock();
    let sections = sections
        .into_iter()
        .map(|(sec, hits)| {
            let views = hits
                .into_iter()
                .map(|hit| {
                    let detail = db.chunk_detail(hit.chunk_id).ok().flatten();
                    let thumb_chunk = match &detail {
                        Some(d) if d.has_thumb => Some(hit.chunk_id),
                        _ if matches!(hit.kind, Kind::Image | Kind::Video) => {
                            db.file_thumb_chunk(hit.file_id).ok().flatten()
                        }
                        _ => None,
                    };
                    let file_name = std::path::Path::new(&hit.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| hit.path.clone());
                    HitView {
                        file_name,
                        snippet: detail.map(|d| d.snippet).unwrap_or_default(),
                        thumb_chunk,
                        hit,
                    }
                })
                .collect();
            (sec, views)
        })
        .collect();
    Outcome {
        label,
        sections,
        elapsed: t0.elapsed(),
        searched_chunks: searched,
        error: None,
    }
}
