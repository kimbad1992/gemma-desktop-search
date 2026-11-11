//! 서비스판: 창 없이 계속 떠서 폴더를 감시하고, 바뀐 파일을 인덱싱하고, 검색 요청에 답한다.
//! 화면이 없는 Linux 서버나 NAS에서 쓰고, 검색은 명령줄(`cli.rs`: search·status·index·rescan)이나
//! 웹 페이지(다른 PC의 브라우저)로 한다.
//!
//!   gemma-search serve [--bind 127.0.0.1:8787] [--token <토큰>] [--no-watch]
//!                      [--root <폴더>]… [--llama <경로>] [--model <gguf>] [--mmproj <gguf>]
//!
//! - 설정과 인덱스는 창이 있는 앱과 같은 데이터 폴더(`config.toml`, `index.db`)를 쓴다.
//! - llama-server는 필요할 때(검색, 인덱싱) 켜고 한동안 쓰지 않으면 끈다 (설정의 `idle_stop_minutes`).
//! - 기본은 이 기기에서만 접속할 수 있다(127.0.0.1). 다른 PC에서 접속하게 하려면 `--bind 0.0.0.0:8787`로 열고,
//!   그때는 토큰이 있어야 한다: 검색 결과로 파일 내용까지 내려받을 수 있기 때문이다.
//! - 서버를 켜고 끄는 일과 인덱싱 시작은 메인 루프 한 스레드에서만 한다 (Linux에서 자식 프로세스는 자기를 만든
//!   스레드가 끝나면 같이 끝나게 해 두었다).

use crate::config::Config;
use crate::extract::{Kind, Modality};
use crate::index::db::Db;
use crate::index::indexer::{IndexParams, Indexer};
use crate::index::progress::{Progress, RunState};
use crate::search::keyword;
use crate::search::query::{self, HitView, QueryKind};
use crate::search::store::{Filter, Section, VectorStore};
use crate::server::{ServerManager, ServerState};
use crate::util::fmt_num;
use crate::watch::{WatchKey, Watcher};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, StatusCode};

/// 검색 페이지 (한 파일: HTML + CSS + JS)
const PAGE: &str = include_str!("../assets/web/index.html");
pub(crate) const DEFAULT_BIND: &str = "127.0.0.1:8787";
/// 검색이 서버가 준비되기를 기다리는 최대 시간
pub(crate) const SERVER_WAIT: Duration = Duration::from_secs(120);
/// 요청을 받는 스레드 수. 영상을 보내는 동안에는 그 스레드가 묶이므로 넉넉히 둔다.
const HTTP_WORKERS: usize = 8;
/// 서버를 켜지 못했을 때 다시 해 보는 간격: 실패할수록 늘린다. 검색이 기다리는 중이면 짧게.
const RETRY_MIN: Duration = Duration::from_secs(15);
const RETRY_MAX: Duration = Duration::from_secs(600);
const RETRY_FOR_SEARCH: Duration = Duration::from_secs(10);

pub const USAGE: &str = "\
처음 한 번:
  gemma-search download           이 기기에 맞는 llama.cpp와 모델을 받는다 (받을 것을 먼저 보여 주고 묻는다)
      --list                      고를 수 있는 llama.cpp 판과 이 기기의 추천
      --llama <판>                판을 직접 고른다: cpu, vulkan, cuda13, cuda12
      --only <model|llama>        하나만 받는다
      --dir <폴더>                받을 곳 (기본: 실행 파일이 있는 폴더). 관리자 권한은 쓰지 않는다
      -y, --yes                   묻지 않고 받는다
  gemma-search serve --root <검색할 폴더>

서비스:
  gemma-search serve [옵션]       폴더를 감시하며 검색 요청을 받는다 (검색 페이지: http://127.0.0.1:8787/)
      --root <폴더>               검색할 폴더 (여러 번 쓸 수 있다)
      --llama <폴더>              llama.cpp가 있는 폴더 (llama-server)
      --model <gguf>              모델 파일
      --mmproj <gguf>             이미지·영상·소리용 mmproj 파일
      --bind <주소:포트>          접속을 받을 주소. 다른 기기에서 접속하게 하려면 0.0.0.0:8787
                                  (그때는 접속 토큰이 생기고, 페이지를 처음 열 때 묻는다)
      --token <토큰>              접속 토큰을 직접 정한다
      --no-watch                  폴더를 감시하지 않는다 (검색만)

  serve에 준 값은 설정 파일(data/config.toml)에 저장되므로 다음부터는 `gemma-search serve`만 쓰면 됩니다.
  데이터 폴더는 실행 파일 옆의 data 폴더이고, 환경 변수 GEMMA_SEARCH_DATA로 바꿀 수 있습니다.

떠 있는 서비스에 묻는 명령:
  gemma-search search <질의…>     검색해서 결과를 출력한다
      --kinds <종류,…>            이 종류만: docs(문서 내용) images(이미지) videos(영상)
                                  audio(오디오) names(파일 이름)
      --path <글자>               경로에 이 글자가 든 파일만 (여러 번 쓰면 모두 든 파일만)
      --min <관련도>              이보다 낮은 결과는 뺀다 (기본: 설정의 min_relevance)
      --limit <개수>              보여 줄 결과 수 (기본 20, 0이면 모두)
      --json                      서비스의 응답을 JSON 그대로 (--limit와 상관없이 모두)
  gemma-search status [--json]    검색 서버, 인덱스, 인덱싱 진행, 폴더 감시 상태
  gemma-search index [--rebuild] [--wait]
                                  바뀐 파일을 지금 인덱싱한다. --rebuild는 전체 재구축.
                                  --wait는 끝날 때까지 진행을 보여 주고, 실패하면 종료 코드 1
  gemma-search rescan             폴더를 지금 훑어 바뀐 파일을 찾는다 (폴더 감시가 켜져 있을 때)

  네 명령 모두:
      --url <http://주소:포트>    다른 기기의 서비스에 묻는다 (환경 변수 GEMMA_SEARCH_URL)
      --token <토큰>              접속 토큰 (환경 변수 GEMMA_SEARCH_TOKEN). 주소를 주지 않았거나
                                  이 기기의 주소면 설정 파일의 토큰을 쓴다";
/// 글 파일을 브라우저로 보낼 때의 최대 크기
const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;

struct Engine {
    data_dir: PathBuf,
    db_path: PathBuf,
    cfg: Config,
    /// 비어 있으면 토큰 없이 받는다 (이 기기에서만 접속할 때)
    token: String,
    db: Arc<Mutex<Db>>,
    store: Arc<RwLock<VectorStore>>,
    server: ServerManager,
    progress: Arc<Mutex<Progress>>,
    watcher: Option<Watcher>,
    /// 마지막 검색·인덱싱 시각 (서버 자동 끄기)
    activity: Mutex<Instant>,
    /// 검색이 서버를 기다리는 중
    want_server: AtomicBool,
    /// 서버를 켜지 못한 마지막 일: (시각, 이유). 기다리던 검색에 알려 준다.
    start_error: Mutex<Option<(Instant, String)>>,
    /// 인덱싱 요청: Some(전체 재구축인지). 메인 루프가 인덱싱을 시작해 본 뒤에 비운다.
    index_request: Mutex<Option<bool>>,
    /// 인덱싱을 시작한 횟수 (감시가 시작한 것 포함). `index --wait`가 자기 요청이 끝났는지 알아보는 데 쓴다.
    index_runs: AtomicU64,
    /// 요청받은 인덱싱을 시작하지 못한 까닭 (다음 요청이 시작되면 지운다)
    index_error: Mutex<Option<String>>,
    stop: Arc<AtomicBool>,
}

fn log(line: impl AsRef<str>) {
    println!("[{}] {}", crate::util::now_local_string(), line.as_ref());
}

// ── 시작 ──

pub fn serve(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let values = |f: &str| -> Vec<String> {
        args.iter()
            .enumerate()
            .filter(|(_, a)| *a == f)
            .filter_map(|(i, _)| args.get(i + 1).cloned())
            .collect()
    };
    let data_dir = crate::config::data_dir();
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("데이터 폴더를 만들 수 없습니다 ({}): {e}", data_dir.display());
        return 1;
    }
    // 한 데이터 폴더에는 하나만 (창이 있는 앱과도 같이 띄우지 않는다)
    let _lock = match crate::config::instance_lock(&data_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let had_config = data_dir.join("config.toml").is_file();
    let mut cfg = Config::load(&data_dir);
    // 명령줄로 준 값은 설정에 넣고 저장한다 (화면 없이 설정하는 방법)
    let mut changed = !had_config;
    let roots = values("--root");
    if !roots.is_empty() {
        cfg.roots = roots;
        changed = true;
    }
    for (flag, field) in [
        ("--llama", &mut cfg.llama_path),
        ("--model", &mut cfg.model_path),
        ("--mmproj", &mut cfg.mmproj_path),
    ] {
        if let Some(v) = value(flag) {
            *field = v;
            changed = true;
        }
    }
    if let Some(b) = value("--bind") {
        cfg.service_bind = b;
        changed = true;
    }
    if let Some(t) = value("--token") {
        cfg.service_token = t;
        changed = true;
    }
    let bind = if cfg.service_bind.trim().is_empty() {
        DEFAULT_BIND.to_string()
    } else {
        cfg.service_bind.trim().to_string()
    };
    // 다른 PC에서 접속할 수 있게 열었으면 토큰이 있어야 한다
    if !bind_is_loopback(&bind) && cfg.service_token.trim().is_empty() {
        cfg.service_token = new_token();
        changed = true;
    }
    cfg.setup_done = true;
    if changed {
        if let Err(e) = cfg.save(&data_dir) {
            eprintln!("설정을 저장하지 못했습니다: {e}");
        }
    }
    let token = cfg.service_token.trim().to_string();

    println!("Gemma Search v{} 서비스", env!("CARGO_PKG_VERSION"));
    println!("데이터 폴더: {}", data_dir.display());
    for problem in problems(&cfg) {
        println!("⚠ {problem}");
    }

    let db_path = data_dir.join("index.db");
    let mut db = match Db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("인덱스 DB를 열 수 없습니다 ({}): {e}", db_path.display());
            return 1;
        }
    };
    let _ = db.backfill_name_fts();
    let mut st = VectorStore::new(cfg.search_dim);
    if let Err(e) = st.fill_from_db(&db) {
        eprintln!("인덱스를 불러오지 못했습니다: {e}");
        return 1;
    }
    crate::search::hub::load_saved(&db, &mut st);
    st.loaded = true;
    println!(
        "인덱스: 파일 {}개 · 벡터 {}개",
        fmt_num(st.file_count() as u64),
        fmt_num(st.live_chunks() as u64)
    );

    let http = match tiny_http::Server::http(bind.as_str()) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("{bind}에서 접속을 받을 수 없습니다: {e}");
            return 1;
        }
    };
    let server = ServerManager::default();
    server.set_log_path(&data_dir.join("logs").join("llama-server.log"));
    let indexer = Indexer::default();
    let watching = !args.iter().any(|a| a == "--no-watch") && !cfg.roots.is_empty();
    let watcher = watching.then(|| {
        Watcher::start(
            WatchKey::new(&cfg.roots, &cfg.exclude_dirs, cfg.watch_rescan_hours),
            &data_dir,
        )
    });
    let stop = Arc::new(AtomicBool::new(false));
    let engine = Arc::new(Engine {
        data_dir,
        db_path,
        token,
        db: Arc::new(Mutex::new(db)),
        store: Arc::new(RwLock::new(st)),
        server,
        progress: indexer.progress.clone(),
        watcher,
        activity: Mutex::new(Instant::now()),
        want_server: AtomicBool::new(false),
        start_error: Mutex::new(None),
        index_request: Mutex::new(None),
        index_runs: AtomicU64::new(0),
        index_error: Mutex::new(None),
        stop: stop.clone(),
        cfg,
    });
    {
        let stop = stop.clone();
        if let Err(e) = ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst)) {
            eprintln!("종료 신호를 받을 수 없습니다: {e}");
        }
    }
    for i in 0..HTTP_WORKERS {
        let (http, engine) = (http.clone(), engine.clone());
        let _ = std::thread::Builder::new().name(format!("http-{i}")).spawn(move || loop {
            match http.recv() {
                Ok(req) => handle(&engine, req),
                Err(_) => break,
            }
        });
    }
    println!("검색 페이지: http://{}/", shown_addr(&bind));
    if !engine.token.is_empty() {
        println!("접속 토큰: {} (페이지를 처음 열 때 묻습니다. 설정 파일의 service_token)", engine.token);
    }
    match &engine.watcher {
        Some(w) => {
            let snap = w.snapshot();
            let network = snap.roots.iter().filter(|r| r.network).count();
            let every = match engine.cfg.watch_rescan_hours {
                0 => "켤 때 한 번 훑어 확인".to_string(),
                h => format!("{h}시간마다 훑어 확인"),
            };
            println!("폴더 감시: 폴더 {}개 · {every}", snap.roots.len());
            if network > 0 {
                println!("  네트워크 폴더 {network}개는 바뀐 것을 바로 알려 주지 않을 수 있어, 훑어 확인할 때 반영됩니다.");
            }
        }
        None => println!("폴더 감시: 끔"),
    }
    if engine.store.read().file_count() == 0 && !engine.cfg.roots.is_empty() {
        println!("인덱스가 비어 있어 처음 인덱싱을 시작합니다");
        *engine.index_request.lock() = Some(false);
    }
    println!("끝내려면 Ctrl+C");

    main_loop(&engine, indexer);
    log("끝냅니다");
    0
}

/// 시작할 때 알려 줄 빠진 준비물
fn problems(cfg: &Config) -> Vec<String> {
    let mut out = Vec::new();
    if crate::server::resolve_binary(&cfg.llama_path).is_err() {
        out.push(format!(
            "llama.cpp를 찾을 수 없습니다 ({}). `gemma-search download`로 받거나 --llama <폴더>로 지정하세요.",
            cfg.llama_path
        ));
    }
    if !Path::new(cfg.model_path.trim()).is_file() {
        out.push(format!(
            "모델 파일이 없습니다 ({}). `gemma-search download`로 받거나 --model <gguf>로 지정하세요.",
            cfg.model_path
        ));
    }
    if cfg.has_mmproj() && !Path::new(cfg.mmproj_path.trim()).is_file() {
        out.push(format!(
            "mmproj 파일이 없습니다 ({}). `gemma-search download`로 받거나 --mmproj <gguf>로 지정하세요.",
            cfg.mmproj_path
        ));
    }
    if cfg.roots.is_empty() {
        out.push("검색할 폴더가 없습니다. --root <폴더>로 지정하세요 (여러 번 쓸 수 있습니다).".to_string());
    }
    if cfg.ffmpeg().is_none() {
        out.push("ffmpeg가 없어 영상·소리는 이름으로만 찾습니다.".to_string());
    }
    out
}

pub(crate) fn bind_is_loopback(bind: &str) -> bool {
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    let host = host.trim_matches(['[', ']']);
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// 안내에 보여 줄 주소: 모든 주소로 열었으면(0.0.0.0) 이 기기에서 쓰는 주소로
pub(crate) fn shown_addr(bind: &str) -> String {
    match bind.rsplit_once(':') {
        Some((host, port)) if host == "0.0.0.0" || host == "[::]" => format!("127.0.0.1:{port}"),
        _ => bind.to_string(),
    }
}

/// 접속 토큰을 새로 만든다. 표준 라이브러리의 해시 키가 OS의 난수로 정해지는 것을 쓴다.
fn new_token() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..3)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            format!("{:016x}", h.finish())
        })
        .collect::<String>()[..32]
        .to_string()
}

// ── 메인 루프: 서버 켜고 끄기, 폴더 감시, 인덱싱 ──

fn main_loop(e: &Arc<Engine>, mut indexer: Indexer) {
    let mut batch: Vec<String> = Vec::new();
    let mut last_start: Option<Instant> = None;
    let mut retry = RETRY_MIN;
    let mut last_state = RunState::Idle;
    let mut our_server = false;
    let mut last_server = ServerState::Stopped;
    while !e.stop.load(Ordering::SeqCst) {
        let state = e.server.state();
        if state != last_server {
            match &state {
                ServerState::Ready => {
                    log(format!("llama-server 준비됨 (포트 {})", e.server.port()));
                    // 켜졌으니 예전 실패는 잊는다 (status와 index --wait가 이것으로 "켜지 못하는 중"을 판단한다)
                    *e.start_error.lock() = None;
                }
                ServerState::Failed(m) => {
                    log(format!("llama-server 오류: {m}"));
                    *e.start_error.lock() = Some((Instant::now(), m.clone()));
                }
                _ => {}
            }
            last_server = state.clone();
        }
        let active = indexer.is_active();
        if active {
            *e.activity.lock() = Instant::now();
        }
        // 인덱싱이 끝났으면 한 줄 남긴다
        let (ps, summary) = {
            let p = e.progress.lock();
            let summary = format!(
                "{}: 처리 {} · 변경 없음 {} · 옮김 {} · 삭제 {} · 오류 {}{}",
                if p.targeted { "바뀐 파일 반영" } else { "인덱싱" },
                fmt_num(p.processed.saturating_sub(p.unchanged + p.moved)),
                fmt_num(p.unchanged),
                fmt_num(p.moved),
                fmt_num(p.removed),
                fmt_num(p.errors),
                if p.message.is_empty() { String::new() } else { format!(" · {}", p.message) }
            );
            (p.state, summary)
        };
        if ps != last_state {
            match ps {
                RunState::Completed => log(format!("{summary} (완료)")),
                RunState::Stopped => log(format!("{summary} (중지)")),
                RunState::Failed => log(format!("{summary} (실패)")),
                _ => {}
            }
            last_state = ps;
        }
        // 감시: 훑을 때가 됐으면 훑고, 조용해진 변경을 꺼낸다
        if let (Some(w), false) = (&e.watcher, active) {
            if w.scan_due() {
                w.start_scan(e.db_path.clone(), Arc::new(|| {}));
            }
            if batch.is_empty() {
                let ready = w.take_ready();
                if !ready.is_empty() {
                    log(format!("바뀐 경로 {}개를 반영합니다", ready.len()));
                    batch = ready;
                }
            }
        }
        let index_request = *e.index_request.lock();
        let need_server = e.want_server.load(Ordering::SeqCst) || !batch.is_empty() || index_request.is_some();
        if state.is_ready() {
            retry = RETRY_MIN;
        } else if need_server && !state.is_running() {
            // 검색이 기다리는 중이면 곧 다시 켜 보고, 그 밖에는 실패할수록 간격을 늘린다
            let wait = if e.want_server.load(Ordering::SeqCst) { RETRY_FOR_SEARCH.min(retry) } else { retry };
            if last_start.is_none_or(|t| t.elapsed() > wait) {
                last_start = Some(Instant::now());
                retry = (retry * 2).min(RETRY_MAX);
                log("llama-server를 켭니다");
                match e.server.start(&e.cfg) {
                    Ok(()) => our_server = true,
                    Err(err) => {
                        log(format!("llama-server를 켜지 못했습니다: {err}"));
                        *e.start_error.lock() = Some((Instant::now(), err.to_string()));
                    }
                }
            }
        }
        // 인덱싱 시작: 직접 요청한 것이 먼저, 그다음 감시가 알려 준 변경
        if !active && state.is_ready() {
            if let Some(caps) = e.server.capabilities() {
                // 요청은 시작해 본 뒤에 비운다: 상태를 묻는 쪽이 "요청 없음"을 보면 그 실행이 이미 시작됐거나
                // 시작하지 못한 것이다 (index --wait가 이 순서에 기댄다)
                let job = match index_request {
                    Some(full) => Some((None, full)),
                    None if !batch.is_empty() => Some((Some(std::mem::take(&mut batch)), false)),
                    None => None,
                };
                if let Some((only, full_rebuild)) = job {
                    let mut cfg = e.cfg.clone();
                    cfg.port = e.server.port();
                    let started = indexer.start(IndexParams {
                        port: cfg.port,
                        cfg,
                        caps,
                        db_path: e.db_path.clone(),
                        data_dir: e.data_dir.clone(),
                        store: e.store.clone(),
                        full_rebuild,
                        only: only.clone(),
                    });
                    let requested = only.is_none();
                    {
                        // 횟수·시작 실패 기록과 요청 비우기는 요청 잠금 안에서 함께 한다. 요청을 받는 쪽(/api/index)도
                        // 같은 잠금 안에서 횟수를 읽으므로, 그 요청이 이 실행에 묻어갔는지 다음 실행을 기다리는지 갈린다.
                        // 상태를 묻는 쪽은 요청이 빈 것을 보면 이 실행의 진행·횟수·실패 까닭도 함께 본다.
                        let mut r = e.index_request.lock();
                        match &started {
                            Ok(()) => {
                                e.index_runs.fetch_add(1, Ordering::SeqCst);
                                if requested {
                                    *e.index_error.lock() = None;
                                }
                            }
                            Err(err) if requested => *e.index_error.lock() = Some(err.to_string()),
                            Err(_) => {}
                        }
                        // 그사이 들어온 다른 요청(재구축 등)은 남겨 둔다
                        if requested && *r == Some(full_rebuild) {
                            *r = None;
                        }
                    }
                    match started {
                        Ok(()) => {
                            last_state = RunState::Running;
                            if requested {
                                log(if full_rebuild { "전체 재구축을 시작합니다" } else { "인덱싱을 시작합니다 (바뀐 파일만)" });
                            }
                        }
                        Err(err) => {
                            log(format!("인덱싱을 시작하지 못했습니다: {err}"));
                            if let (Some(w), Some(paths)) = (&e.watcher, only) {
                                w.requeue(paths);
                            }
                        }
                    }
                }
            }
        }
        // 우리가 켠 서버는 한동안 쓰지 않으면 끈다
        let idle = Duration::from_secs(e.cfg.idle_stop_minutes as u64 * 60);
        if our_server
            && state.is_ready()
            && !active
            && !need_server
            && e.cfg.idle_stop_minutes > 0
            && e.activity.lock().elapsed() > idle
        {
            e.server.stop();
            our_server = false;
            log(format!(
                "{}분 동안 검색·인덱싱이 없어 llama-server를 껐습니다 (다음 검색 때 다시 켭니다)",
                e.cfg.idle_stop_minutes
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    indexer.stop();
    let t0 = Instant::now();
    while indexer.is_active() && t0.elapsed() < Duration::from_secs(15) {
        std::thread::sleep(Duration::from_millis(100));
    }
    e.server.stop();
}

// ── HTTP ──

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("올바른 헤더")
}

fn respond_json(req: Request, status: u16, v: Value) {
    let _ = req.respond(
        Response::from_string(v.to_string())
            .with_status_code(status)
            .with_header(header("Content-Type", "application/json; charset=utf-8"))
            .with_header(header("Cache-Control", "no-store")),
    );
}

fn respond_error(req: Request, status: u16, message: &str) {
    respond_json(req, status, json!({ "error": message }));
}

/// "%EC%98%88%EC%82%B0+회의" → "예산 회의"
fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = b
                    .get(i + 1..i + 3)
                    .filter(|h| h.iter().all(u8::is_ascii_hexdigit))
                    .and_then(|h| std::str::from_utf8(h).ok());
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (url_decode(k), url_decode(v))
        })
        .collect()
}

fn param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn header_value<'a>(req: &'a Request, name: &'static str) -> Option<&'a str> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str())
}

/// 걸린 시간으로 토큰을 짐작할 수 없게, 길이가 같으면 끝까지 견준다
fn same_token(given: &str, token: &str) -> bool {
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |d, (a, b)| d | (a ^ b)) == 0
}

/// 토큰이 맞는지: 주소의 `token=`, `Authorization: Bearer …`, 쿠키 `gs_token=…` 가운데 하나
fn authorized(req: &Request, params: &[(String, String)], token: &str) -> bool {
    if param(params, "token").is_some_and(|v| same_token(v, token)) {
        return true;
    }
    if header_value(req, "Authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|v| same_token(v.trim(), token))
    {
        return true;
    }
    header_value(req, "Cookie").is_some_and(|c| {
        c.split(';')
            .filter_map(|p| p.trim().strip_prefix("gs_token="))
            .any(|v| same_token(&url_decode(v), token))
    })
}

/// 다른 사이트의 페이지가 보낸 요청인지. 브라우저가 붙이는 `Sec-Fetch-Site`와 `Origin`으로 가린다
/// (curl 같은 도구는 둘 다 붙이지 않으므로 통과한다).
fn from_another_site(req: &Request) -> bool {
    if header_value(req, "Sec-Fetch-Site").is_some_and(|v| v != "same-origin" && v != "none") {
        return true;
    }
    match (header_value(req, "Origin"), header_value(req, "Host")) {
        (Some(origin), Some(host)) => origin.split_once("://").map(|(_, h)| h) != Some(host),
        _ => false,
    }
}

fn handle(e: &Engine, req: Request) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let params = parse_query(query);
    let method = req.method().clone();
    if method == Method::Get && (path == "/" || path == "/index.html") {
        let _ = req.respond(
            Response::from_string(PAGE)
                .with_header(header("Content-Type", "text/html; charset=utf-8"))
                .with_header(header("Cache-Control", "no-cache")),
        );
        return;
    }
    // 브라우저로 다른 사이트를 보는 동안 그 사이트가 이 서비스를 부르지 못하게 한다
    if from_another_site(&req) {
        return respond_error(req, 403, "다른 사이트에서 온 요청은 받지 않습니다");
    }
    if e.token.is_empty() {
        // 토큰 없이 받을 때는 이 기기를 가리키는 주소로 온 요청만 받는다
        // (다른 사이트가 자기 도메인을 127.0.0.1로 돌려 읽어 가는 것을 막는다)
        if !header_value(&req, "Host").is_some_and(bind_is_loopback) {
            return respond_error(req, 403, "이 기기의 주소(127.0.0.1)로만 접속할 수 있습니다. 다른 주소로 쓰려면 토큰을 정하세요.");
        }
    } else if !authorized(&req, &params, &e.token) {
        return respond_error(req, 401, "토큰이 필요합니다");
    }
    let id_after = |prefix: &str| path.strip_prefix(prefix).and_then(|s| s.parse::<i64>().ok());
    match method {
        Method::Get if path == "/api/status" => respond_json(req, 200, status(e)),
        Method::Get if path == "/api/search" => match search(e, &params) {
            Ok(v) => respond_json(req, 200, v),
            Err(m) => respond_error(req, 503, &m),
        },
        Method::Post if path == "/api/index" => {
            let full = param(&params, "rebuild") == Some("1");
            // 요청을 넣을 때의 실행 횟수: 이보다 커지고 요청이 비면 이 요청이 시작된 것이다 (메인 루프와 같은 잠금 안에서 읽는다)
            let runs = {
                let mut r = e.index_request.lock();
                // 재구축 요청이 기다리는 중이면 "바뀐 파일만" 요청으로 덮어쓰지 않는다
                *r = Some(full || *r == Some(true));
                e.index_runs.load(Ordering::SeqCst)
            };
            respond_json(req, 200, json!({ "ok": true, "runs": runs }));
        }
        Method::Post if path == "/api/rescan" => {
            if let Some(w) = &e.watcher {
                w.scan_now();
            }
            respond_json(req, 200, json!({ "ok": e.watcher.is_some() }));
        }
        Method::Get if path.starts_with("/api/thumb/") => match id_after("/api/thumb/") {
            Some(id) => serve_thumb(e, req, id),
            None => respond_error(req, 404, "없는 주소입니다"),
        },
        Method::Get if path.starts_with("/api/file/") => match id_after("/api/file/") {
            Some(id) => serve_file(e, req, id),
            None => respond_error(req, 404, "없는 주소입니다"),
        },
        _ => respond_error(req, 404, "없는 주소입니다"),
    }
}

fn section_key(s: Section) -> &'static str {
    match s {
        Section::Docs => "docs",
        Section::Images => "images",
        Section::Videos => "videos",
        Section::Audio => "audio",
        Section::Names => "names",
    }
}

fn kind_key(k: Kind) -> &'static str {
    match k {
        Kind::Text => "text",
        Kind::Doc => "doc",
        Kind::Image => "image",
        Kind::Video => "video",
        Kind::Audio => "audio",
        Kind::Other => "other",
    }
}

fn run_key(s: RunState) -> &'static str {
    match s {
        RunState::Idle => "idle",
        RunState::Running => "running",
        RunState::Paused => "paused",
        RunState::Stopping => "stopping",
        RunState::Stopped => "stopped",
        RunState::Completed => "completed",
        RunState::Failed => "failed",
    }
}

fn status(e: &Engine) -> Value {
    let state = e.server.state();
    let (files, vectors) = {
        let st = e.store.read();
        (st.file_count(), st.live_chunks())
    };
    // 요청 → 진행 → 횟수 순서로 읽는다. 메인 루프는 시작(진행·횟수)한 뒤에 요청을 비우므로,
    // 요청이 없다고 읽었으면 그 실행의 진행 상태도 보인다.
    let request = *e.index_request.lock();
    let p = e.progress.lock().clone();
    let runs = e.index_runs.load(Ordering::SeqCst);
    let start_error = e.index_error.lock().clone();
    let watch = match &e.watcher {
        None => json!({ "enabled": false }),
        Some(w) => {
            let s = w.snapshot();
            let now = Instant::now();
            json!({
                "enabled": true,
                "pending": s.pending,
                "scanning": s.scanning,
                "next_scan_secs": s.next_scan.map(|t| t.saturating_duration_since(now).as_secs()),
                "last_scan": s.last_scan.map(|(at, n)| json!({ "ago_secs": at.elapsed().as_secs(), "found": n })),
                "note": s.scan_note,
                "rescan_hours": e.cfg.watch_rescan_hours,
                "roots": s.roots.iter().map(|r| json!({
                    "path": r.path, "network": r.network, "error": r.error, "registering": r.registering,
                })).collect::<Vec<_>>(),
            })
        }
    };
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "problems": problems(&e.cfg),
        "server": {
            "key": match &state {
                ServerState::Stopped => "stopped",
                ServerState::Starting | ServerState::Loading => "starting",
                ServerState::Ready | ServerState::External => "ready",
                ServerState::Failed(_) => "failed",
            },
            "state": state.label(),
            "ready": state.is_ready(),
            "error": match &state { ServerState::Failed(m) => Some(m.clone()), _ => None },
            // 서버를 마지막으로 켜지 못한 까닭 (경로가 틀려 아예 띄우지 못하면 상태는 "중지됨"으로 남는다)
            "start_error": e.start_error.lock().as_ref().map(|(at, why)| json!({
                "message": why,
                "ago_secs": at.elapsed().as_secs(),
            })),
        },
        "index": { "files": files, "vectors": vectors },
        "indexing": {
            "key": run_key(p.state),
            "state": p.state.label(),
            "active": p.state.is_active(),
            "targeted": p.targeted,
            "scanning": p.scanning,
            "processed": p.processed,
            "discovered": p.discovered,
            "fraction": p.fraction(),
            "current": p.current,
            "unchanged": p.unchanged,
            "moved": p.moved,
            "removed": p.removed,
            "errors": p.errors,
            "message": p.message,
            // 받아 두고 아직 시작하지 않은 요청: null, "changed"(바뀐 파일만), "rebuild"(전체 재구축)
            "pending": request.map(|full| if full { "rebuild" } else { "changed" }),
            "runs": runs,
            "start_error": start_error,
        },
        "watch": watch,
        "roots": e.cfg.roots,
        "min_relevance": rounded(e.cfg.min_relevance, 1),
    })
}

/// 검색. 서버가 꺼져 있으면 켜지기를 기다린다.
fn search(e: &Engine, params: &[(String, String)]) -> Result<Value, String> {
    let q = param(params, "q").unwrap_or("").trim().to_string();
    if q.is_empty() {
        return Err("검색어가 없습니다".to_string());
    }
    if !e.store.read().loaded || e.store.read().file_count() == 0 {
        return Err("검색할 자료가 아직 없습니다. 폴더를 지정하고 인덱싱이 끝나기를 기다리세요.".to_string());
    }
    *e.activity.lock() = Instant::now();
    let waited = wait_for_server(e);
    e.want_server.store(false, Ordering::SeqCst);
    waited?;
    let min = param(params, "min")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(e.cfg.min_relevance);
    let mut sections = [true; 5];
    if let Some(kinds) = param(params, "kinds").filter(|k| !k.is_empty()) {
        for s in Section::ALL {
            sections[s as usize] = kinds.split(',').any(|k| k == section_key(s));
        }
    }
    let filter = Filter {
        sections,
        path_terms: param(params, "path")
            .unwrap_or("")
            .split_whitespace()
            .map(|s| s.to_lowercase())
            .collect(),
        folders: None,
    };
    let o = query::run(
        e.server.port(),
        &e.store,
        &e.db,
        QueryKind::Text(q.clone()),
        filter,
        e.cfg.top_k.max(1),
        min,
        e.cfg.ffmpeg(),
    );
    *e.activity.lock() = Instant::now();
    if let Some(err) = &o.error {
        return Err(format!("검색 실패: {err}"));
    }
    let mut visible: Vec<(Section, Vec<&HitView>)> = o
        .sections
        .iter()
        .map(|(s, h)| (*s, query::passing(h, min)))
        .filter(|(_, h)| !h.is_empty())
        .collect();
    // "고양이 영상"처럼 종류를 말한 질의는 그 섹션을 먼저
    visible.sort_by_key(|(s, _)| o.kind_hints.iter().position(|k| k == s).unwrap_or(usize::MAX));
    let all = query::unified(&visible, &o.kind_hints);
    Ok(json!({
        "query": q,
        "min": rounded(min, 1),
        "elapsed_ms": o.elapsed.as_millis() as u64,
        "searched": o.searched_chunks,
        "all": all.iter().map(|(s, hv)| hit_json(*s, hv)).collect::<Vec<_>>(),
        "sections": visible.iter().map(|(s, hits)| json!({
            "key": section_key(*s),
            "label": s.label(),
            "hits": hits.iter().map(|hv| hit_json(*s, hv)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

/// 서버가 준비될 때까지 기다린다. 꺼져 있으면 메인 루프가 켠다.
fn wait_for_server(e: &Engine) -> Result<(), String> {
    let t0 = Instant::now();
    loop {
        if e.server.state().is_ready() {
            return Ok(());
        }
        e.want_server.store(true, Ordering::SeqCst);
        // 기다리기 시작한 뒤에 난 실패만 알린다 (예전 실패는 메인 루프가 곧 다시 해 본다)
        if let Some((at, why)) = &*e.start_error.lock() {
            if *at >= t0 {
                return Err(format!("검색 서버(llama-server)를 켜지 못했습니다: {why}"));
            }
        }
        if t0.elapsed() > SERVER_WAIT {
            return Err("검색 서버(llama-server)가 준비되지 않았습니다. data/logs/llama-server.log를 확인하세요.".to_string());
        }
        if e.stop.load(Ordering::SeqCst) {
            return Err("끝내는 중입니다".to_string());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// JSON에 넣을 숫자: 소수 `digits`자리까지 (f32를 그대로 넣으면 5.6이 5.599999904632568로 나온다)
fn rounded(v: f32, digits: i32) -> f64 {
    let m = 10f64.powi(digits);
    (v as f64 * m).round() / m
}

/// 강조 표식이 든 발췌를 [글, 강조인지] 조각으로
fn highlight_parts(text: &str) -> Vec<Value> {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() && c != keyword::MARK_START && c != keyword::MARK_END { ' ' } else { c })
        .collect();
    keyword::split_marks(&flat)
        .into_iter()
        .map(|(part, hot)| json!([part, hot]))
        .collect()
}

fn hit_json(sec: Section, hv: &HitView) -> Value {
    let h = &hv.hit;
    // 브라우저가 그 자리에서 열게 하는 주소 꼬리: 영상·소리는 시점, PDF는 페이지
    let place = match (h.modality, h.kind) {
        (Modality::Video | Modality::Audio, _) | (Modality::Image, Kind::Video) => {
            format!("#t={:.1}", h.locator.max(0) as f64 / 1000.0)
        }
        (Modality::Text, Kind::Doc) => crate::extract::pdf::page_of(h.locator)
            .map(|p| format!("#page={p}"))
            .unwrap_or_default(),
        _ => String::new(),
    };
    let text = h.modality == Modality::Text;
    json!({
        "file_id": h.file_id,
        "chunk_id": h.chunk_id,
        "section": section_key(sec),
        "section_label": sec.label(),
        "name": hv.file_name,
        "path": h.path,
        "kind": kind_key(h.kind),
        "score": rounded(h.score, 1),
        "meaning": rounded(h.meaning, 1),
        "cosine": rounded(h.cosine, 3),
        "keyword": hv.keyword_match(),
        "locator": query::locator_label(hv),
        "snippet": if text && hv.highlight.is_none() { hv.snippet.as_str() } else { "" },
        "highlight": hv.highlight.as_deref().map(highlight_parts),
        "thumb": hv.thumb_chunk,
        "open": format!("/api/file/{}{place}", h.file_id),
    })
}

fn serve_thumb(e: &Engine, req: Request, chunk_id: i64) {
    let jpeg = e.db.lock().thumb(chunk_id).ok().flatten();
    match jpeg {
        Some(bytes) => {
            let _ = req.respond(
                Response::from_data(bytes)
                    .with_header(header("Content-Type", "image/jpeg"))
                    .with_header(header("Cache-Control", "private, max-age=3600")),
            );
        }
        None => respond_error(req, 404, "썸네일이 없습니다"),
    }
}

fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => "application/octet-stream",
    }
}

/// "bytes=100-199" → (100, 199). 끝이 없으면 파일 끝까지.
fn parse_range(v: &str, len: u64) -> Option<(u64, u64)> {
    let r = v.trim().strip_prefix("bytes=")?;
    let (a, b) = r.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        // 끝에서 N바이트
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() { len - 1 } else { b.parse::<u64>().ok()?.min(len - 1) };
        (start, end)
    };
    (start <= end && start < len).then_some((start, end))
}

/// 인덱스에 있는 파일의 내용을 보낸다 (검색 결과를 브라우저에서 열 때). 인덱스에 없는 경로는 보내지 않는다.
fn serve_file(e: &Engine, req: Request, file_id: i64) {
    let info = e.store.read().files.get(&file_id).cloned();
    let Some(info) = info else {
        return respond_error(req, 404, "인덱스에 없는 파일입니다");
    };
    let path = PathBuf::from(&info.path);
    let Ok(mut file) = std::fs::File::open(&path) else {
        return respond_error(req, 404, "파일을 열 수 없습니다 (옮겼거나 지웠을 수 있습니다)");
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    // 글 파일은 UTF-8로 바꿔서 보낸다 (CP949 등으로 저장된 파일이 브라우저에서 깨지지 않게)
    if info.kind == Kind::Text && len <= MAX_TEXT_BYTES {
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_ok() {
            if let Some(text) = crate::extract::text::decode_text(&bytes) {
                let _ = req.respond(
                    Response::from_string(text)
                        .with_header(header("Content-Type", "text/plain; charset=utf-8"))
                        .with_header(header("X-Content-Type-Options", "nosniff")),
                );
                return;
            }
        }
        return respond_error(req, 415, "글로 읽을 수 없는 파일입니다");
    }
    let ctype = content_type(&path);
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    // 브라우저가 그 자리에서 보여 줄 수 있는 것만 바로 열고, 나머지는 내려받게 한다
    let inline = ctype.starts_with("image/") || ctype.starts_with("video/") || ctype.starts_with("audio/") || ctype == "application/pdf";
    let mut headers = vec![
        header("Content-Type", ctype),
        header("Accept-Ranges", "bytes"),
        header("Cache-Control", "private, max-age=60"),
        header("X-Content-Type-Options", "nosniff"),
        header(
            "Content-Disposition",
            &format!("{}; filename*=UTF-8''{}", if inline { "inline" } else { "attachment" }, url_encode(&name)),
        ),
    ];
    if ctype == "image/svg+xml" {
        // SVG 안의 스크립트가 이 서비스의 권한으로 돌지 못하게 한다
        headers.push(header("Content-Security-Policy", "sandbox"));
    }
    // 영상·소리를 그 시점부터 틀려면 브라우저가 파일의 일부만 달라고 한다
    let range = header_value(&req, "Range").and_then(|v| parse_range(v, len));
    let (status, start, count) = match range {
        Some((start, end)) => {
            headers.push(header("Content-Range", &format!("bytes {start}-{end}/{len}")));
            (206, start, end - start + 1)
        }
        None => (200, 0, len),
    };
    if start > 0 && file.seek(SeekFrom::Start(start)).is_err() {
        return respond_error(req, 500, "파일을 읽을 수 없습니다");
    }
    let _ = req.respond(Response::new(
        StatusCode(status),
        headers,
        file.take(count),
        Some(count as usize),
        None,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_encodes_query_strings() {
        assert_eq!(url_decode("%EC%98%88%EC%82%B0+%ED%9A%8C%EC%9D%98%EB%A1%9D"), "예산 회의록");
        assert_eq!(url_decode("a%2Fb%"), "a/b%");
        assert_eq!(url_decode("100%25"), "100%");
        assert_eq!(url_decode(&url_encode("예산 회의록 #1 & 2")), "예산 회의록 #1 & 2");
        let p = parse_query("q=%EA%B0%95&min=5.5&kinds=docs,images&x");
        assert_eq!(param(&p, "q"), Some("강"));
        assert_eq!(param(&p, "min"), Some("5.5"));
        assert_eq!(param(&p, "x"), Some(""));
        assert_eq!(param(&p, "none"), None);
    }

    #[test]
    fn range_requests() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
    }

    #[test]
    fn only_loopback_binds_skip_the_token() {
        assert!(bind_is_loopback("127.0.0.1:8787"));
        assert!(bind_is_loopback("localhost:8787"));
        assert!(bind_is_loopback("[::1]:8787"));
        assert!(!bind_is_loopback("0.0.0.0:8787"));
        assert!(!bind_is_loopback("192.168.0.10:8787"));
        assert_eq!(shown_addr("0.0.0.0:8787"), "127.0.0.1:8787");
        assert_eq!(shown_addr("192.168.0.10:8787"), "192.168.0.10:8787");
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(same_token(&a, &a) && !same_token(&a, &b) && !same_token(&a[..31], &a));
    }

    #[test]
    fn numbers_for_json_are_short() {
        assert_eq!(json!(rounded(5.6, 1)).to_string(), "5.6");
        assert_eq!(json!(rounded(12.9, 1)).to_string(), "12.9");
        assert_eq!(json!(rounded(0.61234, 3)).to_string(), "0.612");
    }

    #[test]
    fn stray_percent_signs_stay() {
        assert_eq!(url_decode("%"), "%");
        assert_eq!(url_decode("%4"), "%4");
        assert_eq!(url_decode("%+1"), "% 1");
        assert_eq!(url_decode("%zz%41"), "%zzA");
    }
}
