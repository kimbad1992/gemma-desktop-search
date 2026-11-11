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

/// 시험 명령의 서버 인자: 이 기기의 포트(`8090`)나 주소(`http://host:8090`)
fn target_arg(arg: Option<&String>) -> Option<crate::embed::client::EmbedTarget> {
    let arg = arg?;
    match crate::embed::client::EmbedTarget::parse(arg, "") {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("{e}");
            None
        }
    }
}

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
    // 설정 포트를 못 써서 다른 포트로 떴을 수 있다
    cfg.port = server.port();
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
                target: server.target(),
                server_sig: server.signature(&cfg),
                db_path: data_dir.join("index.db"),
                data_dir: data_dir.clone(),
                store: store.clone(),
                full_rebuild: true,
                only: None,
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

/// 창 없이 변경분 인덱싱 한 번: 서버 시작 → 인덱싱(모델이 바뀌었으면 호환성 검사 포함) → 결과 출력 → 서버 중지
///   gemma-search.exe --index-once   (데이터 폴더는 GEMMA_SEARCH_DATA로 바꿀 수 있음)
/// 서비스나 창이 있는 앱이 같은 데이터 폴더를 쓰는 중이면 하지 않는다 (인덱스를 둘이 함께 고치지 않게).
pub fn index_once() -> i32 {
    let data_dir = crate::config::data_dir();
    let _lock = match crate::config::instance_lock(&data_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            if e.starts_with(crate::config::LOCK_BUSY) {
                eprintln!(
                    "서비스가 떠 있으면 `gemma-search index --wait`로, 창이 있는 앱이 떠 있으면 앱의 인덱스 화면에서 인덱싱하세요."
                );
            }
            return 1;
        }
    };
    let cfg = Config::load(&data_dir);
    println!("데이터 폴더: {}", data_dir.display());
    let server = ServerManager::default();
    if let Err(e) = server.bring_up(&cfg) {
        println!("서버 시작 실패: {e}");
        return 1;
    }
    println!("임베딩 서버: {}", server.place());
    let t0 = Instant::now();
    let caps = loop {
        if let Some(c) = server.capabilities() {
            break c;
        }
        if t0.elapsed() > Duration::from_secs(180) || matches!(server.state(), crate::server::ServerState::Failed(_)) {
            println!("서버 준비 실패: {:?}", server.state());
            return 1;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let store = Arc::new(RwLock::new(VectorStore::new(cfg.search_dim)));
    store.write().loaded = true;
    let mut indexer = Indexer::default();
    if let Err(e) = indexer.start(IndexParams {
        cfg: cfg.clone(),
        caps,
        target: server.target(),
        server_sig: server.signature(&cfg),
        db_path: data_dir.join("index.db"),
        data_dir: data_dir.clone(),
        store,
        full_rebuild: false,
        only: None,
    }) {
        // 검색할 폴더가 없는 경우 등
        println!("인덱싱을 시작하지 못했습니다: {e}");
        server.stop();
        return 1;
    }
    let mut last = Instant::now();
    while indexer.is_active() {
        std::thread::sleep(Duration::from_millis(300));
        if last.elapsed() > Duration::from_secs(20) {
            let p = indexer.progress.lock().clone();
            println!(
                "  {:>5}초  발견 {}  처리 {}  변경없음 {}  오류 {}  | {}",
                t0.elapsed().as_secs(),
                p.discovered,
                p.processed,
                p.unchanged,
                p.errors,
                crate::util::truncate_chars(&p.current, 80)
            );
            last = Instant::now();
        }
    }
    let p = indexer.progress.lock().clone();
    println!(
        "결과: {} · 발견 {} · 새로 처리 {} · 변경없음 {} · 오류 {} · 경고 {} · {}초",
        p.state.label(),
        p.discovered,
        p.processed - p.unchanged,
        p.unchanged,
        p.errors,
        p.warnings,
        t0.elapsed().as_secs()
    );
    if !p.message.is_empty() {
        println!("메시지: {}", p.message);
    }
    for (path, msg) in p.error_list.iter().take(30) {
        println!("  - {path}: {msg}");
    }
    server.stop();
    if p.state == RunState::Completed { 0 } else { 1 }
}

/// 실제 인덱스로 검색만: 섹션별로 화면에 보일 개수와, 그중 최소 일치도 아래인데 검색어 때문에 보이는 개수
///   gemma-search.exe --query-once <최소일치도> <질의>...
pub fn query_once(args: &[String]) -> i32 {
    let Some(min) = args.first().and_then(|s| s.parse::<f32>().ok()) else {
        eprintln!("사용법: --query-once <최소일치도> <질의>...");
        return 2;
    };
    let data_dir = crate::config::data_dir();
    let cfg = Config::load(&data_dir);
    let server = ServerManager::default();
    if let Err(e) = server.bring_up(&cfg) {
        println!("서버 시작 실패: {e}");
        return 1;
    }
    let target = server.target();
    let t0 = Instant::now();
    while server.capabilities().is_none() {
        if t0.elapsed() > Duration::from_secs(180) {
            println!("서버 준비 실패");
            return 1;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let db = Arc::new(Mutex::new(crate::index::db::Db::open(&data_dir.join("index.db")).unwrap()));
    let mut st = VectorStore::new(cfg.search_dim);
    st.fill_from_db(&db.lock()).unwrap();
    st.loaded = true;
    let store = Arc::new(RwLock::new(st));
    for q in &args[1..] {
        // 종류별 평균 점수 (모든 벡터 기준)
        if let Ok(mut v) = crate::embed::client::EmbedClient::new(&target)
            .embed(&[crate::embed::client::EmbedInput::Text(crate::embed::prompt::query(q))])
        {
            let means = store.read().modality_means(&v.remove(0));
            let s: Vec<String> = means.iter().map(|(m, mean, n)| format!("{m:?} {mean:.3}({n})")).collect();
            println!("  평균: {}", s.join(" · "));
        }
        let o = query::run(&target, &store, &db, QueryKind::Text(q.clone()), Filter::default(), cfg.top_k, min, cfg.ffmpeg());
        let mut line = format!("{q:<22} ({:.0}ms) |", o.elapsed.as_secs_f32() * 1000.0);
        for (sec, hits) in &o.sections {
            let shown = hits.iter().filter(|h| h.visible(min)).count();
            let by_kw = hits.iter().filter(|h| h.hit.score < min && h.keyword_match()).count();
            if shown > 0 {
                line += &format!(" {} {shown}(검색어로 {by_kw})", sec.label());
            }
        }
        println!("{line}");
        // 화면의 "전체" 목록과 같은 방식으로 상위 8개
        let visible: Vec<(crate::search::store::Section, Vec<&query::HitView>)> = o
            .sections
            .iter()
            .map(|(s, h)| (*s, crate::search::query::passing(h, min)))
            .filter(|(_, h)| !h.is_empty())
            .collect();
        for (i, (sec, h)) in crate::search::query::unified(&visible, &o.kind_hints).iter().take(8).enumerate() {
            println!(
                "    {:>2}. [{}] {} 일치도 {:.3} (의미 {:.3}, 코사인 {:.3}) 단어{:.2}",
                i + 1,
                sec.label(),
                h.file_name,
                h.hit.score,
                h.hit.meaning,
                h.hit.cosine,
                h.hit.coverage
            );
        }
    }
    server.stop();
    0
}

/// 사양 확인과 추천 설정 출력 (첫 실행 마법사와 같은 내용)
///   gemma-search.exe --sysinfo
pub fn sysinfo() -> i32 {
    let cfg = Config::load(&crate::config::data_dir());
    let t = Instant::now();
    let info = crate::sysinfo::probe(&crate::config::app_dir(), cfg.ffmpeg().is_some());
    let plan = crate::sysinfo::recommend(&info);
    println!("확인 {:.1}초", t.elapsed().as_secs_f32());
    for g in &info.gpus {
        println!("  GPU: {} · {}MB · {:?} · 드라이버 {:?}", g.name, g.vram_mb, g.vendor, g.driver);
    }
    for (lv, k, v) in &plan.checks {
        println!("[{lv:?}] {k}: {v}");
    }
    println!("추천: {}", plan.summary);
    0
}

/// 두 서버(예: BF16과 Q8)의 처리 속도 비교: 같은 입력(텍스트·이미지·영상 구간)을 각각 임베딩해 시간 측정
///   gemma-search.exe --bench-compare <포트A> <포트B> <폴더>
pub fn bench_compare(args: &[String]) -> i32 {
    use crate::embed::client::{EmbedClient, EmbedInput};
    let (Some(pa), Some(pb), Some(dir)) = (target_arg(args.first()), target_arg(args.get(1)), args.get(2)) else {
        eprintln!("사용법: --bench-compare <포트나 주소 A> <포트나 주소 B> <폴더>");
        return 2;
    };
    let cfg = Config::load(&crate::config::data_dir());
    let ff = cfg.ffmpeg();
    let (mut images, mut videos) = (Vec::new(), Vec::new());
    for e in walkdir::WalkDir::new(dir).into_iter().flatten().filter(|e| e.file_type().is_file()) {
        match crate::extract::classify(e.path()) {
            crate::extract::Kind::Image if images.len() < 32 => {
                if let Ok(img) = crate::extract::image::load_image(e.path(), ff.as_deref()) {
                    if let Ok((j, _)) = crate::extract::image::model_and_thumb(&img) {
                        images.push(EmbedInput::Image(j));
                    }
                }
            }
            crate::extract::Kind::Video if videos.len() < 4 => {
                if let Some(f) = ff.as_deref() {
                    if let Ok(mp4) = crate::extract::media::video_segment(f, e.path(), 0.0, 30.0) {
                        videos.push(EmbedInput::Video(mp4));
                    }
                }
            }
            _ => {}
        }
        if images.len() >= 32 && videos.len() >= 4 {
            break;
        }
    }
    let readme = std::fs::read_to_string("README.md").unwrap_or_default();
    let texts: Vec<EmbedInput> = crate::extract::text::chunk_text(&readme, 1000, 150, 64)
        .into_iter()
        .map(|c| EmbedInput::Text(crate::embed::prompt::document("bench", &c)))
        .collect();
    println!("입력: 텍스트 {}개, 이미지 {}개, 영상 30초 구간 {}개", texts.len(), images.len(), videos.len());
    for (label, target) in [("A", pa), ("B", pb)] {
        let c = EmbedClient::new(&target);
        // 한 번 데워 둔다
        let _ = c.embed(&texts[..1.min(texts.len())]);
        let t = Instant::now();
        for b in texts.chunks(16) {
            c.embed(b).expect("텍스트");
        }
        let tt = t.elapsed();
        let t = Instant::now();
        for i in &images {
            c.embed(std::slice::from_ref(i)).expect("이미지");
        }
        let ti = t.elapsed();
        let t = Instant::now();
        for v in &videos {
            c.embed(std::slice::from_ref(v)).expect("영상");
        }
        let tv = t.elapsed();
        let per = |d: Duration, n: usize| d.as_secs_f64() * 1000.0 / n.max(1) as f64;
        println!(
            "{label} ({target}): 텍스트 {:.1}ms/개 · 이미지 {:.1}ms/장 · 영상 구간 {:.0}ms/개",
            per(tt, texts.len()),
            per(ti, images.len()),
            per(tv, videos.len())
        );
    }
    0
}

/// 호환성 검사: 인덱스 표본을 지정한 포트의 서버로 다시 임베딩해 저장된 벡터와 비교
///   gemma-search.exe --compat <포트> [이름 텍스트 이미지 영상 오디오 표본 수]
pub fn compat(args: &[String]) -> i32 {
    use crate::embed::client::EmbedClient;
    use crate::index::compat::{check, Budget};
    let Some(target) = target_arg(args.first()) else {
        eprintln!("사용법: --compat <포트나 주소> [이름 텍스트 이미지 영상 오디오]");
        return 2;
    };
    let n = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let d = Budget::default();
    let budget = Budget {
        names: n(1, d.names),
        text: n(2, d.text),
        images: n(3, d.images),
        video: n(4, d.video),
        audio: n(5, d.audio),
    };
    let dir = crate::config::data_dir();
    let cfg = Config::load(&dir);
    let db = match crate::index::db::Db::open(&dir.join("index.db")) {
        Ok(db) => db,
        Err(e) => {
            println!("DB 열기 실패: {e}");
            return 1;
        }
    };
    match check(&db, &cfg, &EmbedClient::new(&target), budget) {
        Ok(r) => {
            for s in &r.stats {
                println!(
                    "{:<6} 비교 {:>3}개  평균 {:.5}  최소 {:.5}  건너뜀 {}",
                    s.label, s.n, s.mean, s.min, s.skipped
                );
            }
            println!("소요 {:.1}초", r.elapsed.as_secs_f64());
            0
        }
        Err(e) => {
            println!("검사 실패: {e}");
            1
        }
    }
}

/// 질의 쪽 비교: 같은 질의를 두 서버로 임베딩해 기존 인덱스에서 섹션별 Top-10이 얼마나 겹치는지
///   gemma-search.exe --compare-queries <포트A> <포트B> <질의>...
pub fn compare_queries(args: &[String]) -> i32 {
    use crate::embed::client::{EmbedClient, EmbedInput};
    use crate::search::store::Section;
    let (Some(pa), Some(pb)) = (target_arg(args.first()), target_arg(args.get(1))) else {
        eprintln!("사용법: --compare-queries <포트나 주소 A> <포트나 주소 B> <질의>...");
        return 2;
    };
    let dir = crate::config::data_dir();
    let cfg = Config::load(&dir);
    let db = crate::index::db::Db::open(&dir.join("index.db")).expect("DB");
    let mut store = VectorStore::new(cfg.search_dim);
    store.fill_from_db(&db).expect("벡터 적재");
    println!("벡터 {}개 적재", store.live_chunks());
    let (ca, cb) = (EmbedClient::new(&pa), EmbedClient::new(&pb));
    let k = 10;
    let mut totals = [(0usize, 0usize); 5];
    let mut top1_same = [(0usize, 0usize); 5];
    for q in &args[2..] {
        let inp = EmbedInput::Text(crate::embed::prompt::query(q));
        let va = ca.embed(std::slice::from_ref(&inp)).expect("A").remove(0);
        let vb = cb.embed(std::slice::from_ref(&inp)).expect("B").remove(0);
        let ra = store.search(&va, &Filter::default(), k, None);
        let rb = store.search(&vb, &Filter::default(), k, None);
        let mut line = format!("{q:<24} 질의 cos {:.4} |", crate::embed::vecmath::dot(&va, &vb));
        for ((sec, ha), (_, hb)) in ra.iter().zip(rb.iter()) {
            if ha.is_empty() {
                continue;
            }
            let sa: std::collections::HashSet<i64> = ha.iter().map(|h| h.file_id).collect();
            let common = hb.iter().filter(|h| sa.contains(&h.file_id)).count();
            let i = *sec as usize;
            totals[i].0 += common;
            totals[i].1 += ha.len().min(hb.len());
            top1_same[i].1 += 1;
            if ha.first().map(|h| h.file_id) == hb.first().map(|h| h.file_id) {
                top1_same[i].0 += 1;
            }
            let ds: f32 = ha.iter().zip(hb.iter()).map(|(a, b)| (a.score - b.score).abs()).fold(0.0, f32::max);
            line += &format!(" {} {}/{} Δ{:.3}", sec.label(), common, ha.len().min(hb.len()), ds);
        }
        println!("{line}");
    }
    println!("── 합계 (Top-{k} 겹침, 1위 일치)");
    for s in Section::ALL {
        let (c, t) = totals[s as usize];
        let (s1, n1) = top1_same[s as usize];
        if t > 0 {
            println!("{:<8} {c}/{t} ({:.1}%)  1위 {s1}/{n1}", s.label(), c as f64 * 100.0 / t as f64);
        }
    }
    0
}

/// 이미 떠 있는 서버(설정 포트)에 대해 클라이언트 왕복 시간 측정
pub fn bench(file: &str) -> i32 {
    use crate::embed::client::{EmbedClient, EmbedInput, EmbedTarget};
    let cfg = Config::load(&crate::config::data_dir());
    let client = EmbedClient::new(&cfg.connect_target().unwrap_or_else(|_| EmbedTarget::local(cfg.port)));
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
    // 설정 포트를 못 써서 다른 포트로 떴을 수 있다
    cfg.port = server.port();
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
        target: server.target(),
        server_sig: server.signature(&cfg),
        db_path: db_path.clone(),
        data_dir: data_dir.clone(),
        store: store.clone(),
        full_rebuild: full,
        only: None,
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
        let o = query::run(&server.target(), &store, &db, q, Filter::default(), 3, 0.0, cfg.ffmpeg());
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
                    let kw = if h.hit.coverage > 0.0 { format!(" 단어{:.2}", h.hit.coverage) } else { String::new() };
                    format!("{}{} {:.3}{kw}", h.file_name, loc, h.hit.score)
                })
                .collect();
            println!("  {:<6} {}", sec.label(), s.join(" | "));
            for h in hits.iter().filter(|h| h.highlight.is_some()) {
                let t: String = h
                    .highlight
                    .as_deref()
                    .unwrap_or_default()
                    .chars()
                    .map(|c| match c {
                        crate::search::keyword::MARK_START => '[',
                        crate::search::keyword::MARK_END => ']',
                        c if c.is_control() => ' ',
                        c => c,
                    })
                    .collect();
                println!("         └ {}: {}", h.file_name, t);
            }
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
