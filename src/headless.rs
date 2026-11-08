//! GUI 없이 전체 흐름을 점검하는 모드 (개발/검증용)
//!
//!   gemma-search.exe --headless-test <폴더> [질의1] [질의2] ...
//!
//! 서버 시작 → 자가진단 → 인덱싱(일시정지/재개 포함) → 재실행(변경 없음 확인) → 검색 → 서버 중지

use crate::config::Config;
use crate::index::indexer::{IndexParams, Indexer};
use crate::index::progress::RunState;
use crate::search::query::{self, QueryKind};
use crate::search::store::{Filter, VectorStore};
use crate::server::{SelfTest, ServerManager};
use crate::util::fmt_ts;
use parking_lot::{Mutex, RwLock};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 서버 안정성 시험: 폴더를 임시 DB에 반복 재구축하면서 서버가 죽는지 본다.
///   gemma-search.exe --stress <폴더> <초>
pub fn stress(args: &[String]) -> i32 {
    let (Some(dir), Some(secs)) = (args.first(), args.get(1).and_then(|s| s.parse::<u64>().ok())) else {
        eprintln!("사용법: --stress <폴더> <초>");
        return 2;
    };
    let limit = Duration::from_secs(secs);
    let data_dir = std::env::temp_dir().join(format!("gemma-search-stress-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut cfg = Config::load(&crate::config::data_dir());
    cfg.roots = vec![dir.clone()];
    let server = ServerManager::default();
    if let Err(e) = server.start(&cfg) {
        println!("서버 시작 실패: {e}");
        return 1;
    }
    let cmd = server.shared.lock().command_line.clone();
    println!("명령줄: {cmd}");
    let t0 = Instant::now();
    let caps = loop {
        if let Some(c) = server.capabilities() {
            break c;
        }
        if t0.elapsed() > Duration::from_secs(120) || matches!(server.state(), crate::server::ServerState::Failed(_)) {
            println!("서버 준비 실패: {:?}", server.state());
            return 1;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let store = Arc::new(RwLock::new(VectorStore::new(cfg.search_dim)));
    store.write().loaded = true;
    let mut indexer = Indexer::default();
    let start = Instant::now();
    let mut total = 0u64;
    let mut pass = 0;
    let mut crashed = None;
    let mut last_report = Instant::now();
    'outer: while start.elapsed() < limit {
        pass += 1;
        indexer
            .start(IndexParams {
                cfg: cfg.clone(),
                caps: caps.clone(),
                port: cfg.port,
                db_path: data_dir.join("index.db"),
                data_dir: data_dir.clone(),
                store: store.clone(),
                full_rebuild: true,
            })
            .unwrap();
        while indexer.is_active() {
            std::thread::sleep(Duration::from_millis(500));
            if let crate::server::ServerState::Failed(m) = server.state() {
                crashed = Some(m);
                indexer.stop();
            }
            if last_report.elapsed() > Duration::from_secs(30) {
                let p = indexer.progress.lock().clone();
                println!(
                    "  {:>4}초  {}회차  처리 {}  벡터 {}",
                    start.elapsed().as_secs(),
                    pass,
                    total + p.processed,
                    p.chunks
                );
                last_report = Instant::now();
            }
            if start.elapsed() > limit {
                indexer.stop();
            }
        }
        total += indexer.progress.lock().processed;
        if crashed.is_some() {
            break 'outer;
        }
    }
    let elapsed = start.elapsed().as_secs();
    match &crashed {
        Some(m) => {
            println!("결과: 서버 충돌 ({elapsed}초, 처리 {total}개) — {m}");
            for l in server.logs.lock().iter().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
                if l.contains(" E ") || l.contains("error") || l.contains("CUDA") || l.contains("[앱]") {
                    println!("  {l}");
                }
            }
        }
        None => println!("결과: 충돌 없음 ({elapsed}초, {pass}회차, 처리 {total}개)"),
    }
    server.stop();
    let _ = std::fs::remove_dir_all(&data_dir);
    if crashed.is_some() { 3 } else { 0 }
}

/// 이미 떠 있는 서버(설정 포트)에 대해 클라이언트 왕복 시간 측정
pub fn bench(file: &str) -> i32 {
    use crate::embed::client::{EmbedClient, EmbedInput};
    let cfg = Config::load(&crate::config::data_dir());
    let client = EmbedClient::new(cfg.port);
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let chunks: Vec<String> = crate::extract::text::chunk_text(&text, 1000, 150, 16)
        .into_iter()
        .map(|c| crate::embed::prompt::document("bench", &c))
        .collect();
    println!("청크 {}개", chunks.len());
    let t = Instant::now();
    for c in &chunks {
        client.count_tokens(c).unwrap();
    }
    println!("tokenize x{}: {}ms", chunks.len(), t.elapsed().as_millis());
    let t = Instant::now();
    client.embed(&[EmbedInput::Text(chunks[0].clone())]).unwrap();
    println!("embed 1개: {}ms", t.elapsed().as_millis());
    let t = Instant::now();
    let inputs: Vec<EmbedInput> = chunks.iter().cloned().map(EmbedInput::Text).collect();
    client.embed(&inputs).unwrap();
    println!("embed {}개 한 요청: {}ms", inputs.len(), t.elapsed().as_millis());
    for threads in [4usize, 8] {
        let t = Instant::now();
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| client.embed(&inputs).unwrap());
            }
        });
        let ms = t.elapsed().as_millis().max(1);
        println!(
            "embed {}개 x{threads} 병렬: {ms}ms → {:.0} 벡터/초",
            inputs.len(),
            (inputs.len() * threads) as f64 * 1000.0 / ms as f64
        );
    }
    0
}

pub fn run(args: &[String]) -> i32 {
    let Some(dir) = args.first() else {
        eprintln!("사용법: --headless-test <폴더> [질의...]");
        return 2;
    };
    let queries: Vec<String> = args[1..].to_vec();
    let data_dir = std::env::temp_dir().join(format!("gemma-search-headless-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data_dir);
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut cfg = Config::load(&crate::config::data_dir());
    cfg.roots = vec![dir.clone()];
    println!("데이터 폴더(임시): {}", data_dir.display());

    let server = ServerManager::default();
    if let Err(e) = server.start(&cfg) {
        println!("서버 시작 실패: {e}");
        return 1;
    }
    let t0 = Instant::now();
    let caps = loop {
        if let Some(c) = server.capabilities() {
            break c;
        }
        if let crate::server::ServerState::Failed(m) = server.state() {
            println!("서버 실패: {m}");
            for l in server.logs.lock().iter().rev().take(15) {
                println!("  {l}");
            }
            return 1;
        }
        if t0.elapsed() > Duration::from_secs(180) {
            println!("서버 준비 시간 초과");
            return 1;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let pid = server.shared.lock().pid;
    println!(
        "서버 준비 {:.1}초 (PID {:?}) · 자가진단: 텍스트 {:?} | 이미지 {:?} (배치 {}) | 오디오 {:?}",
        t0.elapsed().as_secs_f32(),
        pid,
        caps.text,
        caps.image,
        caps.image_batch,
        caps.audio
    );
    if !matches!(server.shared.lock().selftest, SelfTest::Done(_)) {
        return 1;
    }

    let db_path = data_dir.join("index.db");
    let store = Arc::new(RwLock::new(VectorStore::new(cfg.search_dim)));
    store.write().loaded = true;
    let mut indexer = Indexer::default();
    let params = |full: bool| IndexParams {
        cfg: cfg.clone(),
        caps: caps.clone(),
        port: cfg.port,
        db_path: db_path.clone(),
        data_dir: data_dir.clone(),
        store: store.clone(),
        full_rebuild: full,
    };

    // 1회차: 일시정지/재개 확인 포함
    indexer.start(params(false)).unwrap();
    let mut paused_once = false;
    let mut last = String::new();
    loop {
        let p = indexer.progress.lock().clone();
        let line = format!(
            "  [{}] 발견 {} · 처리 {} · 변경없음 {} · 이름만 {} · 오류 {} · 벡터 {}{}",
            p.state.label(),
            p.discovered,
            p.processed,
            p.unchanged,
            p.name_only,
            p.errors,
            p.chunks,
            if p.scanning { " · 탐색 중" } else { "" }
        );
        if line != last {
            println!("{line}");
            last = line;
        }
        if !paused_once && p.processed >= 2 && p.state == RunState::Running {
            indexer.pause();
            let before = indexer.progress.lock().processed;
            std::thread::sleep(Duration::from_millis(1500));
            let after = indexer.progress.lock().processed;
            println!(
                "  일시정지 검사: 1.5초 동안 처리 {} → {} ({})",
                before,
                after,
                if after <= before + 4 { "정지됨" } else { "정지 안 됨!" }
            );
            indexer.resume();
            paused_once = true;
        }
        if !indexer.is_active() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let p = indexer.progress.lock().clone();
    println!(
        "1회차 결과: {} ({:.1}초) · 종류별 인덱싱 {:?}",
        p.state.label(),
        p.elapsed().as_secs_f32(),
        p.indexed_by_kind
    );
    if !p.message.is_empty() {
        println!("  메시지: {}", p.message);
    }
    for (path, msg) in &p.error_list {
        println!("  오류: {path} — {msg}");
    }

    // 2회차: 전부 변경 없음이어야 함
    indexer.start(params(false)).unwrap();
    while indexer.is_active() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let p2 = indexer.progress.lock().clone();
    println!(
        "2회차(변경분): {} · 처리 {} 중 변경없음 {} ({})",
        p2.state.label(),
        p2.processed,
        p2.unchanged,
        if p2.unchanged == p2.processed { "OK" } else { "다시 처리된 파일 있음" }
    );

    // 3회차: 전체 재구축 도중 중지 → 중지 상태로 끝나야 함
    indexer.start(params(true)).unwrap();
    let t_stop = loop {
        let p = indexer.progress.lock().clone();
        if p.processed >= 1 || !indexer.is_active() {
            indexer.stop();
            break Instant::now();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    while indexer.is_active() {
        std::thread::sleep(Duration::from_millis(20));
    }
    let p3 = indexer.progress.lock().clone();
    println!(
        "3회차(재구축 중 중지): {} · {}ms 만에 멈춤 · 처리 {}/{}",
        p3.state.label(),
        t_stop.elapsed().as_millis(),
        p3.processed,
        p3.discovered
    );
    // 중지된 재구축을 마저 끝내기 (변경분 모드로 이어서)
    indexer.start(params(false)).unwrap();
    while indexer.is_active() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let p4 = indexer.progress.lock().clone();
    println!(
        "4회차(이어서): {} · 새로 처리 {} · 변경없음 {}",
        p4.state.label(),
        p4.processed - p4.unchanged,
        p4.unchanged
    );

    // 검색 (재구축 후 저장소는 기록 스레드가 채움)
    let db = Arc::new(Mutex::new(crate::index::db::Db::open(&db_path).unwrap()));
    let mut qs: Vec<QueryKind> = queries.into_iter().map(QueryKind::Text).collect();
    if qs.is_empty() {
        qs.push(QueryKind::Text("test".into()));
    }
    // 이미지 질의: 폴더 안 첫 jpg
    if let Some(img) = walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .find(|e| e.path().extension().map(|x| x.eq_ignore_ascii_case("jpg")).unwrap_or(false))
    {
        qs.push(QueryKind::Image(PathBuf::from(img.path())));
    }
    for q in qs {
        let o = query::run(cfg.port, &store, &db, q, Filter::default(), 3, cfg.ffmpeg());
        println!("\n질의 {} ({:.0}ms)", o.label, o.elapsed.as_secs_f32() * 1000.0);
        if let Some(e) = &o.error {
            println!("  오류: {e}");
        }
        for (sec, hits) in &o.sections {
            if hits.is_empty() {
                continue;
            }
            let s: Vec<String> = hits
                .iter()
                .map(|h| {
                    let loc = match (h.hit.modality, h.hit.kind) {
                        (crate::extract::Modality::Image, crate::extract::Kind::Video)
                        | (crate::extract::Modality::Video, _)
                        | (crate::extract::Modality::Audio, _) => format!("@{}", fmt_ts(h.hit.locator)),
                        _ => String::new(),
                    };
                    format!("{}{} {:.3}", h.file_name, loc, h.hit.score)
                })
                .collect();
            println!("  {:<6} {}", sec.label(), s.join(" | "));
        }
    }

    // 중지 및 프로세스 정리 확인
    server.stop();
    std::thread::sleep(Duration::from_millis(500));
    if let Some(pid) = pid {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();
        println!(
            "\n서버 중지 후 PID {pid}: {}",
            if out.contains("llama") { "아직 살아 있음!" } else { "종료됨" }
        );
    }
    let _ = std::fs::remove_dir_all(&data_dir);
    0
}
