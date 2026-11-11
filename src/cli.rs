//! 명령줄 명령: 떠 있는 서비스(`gemma-search serve`)에 HTTP로 묻는다.
//! 화면도 브라우저도 없는 서버에 SSH로 들어가 쓰는 경우를 위한 것이다.
//!
//!   gemma-search search <질의…> [--kinds docs,images] [--path <글자>]… [--min <관련도>] [--limit <개수>] [--json]
//!   gemma-search status [--json]
//!   gemma-search index [--rebuild] [--wait]
//!   gemma-search rescan
//!
//! 공통: `--url http://주소:포트`(환경 변수 GEMMA_SEARCH_URL), `--token <토큰>`(환경 변수 GEMMA_SEARCH_TOKEN).
//! 주소를 주지 않으면 이 기기의 설정(`service_bind`)으로 묻는다. 설정 파일의 토큰은 이 기기의 서비스에만 보낸다
//! (다른 기기의 서비스에 이 기기의 토큰을 흘리지 않게).
//!
//! 결과는 표준 출력, 진행과 오류는 표준 오류로 낸다. 종료 코드: 0 성공, 1 실패, 2 인자가 잘못됨.

use crate::config::Config;
use crate::service::{bind_is_loopback, shown_addr, url_encode, DEFAULT_BIND, SERVER_WAIT, USAGE};
use crate::util::{fmt_num, truncate_chars};
use serde_json::Value;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// 검색 결과를 기본으로 몇 개까지 보여 줄지
const DEFAULT_LIMIT: usize = 20;
/// 상태 묻기·인덱싱 요청의 시간 제한 (검색은 서버가 켜지기를 기다리므로 더 길게 준다)
const SHORT_WAIT: Duration = Duration::from_secs(15);
/// `index --wait`가 상태를 묻는 간격
const POLL: Duration = Duration::from_millis(700);
/// `index --wait`: 서버를 켜지 못하고 있을 때 바로 다시 해 보는지 이만큼만 보고 끝낸다
/// (서비스는 실패할수록 간격을 늘려 다시 해 보므로, 그때까지 붙잡고 있지 않는다)
const START_GRACE: Duration = Duration::from_secs(5);
/// 터미널이 아닐 때(`index --wait`를 파일로 돌린 경우) 진행을 한 줄씩 남기는 간격
const LOG_EVERY: Duration = Duration::from_secs(10);
/// 모든 명령이 받는 값 있는 옵션
const COMMON: &[&str] = &["--url", "--token"];

// ── 인자 ──

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Args {
    values: Vec<(String, String)>,
    flags: Vec<String>,
    pub(crate) words: Vec<String>,
}

impl Args {
    /// 마지막으로 준 값
    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.values.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// 여러 번 준 값을 모두
    fn all(&self, name: &str) -> Vec<&str> {
        self.values.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect()
    }

    pub(crate) fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }
}

/// 서비스에 묻는 명령의 인자: 공통 옵션(--url, --token)을 함께 받는다
fn parse(args: &[String], values: &[&str], flags: &[&str], words: bool) -> Result<Args, String> {
    let with_common: Vec<&str> = values.iter().chain(COMMON).copied().collect();
    parse_args(args, &with_common, flags, words)
}

/// 인자를 나눈다. `values`는 값을 받는 옵션, `flags`는 값 없는 옵션이다. `--이름=값`도 받는다.
/// `--` 뒤는 모두 낱말(검색어)로 본다. `words`가 false면 낱말을 받지 않는다. `-y`는 `--yes`로 본다.
pub(crate) fn parse_args(args: &[String], values: &[&str], flags: &[&str], words: bool) -> Result<Args, String> {
    let mut out = Args::default();
    let mut it = args.iter();
    let mut rest = false;
    while let Some(a) = it.next() {
        if !rest && a == "--" {
            rest = true;
            continue;
        }
        if !rest && a == "-y" && flags.contains(&"--yes") {
            out.flags.push("--yes".to_string());
            continue;
        }
        if rest || !a.starts_with("--") {
            if !words {
                return Err(format!("필요 없는 인자입니다: {a}"));
            }
            out.words.push(a.clone());
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        if values.contains(&name) {
            let v = match inline {
                Some(v) => v,
                None => it.next().cloned().ok_or_else(|| format!("{name} 뒤에 값을 주세요"))?,
            };
            out.values.push((name.to_string(), v));
        } else if flags.contains(&name) && inline.is_none() {
            out.flags.push(name.to_string());
        } else {
            return Err(format!("모르는 옵션입니다: {a}"));
        }
    }
    Ok(out)
}

pub(crate) fn wants_help(args: &[String]) -> bool {
    args.iter().take_while(|a| *a != "--").any(|a| a == "--help" || a == "-h")
}

pub(crate) fn usage_error(msg: &str) -> i32 {
    eprintln!("{msg}\n사용법은 `gemma-search --help`로 봅니다.");
    2
}

/// `--kinds` 값: "docs,이미지" → ["docs", "images"]. 서비스의 섹션 키와 한글 이름을 받는다.
fn parse_kinds(v: &str) -> Result<Vec<&'static str>, String> {
    let mut out = Vec::new();
    for k in v.split(',').map(str::trim).filter(|k| !k.is_empty()) {
        let key = match k.to_lowercase().as_str() {
            "docs" | "doc" | "문서" | "문서내용" | "문서 내용" => "docs",
            "images" | "image" | "이미지" | "사진" => "images",
            "videos" | "video" | "영상" | "동영상" => "videos",
            "audio" | "오디오" | "소리" => "audio",
            "names" | "name" | "이름" | "파일이름" | "파일 이름" => "names",
            _ => return Err(format!("모르는 종류입니다: {k} (docs, images, videos, audio, names 가운데서 고르세요)")),
        };
        if !out.contains(&key) {
            out.push(key);
        }
    }
    if out.is_empty() {
        return Err("--kinds에 종류를 하나 이상 주세요 (예: --kinds docs,images)".to_string());
    }
    Ok(out)
}

fn kind_label(key: &str) -> &'static str {
    match key {
        "docs" => "문서 내용",
        "images" => "이미지",
        "videos" => "영상",
        "audio" => "오디오",
        _ => "파일 이름",
    }
}

// ── 서비스에 묻기 ──

struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// "주소:포트", "http://주소:포트/" → "http://주소:포트"
fn base_url(given: &str) -> String {
    let g = given.trim().trim_end_matches('/');
    if g.contains("://") {
        g.to_string()
    } else {
        format!("http://{g}")
    }
}

/// 주소가 이 기기를 가리키는지 ("http://127.0.0.1:8787", "localhost:8787" 등)
fn is_local(base: &str) -> bool {
    let rest = base.split_once("://").map(|(_, r)| r).unwrap_or(base);
    bind_is_loopback(rest.split('/').next().unwrap_or(rest))
}

/// 보낼 토큰: --token, 환경 변수, 그다음 설정 파일의 토큰. 설정 파일의 것은 이 기기의 서비스에 물을 때만 쓴다.
fn pick_token(flag: Option<&str>, env: Option<&str>, saved: &str, local: bool) -> Option<String> {
    flag.or(env)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| (local && !saved.trim().is_empty()).then(|| saved.trim().to_string()))
}

impl Client {
    fn new(a: &Args, timeout: Duration) -> Self {
        let cfg = Config::load(&crate::config::data_dir());
        let given = a.value("--url").map(str::to_string).or_else(|| env("GEMMA_SEARCH_URL"));
        let base = match &given {
            Some(u) => base_url(u),
            None => {
                let bind = cfg.service_bind.trim();
                format!("http://{}", shown_addr(if bind.is_empty() { DEFAULT_BIND } else { bind }))
            }
        };
        let local = given.is_none() || is_local(&base);
        let token = pick_token(a.value("--token"), env("GEMMA_SEARCH_TOKEN").as_deref(), &cfg.service_token, local);
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Self { base, token, agent }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let mut req = self.agent.get(&format!("{}{path}", self.base));
        if let Some(t) = &self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        self.answer(req.call())
    }

    fn post(&self, path: &str) -> Result<Value, String> {
        let mut req = self.agent.post(&format!("{}{path}", self.base));
        if let Some(t) = &self.token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        self.answer(req.send_empty())
    }

    fn answer(&self, r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, String> {
        let mut r = r.map_err(|e| {
            format!(
                "서비스에 연결할 수 없습니다 ({}): {e}\n서비스(`gemma-search serve`)가 떠 있는지, 주소가 맞는지 확인하세요.",
                self.base
            )
        })?;
        let status = r.status().as_u16();
        let v: Value = r
            .body_mut()
            .read_json()
            .map_err(|e| format!("서비스의 응답을 읽지 못했습니다 (HTTP {status}): {e}"))?;
        if let Some(err) = v["error"].as_str() {
            return Err(if status == 401 {
                format!("{err}. --token이나 환경 변수 GEMMA_SEARCH_TOKEN으로 접속 토큰을 주세요 (서비스 쪽 설정 파일의 service_token).")
            } else {
                err.to_string()
            });
        }
        if status != 200 {
            return Err(format!("서비스가 HTTP {status}로 답했습니다"));
        }
        Ok(v)
    }
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

// ── search ──

pub fn search(args: &[String]) -> i32 {
    if wants_help(args) {
        println!("{USAGE}");
        return 0;
    }
    let a = match parse(args, &["--min", "--kinds", "--path", "--limit"], &["--json"], true) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };
    let q = a.words.join(" ").trim().to_string();
    if q.is_empty() {
        return usage_error("검색어를 주세요. 예: gemma-search search 예산 회의록");
    }
    let kinds = match a.value("--kinds").map(parse_kinds).transpose() {
        Ok(k) => k,
        Err(e) => return usage_error(&e),
    };
    let limit = match a.value("--limit").map(str::parse::<usize>) {
        None => DEFAULT_LIMIT,
        Some(Ok(0)) => usize::MAX,
        Some(Ok(n)) => n,
        Some(Err(_)) => return usage_error("--limit에는 0 이상의 정수를 주세요 (0이면 모두)"),
    };
    if a.value("--min").is_some_and(|m| m.trim().parse::<f32>().is_err()) {
        return usage_error("--min에는 숫자를 주세요 (예: --min 6.5)");
    }
    let path = a.all("--path").join(" ").trim().to_string();

    let mut target = format!("/api/search?q={}", url_encode(&q));
    if let Some(m) = a.value("--min") {
        target += &format!("&min={}", url_encode(m.trim()));
    }
    if let Some(k) = &kinds {
        target += &format!("&kinds={}", k.join(","));
    }
    if !path.is_empty() {
        target += &format!("&path={}", url_encode(&path));
    }
    let client = Client::new(&a, SERVER_WAIT + Duration::from_secs(30));
    let v = match client.get(&target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if a.flag("--json") {
        print_json(&v);
        return 0;
    }
    let mut filters = Vec::new();
    if let Some(k) = &kinds {
        filters.push(format!("종류 {}", k.iter().map(|k| kind_label(k)).collect::<Vec<_>>().join("·")));
    }
    if !path.is_empty() {
        filters.push(format!("경로에 \"{path}\""));
    }
    print_hits(&q, &v, limit, &filters);
    0
}

fn print_hits(q: &str, v: &Value, limit: usize, filters: &[String]) {
    let hits = v["all"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut head = format!("“{q}” · {}개", hits.len());
    if hits.len() > limit {
        head += &format!(" 가운데 {limit}개");
    }
    head += &format!(" · 관련도 {} 이상", v["min"]);
    for f in filters {
        head += &format!(" · {f}");
    }
    head += &format!(" · {:.2}초", v["elapsed_ms"].as_f64().unwrap_or(0.0) / 1000.0);
    println!("{head}");
    if hits.is_empty() {
        println!("찾은 것이 없습니다. --min을 낮추거나 다른 말로 찾아 보세요.");
        return;
    }
    for h in hits.iter().take(limit) {
        let mut tags = vec![h["section_label"].as_str().unwrap_or("").to_string()];
        if let Some(l) = h["locator"].as_str() {
            tags.push(l.to_string());
        }
        if h["keyword"].as_bool() == Some(true) {
            tags.push("검색어 포함".to_string());
        }
        println!(
            "{:>5.1}  {}  [{}]",
            h["score"].as_f64().unwrap_or(0.0),
            h["name"].as_str().unwrap_or(""),
            tags.join(" · ")
        );
        println!("       {}", h["path"].as_str().unwrap_or(""));
        let snippet = match h["highlight"].as_array() {
            Some(parts) => parts.iter().map(|p| p[0].as_str().unwrap_or("")).collect::<String>(),
            None => h["snippet"].as_str().unwrap_or("").to_string(),
        };
        let snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
        if !snippet.is_empty() {
            println!("       {}", truncate_chars(&snippet, 110));
        }
    }
    if hits.len() > limit {
        println!("… {}개 더 있습니다 (--limit 0이면 모두 봅니다)", hits.len() - limit);
    }
}

// ── status ──

pub fn status(args: &[String]) -> i32 {
    if wants_help(args) {
        println!("{USAGE}");
        return 0;
    }
    let a = match parse(args, &[], &["--json"], false) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };
    let client = Client::new(&a, SHORT_WAIT);
    let v = match client.get("/api/status") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if a.flag("--json") {
        print_json(&v);
    } else {
        print!("{}", status_text(&client.base, &v));
    }
    0
}

fn num(v: &Value) -> String {
    fmt_num(v.as_u64().unwrap_or(0))
}

pub(crate) fn human_secs(s: u64) -> String {
    match s {
        0..=59 => format!("{s}초"),
        60..=3599 => format!("{}분", s / 60),
        _ => match (s / 3600, s % 3600 / 60) {
            (h, 0) => format!("{h}시간"),
            (h, m) => format!("{h}시간 {m}분"),
        },
    }
}

/// 끝난 인덱싱 한 줄: "완료: 처리 3 · 변경 없음 11 · 옮김 0 · 삭제 0 · 오류 0"
fn run_summary(ix: &Value) -> String {
    let n = |k: &str| ix[k].as_u64().unwrap_or(0);
    let mut s = format!(
        "{}: 처리 {} · 변경 없음 {} · 옮김 {} · 삭제 {} · 오류 {}",
        ix["state"].as_str().unwrap_or("?"),
        fmt_num(n("processed").saturating_sub(n("unchanged") + n("moved"))),
        fmt_num(n("unchanged")),
        fmt_num(n("moved")),
        fmt_num(n("removed")),
        fmt_num(n("errors")),
    );
    if let Some(m) = ix["message"].as_str().filter(|m| !m.is_empty()) {
        s += &format!(" · {m}");
    }
    s
}

/// 인덱싱 상태 한 줄 (status와 index --wait가 함께 쓴다)
fn indexing_line(ix: &Value) -> String {
    let key = ix["key"].as_str().unwrap_or("");
    let mut s = match key {
        "running" | "paused" | "stopping" => {
            let head = if key == "running" && ix["targeted"] == true {
                "바뀐 파일 반영 중"
            } else {
                ix["state"].as_str().unwrap_or("?")
            };
            if ix["scanning"] == true {
                format!("{head} · 파일 찾는 중 (발견 {}개)", num(&ix["discovered"]))
            } else {
                let pct = ix["fraction"].as_f64().map(|f| format!(" ({:.0}%)", f * 100.0)).unwrap_or_default();
                let mut s = format!("{head} · {}/{}개{pct}", num(&ix["processed"]), num(&ix["discovered"]));
                if let Some(name) = ix["current"]
                    .as_str()
                    .filter(|c| !c.is_empty())
                    .map(|c| Path::new(c).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(c.to_string()))
                {
                    s += &format!(" · {}", truncate_chars(&name, 40));
                }
                s
            }
        }
        "idle" | "" => "대기".to_string(),
        _ => format!("마지막 실행 {}", run_summary(ix)),
    };
    match ix["pending"].as_str() {
        Some(p) => {
            let what = if p == "rebuild" { "전체 재구축" } else { "바뀐 파일 인덱싱" };
            let when = if ix["active"] == true { "지금 것이 끝나면 시작" } else { "검색 서버가 준비되면 시작" };
            s += &format!(" · 요청받은 {what}: {when}");
        }
        None => {}
    }
    s
}

fn status_text(base: &str, v: &Value) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out += &s;
        out.push('\n');
    };
    line(format!("Gemma Search v{} 서비스 · {base}", v["version"].as_str().unwrap_or("?")));
    let sv = &v["server"];
    line(format!(
        "검색 서버: {}",
        match sv["key"].as_str() {
            Some("ready") => "준비됨".to_string(),
            Some("stopped") => "꺼짐 (검색하거나 인덱싱할 때 켭니다)".to_string(),
            Some("failed") => format!("오류: {}", sv["error"].as_str().unwrap_or("")),
            _ => format!("{}…", sv["state"].as_str().unwrap_or("?")),
        }
    ));
    if sv["ready"] != true {
        if let Some(why) = sv["start_error"]["message"].as_str() {
            let ago = sv["start_error"]["ago_secs"].as_u64().unwrap_or(0);
            line(format!("  마지막으로 켜지 못한 까닭 ({} 전): {why}", human_secs(ago)));
        }
    }
    line(format!("인덱스: 파일 {}개 · 벡터 {}개", num(&v["index"]["files"]), num(&v["index"]["vectors"])));
    let ix = &v["indexing"];
    line(format!("인덱싱: {}", indexing_line(ix)));
    if let Some(err) = ix["start_error"].as_str() {
        line(format!("  마지막 요청을 시작하지 못했습니다: {err}"));
    }
    let w = &v["watch"];
    if w["enabled"] == true {
        let mut s = format!("폴더 감시: 켜짐 · 반영 기다리는 변경 {}개", num(&w["pending"]));
        if w["scanning"] == true {
            s += " · 폴더를 훑는 중";
        } else if let Some(n) = w["next_scan_secs"].as_u64() {
            s += &format!(" · 다음 훑기 {} 뒤", human_secs(n));
        }
        if let Some(ago) = w["last_scan"]["ago_secs"].as_u64() {
            s += &format!(" · 마지막 훑기 {} 전 (바뀐 것 {}개)", human_secs(ago), num(&w["last_scan"]["found"]));
        }
        line(s);
        if let Some(note) = w["note"].as_str().filter(|n| !n.is_empty()) {
            line(format!("  {note}"));
        }
    } else {
        line("폴더 감시: 꺼짐 (serve --no-watch로 띄웠거나 검색할 폴더가 없습니다)".to_string());
    }
    // 폴더 목록: 감시 중이면 감시 상태(네트워크·오류)와 함께
    let roots: Vec<String> = match w["roots"].as_array() {
        Some(rs) => rs
            .iter()
            .map(|r| {
                let mut s = format!("  {}", r["path"].as_str().unwrap_or(""));
                if r["network"] == true {
                    s += " (네트워크 폴더: 바뀐 것은 훑을 때 반영)";
                }
                if r["registering"] == true {
                    s += " (감시 등록 중)";
                }
                if let Some(e) = r["error"].as_str() {
                    s += &format!(" (감시 오류: {e})");
                }
                s
            })
            .collect(),
        None => v["roots"]
            .as_array()
            .map(|rs| rs.iter().filter_map(|r| r.as_str()).map(|r| format!("  {r}")).collect())
            .unwrap_or_default(),
    };
    if roots.is_empty() {
        line("검색할 폴더: 없음 (serve --root <폴더>로 정합니다)".to_string());
    } else {
        line(format!("검색할 폴더 {}개:", roots.len()));
        for r in roots {
            line(r);
        }
    }
    line(format!("최소 관련도: {}", v["min_relevance"]));
    for p in v["problems"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
        line(format!("⚠ {}", p.as_str().unwrap_or("")));
    }
    out
}

// ── index ──

pub fn index(args: &[String]) -> i32 {
    if wants_help(args) {
        println!("{USAGE}");
        return 0;
    }
    let a = match parse(args, &[], &["--rebuild", "--wait"], false) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };
    let rebuild = a.flag("--rebuild");
    let client = Client::new(&a, SHORT_WAIT);
    let v = match client.post(if rebuild { "/api/index?rebuild=1" } else { "/api/index" }) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("{}", if rebuild { "전체 재구축을 요청했습니다." } else { "바뀐 파일 인덱싱을 요청했습니다." });
    if !a.flag("--wait") {
        println!("진행은 `gemma-search status`로 봅니다.");
        return 0;
    }
    let Some(before) = v["runs"].as_u64() else {
        eprintln!("이 서비스는 --wait를 모르는 예전 버전입니다. 진행은 `gemma-search status`로 보세요.");
        return 1;
    };
    wait_index(&client, before)
}

/// 요청한 인덱싱이 끝날 때까지 진행을 보여 준다. 터미널이면 한 줄을 고쳐 쓰고, 아니면 가끔 한 줄씩 남긴다.
fn wait_index(c: &Client, before: u64) -> i32 {
    let tty = std::io::stderr().is_terminal();
    let t0 = Instant::now();
    let mut drawn = false;
    let mut last_log: Option<(Instant, String)> = None;
    let clear = |drawn: bool| {
        if drawn {
            eprint!("\r\x1b[2K");
        }
    };
    loop {
        let s = match c.get("/api/status") {
            Ok(s) => s,
            Err(e) => {
                clear(drawn);
                eprintln!("{e}");
                return 1;
            }
        };
        let ix = &s["indexing"];
        let pending = ix["pending"].is_string();
        let active = ix["active"] == true;
        if !pending && !active {
            clear(drawn);
            // 요청이 비었으면 그 요청은 시작됐거나(횟수가 늘었다) 시작하지 못했다(까닭이 남았다)
            if let Some(err) = ix["start_error"].as_str() {
                eprintln!("인덱싱을 시작하지 못했습니다: {err}");
                return 1;
            }
            if ix["runs"].as_u64().unwrap_or(0) > before {
                println!("인덱싱 {}", run_summary(ix));
                return if ix["key"] == "completed" { 0 } else { 1 };
            }
            eprintln!("요청한 인덱싱을 찾지 못했습니다 (그사이 서비스가 다시 시작됐을 수 있습니다).");
            return 1;
        }
        let line = if active {
            indexing_line(ix)
        } else {
            let sv = &s["server"];
            // 켜는 중도 아닌데 켜지 못한 기록이 있으면(경로가 틀렸거나 llama-server가 곧바로 죽는 등) 오래 붙잡지 않는다
            let failing = sv["ready"] != true && sv["key"] != "starting" && sv["start_error"].is_object();
            if failing && t0.elapsed() > START_GRACE {
                clear(drawn);
                eprintln!(
                    "검색 서버(llama-server)를 켜지 못해 인덱싱이 시작되지 않았습니다: {}\n\
                     요청은 서비스에 남아 있어 서버가 켜지면 시작합니다. 빠진 준비물은 `gemma-search status`로 확인하세요.",
                    sv["start_error"]["message"].as_str().unwrap_or("")
                );
                return 1;
            }
            if sv["key"] == "ready" {
                "인덱싱이 시작되기를 기다리는 중".to_string()
            } else {
                format!("검색 서버를 기다리는 중 ({})", sv["state"].as_str().unwrap_or("?"))
            }
        };
        if tty {
            eprint!("\r\x1b[2K{}", truncate_chars(&line, 110));
            let _ = std::io::stderr().flush();
            drawn = true;
        } else if last_log.as_ref().is_none_or(|(at, prev)| *prev != line && at.elapsed() >= LOG_EVERY) {
            eprintln!("{line}");
            last_log = Some((Instant::now(), line));
        }
        std::thread::sleep(POLL);
    }
}

// ── rescan ──

pub fn rescan(args: &[String]) -> i32 {
    if wants_help(args) {
        println!("{USAGE}");
        return 0;
    }
    let a = match parse(args, &[], &[], false) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };
    match Client::new(&a, SHORT_WAIT).post("/api/rescan") {
        Ok(v) if v["ok"] == true => {
            println!("폴더를 훑어 바뀐 파일을 찾습니다. 찾으면 이어서 인덱싱합니다 (`gemma-search status`로 확인).");
            0
        }
        Ok(_) => {
            eprintln!("폴더 감시가 꺼져 있어 훑지 않았습니다 (serve --no-watch). 바뀐 파일을 반영하려면 `gemma-search index`를 쓰세요.");
            1
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn splits_words_values_and_flags() {
        let a = parse(&s(&["예산", "--min", "6.5", "회의록", "--json", "--kinds=docs,images"]), &["--min", "--kinds"], &["--json"], true).unwrap();
        assert_eq!(a.words, s(&["예산", "회의록"]));
        assert_eq!(a.value("--min"), Some("6.5"));
        assert_eq!(a.value("--kinds"), Some("docs,images"));
        assert!(a.flag("--json"));
        // 공통 옵션은 어느 명령에서나 받는다
        let a = parse(&s(&["--url", "nas:8787", "--token=abc"]), &[], &[], false).unwrap();
        assert_eq!((a.value("--url"), a.value("--token")), (Some("nas:8787"), Some("abc")));
        // 여러 번 준 값
        let a = parse(&s(&["q", "--path", "보관", "--path", "2024"]), &["--path"], &[], true).unwrap();
        assert_eq!(a.all("--path"), vec!["보관", "2024"]);
        // -- 뒤는 모두 검색어
        let a = parse(&s(&["--", "--json", "-h"]), &[], &["--json"], true).unwrap();
        assert_eq!(a.words, s(&["--json", "-h"]));
        assert!(!a.flag("--json"));
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(parse(&s(&["--nope"]), &[], &[], true).unwrap_err().contains("--nope"));
        assert!(parse(&s(&["q", "--min"]), &["--min"], &[], true).unwrap_err().contains("--min"));
        assert!(parse(&s(&["extra"]), &[], &[], false).unwrap_err().contains("extra"));
        assert!(parse(&s(&["--json=1"]), &[], &["--json"], true).is_err());
        assert!(wants_help(&s(&["q", "--help"])));
        assert!(!wants_help(&s(&["--", "-h"])));
    }

    #[test]
    fn kinds_take_keys_and_korean_names() {
        assert_eq!(parse_kinds("docs, 이미지,영상").unwrap(), vec!["docs", "images", "videos"]);
        assert_eq!(parse_kinds("Audio,소리,names").unwrap(), vec!["audio", "names"]);
        assert!(parse_kinds("pdf").unwrap_err().contains("pdf"));
        assert!(parse_kinds(" , ").is_err());
    }

    #[test]
    fn config_token_goes_only_to_this_machine() {
        assert_eq!(base_url("nas:8787/"), "http://nas:8787");
        assert_eq!(base_url("https://nas.local:8787"), "https://nas.local:8787");
        assert!(is_local("http://127.0.0.1:8787"));
        assert!(is_local("http://localhost:8787/"));
        assert!(is_local("http://[::1]:8787"));
        assert!(!is_local("http://192.168.0.10:8787"));
        assert!(!is_local("http://nas:8787"));
        // --token > 환경 변수 > 설정 파일(이 기기일 때만)
        assert_eq!(pick_token(Some("a"), Some("b"), "c", true).as_deref(), Some("a"));
        assert_eq!(pick_token(None, Some("b"), "c", true).as_deref(), Some("b"));
        assert_eq!(pick_token(None, None, "c", true).as_deref(), Some("c"));
        assert_eq!(pick_token(None, None, "c", false), None);
        assert_eq!(pick_token(Some(" "), None, " ", true), None);
    }

    #[test]
    fn describes_indexing() {
        assert_eq!(human_secs(42), "42초");
        assert_eq!(human_secs(600), "10분");
        assert_eq!(human_secs(7200), "2시간");
        assert_eq!(human_secs(5400), "1시간 30분");
        let running = json!({
            "key": "running", "state": "인덱싱 진행 중", "active": true, "targeted": false, "scanning": false,
            "processed": 7, "discovered": 14, "fraction": 0.5, "current": "/srv/문서/회의록.txt", "pending": null,
        });
        assert_eq!(indexing_line(&running), "인덱싱 진행 중 · 7/14개 (50%) · 회의록.txt");
        let done = json!({
            "key": "completed", "state": "완료", "active": false, "processed": 14, "unchanged": 11, "moved": 1,
            "removed": 2, "errors": 0, "message": "", "pending": "rebuild",
        });
        assert_eq!(run_summary(&done), "완료: 처리 2 · 변경 없음 11 · 옮김 1 · 삭제 2 · 오류 0");
        assert_eq!(
            indexing_line(&done),
            "마지막 실행 완료: 처리 2 · 변경 없음 11 · 옮김 1 · 삭제 2 · 오류 0 · 요청받은 전체 재구축: 검색 서버가 준비되면 시작"
        );
        assert_eq!(indexing_line(&json!({ "key": "idle", "pending": null })), "대기");
    }
}
