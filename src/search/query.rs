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

/// 결과가 파일의 어디인지 알려 주는 짧은 말 ("3페이지", "01:20 장면"). 파일 전체면 None.
pub fn locator_label(hv: &HitView) -> Option<String> {
    use crate::util::fmt_ts;
    match (hv.hit.modality, hv.hit.kind) {
        (Modality::Video, _) => Some(format!("{}~ 구간", fmt_ts(hv.hit.locator))),
        (Modality::Image, Kind::Video) => Some(format!("{} 장면", fmt_ts(hv.hit.locator))),
        (Modality::Audio, _) => Some(format!("{}부터", fmt_ts(hv.hit.locator))),
        (Modality::Text, Kind::Doc) if crate::extract::pdf::page_of(hv.hit.locator).is_some() => {
            crate::extract::pdf::page_of(hv.hit.locator).map(|p| format!("{p}페이지"))
        }
        (Modality::Text, _) if hv.hit.locator > 0 => Some(format!("{}번째 구간", hv.hit.locator + 1)),
        _ => None,
    }
}

/// 최소 일치도 이상이거나 질의 단어가 들어 있는 결과 (순위는 키워드 가산점까지 반영된 순서 그대로)
pub fn passing(hits: &[HitView], min: f32) -> Vec<&HitView> {
    hits.iter().filter(|h| h.visible(min)).collect()
}

/// "전체" 목록 최대 개수
const UNIFIED_LIMIT: usize = 60;

/// 모든 섹션을 합쳐 순위 점수(보정 일치도 + 키워드 가산점) 순으로. 같은 파일은 가장 높은 것 하나만.
/// "고양이 영상"처럼 종류를 말한 질의는 그 종류에 작은 가산점.
pub fn unified<'a>(visible: &[(Section, Vec<&'a HitView>)], hints: &[Section]) -> Vec<(Section, &'a HitView)> {
    // 표시된 일치도 순서 그대로. 종류를 말한 질의(hints)는 같은 점수일 때만 그 종류를 앞으로.
    let mut all: Vec<(Section, &HitView)> =
        visible.iter().flat_map(|(s, h)| h.iter().map(move |hv| (*s, *hv))).collect();
    all.sort_by(|a, b| {
        b.1.hit
            .score
            .total_cmp(&a.1.hit.score)
            .then_with(|| hints.contains(&b.0).cmp(&hints.contains(&a.0)))
    });
    let mut seen = std::collections::HashSet::new();
    all.retain(|(_, hv)| seen.insert(hv.hit.file_id));
    // 같은 파일이 여러 폴더에 있으면(사진 폴더 + Immich 라이브러리 등) 한 번만: 이름이 같고 점수가 사실상 같으면 복사본
    let mut kept: Vec<(Section, &HitView)> = Vec::with_capacity(all.len());
    for (s, hv) in all {
        let dup = kept.iter().any(|(ks, k)| {
            *ks == s && k.file_name.eq_ignore_ascii_case(&hv.file_name) && (k.hit.cosine - hv.hit.cosine).abs() < 0.003
        });
        if !dup {
            kept.push((s, hv));
        }
    }
    kept.truncate(UNIFIED_LIMIT);
    kept
}
