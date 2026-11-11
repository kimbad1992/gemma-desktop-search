//! 메모리 벡터 저장소 + 섹션별 Top-K 검색
//!
//! - DB에는 768차원 원본, 여기에는 검색 차원(768/512/256)으로 잘라 재정규화한 벡터
//! - 인덱싱 중 새 청크는 바로 추가되고, 파일이 다시 인덱싱되면 이전 청크는 무효 표시(나중에 압축)

use crate::embed::vecmath::{dot, truncate_normalized};
use crate::extract::{Kind, Modality};
use rayon::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Docs = 0,
    Images = 1,
    Videos = 2,
    Audio = 3,
    Names = 4,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Docs,
        Section::Images,
        Section::Videos,
        Section::Audio,
        Section::Names,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Section::Docs => "문서 내용",
            Section::Images => "이미지",
            Section::Videos => "영상",
            Section::Audio => "오디오",
            Section::Names => "파일 이름",
        }
    }
    /// 모달리티마다 점수 분포가 달라 섞지 않고 섹션별로 순위를 매긴다.
    pub fn of(modality: Modality, kind: Kind) -> Section {
        match modality {
            Modality::Name => Section::Names,
            Modality::Text => Section::Docs,
            Modality::Image if kind == Kind::Video => Section::Videos,
            Modality::Image => Section::Images,
            Modality::Audio if kind == Kind::Video => Section::Videos,
            Modality::Audio => Section::Audio,
            Modality::Video => Section::Videos,
        }
    }
}

#[derive(Clone)]
pub struct FileInfo {
    pub path: String,
    pub path_lower: String,
    pub kind: Kind,
    /// 이름에 뜻 있는 단어가 있는지 — 없으면("IMG_0001", "430") 이름 벡터는 의미 비교에서 뺀다
    pub name_ok: bool,
}

impl FileInfo {
    pub fn new(path: String, kind: Kind) -> Self {
        Self {
            path_lower: path.to_lowercase(),
            name_ok: crate::search::keyword::name_is_meaningful(&path),
            path,
            kind,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub chunk_id: i64,
    pub file_id: i64,
    pub modality: Modality,
    pub locator: i64,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub file_id: i64,
    pub chunk_id: i64,
    /// 관련도 = 의미 관련도(`meaning`) + 검색어 가산점. 화면의 "관련도", 정렬, 최소 관련도 필터는 이 값.
    pub score: f32,
    /// 의미 관련도: 이 질의에서 대부분의 파일(중앙값)보다 표준편차 몇 배만큼 튀는지.
    /// 종류별 점수대 차이(오디오·영상·이름은 평균이 높고 문서 본문은 낮음)를 먼저 맞춘 뒤 계산한다.
    pub meaning: f32,
    /// 보정 전 코사인 유사도
    pub cosine: f32,
    /// 질의 단어가 파일(이름·폴더·본문)에 들어 있는 비율 (0~1)
    pub coverage: f32,
    pub modality: Modality,
    pub locator: i64,
    pub path: String,
    pub kind: Kind,
}

/// 폴더 비교용 키: 소문자 + 끝에 '\' 하나 (예: `c:\`, `\\nas\share\photos\`)
pub fn folder_key(path: &str) -> String {
    use crate::paths::{is_sep, SEP};
    let mut k: String = path
        .trim()
        .trim_end_matches(is_sep)
        .to_lowercase()
        .chars()
        .map(|c| if is_sep(c) { SEP } else { c })
        .collect();
    k.push(SEP);
    k
}

/// 어느 검색 폴더에도 속하지 않는 파일을 가리키는 특수 키
pub const OTHER_FOLDER_KEY: &str = "<기타>";

/// 검색 대상 폴더 선택
#[derive(Clone, Debug)]
pub struct FolderFilter {
    /// 검색에 포함할 폴더 키들
    pub selected: Vec<String>,
    /// 목록에 있는 모든 폴더 키 ("기타" 판정용)
    pub known: Vec<String>,
    /// 목록의 어느 폴더에도 속하지 않는 파일 포함 여부
    pub include_other: bool,
}

impl FolderFilter {
    pub fn passes(&self, path_lower: &str) -> bool {
        if self.selected.iter().any(|k| path_lower.starts_with(k.as_str())) {
            return true;
        }
        self.include_other && !self.known.iter().any(|k| path_lower.starts_with(k.as_str()))
    }
}

#[derive(Clone)]
pub struct Filter {
    pub sections: [bool; 5],
    /// 소문자 경로에 포함돼야 하는 문자열 (공백으로 여러 개 = AND)
    pub path_terms: Vec<String>,
    /// None = 모든 폴더
    pub folders: Option<FolderFilter>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            sections: [true; 5],
            path_terms: Vec::new(),
            folders: None,
        }
    }
}

/// 키워드 검색 결과를 순위에 반영하는 방법
#[derive(Clone, Debug)]
pub struct Boost {
    /// 파일별 질의 단어 포함 비율 (0~1)
    pub coverage: HashMap<i64, f32>,
    /// 파일 이름·폴더 이름만 본 비율 — "파일 이름" 섹션은 이것으로 (본문에 단어가 있는 소설이 이름 섹션 1위가 되지 않게)
    pub name_coverage: HashMap<i64, f32>,
    /// 포함 비율이 1일 때 더하는 관련도 (표준편차 단위, DigUp: 2, 번호·코드 조회형 질의는 4)
    pub weight: f32,
    /// 의미 점수에 곱하는 값 (보통 1, 번호·코드 조회형 질의는 0.5 — 임베딩은 번호를 담지 못하므로)
    pub meaning_weight: f32,
}

impl Default for Boost {
    fn default() -> Self {
        Self {
            coverage: HashMap::new(),
            name_coverage: HashMap::new(),
            weight: 0.0,
            meaning_weight: 1.0,
        }
    }
}

impl Boost {
    fn bonus(&self, file_id: i64, names_only: bool) -> (f32, f32) {
        let map = if names_only { &self.name_coverage } else { &self.coverage };
        let c = map.get(&file_id).copied().unwrap_or(0.0);
        // 단어 하나가 겹친 것보다 전부 들어 있는 쪽을 훨씬 크게 (coverage^1.5)
        (c, self.weight * c.powf(1.5))
    }
}

/// 이 질의에 대한 (보정한) 점수 전체의 중심과 폭: (중앙값, 1.4826 × MAD). 관련도 = (점수 − 중앙값) / 폭.
/// 폭이 너무 작으면(벡터가 몇 개 없을 때) 0.02로 (DigUp과 같은 하한).
fn score_stats(scores: &[f32], entries: &[Entry], alive: &[bool], offsets: &[f32; 5]) -> (f32, f32) {
    // 중앙값과 MAD(중앙값 절대 편차): 질의에 맞는 파일이 아주 많아도(예: 사진 대부분이 앵무새) 흔들리지 않는다.
    // 평균·표준편차를 쓰면 그 덩어리가 평균과 폭을 끌어올려, 내 사진에 많은 주제일수록 관련도가 낮아진다.
    let mut v: Vec<f32> = scores
        .iter()
        .enumerate()
        .filter(|(i, _)| alive[*i])
        .map(|(i, s)| s + offsets[entries[i].modality as usize])
        .collect();
    if v.is_empty() {
        return (0.0, 1.0);
    }
    let mid = v.len() / 2;
    let median = *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    for x in v.iter_mut() {
        *x = (*x - median).abs();
    }
    let mad = *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    // 정규분포라면 표준편차 ≈ 1.4826 × MAD
    (median, (1.4826 * mad).max(0.02))
}

/// 종류(모달리티)별 보정값: 전체 평균 − 그 종류의 평균 (DigUp의 방식, 단 표준편차로 나누지 않아 코사인과 같은 눈금).
/// 벡터가 20개 미만인 종류는 평균이 불안정해 보정하지 않는다.
fn modality_offsets(scores: &[f32], entries: &[Entry], alive: &[bool]) -> [f32; 5] {
    let mut sum = [0f64; 5];
    let mut cnt = [0usize; 5];
    for (i, s) in scores.iter().enumerate() {
        if alive[i] {
            let m = entries[i].modality as usize;
            sum[m] += *s as f64;
            cnt[m] += 1;
        }
    }
    let total: usize = cnt.iter().sum();
    if total == 0 {
        return [0.0; 5];
    }
    let all = sum.iter().sum::<f64>() / total as f64;
    let mut off = [0f32; 5];
    for m in 0..5 {
        if cnt[m] >= 20 {
            off[m] = (all - sum[m] / cnt[m] as f64) as f32;
        }
    }
    off
}


/// 질의 단어가 전부 들어 있으면 최소 일치도 아래여도 "검색어 포함"으로 보여준다.
/// 일부만 들어 있으면 순위만 올린다 — 날짜·장소 폴더 이름("2023", "제주도")에 단어 하나가 걸렸다고
/// 사진 수천 장이 최소 일치도를 건너뛰지 않도록.
pub const KEYWORD_SHOW: f32 = 0.999;

pub struct VectorStore {
    pub dim: usize,
    entries: Vec<Entry>,
    data: Vec<f32>,
    alive: Vec<bool>,
    by_file: HashMap<i64, Vec<u32>>,
    pub files: HashMap<i64, FileInfo>,
    dead: usize,
    pub loaded: bool,
    /// 허브 보정용 기준 질의 벡터 (검색 차원, 이어 붙임). 비어 있으면 보정 없이 종류별 평균만 맞춘다.
    bank: Vec<f32>,
    /// 항목마다 기준 질의들과의 평균 유사도 ("아무 질의에나 걸리는 정도")
    hub: Vec<f32>,
}

impl VectorStore {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            entries: Vec::new(),
            data: Vec::new(),
            alive: Vec::new(),
            by_file: HashMap::new(),
            files: HashMap::new(),
            dead: 0,
            loaded: false,
            bank: Vec::new(),
            hub: Vec::new(),
        }
    }

    pub fn has_bank(&self) -> bool {
        !self.bank.is_empty()
    }

    /// 허브 값: 기준 질의들과의 유사도 중앙값. 평균을 쓰면 주제가 겹치는 몇몇 질의(예: 반려동물 사진에
    /// "강아지 산책", "고양이가 자는 모습")에 끌려가 그 주제 사진 전체가 깎인다. 중앙값은 그런 몇 개에 흔들리지 않고
    /// 정말로 아무 질의에나 걸리는 항목만 잡는다.
    fn hub_of(bank: &[f32], dim: usize, row: &[f32]) -> f32 {
        let mut sims: Vec<f32> = bank.chunks(dim).map(|b| dot(b, row)).collect();
        if sims.is_empty() {
            return 0.0;
        }
        let mid = sims.len() / 2;
        let (_, m, _) = sims.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
        *m
    }

    /// 기준 질의 벡터(원본 차원)를 넣고 모든 항목의 허브 값을 계산한다 (15만 개 기준 1초 안쪽)
    pub fn set_bank(&mut self, vectors: &[Vec<f32>]) {
        let dim = self.dim;
        self.bank = vectors
            .iter()
            .filter(|v| v.len() >= dim)
            .flat_map(|v| truncate_normalized(v, dim))
            .collect();
        let bank = &self.bank;
        self.hub = self.data.par_chunks(dim).map(|row| Self::hub_of(bank, dim, row)).collect();
    }

    pub fn clear(&mut self) {
        let dim = self.dim;
        *self = Self::new(dim);
        self.loaded = true;
    }

    pub fn live_chunks(&self) -> usize {
        self.entries.len() - self.dead
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn memory_mb(&self) -> f64 {
        (self.data.len() * 4) as f64 / (1024.0 * 1024.0)
    }

    pub fn push(&mut self, e: Entry, full: &[f32]) {
        if full.len() < self.dim {
            return;
        }
        let v = truncate_normalized(full, self.dim);
        let idx = self.entries.len() as u32;
        if self.has_bank() {
            self.hub.push(Self::hub_of(&self.bank, self.dim, &v));
        }
        self.data.extend_from_slice(&v);
        self.entries.push(e);
        self.alive.push(true);
        self.by_file.entry(e.file_id).or_default().push(idx);
    }

    /// DB의 모든 벡터를 올린다
    pub fn fill_from_db(&mut self, db: &crate::index::db::Db) -> anyhow::Result<()> {
        db.for_each_vector(|r| {
            if !self.files.contains_key(&r.file_id) {
                self.set_file(r.file_id, FileInfo::new(r.path, r.kind));
            }
            self.push(
                Entry {
                    chunk_id: r.chunk_id,
                    file_id: r.file_id,
                    modality: r.modality,
                    locator: r.locator,
                },
                &r.vec,
            );
        })
    }

    pub fn set_file(&mut self, file_id: i64, info: FileInfo) {
        self.files.insert(file_id, info);
    }

    /// 파일 재인덱싱: 이전 청크 무효화 후 새 청크 추가
    pub fn replace_file(&mut self, file_id: i64, info: FileInfo, chunks: Vec<(Entry, Vec<f32>)>) {
        self.remove_file(file_id);
        self.files.insert(file_id, info);
        for (e, v) in chunks {
            self.push(e, &v);
        }
    }

    pub fn remove_file(&mut self, file_id: i64) {
        if let Some(idxs) = self.by_file.remove(&file_id) {
            for i in idxs {
                let i = i as usize;
                if self.alive[i] {
                    self.alive[i] = false;
                    self.dead += 1;
                }
            }
        }
        self.files.remove(&file_id);
        if self.dead > 20_000 && self.dead * 3 > self.entries.len() {
            self.compact();
        }
    }

    fn compact(&mut self) {
        let dim = self.dim;
        let mut entries = Vec::with_capacity(self.live_chunks());
        let mut data = Vec::with_capacity(self.live_chunks() * dim);
        let mut hub = Vec::with_capacity(if self.has_bank() { self.live_chunks() } else { 0 });
        let mut by_file: HashMap<i64, Vec<u32>> = HashMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !self.alive[i] {
                continue;
            }
            by_file.entry(e.file_id).or_default().push(entries.len() as u32);
            entries.push(*e);
            data.extend_from_slice(&self.data[i * dim..(i + 1) * dim]);
            if let Some(h) = self.hub.get(i) {
                hub.push(*h);
            }
        }
        self.hub = hub;
        self.alive = vec![true; entries.len()];
        self.entries = entries;
        self.data = data;
        self.by_file = by_file;
        self.dead = 0;
    }

    /// 질의에 대한 종류(모달리티)별 평균 점수와 개수 — 종류마다 점수대가 얼마나 다른지 보는 용도
    pub fn modality_means(&self, query: &[f32]) -> Vec<(Modality, f32, usize)> {
        let dim = self.dim;
        let q = truncate_normalized(query, dim);
        if q.len() != dim {
            return Vec::new();
        }
        let scores: Vec<f32> = self.data.par_chunks(dim).map(|row| dot(row, &q)).collect();
        let mut acc: HashMap<Modality, (f64, usize)> = HashMap::new();
        for (i, s) in scores.iter().enumerate() {
            if self.alive[i] {
                let e = acc.entry(self.entries[i].modality).or_default();
                e.0 += *s as f64;
                e.1 += 1;
            }
        }
        let mut v: Vec<_> = acc.into_iter().map(|(m, (s, n))| (m, (s / n as f64) as f32, n)).collect();
        v.sort_by_key(|x| x.0 as u8);
        v
    }

    /// 섹션별 Top-K (파일 단위로 묶어 가장 높은 청크 점수 사용)
    pub fn search(
        &self,
        query: &[f32],
        filter: &Filter,
        top_k: usize,
        boost: Option<&Boost>,
    ) -> Vec<(Section, Vec<Hit>)> {
        let dim = self.dim;
        let q = truncate_normalized(query, dim);
        if q.len() != dim || self.entries.is_empty() {
            return Section::ALL.iter().map(|s| (*s, Vec::new())).collect();
        }
        let raw: Vec<f32> = self.data.par_chunks(dim).map(|row| dot(row, &q)).collect();
        // 허브 보정: 항목마다 "아무 질의에나 걸리는 정도"를 뺀다 (종류별 점수대 차이도 함께 사라짐).
        // 기준 질의가 아직 없으면 종류별 평균만 맞춘다.
        let use_hub = self.has_bank() && self.hub.len() == raw.len();
        let scores: Vec<f32> = if use_hub {
            raw.iter().zip(&self.hub).map(|(s, h)| s - h).collect()
        } else {
            raw.clone()
        };
        let offsets = if use_hub { [0.0; 5] } else { modality_offsets(&scores, &self.entries, &self.alive) };
        let (mean, sd) = score_stats(&scores, &self.entries, &self.alive, &offsets);
        let none = Boost::default();
        let boost = boost.unwrap_or(&none);

        // 파일별 경로 필터 결과 캐시
        let mut pass_cache: HashMap<i64, bool> = HashMap::new();
        let mut best: [HashMap<i64, (f32, usize)>; 5] = Default::default();
        for (i, &cos) in scores.iter().enumerate() {
            if !self.alive[i] {
                continue;
            }
            let e = &self.entries[i];
            // 관련도: 이 질의의 전체 분포에서 표준편차 몇 배만큼 높은지 (종류별 평균은 먼저 맞춤)
            let score = (cos + offsets[e.modality as usize] - mean) / sd;
            let Some(info) = self.files.get(&e.file_id) else {
                continue;
            };
            let sec = Section::of(e.modality, info.kind);
            if !filter.sections[sec as usize] {
                continue;
            }
            // 뜻 없는 이름의 의미 점수는 잡음 (어떤 질의에도 비슷) — 이름에 검색어가 들어 있을 때만 남긴다
            if e.modality == Modality::Name && !info.name_ok && !boost.name_coverage.contains_key(&e.file_id) {
                continue;
            }
            if !filter.path_terms.is_empty() || filter.folders.is_some() {
                let pass = *pass_cache.entry(e.file_id).or_insert_with(|| {
                    filter.path_terms.iter().all(|t| info.path_lower.contains(t.as_str()))
                        && filter
                            .folders
                            .as_ref()
                            .map(|f| f.passes(&info.path_lower))
                            .unwrap_or(true)
                });
                if !pass {
                    continue;
                }
            }
            let slot = best[sec as usize].entry(e.file_id).or_insert((f32::MIN, i));
            if score > slot.0 {
                *slot = (score, i);
            }
        }

        Section::ALL
            .iter()
            .map(|&sec| {
                // (관련도, 의미 관련도, 포함 비율, 청크 위치)
                let mut v: Vec<(f32, f32, f32, usize)> = best[sec as usize]
                    .iter()
                    .map(|(&fid, &(z, i))| {
                        let (cov, bonus) = boost.bonus(fid, sec == Section::Names);
                        (boost.meaning_weight * z + bonus, z, cov, i)
                    })
                    .collect();
                v.sort_by(|a, b| b.0.total_cmp(&a.0));
                // 상위 K개 + 그 밖에서 질의 단어가 충분히 들어 있는 파일 (최대 20개 더)
                let extra: Vec<_> = v
                    .iter()
                    .skip(top_k)
                    .filter(|x| x.2 >= KEYWORD_SHOW)
                    .take(20)
                    .copied()
                    .collect();
                v.truncate(top_k);
                v.extend(extra);
                let hits = v
                    .into_iter()
                    .map(|(total, meaning, coverage, i)| {
                        let e = self.entries[i];
                        let info = &self.files[&e.file_id];
                        Hit {
                            file_id: e.file_id,
                            chunk_id: e.chunk_id,
                            score: total,
                            meaning,
                            cosine: raw[i],
                            coverage,
                            modality: e.modality,
                            locator: e.locator,
                            path: info.path.clone(),
                            kind: info.kind,
                        }
                    })
                    .collect();
                (sec, hits)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(dim: usize, hot: usize) -> Vec<f32> {
        let mut v = vec![0.01f32; dim];
        v[hot] = 1.0;
        v
    }

    #[test]
    fn topk_per_section_and_replace() {
        let mut s = VectorStore::new(8);
        s.loaded = true;
        s.replace_file(
            1,
            FileInfo::new(r"C:\docs\report.txt".into(), Kind::Text),
            vec![
                (Entry { chunk_id: 10, file_id: 1, modality: Modality::Text, locator: 0 }, unit(8, 0)),
                (Entry { chunk_id: 11, file_id: 1, modality: Modality::Text, locator: 1 }, unit(8, 1)),
                (Entry { chunk_id: 12, file_id: 1, modality: Modality::Name, locator: -1 }, unit(8, 2)),
            ],
        );
        s.replace_file(
            2,
            FileInfo::new(r"C:\pics\b.jpg".into(), Kind::Image),
            vec![(Entry { chunk_id: 20, file_id: 2, modality: Modality::Image, locator: 0 }, unit(8, 1))],
        );
        let res = s.search(&unit(8, 1), &Filter::default(), 10, None);
        let docs = &res[Section::Docs as usize].1;
        assert_eq!(docs.len(), 1, "파일 단위로 묶임");
        assert_eq!(docs[0].chunk_id, 11, "가장 높은 청크");
        assert_eq!(res[Section::Images as usize].1[0].file_id, 2);
        assert_eq!(res[Section::Names as usize].1.len(), 1);

        // 경로 필터
        let f = Filter { path_terms: vec!["pics".into()], ..Default::default() };
        let res = s.search(&unit(8, 1), &f, 10, None);
        assert!(res[Section::Docs as usize].1.is_empty());
        assert_eq!(res[Section::Images as usize].1.len(), 1);

        // 재인덱싱 → 이전 청크 무효
        s.replace_file(
            1,
            FileInfo::new(r"C:\docs\a.txt".into(), Kind::Text),
            vec![(Entry { chunk_id: 30, file_id: 1, modality: Modality::Text, locator: 0 }, unit(8, 3))],
        );
        assert_eq!(s.live_chunks(), 2);
        let res = s.search(&unit(8, 1), &Filter::default(), 10, None);
        assert_eq!(res[Section::Docs as usize].1[0].chunk_id, 30);
        s.remove_file(2);
        assert_eq!(s.live_chunks(), 1);
    }

    #[test]
    fn folder_filter_selects_roots_and_other() {
        use crate::paths::os_path as os;
        assert_eq!(folder_key(&os(r"C:\Users\Me\Photos\")), os(r"c:\users\me\photos\"));
        assert_eq!(folder_key(&os(r"C:\")), os(r"c:\"));
        assert_eq!(folder_key(&os(r"\\NAS\Share")), os(r"\\nas\share\"));
        #[cfg(unix)]
        assert_eq!(folder_key("/"), "/");

        let mut s = VectorStore::new(8);
        s.loaded = true;
        let files = [
            (1, r"D:\Photos\a.jpg"),
            (2, r"\\NAS\Share\b.jpg"),
            (3, r"E:\Misc\c.jpg"),
            (4, r"D:\PhotosOld\d.jpg"), // 'D:\Photos'와 접두사만 같은 다른 폴더
        ];
        for (id, p) in files {
            s.replace_file(
                id,
                FileInfo::new(os(p), Kind::Image),
                vec![(Entry { chunk_id: id * 10, file_id: id, modality: Modality::Image, locator: 0 }, unit(8, 1))],
            );
        }
        let known = vec![folder_key(&os(r"D:\Photos")), folder_key(&os(r"\\NAS\Share"))];
        let run = |selected: Vec<String>, include_other: bool| {
            let f = Filter {
                folders: Some(FolderFilter { selected, known: known.clone(), include_other }),
                ..Default::default()
            };
            let mut ids: Vec<i64> =
                s.search(&unit(8, 1), &f, 10, None)[Section::Images as usize].1.iter().map(|h| h.file_id).collect();
            ids.sort();
            ids
        };
        assert_eq!(run(vec![folder_key(&os(r"D:\Photos"))], false), vec![1]);
        assert_eq!(run(vec![folder_key(&os(r"\\NAS\Share"))], true), vec![2, 3, 4]);
        assert_eq!(run(vec![], true), vec![3, 4]);
        assert!(run(vec![], false).is_empty());
    }

    #[test]
    fn keyword_boost_reorders_but_keeps_score() {
        let mut s = VectorStore::new(8);
        s.loaded = true;
        let mut near = unit(8, 1);
        near[2] = 0.3; // 질의와 조금 덜 비슷
        for (id, v) in [(1, unit(8, 1)), (2, near)] {
            s.replace_file(
                id,
                FileInfo::new(format!(r"C:\d\{id}.txt"), Kind::Text),
                vec![(Entry { chunk_id: id * 10, file_id: id, modality: Modality::Text, locator: 0 }, v)],
            );
        }
        let plain = s.search(&unit(8, 1), &Filter::default(), 10, None);
        assert_eq!(plain[Section::Docs as usize].1[0].file_id, 1);
        let boost = Boost { coverage: HashMap::from([(2, 1.0)]), weight: 4.0, ..Default::default() };
        let res = s.search(&unit(8, 1), &Filter::default(), 10, Some(&boost));
        let docs = &res[Section::Docs as usize].1;
        assert_eq!(docs[0].file_id, 2, "질의 단어가 다 들어 있는 파일이 위로");
        assert!(docs[0].meaning < docs[1].meaning, "의미 점수는 낮아도");
        assert!(docs[0].score > docs[1].score, "일치도(의미+검색어)가 높아 위에");
        assert_eq!(docs[0].coverage, 1.0);
        // 상위 K개 밖이어도 단어가 충분히 들어 있으면 남긴다
        let boost = Boost { coverage: HashMap::from([(2, 1.0)]), weight: 0.0, ..Default::default() };
        let res = s.search(&unit(8, 1), &Filter::default(), 1, Some(&boost));
        let ids: Vec<i64> = res[Section::Docs as usize].1.iter().map(|h| h.file_id).collect();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn modality_offsets_center_each_kind() {
        // 이름 벡터 30개는 평균 0.7, 이미지 30개는 평균 0.5 → 보정 후 둘 다 전체 평균 0.6 근처
        let entries: Vec<Entry> = (0..60)
            .map(|i| Entry {
                chunk_id: i,
                file_id: i,
                modality: if i < 30 { Modality::Name } else { Modality::Image },
                locator: 0,
            })
            .collect();
        let scores: Vec<f32> = (0..60).map(|i| if i < 30 { 0.7 } else { 0.5 }).collect();
        let off = modality_offsets(&scores, &entries, &vec![true; 60]);
        assert!((off[Modality::Name as usize] + 0.1).abs() < 1e-5);
        assert!((off[Modality::Image as usize] - 0.1).abs() < 1e-5);
        assert_eq!(off[Modality::Audio as usize], 0.0, "없는 종류는 보정 없음");
    }

    #[test]
    fn video_frames_go_to_video_section() {
        assert_eq!(Section::of(Modality::Image, Kind::Video), Section::Videos);
        assert_eq!(Section::of(Modality::Audio, Kind::Video), Section::Videos);
        assert_eq!(Section::of(Modality::Audio, Kind::Audio), Section::Audio);
    }
}
