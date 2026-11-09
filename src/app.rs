//! 앱 상태와 공통 동작 (탭 화면은 ui/ 아래 파일들)

use crate::config::{self, Config};
use crate::extract::{self, Kind};
use crate::index::db::{Db, DbStats};
use crate::index::indexer::{IndexParams, Indexer};
use crate::index::progress::RunState;
use crate::search::query::{self, Outcome, QueryKind};
use crate::search::query::HitView;
use crate::search::store::{Filter, Section, VectorStore};
use crate::server::ServerManager;
use crate::shell::{Shell, ShellEvent, TrayIcon};
use crate::ui::widgets;
use crate::util::fmt_num;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, RichText};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
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

/// 빠른 검색창 크기 (논리 픽셀)
const QUICK_SIZE: [f32; 2] = [920.0, 560.0];

/// 빠른 검색창 모드: 들어가기 전 창 상태를 기억해 두었다가 돌려놓는다
pub struct QuickState {
    prev_outer: Option<egui::Rect>,
    prev_inner: Option<egui::Rect>,
    prev_maximized: bool,
    /// 들어가기 전에 창이 보였는지 (숨김이었으면 닫을 때 다시 숨김)
    was_visible: bool,
    /// 한 번이라도 포커스를 받았는지 (그 뒤 포커스를 잃으면 닫는다)
    got_focus: bool,
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
    /// 검색창 글자를 전부 선택 (전역 단축키로 불렀을 때 바로 새로 입력하도록)
    pub select_query: bool,
    /// 선택한 결과 (섹션, 파일) — 오른쪽 미리보기, 키보드 이동
    pub selected: Option<(Section, i64)>,
    pub scroll_to_selected: bool,
    /// 큰 미리보기 그림 (경로, 위치) → 텍스처
    previews: HashMap<(String, i64), Option<egui::TextureHandle>>,
    preview_pending: HashSet<(String, i64)>,
    preview_tx: Sender<((String, i64), Option<egui::ColorImage>)>,
    preview_rx: Receiver<((String, i64), Option<egui::ColorImage>)>,
    /// 트레이·전역 단축키 (None이면 상주 기능 없음)
    shell: Option<Shell>,
    /// 트레이 메뉴의 "종료": 닫기를 취소하지 않고 끝낸다
    quitting: bool,
    /// 빠른 검색창에서 결과를 열었음 → 이번 프레임 끝에 닫기
    pub leave_quick_after_open: bool,
    /// 빠른 검색창에서 Enter: 지금 입력에 대한 결과가 오면 선택한 결과를 연다
    pub quick_enter: bool,
    /// 창이 보이는 상태 (트레이로 숨기면 false)
    pub window_visible: bool,
    /// 창이 숨겨져 있음 (깨우기 스레드가 본다)
    hidden_flag: Arc<AtomicBool>,
    /// eframe는 첫 프레임을 그린 뒤 창을 무조건 보이게 한다 → 트레이로 시작할 때 처음 몇 프레임 동안 다시 숨긴다
    keep_hidden_frames: u8,
    /// 앱 창의 Win32 핸들 (앞으로 가져오기용)
    main_hwnd: usize,
    force_foreground: Option<Instant>,
    /// 빠른 검색창 모드
    pub quick: Option<QuickState>,
    /// 빠른 검색창에서 마지막으로 입력한 시각 (입력을 멈추면 검색)
    pub query_edited: Option<Instant>,
    /// 서버가 뜨면 실행할 검색
    pending_search: Option<QueryKind>,
    pending_since: Option<Instant>,
    /// 검색·인덱싱 때문에 앱이 켠 서버 (유휴 시 자동 종료 대상)
    server_on_demand: bool,
    /// 마지막 검색·인덱싱 시각
    last_activity: Instant,
    /// 새 버전 (태그, 릴리스 주소)
    pub update_available: Option<(String, String)>,
    /// 준비물 받기 진행 상태
    pub dl: Arc<Mutex<crate::download::DlState>>,
    pub dl_cancel: Arc<AtomicBool>,
    /// 인덱싱 오버레이: 이번 인덱싱 동안 숨김 / 직전 상태 / "창 열기" 눌림
    pub overlay_dismissed: bool,
    pub overlay_last_state: RunState,
    pub overlay_tx: crossbeam_channel::Sender<crate::ui::overlay::OverlayAction>,
    pub overlay_rx: Receiver<crate::ui::overlay::OverlayAction>,
    /// 전체 화면 상태 확인 결과 (확인 시각, 바쁨)
    pub overlay_busy_cache: Mutex<(Option<Instant>, bool)>,
    /// 첫 실행 마법사
    pub setup_open: bool,
    pub setup: crate::ui::setup::SetupState,
    update_rx: Option<Receiver<Option<(String, String)>>>,

    // 설정 화면 보조
    pub exclude_text: String,
    pub new_root: String,

    pub notice: Option<Notice>,
    pub confirm_rebuild: bool,
    last_title: String,
    last_index_state: RunState,

    // 명령줄 자동 실행 (--index, --query)
    pub auto_index: bool,
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

        let (preview_tx, preview_rx) = crossbeam_channel::unbounded();
        let (overlay_tx, overlay_rx) = crossbeam_channel::unbounded();
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
            select_query: false,
            selected: None,
            scroll_to_selected: false,
            previews: HashMap::new(),
            preview_pending: HashSet::new(),
            preview_tx,
            preview_rx,
            shell: None,
            quitting: false,
            leave_quick_after_open: false,
            quick_enter: false,
            window_visible: !std::env::args().any(|a| a == "--background"),
            keep_hidden_frames: if std::env::args().any(|a| a == "--background") { 4 } else { 0 },
            hidden_flag: Arc::new(AtomicBool::new(std::env::args().any(|a| a == "--background"))),
            main_hwnd: {
                use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                cc.window_handle()
                    .ok()
                    .and_then(|h| match h.as_raw() {
                        RawWindowHandle::Win32(w) => Some(w.hwnd.get() as usize),
                        _ => None,
                    })
                    .unwrap_or(0)
            },
            force_foreground: None,
            quick: None,
            query_edited: None,
            pending_search: None,
            pending_since: None,
            server_on_demand: false,
            last_activity: Instant::now(),
            update_available: None,
            update_rx: None,
            dl: Arc::new(Mutex::new(Default::default())),
            dl_cancel: Arc::new(AtomicBool::new(false)),
            setup_open: false,
            setup: Default::default(),
            overlay_dismissed: false,
            overlay_last_state: RunState::Idle,
            overlay_tx,
            overlay_rx,
            overlay_busy_cache: Mutex::new((None, false)),
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
        // 창이 숨겨져 있으면 eframe이 스스로 다시 그리지 않아 logic(자동 인덱싱·복구·서버 끄기·오버레이)이 멈춘다
        // → 숨긴 동안 0.5초마다 깨운다
        {
            let flag = app.hidden_flag.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::Builder::new()
                .name("hidden-ticker".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(500));
                    if flag.load(Ordering::SeqCst) {
                        ctx.request_repaint();
                    }
                })
                .ok();
        }
        app.spawn_stats_thread();
        // 트레이·전역 단축키
        match Shell::start(cc.egui_ctx.clone(), app.cfg.global_hotkey) {
            Ok(s) => {
                if let Some(e) = &s.hotkey_error {
                    app.notify(format!("전역 단축키를 쓸 수 없습니다: {e}"), true);
                }
                app.server.log(format!(
                    "[앱] 트레이 시작 · 단축키 {} · 창 {}",
                    match (&s.hotkey_error, app.cfg.global_hotkey) {
                        (_, false) => "끔".to_string(),
                        (None, true) => format!("{} 등록", crate::shell::HOTKEY_LABEL),
                        (Some(e), true) => format!("등록 실패 ({e})"),
                    },
                    if app.window_visible { "보임" } else { "숨김(트레이로 시작)" }
                ));
                app.shell = Some(s);
            }
            Err(e) => app.notify(format!("트레이 아이콘을 만들지 못했습니다: {e}"), true),
        }
        // 로그인 시 실행: exe를 옮겼으면 경로를 다시 맞춘다
        if let Err(e) = crate::autostart::sync(app.cfg.start_with_windows) {
            app.notify(format!("로그인 시 실행 설정 실패: {e}"), true);
        }
        // 새 버전 확인 (하루 한 번)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if app.cfg.check_updates && now.saturating_sub(app.cfg.last_update_check) > 24 * 3600 {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::update::check().ok().flatten());
                ctx.request_repaint();
            });
            app.update_rx = Some(rx);
            app.cfg.last_update_check = now;
            let mut s = app.saved_cfg.clone();
            s.last_update_check = now;
            if s.save(&app.data_dir).is_ok() {
                app.saved_cfg = s;
            }
        }

        // 명령줄: --start-server, --index(서버 준비 후 변경분 인덱싱), --query "질의", --tab 이름
        let args: Vec<String> = std::env::args().skip(1).collect();
        let flag = |f: &str| args.iter().any(|a| a == f);
        let value = |f: &str| {
            args.iter()
                .position(|a| a == f)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        app.auto_index = flag("--index") || app.cfg.index_on_start;
        app.auto_query = value("--query");
        if app.cfg.auto_start_server || flag("--start-server") {
            app.start_server_manual();
        } else if app.auto_index || app.auto_query.is_some() {
            app.start_server_auto();
        }
        // 첫 실행: 준비물·폴더가 이미 다 있으면(예전 버전에서 쓰던 설정) 마법사 없이 끝난 것으로
        if !app.cfg.setup_done {
            let ready = std::path::Path::new(app.cfg.model_path.trim()).is_file()
                && crate::server::resolve_binary(&app.cfg.llama_path).is_ok()
                && !app.cfg.roots.is_empty();
            if ready {
                app.cfg.setup_done = true;
                let mut s = app.saved_cfg.clone();
                s.setup_done = true;
                if s.save(&app.data_dir).is_ok() {
                    app.saved_cfg = s;
                }
            } else {
                app.open_setup(&cc.egui_ctx);
                // 트레이로 시작했어도 마법사는 보여야 한다
                if !app.window_visible {
                    cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    app.window_visible = true;
                    app.hidden_flag.store(false, Ordering::SeqCst);
                }
            }
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
        s.min_relevance = self.cfg.min_relevance;
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
            Ok(()) => {
                let actual = self.server.port();
                if actual != self.cfg.port {
                    // 설정 포트를 쓸 수 없어 다른 포트로 띄움 → 다음에도 그 포트를 쓰도록 저장
                    let old = self.cfg.port;
                    self.cfg.port = actual;
                    let mut s = self.saved_cfg.clone();
                    s.port = actual;
                    if s.save(&self.data_dir).is_ok() {
                        self.saved_cfg = s;
                    }
                    self.notify(
                        format!("포트 {old}를 쓸 수 없어(Windows 예약 범위 등) 포트 {actual}로 서버를 시작합니다"),
                        false,
                    );
                } else {
                    self.notify("llama-server를 시작합니다…", false);
                }
            }
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
        self.server_on_demand = false;
        self.pending_search = None;
        self.notify("서버를 중지했습니다", false);
    }

    // ── 상주: 트레이·빠른 검색창·서버 자동 켜기/끄기 ──

    /// 서버 탭·트레이에서 직접 켠 서버: 유휴 시 자동으로 끄지 않는다
    pub fn start_server_manual(&mut self) {
        self.server_on_demand = false;
        self.start_server();
    }

    /// 검색·인덱싱 때문에 앱이 켠 서버: 쓰지 않으면 자동으로 끈다
    pub fn start_server_auto(&mut self) {
        self.server_on_demand = true;
        self.last_activity = Instant::now();
        self.start_server();
    }

    /// 서버가 뜨기를 기다리는 검색이 있는지
    pub fn pending_search_waiting(&self) -> bool {
        self.pending_search.is_some()
    }

    /// 오버레이의 "창 열기"
    pub fn show_window_from_overlay(&mut self, ctx: &egui::Context) {
        if self.quick.is_some() {
            self.leave_quick(ctx, true);
        } else {
            self.show_window(ctx);
        }
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.window_visible = true;
        self.hidden_flag.store(false, Ordering::SeqCst);
        self.force_foreground = Some(Instant::now());
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        self.window_visible = false;
        self.hidden_flag.store(true, Ordering::SeqCst);
    }

    /// 빠른 검색창: 창을 테두리 없이 작게, 마우스가 있는 화면 위쪽 가운데에, 항상 위로
    pub fn enter_quick(&mut self, ctx: &egui::Context) {
        if self.quick.is_none() {
            let (outer, inner, maximized) = ctx.input(|i| {
                let v = i.viewport();
                (v.outer_rect, v.inner_rect, v.maximized.unwrap_or(false))
            });
            self.quick = Some(QuickState {
                prev_outer: outer,
                prev_inner: inner,
                prev_maximized: maximized,
                was_visible: self.window_visible,
                got_focus: false,
            });
            let size = egui::vec2(QUICK_SIZE[0], QUICK_SIZE[1]);
            let npp = ctx.native_pixels_per_point().unwrap_or(1.0).max(0.5);
            if maximized {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            if let Some((x, y, w, h)) = crate::shell::cursor_work_area() {
                let px = x as f32 + (w as f32 - size.x * npp) / 2.0;
                let py = y as f32 + h as f32 * 0.18;
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(px / npp, py / npp)));
            }
        }
        self.tab = Tab::Search;
        self.focus_search = true;
        self.select_query = true;
        self.show_window(ctx);
    }

    /// 빠른 검색창을 닫는다. keep_open이면 원래 큰 창으로, 아니면 원래 상태(숨김이었으면 숨김)로.
    pub fn leave_quick(&mut self, ctx: &egui::Context, keep_open: bool) {
        let Some(q) = self.quick.take() else { return };
        self.quick_enter = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        if let Some(r) = q.prev_inner {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(r.size()));
        }
        if let Some(r) = q.prev_outer {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(r.min));
        }
        if q.prev_maximized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        if keep_open || q.was_visible {
            self.show_window(ctx);
        } else {
            self.hide_window(ctx);
        }
    }

    /// 빠른 검색창의 검색을 큰 창에서 이어서
    pub fn open_full_from_quick(&mut self, ctx: &egui::Context) {
        self.leave_quick(ctx, true);
        self.result_view = None;
    }

    fn handle_shell(&mut self, ctx: &egui::Context) {
        let events: Vec<ShellEvent> = match &self.shell {
            Some(s) => s.rx.try_iter().collect(),
            None => return,
        };
        for ev in events {
            match ev {
                ShellEvent::QuickSearch => {
                    if self.quick.is_some() {
                        self.leave_quick(ctx, false);
                    } else {
                        self.enter_quick(ctx);
                    }
                }
                ShellEvent::Open => {
                    if self.quick.is_some() {
                        self.leave_quick(ctx, true);
                    } else {
                        self.show_window(ctx);
                    }
                }
                ShellEvent::ToggleIndex => {
                    if self.indexer.is_active() {
                        if self.indexer.progress.lock().state == RunState::Paused {
                            self.indexer.resume();
                        } else {
                            self.indexer.pause();
                        }
                    } else if self.server.state().is_ready() {
                        self.start_index(false);
                    } else {
                        if !self.server.state().is_running() {
                            self.start_server_auto();
                        }
                        self.auto_index = true;
                    }
                }
                ShellEvent::ToggleServer => {
                    if self.server.state().is_running() {
                        self.stop_server();
                    } else {
                        self.start_server_manual();
                    }
                }
                ShellEvent::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    /// 창 닫기: 트레이가 있으면 끝내지 않고 숨긴다 (트레이 메뉴의 "종료"로 끝냄)
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quitting || !self.cfg.close_to_tray || self.shell.is_none() {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.quick.is_some() {
            self.leave_quick(ctx, false);
        }
        self.hide_window(ctx);
        if !self.cfg.tray_notice_shown {
            if let Some(s) = &self.shell {
                let hk = if s.hotkey_registered() {
                    format!("{}로 어디서든 검색할 수 있습니다. ", crate::shell::HOTKEY_LABEL)
                } else {
                    String::new()
                };
                s.balloon(
                    "Gemma Search는 트레이에서 계속 실행됩니다",
                    &format!("{hk}끝내려면 트레이 아이콘을 우클릭 → 종료."),
                );
            }
            self.cfg.tray_notice_shown = true;
            let mut s = self.saved_cfg.clone();
            s.tray_notice_shown = true;
            if s.save(&self.data_dir).is_ok() {
                self.saved_cfg = s;
            }
        }
    }

    /// 트레이 아이콘·툴팁·메뉴 상태
    fn update_tray(&mut self) {
        let Some(shell) = &self.shell else { return };
        let (state, frac, scanning, discovered) = {
            let p = self.indexer.progress.lock();
            (p.state, p.fraction(), p.scanning, p.discovered)
        };
        let files = self.store.read().file_count();
        let server = self.server.state();
        let (icon, indexing, tip) = match state {
            RunState::Running | RunState::Stopping if scanning => (
                TrayIcon::Busy,
                Some(false),
                format!("Gemma Search · 폴더 확인 중 ({}개)", fmt_num(discovered)),
            ),
            RunState::Running | RunState::Stopping => (
                TrayIcon::Busy,
                Some(false),
                format!("Gemma Search · 인덱싱 {:.0}%", frac.unwrap_or(0.0) * 100.0),
            ),
            RunState::Paused => (TrayIcon::Paused, Some(true), "Gemma Search · 인덱싱 일시정지".to_string()),
            _ => (
                TrayIcon::Idle,
                None,
                format!(
                    "Gemma Search · 파일 {}개 · 서버 {}",
                    fmt_num(files as u64),
                    if server.is_ready() { "준비됨" } else if server.is_running() { "시작 중" } else { "꺼짐" }
                ),
            ),
        };
        shell.update(tip, icon, indexing, server.is_running());
    }

    /// 앱이 켠 서버를 오래 쓰지 않으면 끈다 (VRAM 반환)
    fn idle_stop(&mut self) {
        if self.indexer.is_active() || self.search_rx.is_some() || self.pending_search.is_some() {
            self.last_activity = Instant::now();
            return;
        }
        if !self.server_on_demand || self.cfg.idle_stop_minutes == 0 || self.pending_recover.is_some() || self.auto_index {
            return;
        }
        if self.server.state().is_running()
            && self.last_activity.elapsed() > Duration::from_secs(self.cfg.idle_stop_minutes as u64 * 60)
        {
            self.server.stop();
            self.server_on_demand = false;
            self.server.log(format!(
                "[앱] {}분 동안 검색·인덱싱이 없어 서버를 껐습니다 (다음 검색 때 자동으로 켭니다)",
                self.cfg.idle_stop_minutes
            ));
        }
    }

    /// 서버가 뜨면 기다리던 검색 실행, 서버가 실패하면 알림
    fn run_pending_search(&mut self) {
        if self.pending_search.is_none() {
            return;
        }
        let st = self.server.state();
        if st.is_ready() && self.search_rx.is_none() {
            if let Some(q) = self.pending_search.take() {
                self.start_search(q);
            }
        } else if matches!(st, crate::server::ServerState::Failed(_) | crate::server::ServerState::Stopped)
            && !self.server.state().is_running()
            && self.pending_since.is_some_and(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.pending_search = None;
            self.notify("서버를 켜지 못해 검색하지 못했습니다. 서버 탭을 확인하세요.", true);
        }
    }

    /// 빠른 검색창: 입력을 멈추면 검색, 다른 곳을 누르면 닫기, 첫 표시 때 앞으로 가져오기
    fn quick_tick(&mut self, ctx: &egui::Context) {
        if let Some(t) = self.query_edited {
            if t.elapsed() > Duration::from_millis(250) && self.search_rx.is_none() {
                self.query_edited = None;
                let q = self.query.trim().to_string();
                if !q.is_empty() {
                    self.start_search(QueryKind::Text(q));
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(60));
            }
        }
        // 창을 띄운 직후 포커스를 못 받았으면 Win32로 한 번 더 앞으로
        if let Some(t) = self.force_foreground {
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if focused || t.elapsed() > Duration::from_secs(2) {
                self.force_foreground = None;
            } else if t.elapsed() > Duration::from_millis(200) && self.main_hwnd != 0 {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(self.main_hwnd as _);
                }
                self.force_foreground = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        let focused = ctx.input(|i| i.viewport().focused);
        if let Some(q) = &mut self.quick {
            match focused {
                Some(true) => q.got_focus = true,
                Some(false) if q.got_focus => {
                    // PowerToys Run처럼 다른 곳을 누르면 사라진다
                    self.leave_quick(ctx, false);
                }
                _ => {}
            }
        }
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
                // 키워드 색인 이전 인덱스: 파일 이름을 색인에 채운다 (파일을 읽지 않음, 처음 한 번)
                *status.lock() = "키워드 색인 준비 중…".into();
                if let Ok(n) = db.backfill_name_fts() {
                    if n > 0 {
                        *notice.lock() = Some((
                            format!("파일 이름 {}개를 키워드 색인에 넣었습니다. 문서 본문은 다음 인덱싱 때 들어갑니다.", fmt_num(n)),
                            false,
                        ));
                    }
                }
                *status.lock() = "인덱스 불러오는 중…".into();
                let _ = st.fill_from_db(&db);
                // 허브 보정 기준 질의 (저장돼 있으면 서버 없이 바로)
                crate::search::hub::load_saved(&db, &mut st);
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
        if let QueryKind::Text(s) = &q {
            if s.trim().is_empty() {
                return;
            }
        }
        self.last_activity = Instant::now();
        if !self.server.state().is_ready() {
            // 서버가 꺼져 있으면 켜고, 준비되면 이 검색을 실행
            let st = self.server.state();
            if st.is_running() {
                self.pending_search = Some(q);
                self.pending_since = Some(Instant::now());
            } else if self.cfg.server_on_demand {
                self.start_server_auto();
                self.pending_search = Some(q);
                self.pending_since = Some(Instant::now());
                self.notify("서버를 켜는 중입니다. 준비되면 바로 검색합니다.", false);
            } else {
                self.notify("서버가 준비되지 않았습니다. 서버 탭에서 시작하세요.", true);
            }
            return;
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
        let min_score = self.cfg.min_relevance;
        std::thread::spawn(move || {
            let o = query::run(port, &store, &db, q, filter, top_k, min_score, ff);
            let _ = tx.send(o);
        });
        self.search_rx = Some(rx);
        self.search_started = Some(Instant::now());
    }

    /// 미리보기 그림 (없으면 백그라운드에서 만들기 시작하고 None)
    pub fn preview_texture(&mut self, ctx: &egui::Context, hv: &HitView) -> Option<egui::TextureHandle> {
        let key = (hv.hit.path.clone(), hv.hit.locator);
        if let Some(t) = self.previews.get(&key) {
            return t.clone();
        }
        if self.preview_pending.insert(key.clone()) {
            let tx = self.preview_tx.clone();
            let ff = self.cfg.ffmpeg();
            let seg = self.cfg.video_window_secs() as f64;
            let (kind, modality) = (hv.hit.kind, hv.hit.modality);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let img = crate::ui::preview::make_preview(&key.0, kind, modality, key.1, ff.as_deref(), seg);
                let _ = tx.send((key, img));
                ctx.request_repaint();
            });
        }
        None
    }

    pub fn preview_loading(&self, hv: &HitView) -> bool {
        self.preview_pending.contains(&(hv.hit.path.clone(), hv.hit.locator))
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
                self.selected = None;
                self.outcome = Some(o);
                self.search_rx = None;
                self.search_started = None;
            }
        }

        // 큰 미리보기 그림이 준비됨
        while let Ok((key, img)) = self.preview_rx.try_recv() {
            self.preview_pending.remove(&key);
            if self.previews.len() > 24 {
                self.previews.clear();
            }
            let tex = img.map(|ci| ctx.load_texture(format!("preview-{}-{}", key.0, key.1), ci, egui::TextureOptions::LINEAR));
            self.previews.insert(key, tex);
        }
        // 준비물 받기가 끝나면 경로를 맞추고 저장
        let finished = {
            let mut g = self.dl.lock();
            g.finished.take().map(|f| (g.pack, f))
        };
        if let Some((Some(pack), (ok, msg))) = finished {
            if ok {
                let dir = std::path::PathBuf::from(&msg);
                let mut s = self.saved_cfg.clone();
                if pack == crate::download::Pack::Model {
                    self.cfg.model_path = dir.join("embeddinggemma-2-Q8_0.gguf").display().to_string();
                    self.cfg.mmproj_path = dir.join("mmproj-embeddinggemma-2-Q8_0.gguf").display().to_string();
                    s.model_path = self.cfg.model_path.clone();
                    s.mmproj_path = self.cfg.mmproj_path.clone();
                } else {
                    self.cfg.llama_path = msg.clone();
                    s.llama_path = msg.clone();
                }
                if s.save(&self.data_dir).is_ok() {
                    self.saved_cfg = s;
                }
                self.notify(format!("{} 받기 완료: {msg} (설정에 반영했습니다)", pack.label()), false);
            } else {
                self.notify(format!("{} 받기 실패: {msg}", pack.label()), true);
            }
            // 마법사의 "모두 받기": 다음 준비물
            self.setup_next_download(ctx, ok);
        }

        // 트레이·단축키, 기다리던 검색, 유휴 서버, 빠른 검색창
        self.handle_shell(ctx);
        self.run_pending_search();
        self.idle_stop();
        self.update_tray();
        self.quick_tick(ctx);
        if let Some(rx) = &self.update_rx {
            if let Ok(r) = rx.try_recv() {
                if let Some((tag, _)) = &r {
                    self.notify(format!("새 버전 {tag}이 나왔습니다. 상태 표시줄의 링크에서 받을 수 있습니다."), false);
                }
                self.update_available = r;
                self.update_rx = None;
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

            if let Some((tag, url)) = &self.update_available {
                ui.separator();
                ui.hyperlink_to(RichText::new(format!("새 버전 {tag} 받기")).color(widgets::GREEN), url);
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
    /// 창이 숨겨져 있어도(트레이) 불린다: 이벤트 처리는 전부 여기서.
    /// (eframe은 숨긴 창에서 `ui`를 부르지 않고 `logic`만 부른다)
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.keep_hidden_frames > 0 {
            self.keep_hidden_frames -= 1;
            if !self.window_visible {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                ctx.request_repaint();
            }
        }
        self.poll(ctx);
        self.handle_close(ctx);
        self.overlay_logic(ctx);
        // 트레이로 숨겨져 있어도 인덱싱 오버레이는 그려야 한다 → 이번 프레임은 ui를 부르게
        if !self.window_visible && self.overlay_should_show(ctx) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.index_overlay(&ctx);
        // 숨긴 상태에서 오버레이 때문에 불렸으면 본 창은 그리지 않는다
        if !self.window_visible && self.quick.is_none() {
            return;
        }
        if self.quick.is_some() {
            // Esc: 입력칸에 포커스가 있든 없든 바로 닫는다 (입력칸이 Esc를 먹어 포커스만 잃던 문제)
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.leave_quick(&ctx, false);
            } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::O)) {
                self.open_full_from_quick(&ctx);
            } else {
                self.quick_ui(ui);
                if self.leave_quick_after_open {
                    self.leave_quick_after_open = false;
                    self.leave_quick(&ctx, false);
                }
                return;
            }
        }
        if self.setup_open {
            egui::CentralPanel::default().show(ui, |ui| self.setup_ui(ui));
            return;
        }
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
