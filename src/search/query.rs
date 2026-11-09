//! 질의 실행 (백그라운드 스레드): 질의 임베딩 + 키워드 검색 → 저장소 검색 → 스니펫/썸네일 정보

use super::keyword::{self, KeywordResult};
use super::store::{Boost, Filter, Hit, Section, VectorStore, KEYWORD_SHOW};
use crate::embed::client::{EmbedClient, EmbedInput};
use crate::embed::prompt;
use crate::extract::{Kind, Modality};
use crate::index::db::Db;
use parking_lot::{Mutex, RwLock};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// 키워드 가산점 (관련도 단위 = 표준편차). 번호·코드 조회형 질의는 두 배이고 의미는 절반만 (DigUp과 같은 값).
const WORD_WEIGHT: f32 = 2.0;
const LOOKUP_WEIGHT: f32 = 4.0;
const LOOKUP_MEANING: f32 = 0.5;

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
    /// 질의 단어를 강조 표식(MARK_START/END)으로 감싼 발췌
    pub highlight: Option<String>,
    pub thumb_chunk: Option<i64>,
}

impl HitView {
    /// 질의 단어가 충분히 들어 있어 최소 일치도와 상관없이 보여줄 결과
    pub fn keyword_match(&self) -> bool {
        self.hit.coverage >= KEYWORD_SHOW
    }

    pub fn visible(&self, min_score: f32) -> bool {
        self.hit.score >= min_score || self.keyword_match()
    }
}

#[derive(Clone)]
pub struct Outcome {
    pub label: String,
    pub sections: Vec<(Section, Vec<HitView>)>,
    pub elapsed: Duration,
    pub searched_chunks: usize,
    pub error: Option<String>,
    /// 질의가 가리키는 종류 ("고양이 영상" → 영상) — 이 섹션을 먼저 보여준다
    pub kind_hints: Vec<Section>,
    /// 미리보기 강조용 키워드 식
    pub expr: Option<String>,
}

impl Outcome {
    pub fn total_files(&self) -> usize {
        self.sections.iter().map(|(_, v)| v.len()).sum()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    port: u16,
    store: &RwLock<VectorStore>,
    db: &Mutex<Db>,
    q: QueryKind,
    filter: Filter,
    top_k: usize,
    min_score: f32,
    ffmpeg: Option<PathBuf>,
) -> Outcome {
    let t0 = Instant::now();
    let fail = |label: String, msg: String| Outcome {
        label,
        sections: Vec::new(),
        elapsed: t0.elapsed(),
        searched_chunks: 0,
        error: Some(msg),
        kind_hints: Vec::new(),
        expr: None,
    };
    let client = EmbedClient::new(port);
    let (label, input, text) = match &q {
        QueryKind::Text(s) => (format!("“{}”", s.trim()), EmbedInput::Text(prompt::query(s)), Some(s.clone())),
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
                Ok((jpeg, _)) => (label, EmbedInput::Image(jpeg), None),
                Err(e) => return fail(label, e.to_string()),
            }
        }
    };
    // 키워드 검색은 질의 임베딩과 동시에 (DB만 보면 됨)
    let (qv, kw) = std::thread::scope(|s| {
        let kw = s.spawn(|| match &text {
            Some(t) => keyword::search(&db.lock(), &keyword::terms(t)),
            None => KeywordResult::default(),
        });
        let qv = client.embed(std::slice::from_ref(&input));
        (qv, kw.join().unwrap_or_default())
    });
    let qv = match qv {
        Ok(mut v) => v.remove(0),
        Err(e) => return fail(label, e.to_string()),
    };
    // 허브 보정 기준 질의 (처음 한 번)
    super::hub::ensure(&client, store, db);
    let boost = Boost {
        coverage: kw.coverage.clone(),
        name_coverage: kw.name_coverage.clone(),
        weight: if kw.lookup { LOOKUP_WEIGHT } else { WORD_WEIGHT },
        meaning_weight: if kw.lookup { LOOKUP_MEANING } else { 1.0 },
    };
    let (sections, searched) = {
        let st = store.read();
        (st.search(&qv, &filter, top_k, Some(&boost)), st.live_chunks())
    };
    let db = db.lock();
    let expr = kw.expr.as_deref();
    let sections = sections
        .into_iter()
        .map(|(sec, hits)| {
            let views = hits
                .into_iter()
                .map(|mut hit| {
                    let mut detail = db.chunk_detail(hit.chunk_id).ok().flatten();
                    let mut highlight = None;
                    if let (Some(expr), true) = (expr, hit.coverage > 0.0 && hit.modality == Modality::Text) {
                        highlight = db.fts_snippet(hit.chunk_id, expr, 24);
                        // 의미로는 약하고 단어로 걸린 결과는 단어가 들어 있는 구간을 가리킨다
                        if highlight.is_none() && hit.score < min_score {
                            if let Some((cid, loc, snip)) = db.fts_best_text_chunk(hit.file_id, expr) {
                                hit.chunk_id = cid;
                                hit.locator = loc;
                                detail = db.chunk_detail(cid).ok().flatten();
                                highlight = Some(snip);
                            }
                        }
                    }
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
                        highlight,
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
        kind_hints: text.as_deref().map(keyword::kind_hints).unwrap_or_default(),
        expr: kw.expr.clone(),
    }
}
