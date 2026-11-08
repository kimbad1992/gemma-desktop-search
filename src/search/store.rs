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
        }
    }
}

#[derive(Clone)]
pub struct FileInfo {
    pub path: String,
    pub path_lower: String,
    pub kind: Kind,
}

impl FileInfo {
    pub fn new(path: String, kind: Kind) -> Self {
        Self {
            path_lower: path.to_lowercase(),
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
    pub score: f32,
    pub modality: Modality,
    pub locator: i64,
    pub path: String,
    pub kind: Kind,
}

#[derive(Clone)]
pub struct Filter {
    pub sections: [bool; 5],
    /// 소문자 경로에 포함돼야 하는 문자열 (공백으로 여러 개 = AND)
    pub path_terms: Vec<String>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            sections: [true; 5],
            path_terms: Vec::new(),
        }
    }
}

pub struct VectorStore {
    pub dim: usize,
    entries: Vec<Entry>,
    data: Vec<f32>,
    alive: Vec<bool>,
    by_file: HashMap<i64, Vec<u32>>,
    pub files: HashMap<i64, FileInfo>,
    dead: usize,
    pub loaded: bool,
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
        }
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
        self.data.extend_from_slice(&v);
        self.entries.push(e);
        self.alive.push(true);
        self.by_file.entry(e.file_id).or_default().push(idx);
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
        let mut by_file: HashMap<i64, Vec<u32>> = HashMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !self.alive[i] {
                continue;
            }
            by_file.entry(e.file_id).or_default().push(entries.len() as u32);
            entries.push(*e);
            data.extend_from_slice(&self.data[i * dim..(i + 1) * dim]);
        }
        self.alive = vec![true; entries.len()];
        self.entries = entries;
        self.data = data;
        self.by_file = by_file;
        self.dead = 0;
    }

    /// 섹션별 Top-K (파일 단위로 묶어 가장 높은 청크 점수 사용)
    pub fn search(&self, query: &[f32], filter: &Filter, top_k: usize) -> Vec<(Section, Vec<Hit>)> {
        let dim = self.dim;
        let q = truncate_normalized(query, dim);
        if q.len() != dim || self.entries.is_empty() {
            return Section::ALL.iter().map(|s| (*s, Vec::new())).collect();
        }
        let scores: Vec<f32> = self.data.par_chunks(dim).map(|row| dot(row, &q)).collect();

        // 파일별 경로 필터 결과 캐시
        let mut pass_cache: HashMap<i64, bool> = HashMap::new();
        let mut best: [HashMap<i64, (f32, usize)>; 5] = Default::default();
        for (i, &score) in scores.iter().enumerate() {
            if !self.alive[i] {
                continue;
            }
            let e = &self.entries[i];
            let Some(info) = self.files.get(&e.file_id) else {
                continue;
            };
            let sec = Section::of(e.modality, info.kind);
            if !filter.sections[sec as usize] {
                continue;
            }
            if !filter.path_terms.is_empty() {
                let pass = *pass_cache.entry(e.file_id).or_insert_with(|| {
                    filter.path_terms.iter().all(|t| info.path_lower.contains(t.as_str()))
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
                let mut v: Vec<(f32, usize)> = best[sec as usize].values().copied().collect();
                v.sort_by(|a, b| b.0.total_cmp(&a.0));
                v.truncate(top_k);
                let hits = v
                    .into_iter()
                    .map(|(score, i)| {
                        let e = self.entries[i];
                        let info = &self.files[&e.file_id];
                        Hit {
                            file_id: e.file_id,
                            chunk_id: e.chunk_id,
                            score,
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
            FileInfo::new(r"C:\docs\a.txt".into(), Kind::Text),
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
        let res = s.search(&unit(8, 1), &Filter::default(), 10);
        let docs = &res[Section::Docs as usize].1;
        assert_eq!(docs.len(), 1, "파일 단위로 묶임");
        assert_eq!(docs[0].chunk_id, 11, "가장 높은 청크");
        assert_eq!(res[Section::Images as usize].1[0].file_id, 2);
        assert_eq!(res[Section::Names as usize].1.len(), 1);

        // 경로 필터
        let f = Filter { path_terms: vec!["pics".into()], ..Default::default() };
        let res = s.search(&unit(8, 1), &f, 10);
        assert!(res[Section::Docs as usize].1.is_empty());
        assert_eq!(res[Section::Images as usize].1.len(), 1);

        // 재인덱싱 → 이전 청크 무효
        s.replace_file(
            1,
            FileInfo::new(r"C:\docs\a.txt".into(), Kind::Text),
            vec![(Entry { chunk_id: 30, file_id: 1, modality: Modality::Text, locator: 0 }, unit(8, 3))],
        );
        assert_eq!(s.live_chunks(), 2);
        let res = s.search(&unit(8, 1), &Filter::default(), 10);
        assert_eq!(res[Section::Docs as usize].1[0].chunk_id, 30);
        s.remove_file(2);
        assert_eq!(s.live_chunks(), 1);
    }

    #[test]
    fn video_frames_go_to_video_section() {
        assert_eq!(Section::of(Modality::Image, Kind::Video), Section::Videos);
        assert_eq!(Section::of(Modality::Audio, Kind::Video), Section::Videos);
        assert_eq!(Section::of(Modality::Audio, Kind::Audio), Section::Audio);
    }
}
