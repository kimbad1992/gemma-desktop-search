//! 키워드(정확한 단어) 검색: SQLite FTS5
//!
//! 임베딩은 송장 번호·오류 코드·이름 같은 정확한 문자열을 담지 못한다. 그래서 파일 이름·폴더 이름·문서 본문을
//! 키워드 색인에도 넣고, 질의 단어가 들어 있는 파일은 순위를 올린다 (DigUp의 하이브리드 검색과 같은 생각).
//!
//! - 한국어는 조사가 붙어 "계약서에서"가 한 토큰이 된다 → 두 글자 이상 한글 단어는 접두어로 찾는다 ("계약서"*).
//! - 질의 단어에 조사가 붙어 있고("회의록을") 그대로는 색인에 없으면 조사를 떼고 다시 찾는다.
//! - 세 글자 이상 영문 단어가 그대로는 색인에 없으면 접두어로 찾는다 ("grok" → "Grok4").

use crate::index::db::Db;
use crate::search::store::Section;
use std::collections::HashMap;
use std::path::Path;

/// 강조 표시 시작/끝 (snippet()이 넣는 표식)
pub const MARK_START: char = '\u{1}';
pub const MARK_END: char = '\u{2}';

/// 질의 단어 하나
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    /// 정규화된 단어 (소문자)
    pub text: String,
    /// 조사를 뗀 후보 (긴 것부터) — 원래 단어가 색인에 없을 때 차례로 시도
    pub stems: Vec<String>,
}

fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c)
}

fn hangul_count(s: &str) -> usize {
    s.chars().filter(|c| is_hangul(*c)).count()
}

const STOPWORDS: &[&str] = &[
    // 영어
    "a", "an", "the", "of", "in", "on", "at", "to", "for", "with", "and", "or", "my", "me", "that", "this", "these",
    "those", "from", "about", "where", "what", "which", "who", "when", "is", "was", "are", "were", "be", "it", "its",
    "some", "any", "find", "show", "file", "files",
    // 한국어
    "그", "저", "것", "거", "좀", "및", "등", "관련", "대한", "대해", "있는", "없는", "찾아", "찾기", "찾아줘", "보여줘",
    "파일", "어디", "무슨", "어떤",
];

/// 종류를 가리키는 단어: 키워드로는 쓰지 않고(내용이 아니라 찾는 종류를 말함) 해당 섹션을 먼저 보여준다
const KIND_WORDS: &[(&str, Section)] = &[
    ("사진", Section::Images),
    ("이미지", Section::Images),
    ("그림", Section::Images),
    ("스크린샷", Section::Images),
    ("캡처", Section::Images),
    ("캡쳐", Section::Images),
    ("photo", Section::Images),
    ("photos", Section::Images),
    ("picture", Section::Images),
    ("pictures", Section::Images),
    ("image", Section::Images),
    ("images", Section::Images),
    ("screenshot", Section::Images),
    ("screenshots", Section::Images),
    ("영상", Section::Videos),
    ("동영상", Section::Videos),
    ("비디오", Section::Videos),
    ("클립", Section::Videos),
    ("video", Section::Videos),
    ("videos", Section::Videos),
    ("clip", Section::Videos),
    ("movie", Section::Videos),
    ("오디오", Section::Audio),
    ("음성", Section::Audio),
    ("녹음", Section::Audio),
    ("노래", Section::Audio),
    ("음악", Section::Audio),
    ("팟캐스트", Section::Audio),
    ("audio", Section::Audio),
    ("song", Section::Audio),
    ("music", Section::Audio),
    ("podcast", Section::Audio),
    ("recording", Section::Audio),
    ("문서", Section::Docs),
    ("메모", Section::Docs),
    ("pdf", Section::Docs),
    ("document", Section::Docs),
    ("documents", Section::Docs),
    ("note", Section::Docs),
    ("notes", Section::Docs),
];

/// 질의 단어 끝에 붙는 조사·어미 (긴 것부터)
const PARTICLES: &[&str] = &[
    "에서는", "에게서", "으로는", "이라는", "에서", "에게", "한테", "으로", "처럼", "까지", "부터", "보다", "이랑", "하고",
    "라는", "은", "는", "이", "가", "을", "를", "의", "에", "와", "과", "도", "만", "로", "랑",
];

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
}

fn kind_of(word: &str) -> Option<Section> {
    // "사진에서", "영상으로"처럼 조사가 붙은 종류 단어도 인정
    KIND_WORDS.iter().find_map(|(k, s)| {
        let hit = word == *k
            || (hangul_count(k) >= 2
                && word.starts_with(k)
                && PARTICLES.iter().any(|p| word[k.len()..] == **p));
        hit.then_some(*s)
    })
}

/// 질의의 내용 단어 (중복·불용어·종류 단어 제외)
pub fn terms(query: &str) -> Vec<Term> {
    let mut out: Vec<Term> = Vec::new();
    for tok in query.split_whitespace() {
        // "INV-2047", "ERR_CONNECTION_RESET"처럼 기호로 이어진 영문·숫자는 붙어 있는 그대로(구문) 찾는다
        let subs: Vec<String> = words(tok).collect();
        if subs.len() >= 2 && subs.iter().all(|w| w.chars().all(|c| c.is_ascii_alphanumeric())) {
            let text = subs.join(" ");
            if !out.iter().any(|t| t.text == text) {
                out.push(Term { text, stems: Vec::new() });
            }
            continue;
        }
        add_words(&mut out, subs);
    }
    out
}

fn add_words(out: &mut Vec<Term>, words: Vec<String>) {
    for w in words {
        if STOPWORDS.contains(&w.as_str()) || kind_of(&w).is_some() {
            continue;
        }
        let hangul = hangul_count(&w);
        // 한글은 한 글자 단어도 뜻이 있다(꽃, 책). 그 밖에는 두 글자 이상.
        if hangul == 0 && w.chars().count() < 2 {
            continue;
        }
        if out.iter().any(|t| t.text == w) {
            continue;
        }
        let mut stems = Vec::new();
        if hangul >= 2 {
            for p in PARTICLES {
                if let Some(base) = w.strip_suffix(p) {
                    if hangul_count(base) >= 1 && !base.is_empty() && !stems.contains(&base.to_string()) {
                        stems.push(base.to_string());
                    }
                }
            }
        }
        out.push(Term { text: w, stems });
    }
}

/// 질의가 가리키는 종류 (섹션을 먼저 보여주는 데 쓴다)
pub fn kind_hints(query: &str) -> Vec<Section> {
    let mut v = Vec::new();
    for w in words(query) {
        if let Some(s) = kind_of(&w) {
            if !v.contains(&s) {
                v.push(s);
            }
        }
    }
    v
}

/// 코드·번호 조회형 질의 ("INV-2047", "ERR_CONNECTION_RESET 0x80070005", "4815162342"):
/// 단어마다 숫자가 들어 있고 (글자도 있거나 숫자가 네 자리 이상) → 키워드 비중을 높인다
pub fn is_lookup(terms: &[Term]) -> bool {
    !terms.is_empty()
        && terms.iter().all(|t| {
            let digits = t.text.chars().filter(|c| c.is_ascii_digit()).count();
            digits > 0 && (t.text.chars().any(|c| c.is_alphabetic()) || digits >= 4)
        })
}

/// FTS5 패턴. 두 글자 이상 한글 단어는 항상 접두어(조사가 붙어 색인됨), 나머지는 그대로.
pub fn pattern(word: &str, prefix: bool) -> String {
    let prefix = prefix || hangul_count(word) >= 2;
    format!("\"{}\"{}", word.replace('"', ""), if prefix { "*" } else { "" })
}

/// 영문 단어가 그대로는 색인에 없을 때 접두어로 바꿔도 되는지 (세 글자 이상, 숫자만은 아님)
pub fn may_widen(word: &str) -> bool {
    hangul_count(word) == 0 && word.chars().count() >= 3 && !word.chars().all(|c| c.is_ascii_digit())
}

/// 키워드 검색 결과
#[derive(Clone, Debug, Default)]
pub struct KeywordResult {
    /// 강조 발췌용 FTS5 식 (단어들의 OR). 질의 단어가 없으면 None
    pub expr: Option<String>,
    /// 파일별 질의 단어 포함 비율 (이름·폴더·본문 어디든)
    pub coverage: HashMap<i64, f32>,
    /// 파일 이름·폴더 이름에만 들어 있는 비율 ("파일 이름" 섹션용)
    pub name_coverage: HashMap<i64, f32>,
    /// 번호·코드 조회형 질의
    pub lookup: bool,
}

/// 단어마다 색인에 있는 형태(그대로 → 조사 뗀 형태 → 접두어)를 고르고, 파일별 포함 비율을 센다
pub fn search(db: &Db, terms: &[Term]) -> KeywordResult {
    if terms.is_empty() {
        return KeywordResult::default();
    }
    let mut patterns = Vec::new();
    for t in terms {
        let exact = pattern(&t.text, false);
        let chosen = std::iter::once(exact.clone())
            .chain(t.stems.iter().map(|s| pattern(s, false)))
            .chain(may_widen(&t.text).then(|| pattern(&t.text, true)))
            .find(|p| db.fts_exists(p));
        patterns.push(chosen.unwrap_or(exact));
    }
    let n = patterns.len() as f32;
    let mut coverage: HashMap<i64, f32> = HashMap::new();
    let mut name_coverage: HashMap<i64, f32> = HashMap::new();
    for p in &patterns {
        if let Ok(files) = db.fts_files(p, 50_000) {
            for f in files {
                *coverage.entry(f).or_default() += 1.0 / n;
            }
        }
        if let Ok(files) = db.fts_name_files(p, 50_000) {
            for f in files {
                *name_coverage.entry(f).or_default() += 1.0 / n;
            }
        }
    }
    KeywordResult {
        expr: Some(patterns.join(" OR ")),
        coverage,
        name_coverage,
        lookup: is_lookup(terms),
    }
}

/// 카메라·앱이 붙이는 이름 조각 (뜻이 없음)
const MACHINE_WORDS: &[&str] = &[
    "img", "image", "dsc", "dscn", "dscf", "dcim", "mvi", "vid", "video", "pxl", "mov", "gopr", "dji", "screenshot",
    "screen", "shot", "capture", "kakaotalk", "photo", "pic", "scan", "rec", "record", "cam", "camera", "copy", "edit",
    "edited", "final", "new", "file", "untitled", "pano", "burst", "cover", "raw", "jpg", "jpeg", "png", "mp4",
    "samsung", "internet", "chrome", "smartselect", "call", "temp", "tmp", "thumb", "thumbnail",
];

/// 파일 이름에 뜻 있는 단어가 있는지. "IMG_0001", "20170420_183051 (1)", "430", "49a2d27f52689" 같은
/// 기계가 붙인 이름은 의미 벡터가 어떤 질의와도 비슷하게(0.68 안팎) 나와 결과만 어지럽힌다 → 의미 비교에서 뺀다.
/// (키워드로는 계속 찾을 수 있다)
pub fn name_is_meaningful(path: &str) -> bool {
    let stem = Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    // 글자/숫자 경계에서도 나눈다 ("img1234" → img, 1234)
    let mut tokens: Vec<String> = Vec::new();
    for w in stem.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let mut cur = String::new();
        let mut cur_digit = None;
        for c in w.chars() {
            let d = c.is_ascii_digit();
            if cur_digit.is_some_and(|x| x != d) {
                tokens.push(std::mem::take(&mut cur));
            }
            cur.push(c);
            cur_digit = Some(d);
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
    }
    let is_hex = |t: &str| t.len() >= 6 && t.chars().all(|c| c.is_ascii_hexdigit());
    tokens.iter().any(|t| {
        let hangul = hangul_count(t);
        if hangul > 0 {
            return true;
        }
        let letters = t.chars().filter(|c| c.is_alphabetic()).count();
        letters >= 3 && !is_hex(t) && !MACHINE_WORDS.contains(&t.as_str())
    })
}

/// 파일 이름 청크의 키워드 본문: 파일 이름 + 가까운 상위 폴더 이름 3개
pub fn name_body(path: &str) -> String {
    let p = Path::new(path);
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = p.file_name() {
        parts.push(n.to_string_lossy().to_string());
    }
    let mut dir = p.parent();
    for _ in 0..3 {
        let Some(d) = dir else { break };
        let Some(n) = d.file_name() else { break };
        parts.push(n.to_string_lossy().to_string());
        dir = d.parent();
    }
    parts.join(" ")
}

/// snippet() 결과에서 강조 구간을 나눈다: (글자, 강조 여부)
pub fn split_marks(s: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut hot = false;
    for c in s.chars() {
        if c == MARK_START || c == MARK_END {
            if !cur.is_empty() {
                out.push((std::mem::take(&mut cur), hot));
            }
            hot = c == MARK_START;
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push((cur, hot));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(q: &str) -> Vec<String> {
        terms(q).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn terms_drop_stopwords_and_kind_words() {
        assert_eq!(texts("바다에서 찍은 사진"), vec!["바다에서", "찍은"]);
        assert_eq!(texts("a dog on the beach photo"), vec!["dog", "beach"]);
        assert_eq!(texts("INV-2047 invoice"), vec!["inv 2047", "invoice"], "기호로 이어진 번호는 구문 하나");
        assert_eq!(texts("꽃 영상으로"), vec!["꽃"], "한글 한 글자는 유지, 조사 붙은 종류 단어는 제외");
    }

    #[test]
    fn particle_stems() {
        let t = &terms("회의록을")[0];
        assert_eq!(t.text, "회의록을");
        assert_eq!(t.stems, vec!["회의록"]);
        let t = &terms("바다에서")[0];
        assert!(t.stems.contains(&"바다".to_string()));
        assert!(terms("cat")[0].stems.is_empty());
    }

    #[test]
    fn lookup_and_hints() {
        assert!(is_lookup(&terms("K7Q2LM TRX50")));
        assert!(is_lookup(&terms("4815162342")));
        assert!(!is_lookup(&terms("2024 가족 여행")));
        assert_eq!(kind_hints("고양이 영상"), vec![Section::Videos]);
        assert_eq!(kind_hints("바다 사진에서"), vec![Section::Images]);
    }

    #[test]
    fn search_against_db() {
        use crate::extract::{Kind, Modality};
        use crate::index::db::{ChunkRecord, FileRecord, ST_OK};
        let dir = std::env::temp_dir().join(format!("gs-kw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut db = Db::open(&dir.join("t.db")).unwrap();
        let rec = |path: &str, body: &str| FileRecord {
            path: path.into(),
            size: 1,
            mtime: 1,
            kind: Kind::Text,
            status: ST_OK,
            error: None,
            chunks: vec![
                ChunkRecord {
                    modality: Modality::Name,
                    locator: -1,
                    snippet: String::new(),
                    vec: vec![1.0; 4],
                    thumb: None,
                    text: Some(name_body(path)),
                },
                ChunkRecord {
                    modality: Modality::Text,
                    locator: 0,
                    snippet: String::new(),
                    vec: vec![1.0; 4],
                    thumb: None,
                    text: Some(body.into()),
                },
            ],
        };
        let (a, _) = db.write_file(&rec(r"C:\문서\2024 예산 회의록.txt", "마케팅 예산을 15% 증액하기로 했다")).unwrap();
        let (b, _) = db.write_file(&rec(r"C:\문서\메모.txt", "관리번호 INV-2047-KR 제출")).unwrap();

        // 조사 붙은 질의 단어("회의록을")는 조사를 떼고, "예산"은 접두어로 본문 "예산을"까지
        let r = search(&db, &terms("예산 회의록을"));
        assert_eq!(r.coverage.get(&a), Some(&1.0));
        assert!(!r.coverage.contains_key(&b));
        // 번호 조회형
        let r = search(&db, &terms("INV-2047"));
        assert!(r.lookup);
        assert_eq!(r.coverage.get(&b), Some(&1.0));
        // 색인에 없는 단어는 비율 분모에만 들어간다
        let r = search(&db, &terms("예산 고양이"));
        assert_eq!(r.coverage.get(&a), Some(&0.5));
        // 본문에만 있는 단어는 이름 비율에 들어가지 않는다 ("마케팅"은 본문, "회의록"은 이름)
        let r = search(&db, &terms("마케팅 회의록"));
        assert_eq!(r.coverage.get(&a), Some(&1.0));
        assert_eq!(r.name_coverage.get(&a), Some(&0.5));
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn meaningless_names() {
        for p in [
            r"C:\a\430.png",
            r"C:\a\20170420_183051 (1).jpg",
            r"C:\a\IMG_0588.JPG",
            r"C:\a\DSC_0289.JPG",
            r"C:\a\Screenshot_20190208-232221_Samsung Internet.jpg",
            r"C:\a\53176ca4eb9ae76e9219625496a208c291fd4b77.jpeg",
            r"C:\a\1604641145052.jpg",
            r"C:\a\KakaoTalk_20230101_123456789.jpg",
        ] {
            assert!(!name_is_meaningful(p), "{p}");
        }
        for p in [
            r"C:\a\2024 예산 회의록.txt",
            r"C:\a\통화 녹음 조은꽃_190315_181642.m4a",
            r"C:\a\contracts.txt",
            r"C:\a\Screenshot_20230622_200434_Melon.jpg",
            r"C:\a\제주도.jpg",
        ] {
            assert!(name_is_meaningful(p), "{p}");
        }
    }

    #[test]
    fn patterns_and_names() {
        assert_eq!(pattern("계약서", false), "\"계약서\"*");
        assert_eq!(pattern("꽃", false), "\"꽃\"");
        assert_eq!(pattern("grok", true), "\"grok\"*");
        assert!(may_widen("grok") && !may_widen("2047") && !may_widen("ab"));
        assert_eq!(
            name_body(&crate::paths::os_path(r"\\nas\share\사진\2023 제주도\여름\IMG_1234.jpg")),
            "IMG_1234.jpg 여름 2023 제주도 사진"
        );
        assert_eq!(
            split_marks("앞 \u{1}계약\u{2} 뒤"),
            vec![("앞 ".to_string(), false), ("계약".to_string(), true), (" 뒤".to_string(), false)]
        );
    }
}
