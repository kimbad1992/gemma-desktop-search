//! 앱 상태와 공통 동작 (탭 화면은 ui/ 아래 파일들)

use crate::config::{self, Config};
use crate::extract::{self, Kind};
use crate::index::db::{Db, DbStats};
use crate::index::indexer::{IndexParams, Indexer};
use crate::index::progress::RunState;
use crate::search::query::{self, Outcome, QueryKind};
use crate::search::store::{Entry, FileInfo, Filter, VectorStore};
use crate::server::ServerManager;
use crate::ui::widgets;
use crate::util::fmt_num;
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Index,
    Server,
    Settings,
}

pub struct Notice {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// 검색 폴더 목록의 한 줄 (지금 대상 폴더 + 예전에 인덱싱한 폴더)
#[derive(Clone, Debug)]
pub struct FolderStat {
    pub path: String,
    pub key: String,
    pub files: u64,
    /// 지금 설정의 인덱싱 대상인지 (false = 예전에 인덱싱하고 대상에서 뺀 폴더)
    pub current: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FolderStats {
    pub list: Vec<FolderStat>,
    /// 목록의 어느 폴더에도 속하지 않는 인덱스 파일 수
    pub other: u64,
}

pub struct GemmaApp {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub cfg: Config,
    pub saved_cfg: Config,
    pub server: ServerManager,
    pub indexer: Indexer,
    pub store: Arc<RwLock<VectorStore>>,
    pub store_loading: Arc<AtomicBool>,
    /// 저장소 적재 단계 표시 ("FP16 변환 중… 1,000/42,028" 등)
    pub store_status: Arc<Mutex<String>>,
    pub db: Arc<Mutex<Db>>,
    pub db_stats: Arc<Mutex<DbStats>>,
    pub folders: Arc<Mutex<FolderStats>>,
    /// 통계 스레드가 '현재 대상 폴더'를 알 수 있도록 공유
    cfg_roots_shared: Arc<Mutex<Vec<String>>>,
    /// 백그라운드 작업이 남기는 알림
    bg_notice: Arc<Mutex<Option<(String, bool)>>>,
    pub tab: Tab,

    // 검색
    pub query: String,
    pub filter_sections: [bool; 5],
    pub path_filter: String,
    pub search_rx: Option<Receiver<Outcome>>,
    pub search_started: Option<Instant>,
    pub outcome: Option<Outcome>,
    /// 폴더 선택을 바꾸면 같은 질의로 다시 검색
    pub last_query: Option<QueryKind>,
    /// None = 전체 요약, Some = 해당 섹션 전체 보기
    pub result_view: Option<crate::search::store::Section>,
    pub thumbs: HashMap<i64, Option<egui::TextureHandle>>,
    pub focus_search: bool,

    // 설정 화면 보조
    pub exclude_text: String,
    pub new_root: String,

    pub notice: Option<Notice>,
    pub confirm_rebuild: bool,
    last_title: String,
    last_index_state: RunState,

    // 명령줄 자동 실행 (--index, --query)
    auto_index: bool,
    auto_query: Option<String>,

    /// 이번 인덱싱 작업에서 서버 충돌 후 자동 복구한 횟수
    pub recover_attempts: u32,
    pending_recover: Option<Instant>,
}

/// 한 번의 인덱싱 작업에서 서버 자동 재시작 최대 횟수
pub const MAX_RECOVER_ATTEMPTS: u32 = 5;

impl GemmaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_fonts(&cc.egui_ctx);
        cc.egui_ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(10.0, 4.0);
        });

        let data_dir = config::data_dir();
        let _ = std::fs::create_dir_all(&data_dir);
        let cfg = Config::load(&data_dir);
        let db_path = data_dir.join("index.db");
        let (db, db_error) = match Db::open(&db_path) {
            Ok(db) => (db, None),
            Err(e) => {
                let tmp = std::env::temp_dir().join("gemma-search-fallback.db");
                (
                    Db::open(&tmp).expect("임시 DB도 열 수 없습니다"),
                    Some(format!("인덱스 DB를 열 수 없어 임시 DB를 사용합니다: {e}")),
                )
            }
        };

        let mut app = Self {
            exclude_text: cfg.exclude_dirs.join("\n"),
            store: Arc::new(RwLock::new(VectorStore::new(cfg.search_dim))),
            store_loading: Arc::new(AtomicBool::new(false)),
            store_status: Arc::new(Mutex::new(String::new())),
            db: Arc::new(Mutex::new(db)),
            db_stats: Arc::new(Mutex::new(DbStats::default())),
            folders: Arc::new(Mutex::new(FolderStats::default())),
            cfg_roots_shared: Arc::new(Mutex::new(cfg.roots.clone())),
            bg_notice: Arc::new(Mutex::new(None)),
            saved_cfg: cfg.clone(),
            cfg,
            data_dir,
            db_path,
            server: ServerManager::default(),
            indexer: Indexer::default(),
            tab: Tab::Search,
            query: String::new(),
            filter_sections: [true; 5],
            path_filter: String::new(),
            search_rx: None,
            search_started: None,
            outcome: None,
            last_query: None,
            result_view: None,
            thumbs: HashMap::new(),
            focus_search: true,
            new_root: String::new(),
            notice: None,
            confirm_rebuild: false,
            last_title: String::new(),
            last_index_state: RunState::Idle,
            auto_index: false,
            auto_query: None,
            recover_attempts: 0,
            pending_recover: None,
        };
        app.server
            .set_log_path(&app.data_dir.join("logs").join("llama-server.log"));
        if let Some(e) = db_error {
            app.notify(e, true);
        }
        app.reload_store();
        app.spawn_stats_thread();

        // 명령줄: --start-server, --index(서버 준비 후 변경분 인덱싱), --query "질의", --tab 이름
        let args: Vec<String> = std::env::args().skip(1).collect();
        let flag = |f: &str| args.iter().any(|a| a == f);
        let value = |f: &str| {
            args.iter()
                .position(|a| a == f)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        app.auto_index = flag("--index");
        app.auto_query = value("--query");
        if app.cfg.auto_start_server || flag("--start-server") || app.auto_index || app.auto_query.is_some() {
            app.start_server();
        }
        if app.cfg.roots.is_empty() {
            app.tab = Tab::Settings;
            app.notify("처음 실행: 설정에서 경로를 확인하고 인덱싱할 폴더를 추가하세요.", false);
        }
        if let Some(t) = value("--tab") {
            app.tab = match t.as_str() {
                "index" => Tab::Index,
                "server" => Tab::Server,
                "settings" => Tab::Settings,
                _ => Tab::Search,
            };
        }
        app
    }

    fn run_auto_actions(&mut self) {
        if self.auto_index && self.index_blocker().is_none() {
            self.auto_index = false;
            self.start_index(false);
            return;
        }
        if !self.auto_index && self.auto_query.is_some() && !self.indexer.is_active() {
            let ready = self.server.state().is_ready()
                && !self.store_loading.load(Ordering::SeqCst)
                && self.search_rx.is_none();
            if ready {
                let q = self.auto_query.take().unwrap_or_default();
                self.query = q.clone();
                self.tab = Tab::Search;
                self.start_search(QueryKind::Text(q));
            }
        }
    }

    /// 검색 화면 설정(최소 일치도, 폴더 선택)만 바로 저장 (설정 탭의 다른 미저장 변경은 그대로 둠)
    pub fn persist_search_prefs(&mut self) {
        let mut s = self.saved_cfg.clone();
        s.min_score = self.cfg.min_score;
        s.search_excluded_folders = self.cfg.search_excluded_folders.clone();
        if s.save(&self.data_dir).is_ok() {
            self.saved_cfg = s;
        }
    }

    pub fn persist_min_score(&mut self) {
        self.persist_search_prefs();
    }

    /// 지금 폴더 선택으로 만든 필터 (전부 선택이면 None)
    pub fn folder_filter(&self) -> Option<crate::search::store::FolderFilter> {
        use crate::search::store::OTHER_FOLDER_KEY;
        let excluded = &self.cfg.search_excluded_folders;
        if excluded.is_empty() {
            return None;
        }
        let folders = self.folders.lock().clone();
        let known: Vec<String> = folders.list.iter().map(|f| f.key.clone()).collect();
        let relevant = known.iter().any(|k| excluded.contains(k))
            || (folders.other > 0 && excluded.iter().any(|k| k == OTHER_FOLDER_KEY));
        if !relevant {
            return None;
        }
        Some(crate::search::store::FolderFilter {
            selected: known.iter().filter(|k| !excluded.contains(k)).cloned().collect(),
            include_other: !excluded.iter().any(|k| k == OTHER_FOLDER_KEY),
            known,
        })
    }

    /// 폴더 선택이 바뀌면 저장하고, 결과가 있으면 같은 질의로 다시 검색
    pub fn on_folder_selection_changed(&mut self) {
        self.persist_search_prefs();
        if self.outcome.is_some() {
            if let Some(q) = self.last_query.clone() {
                self.start_search(q);
            }
        }
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.notice = Some(Notice {
            text: text.into(),
            error,
            at: Instant::now(),
        });
    }

    // ── 서버 ──
    pub fn start_server(&mut self) {
        match self.server.start(&self.cfg) {
            Ok(()) => self.notify("llama-server를 시작합니다…", false),
            Err(e) => {
                self.server.log(format!("[앱] 시작 실패: {e}"));
                self.notify(format!("서버 시작 실패: {e}"), true);
            }
        }
    }

    pub fn stop_server(&mut self) {
        if self.indexer.is_active() {
            self.indexer.stop();
        }
        self.server.stop();
        self.notify("서버를 중지했습니다", false);
    }

    // ── 저장소 ──
    pub fn reload_store(&mut self) {
        if self.store_loading.load(Ordering::SeqCst) {
            return;
        }
        self.store_loading.store(true, Ordering::SeqCst);
        let store = self.store.clone();
        let flag = self.store_loading.clone();
        let status = self.store_status.clone();
        let notice = self.bg_notice.clone();
        let db_path = self.db_path.clone();
        let dim = self.cfg.search_dim;
        std::thread::spawn(move || {
            let mut st = VectorStore::new(dim);
            if let Ok(mut db) = Db::open(&db_path) {
                // 예전(f32) 인덱스는 처음 한 번 FP16으로 변환 + DB 압축
                if !db.is_f16() {
                    let size_of = || {
                        let s = |p: &std::path::Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                        s(&db_path) + s(&db_path.with_extension("db-wal"))
                    };
                    let before = size_of();
                    let st2 = status.clone();
                    let r = db.migrate_to_f16(|stage, done, total| {
                        *st2.lock() = format!("{stage} 중… {}/{}", fmt_num(done), fmt_num(total));
                    });
                    let after = size_of();
                    *notice.lock() = Some(match r {
                        Ok(()) => (
                            format!(
                                "인덱스를 FP16으로 변환했습니다: {:.0}MB → {:.0}MB",
                                before as f64 / 1048576.0,
                                after as f64 / 1048576.0
                            ),
                            false,
                        ),
                        Err(e) => (format!("FP16 변환 실패 (다음 실행 때 이어서 시도): {e}"), true),
                    });
                }
                *status.lock() = "인덱스 불러오는 중…".into();
                let _ = db.for_each_vector(|r| {
                    if !st.files.contains_key(&r.file_id) {
                        st.set_file(r.file_id, FileInfo::new(r.path, r.kind));
                    }
                    st.push(
                        Entry {
                            chunk_id: r.chunk_id,
                            file_id: r.file_id,
                            modality: r.modality,
                            locator: r.locator,
                        },
                        &r.vec,
                    );
                });
            }
            st.loaded = true;
            *store.write() = st;
            status.lock().clear();
            flag.store(false, Ordering::SeqCst);
        });
    }

    /// DB 통계와 폴더별 파일 수를 2초마다 갱신
    fn spawn_stats_thread(&self) {
        use crate::search::store::folder_key;
        let stats = self.db_stats.clone();
        let folders = self.folders.clone();
        let cfg_roots = self.cfg_roots_shared.clone();
        let store = self.store.clone();
        let loading = self.store_loading.clone();
        let path = self.db_path.clone();
        std::thread::spawn(move || {
            let Ok(db) = Db::open(&path) else { return };
            loop {
                if let Ok(s) = db.stats() {
                    *stats.lock() = s;
                }
                if !loading.load(Ordering::SeqCst) {
                    // 목록 = 지금 대상 폴더 + 예전에 인덱싱한 폴더 (키 기준 중복 제거)
                    let current: Vec<String> = cfg_roots.lock().clone();
                    let mut paths: Vec<String> = Vec::new();
                    for p in current.iter().cloned().chain(db.roots().unwrap_or_default()) {
                        if !p.trim().is_empty() && !paths.iter().any(|q| folder_key(q) == folder_key(&p)) {
                            paths.push(p);
                        }
                    }
                    let keys: Vec<String> = paths.iter().map(|p| folder_key(p)).collect();
                    let mut counts = vec![0u64; keys.len()];
                    let mut other = 0u64;
                    {
                        let st = store.read();
                        for info in st.files.values() {
                            let mut any = false;
                            for (i, k) in keys.iter().enumerate() {
                                if info.path_lower.starts_with(k.as_str()) {
                                    counts[i] += 1;
                                    any = true;
                                }
                            }
                            if !any {
                                other += 1;
                            }
                        }
                    }
                    let current_keys: Vec<String> = current.iter().map(|p| folder_key(p)).collect();
                    let mut list: Vec<FolderStat> = paths
                        .into_iter()
                        .zip(keys)
                        .zip(counts)
                        .map(|((path, key), files)| FolderStat {
                            current: current_keys.contains(&key),
                            path,
                            key,
                            files,
                        })
                        .collect();
                    // 지금 대상 폴더 먼저, 그다음 예전 폴더
                    list.sort_by(|a, b| b.current.cmp(&a.current).then(a.path.cmp(&b.path)));
                    *folders.lock() = FolderStats { list, other };
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    // ── 인덱싱 ──
    pub fn index_blocker(&self) -> Option<String> {
        if self.indexer.is_active() {
            return Some("이미 진행 중".into());
        }
        if self.store_loading.load(Ordering::SeqCst) {
            return Some("인덱스를 불러오는 중입니다".into());
        }
        if !self.server.state().is_ready() {
            return Some("서버가 준비되지 않았습니다 (서버 탭에서 시작)".into());
        }
        if self.server.capabilities().is_none() {
            return Some("서버 자가진단이 끝나기를 기다리는 중".into());
        }
        if self.cfg.roots.is_empty() {
            return Some("설정에서 인덱싱할 폴더를 추가하세요".into());
        }
        None
    }

    pub fn start_index(&mut self, full_rebuild: bool) {
        if let Some(why) = self.index_blocker() {
            self.notify(why, true);
            return;
        }
        let caps = self.server.capabilities().expect("checked");
        let params = IndexParams {
            cfg: self.cfg.clone(),
            caps,
            port: self.server.port(),
            db_path: self.db_path.clone(),
            data_dir: self.data_dir.clone(),
            store: self.store.clone(),
            full_rebuild,
        };
        match self.indexer.start(params) {
            Ok(()) => {
                if full_rebuild {
                    self.thumbs.clear();
                    self.outcome = None;
                }
                self.tab = Tab::Index;
            }
            Err(e) => self.notify(format!("인덱싱 시작 실패: {e}"), true),
        }
    }

    /// 인덱싱이 서버 충돌로 끝났으면 서버를 다시 띄우고 변경분 인덱싱으로 이어간다.
    /// 처리 중이던 파일은 충돌 의심으로 기록되고, 같은 파일이 두 번 걸리면 건너뛴다.
    fn try_recover_indexing(&mut self) {
        let msg = self.indexer.progress.lock().message.clone();
        if self.cfg.auto_recover && msg.contains("서버 연결이 끊겼습니다") {
            // 서버 감시 스레드가 프로세스 종료를 알아챌 시간을 잠시 준다
            self.pending_recover = Some(Instant::now());
        }
    }

    fn poll_recovery(&mut self) {
        let Some(since) = self.pending_recover else { return };
        let state = self.server.state();
        let server_dead = matches!(
            state,
            crate::server::ServerState::Failed(_) | crate::server::ServerState::Stopped
        );
        if !server_dead && since.elapsed() < Duration::from_secs(3) {
            return;
        }
        self.pending_recover = None;
        if self.recover_attempts >= MAX_RECOVER_ATTEMPTS {
            self.notify(
                format!(
                    "서버가 {MAX_RECOVER_ATTEMPTS}번 연속 비정상 종료되어 자동 복구를 멈췄습니다. 서버 탭의 로그(data\\logs\\llama-server.log)를 확인하세요."
                ),
                true,
            );
            return;
        }
        self.recover_attempts += 1;
        let n = self.recover_attempts;
        if server_dead {
            self.server.log(format!(
                "[앱] 인덱싱 중 서버 비정상 종료 → 자동 복구 {n}/{MAX_RECOVER_ATTEMPTS}: 서버 재시작 후 이어서 인덱싱"
            ));
            self.notify(
                format!("서버가 비정상 종료되어 다시 시작하고 이어서 인덱싱합니다 ({n}/{MAX_RECOVER_ATTEMPTS})"),
                true,
            );
            self.start_server();
        } else {
            // 서버는 살아 있음 (일시적 연결 끊김) → 인덱싱만 이어서
            self.server.log(format!(
                "[앱] 서버 연결이 잠시 끊겼음 → 이어서 인덱싱 ({n}/{MAX_RECOVER_ATTEMPTS})"
            ));
        }
        self.auto_index = true;
    }

    // ── 검색 ──
    pub fn start_search(&mut self, q: QueryKind) {
        if self.search_rx.is_some() {
            return;
        }
        if !self.server.state().is_ready() {
            self.notify("서버가 준비되지 않았습니다. 서버 탭에서 시작하세요.", true);
            return;
        }
        if let QueryKind::Text(s) = &q {
            if s.trim().is_empty() {
                return;
            }
        }
        let filter = Filter {
            sections: self.filter_sections,
            path_terms: self
                .path_filter
                .split_whitespace()
                .map(|s| s.to_lowercase())
                .collect(),
            folders: self.folder_filter(),
        };
        self.last_query = Some(q.clone());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let port = self.server.port();
        let store = self.store.clone();
        let db = self.db.clone();
        let top_k = self.cfg.top_k.max(1);
        let ff = self.cfg.ffmpeg();
        std::thread::spawn(move || {
            let o = query::run(port, &store, &db, q, filter, top_k, ff);
            let _ = tx.send(o);
        });
        self.search_rx = Some(rx);
        self.search_started = Some(Instant::now());
    }

    pub fn thumb_texture(&mut self, ctx: &egui::Context, chunk_id: i64) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(&chunk_id) {
            return t.clone();
        }
        let tex = self
            .db
            .lock()
            .thumb(chunk_id)
            .ok()
            .flatten()
            .and_then(|jpeg| image::load_from_memory(&jpeg).ok())
            .map(|img| {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let ci = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                ctx.load_texture(format!("thumb-{chunk_id}"), ci, egui::TextureOptions::LINEAR)
            });
        self.thumbs.insert(chunk_id, tex.clone());
        tex
    }

    // ── 매 프레임 ──
    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_recovery();
        self.run_auto_actions();
        if let Some((m, err)) = self.bg_notice.lock().take() {
            self.notice = Some(Notice {
                text: m,
                error: err,
                at: Instant::now(),
            });
        }
        {
            let mut shared = self.cfg_roots_shared.lock();
            if *shared != self.cfg.roots {
                *shared = self.cfg.roots.clone();
            }
        }
        if let Some(rx) = &self.search_rx {
            if let Ok(o) = rx.try_recv() {
                if let Some(e) = &o.error {
                    self.notify(format!("검색 실패: {e}"), true);
                }
                if self.thumbs.len() > 600 {
                    self.thumbs.clear();
                }
                self.outcome = Some(o);
                self.search_rx = None;
                self.search_started = None;
            }
        }

        let (state, title_prefix, done_msg) = {
            let p = self.indexer.progress.lock();
            let prefix = match p.state {
                RunState::Running if p.scanning => format!("[탐색 중 {}개] ", fmt_num(p.discovered)),
                RunState::Running => format!(
                    "[인덱싱 {:.0}%] ",
                    p.fraction().unwrap_or(0.0) * 100.0
                ),
                RunState::Paused => "[일시정지] ".to_string(),
                RunState::Stopping => "[중지 중] ".to_string(),
                _ => String::new(),
            };
            let msg = match p.state {
                RunState::Completed => Some((
                    format!(
                        "인덱싱 완료: {}개 처리 (새로 인덱싱 {} · 변경 없음 {} · 오류 {})",
                        fmt_num(p.processed),
                        fmt_num(p.processed - p.unchanged),
                        fmt_num(p.unchanged),
                        fmt_num(p.errors)
                    ),
                    false,
                )),
                RunState::Stopped => Some(("인덱싱을 중지했습니다".to_string(), false)),
                RunState::Failed => Some((format!("인덱싱 오류: {}", p.message), true)),
                _ => None,
            };
            (p.state, prefix, msg)
        };
        if state != self.last_index_state {
            if let Some((m, err)) = done_msg {
                self.notify(m, err);
            }
            match state {
                RunState::Completed | RunState::Stopped => self.recover_attempts = 0,
                RunState::Failed => self.try_recover_indexing(),
                _ => {}
            }
            // SQLite가 지운 청크의 rowid를 재사용할 수 있으므로 썸네일 캐시를 비운다
            if matches!(state, RunState::Completed | RunState::Stopped | RunState::Failed) {
                self.thumbs.clear();
            }
            self.last_index_state = state;
        }
        let title = format!("{title_prefix}Gemma Search");
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        // 이미지 드래그&드롭 → 이미지로 검색
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.first().map(|f| f.path().to_path_buf()) {
            if extract::classify(&path) == Kind::Image {
                self.tab = Tab::Search;
                self.start_search(QueryKind::Image(path));
            } else {
                self.notify("이미지 파일을 놓으면 비슷한 이미지·영상을 찾습니다", true);
            }
        }

        let busy = self.indexer.is_active()
            || self.search_rx.is_some()
            || self.store_loading.load(Ordering::SeqCst)
            || matches!(
                self.server.state(),
                crate::server::ServerState::Starting | crate::server::ServerState::Loading
            );
        ctx.request_repaint_after(if busy {
            Duration::from_millis(150)
        } else {
            Duration::from_millis(800)
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Gemma Search").size(20.0).strong());
            ui.add_space(12.0);
            for (tab, label) in [
                (Tab::Search, "검색"),
                (Tab::Index, "인덱스"),
                (Tab::Server, "서버"),
                (Tab::Settings, "설정"),
            ] {
                ui.selectable_value(&mut self.tab, tab, RichText::new(label).size(16.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ss = self.server.state();
                let r = widgets::badge(
                    ui,
                    widgets::server_color(&ss),
                    &format!("서버 {}", ss.label()),
                    false,
                    matches!(
                        ss,
                        crate::server::ServerState::Starting | crate::server::ServerState::Loading
                    ),
                );
                if r.interact(egui::Sense::click()).clicked() {
                    self.tab = Tab::Server;
                }
                let p = self.indexer.progress.lock().clone();
                if p.state != RunState::Idle {
                    let text = match p.fraction() {
                        Some(f) if p.state.is_active() => format!("{} {:.0}%", p.state.label(), f * 100.0),
                        _ => p.state.label().to_string(),
                    };
                    let r = widgets::badge(
                        ui,
                        widgets::run_color(p.state),
                        &text,
                        false,
                        p.state == RunState::Running,
                    );
                    if r.interact(egui::Sense::click()).clicked() {
                        self.tab = Tab::Index;
                    }
                }
            });
        });
        ui.add_space(4.0);
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let ss = self.server.state();
            widgets::dot(ui, widgets::server_color(&ss));
            let port = self.server.port();
            if ss.is_ready() {
                ui.label(format!("서버 {} :{port}", ss.label()));
            } else {
                ui.label(format!("서버 {}", ss.label()));
            }
            ui.separator();

            let (files, chunks, loading) = {
                let st = self.store.read();
                (st.file_count(), st.live_chunks(), self.store_loading.load(Ordering::SeqCst))
            };
            if loading {
                ui.spinner();
                let s = self.store_status.lock().clone();
                ui.label(if s.is_empty() { "인덱스 불러오는 중…".to_string() } else { s });
            } else {
                ui.label(format!(
                    "인덱스 파일 {}개 · 벡터 {}개",
                    fmt_num(files as u64),
                    fmt_num(chunks as u64)
                ));
            }
            ui.separator();

            let p = self.indexer.progress.lock().clone();
            widgets::dot(ui, widgets::run_color(p.state));
            match p.state {
                RunState::Idle => {
                    ui.label("인덱싱 대기");
                }
                s if s.is_active() => {
                    let pct = p
                        .fraction()
                        .map(|f| format!("{:.0}%", f * 100.0))
                        .unwrap_or_else(|| "탐색 중".into());
                    ui.label(format!(
                        "{} {pct} ({}/{})",
                        s.label(),
                        fmt_num(p.processed),
                        fmt_num(p.discovered)
                    ));
                }
                s => {
                    ui.label(format!("인덱싱 {} — {}개 처리", s.label(), fmt_num(p.processed)));
                }
            }
            ui.separator();

            if self.search_rx.is_some() {
                ui.spinner();
                ui.label("검색 중…");
            } else if let Some(o) = &self.outcome {
                if o.error.is_some() {
                    ui.colored_label(widgets::RED, "검색 실패");
                } else {
                    ui.label(format!(
                        "검색: {}개 ({:.2}초)",
                        o.total_files(),
                        o.elapsed.as_secs_f32()
                    ));
                }
            }

            if let Some(n) = &self.notice {
                if n.at.elapsed() < Duration::from_secs(8) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let c = if n.error { widgets::RED } else { widgets::BLUE };
                        ui.colored_label(c, crate::util::truncate_chars(&n.text, 120))
                            .on_hover_text(&n.text);
                    });
                }
            }
        });
    }

    fn rebuild_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_rebuild {
            return;
        }
        egui::Window::new("전체 재구축")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("기존 인덱스를 모두 지우고 처음부터 다시 만듭니다.");
                ui.label("파일이 많으면 오래 걸릴 수 있습니다. 계속할까요?");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("재구축 시작").color(widgets::RED)).clicked() {
                        self.confirm_rebuild = false;
                        self.recover_attempts = 0;
                        self.start_index(true);
                    }
                    if ui.button("취소").clicked() {
                        self.confirm_rebuild = false;
                    }
                });
            });
    }
}

impl eframe::App for GemmaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        egui::Panel::top("top_bar").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status_bar").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Search => self.search_tab(ui),
            Tab::Index => self.index_tab(ui),
            Tab::Server => self.server_tab(ui),
            Tab::Settings => self.settings_tab(ui),
        });
        self.rebuild_dialog(&ctx);
    }
}

impl Drop for GemmaApp {
    fn drop(&mut self) {
        self.indexer.stop();
        self.server.stop();
    }
}

/// Windows 기본 조합: 라틴은 Segoe UI, 한글은 맑은 고딕.
/// (맑은 고딕만 쓰면 경로의 '\'가 '₩'로 보인다)
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // (이름, 경로, Proportional 앞쪽에 넣을 위치 / None이면 맨 뒤)
    let candidates: [(&str, &str, Option<usize>); 3] = [
        ("segoe-ui", r"C:\Windows\Fonts\segoeui.ttf", Some(0)),
        ("malgun", r"C:\Windows\Fonts\malgun.ttf", Some(1)),
        ("segoe-symbol", r"C:\Windows\Fonts\seguisym.ttf", None),
    ];
    let mut inserted = 0usize;
    for (name, path, pos) in candidates {
        let Ok(bytes) = std::fs::read(path) else { continue };
        fonts
            .font_data
            .insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
        let prop = fonts.families.entry(egui::FontFamily::Proportional).or_default();
        match pos {
            Some(_) => {
                prop.insert(inserted.min(prop.len()), name.to_string());
                inserted += 1;
            }
            None => prop.push(name.to_string()),
        }
        if name != "segoe-ui" {
            // 고정폭(로그)은 기본 Hack 뒤에 한글/기호 대체 글꼴로
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push(name.to_string());
        }
    }
    ctx.set_fonts(fonts);
}
