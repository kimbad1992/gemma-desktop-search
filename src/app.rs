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

/// 설정 탭에서 보고 있는 화면
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SettingsView {
    /// 쉬운 말로 된 기본 설정
    #[default]
    Basic,
    /// 전문 항목
    Advanced,
}

/// 지금 바로 검색할 수 있는 상태인지 (일반 설정의 "지금 상태")
#[derive(Clone, Debug, Default)]
pub struct Health {
    /// llama.cpp 실행 파일이 있는지
    pub engine: bool,
    pub model: bool,
    /// None = 설정하지 않음(글만 검색), Some(false) = 설정했는데 파일이 없음
    pub mmproj: Option<bool>,
    pub folders: usize,
    pub missing_folders: usize,
    /// 설정의 폴더가 지금 있는지 (설정의 폴더 순서대로)
    pub root_exists: Vec<bool>,
    pub indexed_files: usize,
    /// ffmpeg와 ffprobe가 모두 있는지
    pub ffmpeg: bool,
}

impl Health {
    /// 인덱싱·검색을 시작할 준비물이 다 있는지 (인덱스가 있는지는 따로 본다)
    pub fn ready(&self) -> bool {
        self.engine && self.model && self.folders > 0
    }
}

/// 서버를 켤 때 쓰인 설정. 켠 뒤에 이 값들이 바뀌면 다시 시작해야 적용된다.
#[derive(Clone, PartialEq)]
struct ServerKey {
    llama_path: String,
    model_path: String,
    mmproj_path: String,
    extra_dll_dir: String,
    port: u16,
    n_gpu_layers: i32,
    ctx_size: u32,
    parallel: u32,
    batch_size: u32,
    ubatch_size: u32,
    flash_attn: String,
    extra_args: String,
}

impl ServerKey {
    fn of(c: &Config) -> Self {
        Self {
            llama_path: c.llama_path.clone(),
            model_path: c.model_path.clone(),
            mmproj_path: c.mmproj_path.clone(),
            extra_dll_dir: c.extra_dll_dir.clone(),
            port: c.port,
            n_gpu_layers: c.n_gpu_layers,
            ctx_size: c.ctx_size,
            parallel: c.parallel,
            batch_size: c.batch_size,
            ubatch_size: c.ubatch_size,
            flash_attn: c.flash_attn.clone(),
            extra_args: c.extra_args.clone(),
        }
    }
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

/// 화면 확인용 (`--shot <png>`): 창을 화면에 띄우지 않고 그린 뒤 PNG로 저장하고 끝낸다.
/// 누가 PC를 쓰는 중에도 포커스를 뺏지 않고 화면을 확인할 수 있다.
///
/// 같이 쓰는 인자: `--tab`, `--theme`, `--scroll`, `--size 1280x860`, `--query "질의"`,
/// `--select`(첫 결과를 고른 상태), `--quick`(빠른 검색창), `--setup`(처음 설정), `--shot-wait <초>`,
/// `--index --shot-indexing`(인덱싱이 도는 중), `--overlay`(인덱싱 알림 창), `--view images`(결과의 섹션 보기),
/// `--open filter_kinds`(필터 드롭다운을 펼친 채로), `--update-on-start`(업데이트를 누른 것처럼. 다시 시작은 하지 않는다),
/// `--fake-update v9.9.9`(새 버전이 있는 것처럼).
/// 쓰는 인덱스와 섞이지 않게 `GEMMA_SEARCH_DATA`로 별도 데이터 폴더를 준다.
/// 예전 데이터를 옮길지 묻는 화면(`ui::launch`)은 `GEMMA_SEARCH_DATA` 대신 실행 파일을 시험 폴더에 복사하고
/// `LOCALAPPDATA`를 시험 폴더로 바꿔 찍는다. `--shot-move`면 "옮기기"를 누른 것처럼 옮긴 뒤 열린 앱을 찍는다.
/// 설정의 옮기기 확인 창은 `--propose-import <폴더>`, `--propose-move-to <폴더>` (폴더를 고른 것처럼. 옮기지는 않는다).
pub struct Shot {
    path: PathBuf,
    /// 화면이 자리 잡은 뒤 이만큼 더 기다렸다가 찍는다
    wait: Duration,
    started: Instant,
    settled: Option<Instant>,
    requested: bool,
    done: Arc<AtomicBool>,
    /// 첫 결과를 고른 상태로 찍는다 (미리보기 패널 확인)
    pub select: bool,
    /// 인덱싱이 도는 중에 찍는다 (`--index`와 함께)
    when_indexing: bool,
    /// 큰 창 대신 인덱싱 알림 창을 그린다 (크기는 `--size 392x136`)
    pub overlay: bool,
}

impl Shot {
    /// 한 프레임마다: 화면이 `busy`가 아닌 채로 자리 잡으면 찍는다. 다 찍었으면 true (창을 닫는다).
    pub(crate) fn tick(&mut self, ctx: &egui::Context, busy: bool) -> bool {
        if self.done.load(Ordering::SeqCst) {
            return true;
        }
        if self.started.elapsed() > Duration::from_secs(120) {
            // 서버가 뜨지 않는 등으로 끝나지 않는 경우
            eprintln!("--shot: 화면이 준비되지 않아 찍지 못했습니다");
            std::process::exit(2);
        }
        // 숨긴 창은 eframe이 그리지 않으므로 매번 그려 달라고 한다 (배치가 몇 프레임에 걸쳐 자리 잡는다)
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        ctx.request_repaint_after(Duration::from_millis(50));
        if busy {
            self.settled = None;
            return false;
        }
        let since = *self.settled.get_or_insert_with(Instant::now);
        if !self.requested && since.elapsed() >= self.wait {
            self.requested = true;
            let (path, done, ctx2) = (self.path.clone(), self.done.clone(), ctx.clone());
            ctx.request_screenshot(move |img| {
                let [w, h] = img.size;
                if let Err(e) = image::save_buffer(&path, img.as_raw(), w as u32, h as u32, image::ColorType::Rgba8) {
                    eprintln!("--shot: 저장 실패: {e}");
                }
                done.store(true, Ordering::SeqCst);
                ctx2.request_repaint();
            });
        }
        false
    }

    pub(crate) fn from_args() -> Option<Self> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
        let path = PathBuf::from(value("--shot")?);
        Some(Self {
            path,
            wait: Duration::from_secs_f32(value("--shot-wait").and_then(|v| v.parse().ok()).unwrap_or(1.0)),
            started: Instant::now(),
            settled: None,
            requested: false,
            done: Arc::new(AtomicBool::new(false)),
            select: args.iter().any(|a| a == "--select"),
            when_indexing: args.iter().any(|a| a == "--shot-indexing"),
            overlay: args.iter().any(|a| a == "--overlay"),
        })
    }
}

/// 백그라운드 작업이 끝났을 때 화면을 다시 그리게 하는 함수
pub(crate) fn waker(ctx: &egui::Context) -> crate::util::Waker {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

/// `gemma-search.exe lend …`로 실행됨: 트레이로 시작해 바로 계산 빌려주기를 한다 (main.rs의 gui_lend)
pub static LEND_ON_START: AtomicBool = AtomicBool::new(false);

/// 창 없이 시작하는지: 트레이로 시작(`--background`), 화면 확인용(`--shot`), 계산 빌려주기(`lend`)
fn starts_hidden() -> bool {
    std::env::args().any(|a| a == "--background" || a == "--shot") || LEND_ON_START.load(Ordering::SeqCst)
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
    /// 사용자가 결과를 직접 고른(↑↓·클릭) 때의 검색어. 같은 검색어의 결과가 다시 오면 고르던 줄을 지킨다.
    pub nav_query: Option<String>,
    /// 지금 선택이 화면 목록의 몇 번째 줄인지
    pub selected_row: Option<usize>,
    /// 새 결과가 와서 선택을 다시 맞춰야 할 때: 고르던 줄 번호 (화면 목록이 정해지면 맞춘다)
    pub reselect: Option<usize>,
    /// 큰 미리보기 그림 (경로, 위치) → 텍스처
    previews: HashMap<(String, i64), Option<egui::TextureHandle>>,
    preview_pending: HashSet<(String, i64)>,
    preview_tx: Sender<((String, i64), Option<egui::ColorImage>)>,
    preview_rx: Receiver<((String, i64), Option<egui::ColorImage>)>,
    /// 트레이·전역 단축키 (None이면 상주 기능 없음)
    shell: Option<Shell>,
    /// 화면 요소 미리보기 (`--tab gallery`, 개발용)
    show_gallery: bool,
    /// 마지막으로 본 트레이·단축키 등록 상태 (바뀐 것만 로그·알림)
    last_shell_status: crate::shell::ShellStatus,
    /// 트레이 아이콘 없이 창이 숨겨진 시각 (오래 이어지면 창을 꺼낸다)
    hidden_no_tray_since: Option<Instant>,
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
    /// 화면 확인용 실행 (`--shot`)
    pub shot: Option<Shot>,
    /// 폴더 감시 (설정에서 켰을 때만)
    pub watcher: Option<crate::watch::Watcher>,
    /// 감시가 알려 준 바뀐 경로 가운데, 서버가 준비되기를 기다리는 것
    watch_batch: Vec<String>,
    /// 감시 때문에 서버를 켜려고 한 마지막 시각 (서버가 뜨지 않을 때 계속 다시 켜지 않게)
    watch_server_try: Option<Instant>,
    /// 업데이트 진행 상태 (받기·교체는 백그라운드 스레드가 한다)
    pub updater: crate::update::Shared,
    /// 마지막으로 처리한 업데이트 단계 (바뀌었을 때 한 번만 알리려고)
    update_seen: crate::update::Stage,
    /// 화면 확인용 (`--view images`): 결과가 오면 이 섹션 보기로 바꾼다
    shot_view: Option<Section>,
    /// 메뉴 맨 위의 앱 아이콘
    logo: Option<egui::TextureHandle>,
    /// 제목 표시줄 색을 맞춰 둔 테마 (어두운 테마면 true)
    titlebar_dark: Option<bool>,
    /// 빠른 검색창 모양인 채로 숨겨 둔 창의 원래(큰 창) 모양. 다음에 큰 창을 보일 때 되돌린다.
    parked_shape: Option<QuickState>,
    /// logic 쪽에서 빠른 검색창을 닫아 달라고 함 (ui가 이번 프레임을 그린 뒤 닫는다)
    quick_close: bool,
    /// 빠른 검색창에서 마지막으로 입력한 시각 (입력을 멈추면 검색)
    pub query_edited: Option<Instant>,
    /// 서버가 뜨면 실행할 검색
    pending_search: Option<QueryKind>,
    pending_since: Option<Instant>,
    /// 검색·인덱싱 때문에 앱이 켠 서버 (유휴 시 자동 종료 대상)
    server_on_demand: bool,
    /// 마지막 검색·인덱싱 시각
    pub(crate) last_activity: Instant,
    /// 새 버전 (태그, 릴리스 주소)
    pub update_available: Option<(String, String)>,
    /// 준비물 받기 진행 상태
    pub dl: Arc<Mutex<crate::download::DlState>>,
    pub dl_cancel: Arc<AtomicBool>,
    /// 차례로 받을 준비물 ("필요한 것 모두 받기")
    pub dl_queue: Vec<crate::download::Pack>,
    /// PC 사양 확인 (첫 실행 마법사와 설정이 같이 쓴다)
    pub probe: crate::ui::probe::ProbeState,
    /// llama.cpp·모델 파일이 쓸 만한지 확인한 결과 (마법사, 고급 설정)
    pub file_checks: crate::ui::probe::FileChecks,
    /// 사양 확인이 끝나면: 추천값을 바로 적용 / 빠진 준비물 받기 시작
    pub apply_plan_when_probed: bool,
    pub download_when_probed: bool,
    /// 설정 탭에서 보고 있는 화면 (일반 | 고급)
    pub settings_view: SettingsView,
    /// 준비 상태 요약 (잠깐 캐시: 네트워크 폴더 확인을 매 프레임 하지 않도록)
    health_cache: Option<(Instant, Health)>,
    /// 서버를 켤 때의 서버 관련 설정 (그 뒤 바뀌었으면 재시작 안내)
    server_cfg_at_start: Option<ServerKey>,
    /// 인덱싱 오버레이: 이번 인덱싱 동안 숨김 / 직전 상태 / "창 열기" 눌림
    pub overlay_dismissed: bool,
    pub overlay_last_state: RunState,
    pub overlay_tx: crossbeam_channel::Sender<crate::ui::overlay::OverlayAction>,
    pub overlay_rx: Receiver<crate::ui::overlay::OverlayAction>,
    /// 전체 화면 상태 확인 결과 (확인 시각, 바쁨)
    pub overlay_busy_cache: Mutex<(Option<Instant>, bool)>,
    /// 계산 빌려주기 (ui/lend.rs): 돌고 있는 세션 / 실패로 끝난 마지막 세션의 상태(닫기 전까지) /
    /// 세션이 서버를 켜 달라고 함 / 이번 세션 동안 오버레이 숨김
    pub lend: Option<crate::lend::LendSession>,
    pub lend_last: Option<Arc<Mutex<crate::lend::LendStatus>>>,
    pub lend_want_server: Arc<AtomicBool>,
    pub lend_overlay_dismissed: bool,
    /// 첫 실행 마법사
    pub setup_open: bool,
    pub setup: crate::ui::setup::SetupState,
    update_rx: Option<Receiver<Option<(String, String)>>>,

    // 설정 화면 보조
    pub exclude_text: String,
    pub new_root: String,

    pub notice: Option<Notice>,
    pub confirm_rebuild: bool,
    /// 데이터 옮기기·가져오기 확인 창에 띄운 계획 (`ui::data_location`)
    pub data_move: Option<crate::location::Plan>,
    last_title: String,
    last_index_state: RunState,

    // 명령줄 자동 실행 (--index, --query)
    pub auto_index: bool,
    auto_query: Option<String>,

    /// 이번 인덱싱 작업에서 서버 충돌 후 자동 복구한 횟수
    pub recover_attempts: u32,
    pending_recover: Option<Instant>,

    /// 펼침 메뉴 밑의 진짜 흐림 (wgpu로 그릴 때만, `ui::backdrop`)
    glass_blur: Option<crate::ui::backdrop::Blur>,
    /// 첫 프레임에 OS 바탕(Acrylic)을 쓸 수 있는지 확인했는지 (`glass::OS_BACKDROP`)
    backdrop_checked: bool,
    /// 빠른 검색창이 지금 OS의 Acrylic을 깔고 있는지 (창 뒤가 흐리게 비친다)
    pub(crate) quick_acrylic: bool,
}

/// 왼쪽 메뉴(진한 유리)의 안쪽 여백
const NAV_PAD: egui::Margin = egui::Margin {
    left: 10,
    right: 10,
    top: 14,
    bottom: 10,
};

/// 한 번의 인덱싱 작업에서 서버 자동 재시작 최대 횟수
pub const MAX_RECOVER_ATTEMPTS: u32 = 5;

/// 글꼴과 테마 (창을 만들 때 한 번). 글꼴을 먼저: 테마의 굵은 글자 스타일이 "bold" 패밀리를 쓴다.
pub fn install_look(ctx: &egui::Context) {
    setup_fonts(ctx);
    crate::ui::theme::install(ctx);
}

impl GemmaApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_look(&cc.egui_ctx);
        Self::start(&cc.egui_ctx, crate::platform::native_window(cc))
    }

    /// 앱을 연다: 데이터 폴더의 설정·인덱스를 읽고 트레이·단축키 등을 시작한다.
    /// 데이터를 옮긴 뒤에 여는 경우(`ui::launch`)에는 창이 이미 있으므로 따로 부른다. `install_look`은 먼저 해 둔다.
    pub fn start(egui_ctx: &egui::Context, main_hwnd: usize) -> Self {
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
            nav_query: None,
            selected_row: None,
            reselect: None,
            previews: HashMap::new(),
            preview_pending: HashSet::new(),
            preview_tx,
            preview_rx,
            shell: None,
            show_gallery: false,
            last_shell_status: Default::default(),
            hidden_no_tray_since: None,
            quitting: false,
            leave_quick_after_open: false,
            quick_enter: false,
            window_visible: !starts_hidden(),
            keep_hidden_frames: if starts_hidden() { 4 } else { 0 },
            hidden_flag: Arc::new(AtomicBool::new(starts_hidden())),
            shot: Shot::from_args(),
            watcher: None,
            watch_batch: Vec::new(),
            watch_server_try: None,
            updater: Default::default(),
            update_seen: Default::default(),
            shot_view: None,
            logo: None,
            titlebar_dark: None,
            main_hwnd,
            force_foreground: None,
            quick: None,
            parked_shape: None,
            quick_close: false,
            query_edited: None,
            pending_search: None,
            pending_since: None,
            server_on_demand: false,
            last_activity: Instant::now(),
            update_available: None,
            update_rx: None,
            dl: Arc::new(Mutex::new(Default::default())),
            dl_cancel: Arc::new(AtomicBool::new(false)),
            dl_queue: Vec::new(),
            probe: Default::default(),
            file_checks: Default::default(),
            apply_plan_when_probed: false,
            download_when_probed: false,
            settings_view: SettingsView::Basic,
            health_cache: None,
            server_cfg_at_start: None,
            setup_open: false,
            setup: Default::default(),
            overlay_dismissed: false,
            overlay_last_state: RunState::Idle,
            overlay_tx,
            overlay_rx,
            overlay_busy_cache: Mutex::new((None, false)),
            lend: None,
            lend_last: None,
            lend_want_server: Arc::new(AtomicBool::new(false)),
            lend_overlay_dismissed: false,
            new_root: String::new(),
            notice: None,
            confirm_rebuild: false,
            data_move: None,
            last_title: String::new(),
            last_index_state: RunState::Idle,
            auto_index: false,
            auto_query: None,
            recover_attempts: 0,
            pending_recover: None,
            glass_blur: None,
            backdrop_checked: false,
            quick_acrylic: false,
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
            let ctx = egui_ctx.clone();
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
        // 트레이·전역 단축키 (화면 확인용 실행에서는 만들지 않는다: 쓰고 있는 앱의 단축키와 겹치지 않게)
        let shell = if app.shot.is_some() {
            Err(String::new())
        } else {
            Shell::start(egui_ctx.clone(), app.cfg.global_hotkey).map_err(|e| e.to_string())
        };
        match shell {
            Ok(s) => {
                let st = s.status();
                // 이 알림은 8초면 사라지고 첫 실행 마법사에 가려진다 → 상태 표시줄·설정 탭에도 계속 보여준다 (shell_warnings)
                if let Some(e) = st.tray_problem().or_else(|| st.hotkey_problem()) {
                    app.notify(e, true);
                }
                app.server.log(format!(
                    "[앱] 트레이 {} · 단축키 {} · 창 {}",
                    if st.tray_ok {
                        "등록".to_string()
                    } else {
                        format!("등록 실패 (오류 {})", st.tray_error)
                    },
                    match (st.hotkey_wanted, st.hotkey_ok) {
                        (false, _) => "끔".to_string(),
                        (true, true) => format!("{} 등록", crate::shell::HOTKEY_LABEL),
                        (true, false) => format!("등록 실패 (오류 {})", st.hotkey_error),
                    },
                    if app.window_visible { "보임" } else { "숨김(트레이로 시작)" }
                ));
                app.last_shell_status = st;
                app.shell = Some(s);
            }
            Err(e) if e.is_empty() => {}
            Err(e) => app.notify(format!("트레이 아이콘을 만들지 못했습니다: {e}"), true),
        }
        // 로그인 시 실행: exe를 옮겼으면 경로를 다시 맞춘다 (화면 확인용 실행은 레지스트리를 건드리지 않는다)
        if app.shot.is_none() {
            if let Err(e) = crate::autostart::sync(app.cfg.start_with_windows) {
                app.notify(format!("로그인 시 실행 설정 실패: {e}"), true);
            }
        }
        // 새 버전 확인 (하루 한 번)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if app.shot.is_none() && app.cfg.check_updates && now.saturating_sub(app.cfg.last_update_check) > 24 * 3600 {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let ctx = egui_ctx.clone();
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
        // gemma-search.exe lend …: 설정에 저장된 주소로 바로 빌려주기
        if LEND_ON_START.load(Ordering::SeqCst) {
            if let Err(e) = app.start_lending(egui_ctx) {
                app.notify(e, true);
            }
        }
        // 화면 확인용: 빌려주는 중인 것처럼 (오버레이·설정 화면)
        if flag("--lend-demo") {
            app.lend_last = Some(Arc::new(Mutex::new(crate::ui::lend::demo_status())));
        }
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
                app.open_setup(egui_ctx);
                // 트레이로 시작했어도 마법사는 보여야 한다
                if !app.window_visible && app.shot.is_none() {
                    egui_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    app.window_visible = true;
                    app.hidden_flag.store(false, Ordering::SeqCst);
                }
            }
        }
        // 화면 확인용: 긴 화면을 아래로 내린 채 고정(--scroll 픽셀), 시작하자마자 PC 사양 확인(--probe)
        if let Some(px) = value("--scroll").and_then(|v| v.parse::<u32>().ok()) {
            widgets::TEST_SCROLL.store(px, Ordering::Relaxed);
        }
        if flag("--probe") {
            let ffmpeg = app.cfg.ffmpeg().is_some();
            app.probe.start(ffmpeg, egui_ctx);
        }
        if flag("--after-update") {
            app.notify(format!("v{} 버전으로 업데이트했습니다.", env!("CARGO_PKG_VERSION")), false);
        }
        // 화면 확인용: "업데이트"를 누른 것처럼 시작한다
        if flag("--update-on-start") {
            app.start_update(egui_ctx);
        }
        // 화면 확인용: 빠른 검색창·처음 설정 화면을 창 모양을 바꾸지 않고 그린다 (크기는 --size로 맞춘다)
        if app.shot.is_some() {
            if flag("--quick") {
                app.quick = Some(QuickState {
                    prev_outer: None,
                    prev_inner: None,
                    prev_maximized: false,
                    was_visible: false,
                    got_focus: false,
                });
            }
            if flag("--setup") && !app.setup_open {
                app.open_setup(egui_ctx);
            }
            *widgets::TEST_OPEN.lock() = value("--open");
            // 새 버전이 있는 것처럼 ("업데이트 가능" 표시 확인)
            if let Some(tag) = value("--fake-update") {
                let page = format!("https://github.com/{}/releases", crate::update::REPO);
                app.update_available = Some((tag, page));
            }
            // 데이터 옮기기·가져오기 확인 창 (폴더를 고른 것처럼. 옮기지는 않는다)
            if let Some(dir) = value("--propose-import") {
                app.propose_move(crate::location::Job::Import(dir.into()));
            }
            if let Some(dir) = value("--propose-move-to") {
                app.propose_move(crate::location::Job::MoveTo(dir.into()));
            }
            app.shot_view = match value("--view").as_deref() {
                Some("docs") => Some(Section::Docs),
                Some("images") => Some(Section::Images),
                Some("videos") => Some(Section::Videos),
                Some("audio") => Some(Section::Audio),
                Some("names") => Some(Section::Names),
                _ => None,
            };
            // 화면 밖에서 잠깐 보이게 되더라도 포커스를 받지 않게 한다
            crate::platform::make_window_passive(app.main_hwnd);
        }
        // 화면 확인용: 시스템 테마와 상관없이 밝은/어두운 테마로 고정
        match value("--theme").as_deref() {
            Some("light") => egui_ctx.set_theme(egui::ThemePreference::Light),
            Some("dark") => egui_ctx.set_theme(egui::ThemePreference::Dark),
            _ => {}
        }
        if let Some(t) = value("--tab") {
            app.tab = match t.as_str() {
                "index" => Tab::Index,
                "server" => Tab::Server,
                "settings" | "settings-advanced" => Tab::Settings,
                _ => Tab::Search,
            };
            if t == "settings-advanced" {
                app.settings_view = SettingsView::Advanced;
            }
            // 화면 확인용: 부품 미리보기와 알림 예시
            if t == "gallery" {
                app.show_gallery = true;
                app.notify("설정을 저장했습니다. 알림은 이렇게 오른쪽 아래에 잠깐 뜹니다.", false);
            }
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
        // 설정에 외부 서버(embed_server)가 있으면 켜지 않고 그 서버에 연결한다
        if self.cfg.external_server().is_some() {
            match self.server.bring_up(&self.cfg) {
                Ok(()) => {
                    self.server_cfg_at_start = None;
                    self.notify("외부 서버에 연결했습니다", false);
                }
                Err(e) => {
                    self.server.log(format!("[앱] 외부 서버 연결 실패: {e}"));
                    self.notify(format!("외부 서버 연결 실패: {e}"), true);
                }
            }
            return;
        }
        match self.server.start(&self.cfg) {
            Ok(()) => {
                let actual = self.server.port();
                // 포트는 실제로 뜬 값으로 기억한다 (아래에서 설정도 그 값으로 맞춘다)
                let mut key = ServerKey::of(&self.cfg);
                key.port = actual;
                self.server_cfg_at_start = Some(key);
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

    /// 서버를 켠 뒤 서버 관련 설정이 바뀌어, 다시 시작해야 적용되는 상태인지
    pub fn server_restart_needed(&self) -> bool {
        self.server.state().is_running()
            && self
                .server_cfg_at_start
                .as_ref()
                .is_some_and(|k| *k != ServerKey::of(&self.cfg))
    }

    /// 서버를 껐다 켠다 (앱이 알아서 켠 서버였으면 그 성격을 유지)
    pub fn restart_server(&mut self) {
        let on_demand = self.server_on_demand;
        self.stop_server();
        if on_demand {
            self.start_server_auto();
        } else {
            self.start_server_manual();
        }
    }

    /// 변경분 인덱싱을 시작한다. 서버가 꺼져 있으면 켜고, 준비되는 대로 시작한다.
    pub fn start_index_when_ready(&mut self) {
        if self.server.state().is_ready() {
            self.start_index(false);
        } else {
            if !self.server.state().is_running() {
                self.start_server_auto();
            }
            self.auto_index = true;
        }
    }

    // ── 준비 상태 · 사양에 맞춘 추천 · 준비물 받기 (마법사와 설정이 같이 쓴다) ──

    /// 지금 바로 검색할 수 있는 상태인지 요약한다
    pub fn health(&mut self) -> Health {
        if let Some((at, h)) = &self.health_cache {
            if at.elapsed() < Duration::from_secs(2) {
                return h.clone();
            }
        }
        let c = &self.cfg;
        let is_file = |p: &str| std::path::Path::new(p.trim()).is_file();
        let root_exists: Vec<bool> = c.roots.iter().map(|r| std::path::Path::new(r.as_str()).is_dir()).collect();
        let h = Health {
            engine: crate::server::resolve_binary(&c.llama_path).is_ok(),
            model: is_file(&c.model_path),
            mmproj: c.has_mmproj().then(|| is_file(&c.mmproj_path)),
            folders: c.roots.len(),
            missing_folders: root_exists.iter().filter(|e| !**e).count(),
            root_exists,
            indexed_files: self.store.read().file_count(),
            ffmpeg: c.ffmpeg().is_some() && c.ffprobe().is_some(),
        };
        self.health_cache = Some((Instant::now(), h.clone()));
        h
    }

    /// 폴더·경로를 바꾼 직후처럼 준비 상태를 바로 다시 보게 한다
    pub fn invalidate_health(&mut self) {
        self.health_cache = None;
    }

    /// 지금 설정이 이 추천과 같은지
    pub fn plan_applied(&self, plan: &crate::sysinfo::Plan) -> bool {
        let c = &self.cfg;
        c.n_gpu_layers == plan.n_gpu_layers
            && c.ctx_size == plan.ctx_size
            && c.parallel == plan.parallel
            && c.batch_size == plan.batch_size
            && c.ubatch_size == plan.ubatch_size
            && c.workers == plan.workers
    }

    /// 사양에 맞춘 추천을 설정에 반영한다 (저장은 하지 않는다)
    pub fn apply_plan(&mut self, plan: &crate::sysinfo::Plan) {
        self.cfg.n_gpu_layers = plan.n_gpu_layers;
        self.cfg.ctx_size = plan.ctx_size;
        self.cfg.parallel = plan.parallel;
        self.cfg.batch_size = plan.batch_size;
        self.cfg.ubatch_size = plan.ubatch_size;
        self.cfg.workers = plan.workers;
    }

    /// 추천을 반영하고 그 값들만 바로 저장한다 (설정 탭의 다른 미저장 변경은 그대로 둔다)
    pub fn apply_plan_and_persist(&mut self, plan: &crate::sysinfo::Plan) {
        self.apply_plan(plan);
        let mut s = self.saved_cfg.clone();
        s.n_gpu_layers = plan.n_gpu_layers;
        s.ctx_size = plan.ctx_size;
        s.parallel = plan.parallel;
        s.batch_size = plan.batch_size;
        s.ubatch_size = plan.ubatch_size;
        s.workers = plan.workers;
        match s.save(&self.data_dir) {
            Ok(()) => self.saved_cfg = s,
            Err(e) => self.notify(format!("설정 저장 실패: {e}"), true),
        }
    }

    /// 빠진 준비물(모델, llama.cpp)을 차례로 받는다. llama.cpp는 `llama` 판으로.
    pub fn download_missing(&mut self, ctx: &egui::Context, llama: crate::download::Pack) {
        use crate::download::Pack;
        if self.dl.lock().running {
            return;
        }
        let mut todo = Vec::new();
        if !std::path::Path::new(self.cfg.model_path.trim()).is_file() {
            todo.push(Pack::Model);
        }
        if crate::server::resolve_binary(&self.cfg.llama_path).is_err() {
            todo.push(llama);
        }
        if todo.is_empty() {
            return;
        }
        let first = todo.remove(0);
        self.dl_queue = todo;
        crate::download::start(first, config::prep_dir(), self.dl.clone(), self.dl_cancel.clone(), waker(ctx));
    }

    /// 받기가 끝날 때마다 대기열의 다음 준비물을 받는다 (실패했으면 대기열을 비운다)
    fn next_download(&mut self, ctx: &egui::Context, last_ok: bool) {
        if !last_ok {
            self.dl_queue.clear();
            return;
        }
        if !self.dl_queue.is_empty() {
            let next = self.dl_queue.remove(0);
            crate::download::start(next, config::prep_dir(), self.dl.clone(), self.dl_cancel.clone(), waker(ctx));
        }
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

    pub(crate) fn show_window(&mut self, ctx: &egui::Context) {
        // 빠른 검색창 모양인 채로 숨겨 둔 창이면, 보이기 전에 큰 창 모양으로 되돌린다
        let mut maximize = false;
        if self.quick.is_none() {
            if let Some(q) = self.parked_shape.take() {
                Self::send_main_shape(ctx, &q);
                self.end_quick_backdrop(ctx);
                maximize = q.prev_maximized;
            }
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        if maximize {
            // 숨긴 창을 최대화하면 창이 먼저 나타나므로, 보인 다음에 한다
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
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

    /// 빠른 검색창 모양(테두리 없음·작게·항상 위)을 큰 창 모양으로 되돌리는 명령 (최대화는 따로)
    fn send_main_shape(ctx: &egui::Context, q: &QuickState) {
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        if let Some(r) = q.prev_inner {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(r.size()));
        }
        if let Some(r) = q.prev_outer {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(r.min));
        }
    }

    /// 빠른 검색창: 창을 테두리 없이 작게, 마우스가 있는 화면 위쪽 가운데에, 항상 위로
    pub fn enter_quick(&mut self, ctx: &egui::Context) {
        if self.quick.is_none() {
            let mut maximized = false;
            let state = match self.parked_shape.take() {
                // 빠른 검색창 모양인 채로 숨겨 둔 창: 그때 적어 둔 큰 창 모양을 이어받는다
                Some(q) => QuickState {
                    was_visible: false,
                    got_focus: false,
                    ..q
                },
                None => {
                    let (outer, inner, max) = ctx.input(|i| {
                        let v = i.viewport();
                        (v.outer_rect, v.inner_rect, v.maximized.unwrap_or(false))
                    });
                    maximized = max;
                    QuickState {
                        prev_outer: outer,
                        prev_inner: inner,
                        prev_maximized: max,
                        was_visible: self.window_visible,
                        got_focus: false,
                    }
                }
            };
            self.quick = Some(state);
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
        self.start_quick_backdrop(ctx);
        self.tab = Tab::Search;
        self.focus_search = true;
        self.select_query = true;
        self.show_window(ctx);
    }

    /// 빠른 검색창 모양일 때 OS의 Acrylic을 깐다 (Windows 11, 투명 효과가 켜져 있을 때).
    /// 테두리 없는 창이라 둥근 모서리와 강조색 테두리는 OS가 그린다. 화면 확인용 실행에서는 쓰지 않는다
    /// (Acrylic은 OS가 화면을 합칠 때 입히므로 찍히지 않는다).
    fn start_quick_backdrop(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering::SeqCst;
        let usable = crate::ui::glass::OS_BACKDROP.load(SeqCst)
            && self.shot.is_none()
            && crate::platform::transparency_effects_on();
        if usable == self.quick_acrylic {
            return;
        }
        if !usable {
            self.end_quick_backdrop(ctx);
            return;
        }
        let dark = ctx.theme() == egui::Theme::Dark;
        let p = crate::ui::theme::palette_of(dark);
        let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
        crate::platform::set_backdrop(self.main_hwnd, true, dark);
        // 뜰 때 단색이었다가 흐려지지 않게, 빠른 검색창 모양인 동안은 늘 활성으로 그린다
        crate::platform::keep_backdrop_active(self.main_hwnd, true);
        crate::platform::set_titlebar_colors(
            self.main_hwnd,
            dark,
            rgb(p.panel),
            rgb(widgets::mix(p.panel, p.accent, 0.75)),
        );
        self.quick_acrylic = true;
    }

    /// 큰 창 모양으로 돌아갈 때: OS 바탕을 걷고, 제목 표시줄·테두리 색은 다음 프레임에 큰 창 것으로 다시 맞춘다
    fn end_quick_backdrop(&mut self, ctx: &egui::Context) {
        if !self.quick_acrylic {
            return;
        }
        crate::platform::set_backdrop(self.main_hwnd, false, ctx.theme() == egui::Theme::Dark);
        crate::platform::keep_backdrop_active(self.main_hwnd, false);
        self.quick_acrylic = false;
        self.titlebar_dark = None;
    }

    /// 빠른 검색창을 닫는다. keep_open이면 원래 큰 창으로, 아니면 원래 상태(숨김이었으면 숨김)로.
    pub fn leave_quick(&mut self, ctx: &egui::Context, keep_open: bool) {
        let Some(q) = self.quick.take() else { return };
        self.quick_enter = false;
        self.quick_close = false;
        if keep_open || q.was_visible {
            Self::send_main_shape(ctx, &q);
            self.end_quick_backdrop(ctx);
            if q.prev_maximized {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
            self.show_window(ctx);
        } else {
            // 숨길 때는 창 모양을 건드리지 않고 숨기기만 한다. Windows는 창을 숨길 때 잠깐 서서히 사라지게
            // 하는데, 그 사이에(앞이든 뒤든) 큰 창 모양으로 되돌리면 큰 창이 번쩍 보였다 사라진다.
            // 되돌리기는 다음에 큰 창을 보일 때 한다 (`show_window`). 다시 빠른 검색창으로 열면 그대로 쓴다.
            self.parked_shape = Some(q);
            self.hide_window(ctx);
        }
    }

    /// logic 쪽(단축키, 포커스 잃음)에서 빠른 검색창을 닫을 때. 숨기게 되는 경우에는 ui가 이번 프레임을
    /// 평소대로 그린 뒤 닫는다 (그리지 않고 숨기면 빈 창이 사라지는 모습이 보인다).
    /// 다음에 불릴 때까지 ui가 처리하지 못했으면 바로 닫는다.
    fn request_quick_close(&mut self, ctx: &egui::Context) {
        let to_tray = self.quick.as_ref().is_some_and(|q| !q.was_visible);
        if to_tray && !self.quick_close {
            self.quick_close = true;
            ctx.request_repaint();
        } else {
            self.leave_quick(ctx, false);
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
                        self.request_quick_close(ctx);
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
                    } else {
                        self.start_index_when_ready();
                    }
                }
                ShellEvent::ToggleServer => {
                    if self.server.state().is_running() {
                        self.stop_server();
                    } else {
                        self.start_server_manual();
                    }
                }
                ShellEvent::ToggleLend => self.toggle_lending(ctx),
                ShellEvent::StartLend => self.start_lending_from_command(ctx),
                ShellEvent::ShowLendOverlay => self.lend_overlay_dismissed = false,
                ShellEvent::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                ShellEvent::Status => self.on_shell_status(),
            }
        }
    }

    /// 트레이 아이콘·단축키 등록 상태가 바뀜 (셸 스레드가 실패한 등록을 계속 다시 시도한다)
    fn on_shell_status(&mut self) {
        let Some(st) = self.shell.as_ref().map(|s| s.status()) else { return };
        let old = std::mem::replace(&mut self.last_shell_status, st);
        if st.tray_ok != old.tray_ok {
            self.server.log(if st.tray_ok {
                "[앱] 트레이 아이콘 등록".to_string()
            } else {
                format!("[앱] 트레이 아이콘 등록 실패 (오류 {})", st.tray_error)
            });
        }
        if st.hotkey_ok != old.hotkey_ok {
            if st.hotkey_ok {
                self.server.log(format!("[앱] 단축키 {} 등록", crate::shell::HOTKEY_LABEL));
                self.notify(
                    format!("이제 {}로 어디서든 검색할 수 있습니다", crate::shell::HOTKEY_LABEL),
                    false,
                );
            } else {
                self.server.log(format!("[앱] 단축키 등록 실패 (오류 {})", st.hotkey_error));
            }
        }
    }

    /// 트레이 아이콘이 실제로 있는지 (없으면 창을 숨겨서는 안 된다: 다시 꺼낼 방법도 끝낼 메뉴도 없다)
    pub fn tray_available(&self) -> bool {
        self.shell.as_ref().is_some_and(|s| s.status().tray_ok)
    }

    /// 전역 단축키가 실제로 등록돼 있는지
    pub fn hotkey_available(&self) -> bool {
        self.shell.as_ref().is_some_and(|s| s.hotkey_registered())
    }

    /// 상주 기능(트레이·단축키) 중 지금 안 되는 것. 상태 표시줄과 설정 탭에 계속 보여준다.
    pub fn shell_warnings(&self) -> Vec<String> {
        // 화면 확인용 실행은 일부러 트레이를 만들지 않는다
        if self.shot.is_some() {
            return Vec::new();
        }
        let Some(shell) = &self.shell else {
            return vec!["트레이·전역 단축키를 시작하지 못했습니다. 창을 닫으면 앱이 끝납니다.".to_string()];
        };
        let st = shell.status();
        let mut out = Vec::new();
        if let Some(mut m) = st.tray_problem() {
            if self.cfg.close_to_tray {
                m.push_str(" 트레이 아이콘이 없는 동안은 창을 닫으면 숨기지 않고 앱을 끝냅니다.");
            }
            out.push(m);
        }
        if let Some(m) = st.hotkey_problem() {
            out.push(m);
        }
        out
    }

    /// 트레이 아이콘 없이 숨어 있으면(트레이로 시작했는데 등록이 안 된 경우) 꺼낼 방법이 없다 → 창을 띄운다.
    /// 로그인 직후에는 작업 표시줄이 늦게 뜰 수 있어 잠시 기다린다 (그동안 셸 스레드가 다시 시도한다).
    fn rescue_hidden_window(&mut self, ctx: &egui::Context) {
        if self.window_visible || self.quick.is_some() || self.tray_available() || self.shot.is_some() {
            self.hidden_no_tray_since = None;
            return;
        }
        let since = *self.hidden_no_tray_since.get_or_insert_with(Instant::now);
        if since.elapsed() > Duration::from_secs(20) {
            self.hidden_no_tray_since = None;
            self.show_window(ctx);
            self.notify(
                "트레이 아이콘을 만들 수 없어 창을 띄웠습니다. 설정 탭의 '상주 · 자동 실행'에서 이유를 볼 수 있습니다.",
                true,
            );
        }
    }

    /// 창 닫기: 트레이 아이콘이 있으면 끝내지 않고 숨긴다 (트레이 메뉴의 "종료"로 끝냄)
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.quitting || !self.cfg.close_to_tray || self.shell.is_none() {
            return;
        }
        // 트레이 아이콘을 못 만들었으면 숨기지 않는다: 숨기면 창을 다시 꺼낼 수도, 앱을 끝낼 수도 없게 된다
        if !self.tray_available() {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if let Some(q) = &mut self.quick {
            // 닫기는 숨기는 것이므로, 빠른 검색창을 열기 전에 큰 창이 보였더라도 다시 띄우지 않는다
            q.was_visible = false;
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
        if self.shell.is_none() {
            return;
        }
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
            _ => match self.lend_tray_tip() {
                // 빌려주는 중이면 그것을 알린다
                Some(tip) => (TrayIcon::Busy, None, tip),
                None => (
                    TrayIcon::Idle,
                    None,
                    format!(
                        "Gemma Search · 파일 {}개 · 서버 {}",
                        fmt_num(files as u64),
                        if server.is_ready() { "준비됨" } else if server.is_running() { "시작 중" } else { "꺼짐" }
                    ),
                ),
            },
        };
        let lending = self.lend.as_ref().map(|s| s.is_paused());
        let Some(shell) = &self.shell else { return };
        shell.update(tip, icon, indexing, server.is_running(), lending, self.lend_overlay_dismissed);
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
                crate::platform::bring_to_front(self.main_hwnd);
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
                    self.request_quick_close(ctx);
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
        if let Err(why) = self.start_index_run(full_rebuild, None) {
            self.notify(why, true);
            return;
        }
        if full_rebuild {
            self.thumbs.clear();
            self.outcome = None;
        }
        self.tab = Tab::Index;
    }

    /// 인덱싱을 시작한다. `only`가 있으면 그 경로만 처리한다 (폴더 감시). 시작할 수 없으면 이유를 돌려준다.
    fn start_index_run(&mut self, full_rebuild: bool, only: Option<Vec<String>>) -> Result<(), String> {
        if let Some(why) = self.index_blocker() {
            return Err(why);
        }
        let Some(caps) = self.server.capabilities() else {
            return Err("서버 자가진단이 끝나기를 기다리는 중".to_string());
        };
        let params = IndexParams {
            cfg: self.cfg.clone(),
            caps,
            target: self.server.target(),
            server_sig: self.server.signature(&self.cfg),
            db_path: self.db_path.clone(),
            data_dir: self.data_dir.clone(),
            store: self.store.clone(),
            full_rebuild,
            only,
            lend: None,
        };
        self.indexer.start(params).map_err(|e| format!("인덱싱 시작 실패: {e}"))
    }

    /// 폴더 감시: 설정에 맞춰 켜고 끄고, 바뀐 파일이 조용해지면 그 파일만 인덱싱한다.
    /// 숨겨져 있을 때도 돈다 (`logic` → `poll`).
    fn watch_tick(&mut self, ctx: &egui::Context) {
        use crate::watch::{WatchKey, Watcher};
        // 저장된 설정을 따른다 (설정 화면에서 고치는 중인 값은 저장한 뒤에 적용)
        let c = &self.saved_cfg;
        if !c.watch_folders || !c.setup_done || c.roots.is_empty() {
            if self.watcher.take().is_some() {
                self.watch_batch.clear();
                self.server.log("[감시] 폴더 감시를 껐습니다");
            }
            return;
        }
        let key = WatchKey::new(&c.roots, &c.exclude_dirs, c.watch_rescan_hours);
        if self.watcher.as_ref().is_none_or(|w| w.key() != &key) {
            let w = Watcher::start(key, &self.data_dir);
            let snap = w.snapshot();
            let network = snap.roots.iter().filter(|r| r.network).count();
            self.server.log(format!(
                "[감시] 폴더 {}개 감시 시작 (네트워크 {network}개)",
                snap.roots.len()
            ));
            self.watcher = Some(w);
            self.watch_batch.clear();
        }
        // 창이 그려지지 않는 동안에도 가끔 깨어나 확인한다
        ctx.request_repaint_after(Duration::from_secs(2));
        // 다른 인덱싱이 돌거나 준비가 안 됐으면 모아 두기만 한다
        if self.indexer.is_active()
            || self.auto_index
            || self.setup_open
            || self.pending_recover.is_some()
            || self.store_loading.load(Ordering::SeqCst)
        {
            return;
        }
        // 전체 화면 앱(게임·영상·발표)을 쓰는 동안은 그래픽카드를 쓰지 않게 미룬다
        if self.overlay_user_busy() {
            return;
        }
        let Some(w) = &self.watcher else { return };
        if w.scan_due() {
            w.start_scan(self.db_path.clone(), waker(ctx));
        }
        if self.watch_batch.is_empty() {
            let ready = w.take_ready();
            if !ready.is_empty() {
                self.server
                    .log(format!("[감시] 바뀐 경로 {}개를 반영합니다", ready.len()));
                self.watch_batch = ready;
            }
        }
        if self.watch_batch.is_empty() {
            return;
        }
        let state = self.server.state();
        if state.is_ready() {
            if self.server.capabilities().is_none() {
                return; // 자가진단이 끝나면 다음에
            }
            let batch = std::mem::take(&mut self.watch_batch);
            if let Err(why) = self.start_index_run(false, Some(batch.clone())) {
                self.server.log(format!("[감시] 시작하지 못해 다음에 다시 합니다: {why}"));
                if let Some(w) = &self.watcher {
                    w.requeue(batch);
                }
            }
        } else if !state.is_running() && self.watch_server_try.is_none_or(|t| t.elapsed() > Duration::from_secs(300)) {
            // 서버가 꺼져 있으면 켠다. 이렇게 켠 서버는 한동안 쓰지 않으면 다시 꺼진다 (설정의 자동 끄기).
            self.watch_server_try = Some(Instant::now());
            self.start_server_auto();
        }
    }

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
        let target = self.server.target();
        let store = self.store.clone();
        let db = self.db.clone();
        let top_k = self.cfg.top_k.max(1);
        let ff = self.cfg.ffmpeg();
        let min_score = self.cfg.min_relevance;
        std::thread::spawn(move || {
            let o = query::run(&target, &store, &db, q, filter, top_k, min_score, ff);
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
                // 같은 검색어로 검색하는 동안 사용자가 고른 줄은 지킨다. 한글 입력은 ↓를 누를 때 조합이 확정되며
                // 같은 검색이 한 번 더 돌고, 입력하는 대로 찾을 때는 고르는 사이에 결과가 바뀐다.
                // 검색어가 달라졌으면 선택을 푼다.
                let same_query = matches!(&self.last_query, Some(QueryKind::Text(q))
                    if self.nav_query.as_deref() == Some(q.trim()));
                if same_query && self.selected.is_some() {
                    self.reselect = Some(self.selected_row.unwrap_or(0));
                } else {
                    self.selected = None;
                    self.reselect = None;
                    // 예전 검색어로 고른 기록은 버린다 (나중에 같은 검색어를 다시 쳤을 때 엉뚱한 줄을 지키지 않게)
                    self.nav_query = None;
                }
                self.outcome = Some(o);
                self.search_rx = None;
                self.search_started = None;
                if let Some(v) = self.shot_view.take() {
                    self.result_view = Some(v);
                }
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
            // "필요한 것 모두 받기": 다음 준비물
            self.next_download(ctx, ok);
            self.invalidate_health();
        }
        // 사양 확인이 끝났으면 기다리던 동작(추천값 적용, 준비물 받기)을 이어서
        if self.probe.poll() {
            if let Some(plan) = self.probe.plan().cloned() {
                if self.apply_plan_when_probed {
                    self.apply_plan_and_persist(&plan);
                    self.notify("이 PC에 맞게 설정했습니다", false);
                }
                if self.download_when_probed {
                    self.download_missing(ctx, plan.llama);
                }
            }
            self.apply_plan_when_probed = false;
            self.download_when_probed = false;
        }

        // 트레이·단축키, 기다리던 검색, 유휴 서버, 빠른 검색창
        self.handle_shell(ctx);
        self.lend_logic();
        self.run_pending_search();
        self.idle_stop();
        self.update_tray();
        self.quick_tick(ctx);
        if let Some(rx) = &self.update_rx {
            if let Ok(r) = rx.try_recv() {
                if let Some((tag, _)) = &r {
                    self.notify(
                        format!("새 버전({tag})이 나왔습니다. 왼쪽 메뉴 아래의 '업데이트 가능'을 누르면 받아서 바꿉니다."),
                        false,
                    );
                }
                self.update_available = r;
                self.update_rx = None;
            }
        }
        self.poll_update(ctx);
        self.watch_tick(ctx);

        let (state, title_prefix, done_msg, watch_log) = {
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
            // 폴더 감시가 돌린 실행은 알림을 띄우지 않고 로그에만 남긴다 (파일이 바뀔 때마다 뜨지 않게)
            let watch_log = (p.targeted && p.state == RunState::Completed).then(|| {
                format!(
                    "[감시] 반영 완료: 처리 {} · 변경 없음 {} · 옮김 {} · 삭제 {} · 오류 {}",
                    fmt_num(p.processed.saturating_sub(p.unchanged + p.moved)),
                    fmt_num(p.unchanged),
                    fmt_num(p.moved),
                    fmt_num(p.removed),
                    fmt_num(p.errors)
                )
            });
            let msg = match p.state {
                RunState::Completed if p.targeted => None,
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
            (p.state, prefix, msg, watch_log)
        };
        if state != self.last_index_state {
            if let Some((m, err)) = done_msg {
                self.notify(m, err);
            }
            if let Some(line) = watch_log {
                self.server.log(line);
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

    /// 앱 아이콘 (메뉴 맨 위에 그린다)
    fn logo(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        if let Some(t) = &self.logo {
            return t.clone();
        }
        // 그려질 크기(28pt)의 화면 픽셀 수에 맞춰 줄여 둔다 (큰 그림을 그대로 줄여 그리면 거칠다)
        let px = (28.0 * ctx.pixels_per_point()).round().max(28.0) as u32;
        let image = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .map(|i| i.resize_exact(px, px, image::imageops::FilterType::Lanczos3).to_rgba8())
            .map(|i| egui::ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], i.as_raw()))
            .unwrap_or_else(|_| egui::ColorImage::filled([1, 1], egui::Color32::TRANSPARENT));
        let t = ctx.load_texture("logo", image, egui::TextureOptions::LINEAR);
        self.logo = Some(t.clone());
        t
    }

    /// 왼쪽 메뉴: 앱 이름, 화면 전환, 맨 아래에 서버·인덱스 상태. `compact`면 아이콘만 (창이 좁을 때).
    fn nav_panel(&mut self, ui: &mut egui::Ui, compact: bool) {
        use crate::ui::theme;
        use widgets::Icon;
        // 창에서 떨어져 떠 있는 진한 유리 기둥 (틀의 안쪽 여백까지)
        crate::ui::glass::backdrop(ui, crate::ui::glass::Glass::Thick, 22.0, NAV_PAD);

        ui.spacing_mut().item_spacing.y = 4.0;
        let logo = self.logo(ui.ctx());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 9.0;
            ui.add_space(if compact { ((ui.available_width() - 28.0) / 2.0).max(0.0) } else { 6.0 });
            ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::Vec2::splat(28.0)));
            if !compact {
                ui.label(RichText::new("Gemma Search").text_style(theme::ts_item()).strong());
            }
        });
        ui.add_space(14.0);
        // 저장하지 않은 설정이 있으면 설정 메뉴에 점을 붙인다
        let unsaved = self.cfg != self.saved_cfg;
        for (tab, icon, label, mark) in [
            (Tab::Search, Icon::Search, "검색", false),
            (Tab::Index, Icon::Database, "인덱스", false),
            (Tab::Server, Icon::Server, "서버", false),
            (Tab::Settings, Icon::Settings, "설정", unsaved),
        ] {
            let selected = self.tab == tab && !self.show_gallery;
            let mut r = widgets::nav_item(ui, selected, icon, label, compact, mark);
            if mark {
                r = r.on_hover_text("저장하지 않은 변경이 있습니다");
            }
            if r.clicked() {
                self.tab = tab;
            }
        }
        // 맨 아래: 지금 상태
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| self.nav_status(ui, compact));
    }

    /// 메뉴 맨 아래의 상태: 서버와 파일 수, (진행 중일 때만) 인덱싱, 계속 알아야 하는 경고.
    /// 아래에서 위로 쌓는 배치라 넣는 순서가 화면과 거꾸로다. 알림은 토스트로 따로 뜬다.
    fn nav_status(&mut self, ui: &mut egui::Ui, compact: bool) {
        use crate::ui::theme::{self, palette, Tone};
        use widgets::Icon;
        let p = palette(ui.visuals());
        let ss = self.server.state();
        let server_text = format!("서버 {}", ss.label());
        let prog = self.indexer.progress.lock().clone();
        let (files, chunks, loading) = {
            let st = self.store.read();
            (st.file_count(), st.live_chunks(), self.store_loading.load(Ordering::SeqCst))
        };
        let files_text = if loading {
            let s = self.store_status.lock().clone();
            if s.is_empty() {
                "인덱스 불러오는 중…".to_string()
            } else {
                s
            }
        } else {
            format!("파일 {}개", fmt_num(files as u64))
        };
        let index_text = prog.state.is_active().then(|| {
            let pct = prog
                .fraction()
                .map(|f| format!("{:.0}%", f * 100.0))
                .unwrap_or_else(|| "탐색 중".into());
            format!("{} {pct}", prog.state.label())
        });
        let warnings = self.shell_warnings();
        let (mut go_server, mut go_index) = (false, false);
        let semibold = || egui::FontFamily::Name(theme::SEMIBOLD.into());

        if compact {
            // 아이콘만 있는 메뉴: 점과 아이콘으로만 알리고, 자세한 내용은 올렸을 때
            ui.spacing_mut().item_spacing.y = 10.0;
            let centered = |ui: &mut egui::Ui, w: f32, add: &mut dyn FnMut(&mut egui::Ui)| {
                ui.horizontal(|ui| {
                    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                    add(ui);
                })
                .response
            };
            if !warnings.is_empty() {
                centered(ui, 16.0, &mut |ui| {
                    let fg = Tone::Warn.fg(ui.visuals());
                    widgets::icon(ui, Icon::TriangleAlert, 16.0, fg);
                })
                .on_hover_text(warnings.join("\n\n"));
            }
            self.nav_update(ui, true);
            let r = centered(ui, 14.0, &mut |ui| widgets::dot(ui, widgets::server_color(&ss)));
            if r
                .interact(egui::Sense::click())
                .on_hover_text(format!("{server_text}\n{files_text}"))
                .clicked()
            {
                go_server = true;
            }
            if let Some(t) = &index_text {
                let r = centered(ui, 14.0, &mut |ui| widgets::dot(ui, widgets::run_color(prog.state)));
                if r.interact(egui::Sense::click()).on_hover_text(t).clicked() {
                    go_index = true;
                }
            }
        } else {
            ui.spacing_mut().item_spacing.y = 8.0;
            let small = |s: &str| RichText::new(s).small();
            if !warnings.is_empty() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0;
                    ui.add_space(2.0);
                    let fg = Tone::Warn.fg(ui.visuals());
                    widgets::icon(ui, Icon::TriangleAlert, 14.0, fg);
                    ui.label(small("상주 기능 제한").color(fg));
                })
                .response
                .on_hover_text(warnings.join("\n\n"));
            }
            self.nav_update(ui, false);
            // 메뉴 아래의 상태 카드: 움푹 들어간 홈
            let card = || {
                egui::Frame::new()
                    .fill(p.field_fill)
                    .stroke(egui::Stroke::new(1.0, p.well_stroke))
                    .corner_radius(15)
                    .inner_margin(egui::Margin::symmetric(12, 9))
            };
            // 서버 · 파일 수 (아래 줄부터 넣는다)
            let r = card()
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_space(2.0);
                        if loading {
                            ui.add(egui::Spinner::new().size(12.0));
                        }
                        ui.label(small(&files_text).weak());
                    });
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        widgets::dot(ui, widgets::server_color(&ss));
                        ui.label(RichText::new(&server_text).size(13.0).family(semibold()));
                    });
                })
                .response
                .interact(egui::Sense::click());
            let hover = if ss.is_ready() {
                format!(
                    "{} · 벡터 {}개\n누르면 서버 화면을 엽니다",
                    self.server.place(),
                    fmt_num(chunks as u64)
                )
            } else {
                "누르면 서버 화면을 엽니다".to_string()
            };
            if r.on_hover_text(hover).clicked() {
                go_server = true;
            }
            // 인덱싱 (진행 중일 때만)
            if let Some(t) = &index_text {
                let r = card()
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 5.0;
                        let frac = prog.fraction().unwrap_or(0.0);
                        if prog.state == RunState::Running {
                            widgets::progress_accent(ui, frac, 4.0, true);
                        } else {
                            widgets::progress(ui, frac, widgets::run_color(prog.state), 4.0, false);
                        }
                        ui.label(
                            small(&format!("{} / {}", fmt_num(prog.processed), fmt_num(prog.discovered))).weak(),
                        );
                        ui.label(RichText::new(t).size(13.0).family(semibold()));
                    })
                    .response
                    .interact(egui::Sense::click());
                if r.on_hover_text("누르면 인덱스 화면을 엽니다").clicked() {
                    go_index = true;
                }
            }
        }
        if go_server {
            self.tab = Tab::Server;
        }
        if go_index {
            self.tab = Tab::Index;
        }
    }

    /// 업데이트를 시작한다: 새 버전이 있는지 확인하고, 있으면 받아서 지금 exe와 바꾼다 (`update.rs`).
    /// 바꾼 뒤에는 `poll_update`가 다시 시작한다.
    pub fn start_update(&mut self, ctx: &egui::Context) {
        crate::update::start(self.updater.clone(), waker(ctx));
    }

    /// 새 exe로 다시 시작한다 (창을 숨겨 둔 상태였으면 다시 숨긴 채로)
    pub fn restart_for_update(&mut self, ctx: &egui::Context) {
        match crate::update::spawn_restart(!self.window_visible) {
            Ok(()) => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.notify(format!("다시 시작하지 못했습니다. 앱을 껐다 켜 주세요: {e}"), true),
        }
    }

    /// 데이터를 옮기려고 다시 시작한다: 새 프로세스가 이 프로세스가 끝나기를 기다렸다가 옮기고 앱을 연다
    /// (`--move-data`, main.rs와 `ui::launch`). 인덱스가 열려 있는 동안에는 폴더를 옮길 수 없다.
    pub fn restart_to_move(&mut self, ctx: &egui::Context, job: &crate::location::Job) {
        let [kind, path] = job.to_args();
        let spawned = std::process::Command::new(crate::update::exe_path())
            .arg("--move-data")
            .arg(kind)
            .arg(path)
            .arg(std::process::id().to_string())
            .spawn();
        match spawned {
            Ok(_) => {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self.notify(format!("다시 시작하지 못해 옮기지 않았습니다: {e}"), true),
        }
    }

    /// 업데이트 단계가 바뀌었을 때 한 번씩: 결과를 알리고, 바꾸기가 끝났으면 다시 시작한다
    fn poll_update(&mut self, ctx: &egui::Context) {
        use crate::update::Stage;
        let stage = self.updater.lock().clone();
        if stage == self.update_seen {
            return;
        }
        match &stage {
            Stage::UpToDate => {
                self.update_available = None;
                self.notify(stage.text(), false);
            }
            Stage::Ready { tag } => {
                self.update_available = None;
                if self.indexer.is_active() {
                    // 인덱싱을 끊지 않는다: 끝난 뒤 "다시 시작"을 누르게 한다
                    self.notify(
                        format!("{tag} 버전으로 바꿨습니다. 인덱싱이 끝난 뒤 '다시 시작'을 누르면 적용됩니다."),
                        false,
                    );
                } else if self.shot.is_none() {
                    self.restart_for_update(ctx);
                }
            }
            Stage::Failed { message, .. } => self.notify(format!("업데이트하지 못했습니다: {message}"), true),
            _ => {}
        }
        self.update_seen = stage;
    }

    /// 메뉴 아래의 업데이트 표시: 새 버전이 있으면 "업데이트 가능"(누르면 받아서 바꾼다),
    /// 진행 중이면 진행 상황, 바꾼 뒤 다시 시작을 기다리는 중이면 "다시 시작".
    fn nav_update(&mut self, ui: &mut egui::Ui, compact: bool) {
        use crate::ui::theme::{self, palette};
        use crate::update::Stage;
        use widgets::{Btn, Icon};
        let stage = self.updater.lock().clone();
        let ctx = ui.ctx().clone();
        let p = palette(ui.visuals());
        // 아이콘만 있는 메뉴에서는 가운데에 놓는다
        let centered = |ui: &mut egui::Ui, w: f32, add: &mut dyn FnMut(&mut egui::Ui) -> egui::Response| {
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                add(ui)
            })
            .inner
        };
        // 누르는 카드 한 장: 아이콘 + 제목, 그 아래 작은 설명 (아래에서 위로 쌓는 배치라 아래 줄부터 넣는다)
        let card = |ui: &mut egui::Ui, glyph: Icon, title: &str, sub: &str| {
            egui::Frame::new()
                .fill(theme::alpha(p.accent, 0.14))
                .stroke(egui::Stroke::new(1.0, theme::alpha(p.accent, 0.40)))
                .corner_radius(15)
                .inner_margin(egui::Margin::symmetric(12, 9))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        ui.add_space(2.0);
                        ui.label(RichText::new(sub).small().weak());
                    });
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_space(2.0);
                        widgets::icon(ui, glyph, 14.0, p.accent_text);
                        ui.label(
                            RichText::new(title)
                                .size(13.0)
                                .family(egui::FontFamily::Name(theme::SEMIBOLD.into()))
                                .color(p.accent_text),
                        );
                    });
                })
                .response
                .interact(egui::Sense::click())
        };
        match &stage {
            s if s.busy() => {
                if compact {
                    centered(ui, 16.0, &mut |ui| widgets::spinner(ui)).on_hover_text(stage.text());
                } else {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_space(2.0);
                        widgets::spinner(ui);
                        ui.label(RichText::new(stage.text()).small());
                    });
                }
            }
            Stage::Ready { tag } => {
                let r = if compact {
                    centered(ui, theme::CONTROL_H, &mut |ui| widgets::icon_button(ui, Btn::Primary, Icon::RotateCw, ""))
                } else {
                    card(ui, Icon::RotateCw, "다시 시작", &format!("{tag} 설치됨 · 누르면 적용"))
                };
                if r.on_hover_text(stage.text()).clicked() {
                    self.restart_for_update(&ctx);
                }
            }
            _ => {
                let Some((tag, _)) = self.update_available.clone() else { return };
                let hover = format!("새 버전({tag})이 있습니다. 누르면 받아서 지금 프로그램과 바꾸고 다시 시작합니다.");
                let r = if compact {
                    centered(ui, theme::CONTROL_H, &mut |ui| widgets::icon_button(ui, Btn::Secondary, Icon::Download, ""))
                } else {
                    card(ui, Icon::Download, "업데이트 가능", &format!("{tag} · 누르면 설치"))
                };
                if r.on_hover_text(hover).clicked() {
                    self.start_update(&ctx);
                }
            }
        }
    }

    /// Windows 11: 제목 표시줄 색을 앱 바탕에 맞춘다 (테마가 바뀌면 다시). Windows 10에서는 아무 일도 없다.
    fn sync_titlebar(&mut self, dark: bool) {
        if self.titlebar_dark == Some(dark) || self.main_hwnd == 0 {
            return;
        }
        self.titlebar_dark = Some(dark);
        // 제목 표시줄은 벽의 바닥색으로 (벽의 위쪽 가운데는 번짐이 닿지 않아 바닥색 그대로다)
        let p = crate::ui::theme::palette_of(dark);
        let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
        let border = widgets::mix(p.panel, p.text, 0.12);
        crate::platform::set_titlebar_colors(self.main_hwnd, dark, rgb(p.panel), rgb(border));
    }

    /// 알림을 오른쪽 아래 토스트로 보여준다 (오류는 15초, 안내는 6초 또는 닫을 때까지)
    fn show_toast(&mut self, ctx: &egui::Context) {
        // 화면 확인용 실행: "서버를 시작합니다" 같은 안내가 화면을 가리지 않게 한다 (오류와 부품 미리보기의 예시는 보인다)
        if self.shot.is_some() && !self.show_gallery && self.notice.as_ref().is_some_and(|n| !n.error) {
            return;
        }
        let Some((text, error, age)) = self.notice.as_ref().map(|n| (n.text.clone(), n.error, n.at.elapsed())) else {
            return;
        };
        let ttl = Duration::from_secs(if error { 15 } else { 6 });
        if age > ttl || widgets::toast(ctx, &text, error) {
            self.notice = None;
        }
    }

    /// 화면 확인용 실행: 숨긴 창을 계속 그리게 하다가, 화면이 자리 잡으면 찍어서 저장하고 끝낸다
    fn shot_tick(&mut self, ctx: &egui::Context) {
        if self.shot.is_none() {
            return;
        }
        let indexing = {
            let p = self.indexer.progress.lock();
            p.state == RunState::Running && !p.scanning && p.processed >= 8
        };
        let mut busy = self.updater.lock().busy()
            || self.search_rx.is_some()
            || self.auto_query.is_some()
            || self.pending_search.is_some()
            || self.store_loading.load(Ordering::SeqCst)
            || !self.preview_pending.is_empty();
        if let Some(s) = &mut self.shot {
            if s.when_indexing {
                // 인덱싱이 어느 정도 돈 순간에 바로 찍는다
                busy = !indexing;
                s.wait = Duration::ZERO;
            }
            if s.tick(ctx, busy) {
                self.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
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
                    if widgets::button(ui, widgets::Btn::Danger, "재구축 시작").clicked() {
                        self.confirm_rebuild = false;
                        self.recover_attempts = 0;
                        self.start_index(true);
                    }
                    if widgets::button(ui, widgets::Btn::Secondary, "취소").clicked() {
                        self.confirm_rebuild = false;
                    }
                });
            });
    }
}

impl eframe::App for GemmaApp {
    /// 창을 지우는 색. OS 바탕(Acrylic)을 쓸 수 있는 투명 창이면 완전히 투명하게 (빠른 검색창에서 창 뒤가 비치게).
    /// 큰 창은 그 위에 늘 벽을 불투명하게 칠한다. 그 밖에는 eframe 기본값 그대로.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        if crate::ui::glass::OS_BACKDROP.load(Ordering::SeqCst) {
            [0.0; 4]
        } else {
            egui::Color32::from_rgba_unmultiplied(12, 12, 12, 180).to_normalized_gamma_f32()
        }
    }

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
        self.rescue_hidden_window(ctx);
        self.shot_tick(ctx);
        self.overlay_logic(ctx);
        // 트레이로 숨겨져 있어도 인덱싱 오버레이는 그려야 한다 → 이번 프레임은 ui를 부르게
        if !self.window_visible && self.overlay_should_show(ctx) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // 펼침 메뉴의 흐림 엔진은 첫 프레임에 한 번 만든다 (데이터를 옮긴 뒤 여는 경우에도 같은 길)
        if self.glass_blur.is_none() {
            self.glass_blur = frame.wgpu_render_state().map(crate::ui::backdrop::Blur::new);
        }
        // OS 바탕: 실제로 DX12로 그릴 때만 투명하게 내보낼 수 있다. 아니면(Vulkan으로 돌아갔으면) 끈다.
        if !self.backdrop_checked {
            self.backdrop_checked = true;
            if crate::ui::glass::OS_BACKDROP.load(Ordering::SeqCst) {
                let dx12 = frame
                    .wgpu_render_state()
                    .is_some_and(|rs| rs.adapter.get_info().backend == eframe::wgpu::Backend::Dx12);
                if dx12 {
                    crate::platform::prepare_backdrop_window(self.main_hwnd);
                } else {
                    crate::ui::glass::OS_BACKDROP.store(false, Ordering::SeqCst);
                    let ctx = ui.ctx().clone();
                    self.end_quick_backdrop(&ctx);
                }
            }
        }
        self.draw(ui);
        // 펼침 메뉴의 흐림은 이번 프레임의 그림이 다 모인 뒤에 (메뉴 밑에 깔릴 층을 다시 그려 흐린다)
        if let (Some(blur), Some(rs)) = (&mut self.glass_blur, frame.wgpu_render_state()) {
            blur.run(ui.ctx(), rs);
        }
    }
}

impl GemmaApp {
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.index_overlay(&ctx);
        // 숨긴 상태에서 오버레이 때문에 불렸으면 본 창은 그리지 않는다 (화면 확인용 실행은 숨긴 채로 그린다)
        if !self.window_visible && self.quick.is_none() && self.shot.is_none() {
            return;
        }
        if self.quick.is_some() {
            let to_tray = self.quick.as_ref().is_some_and(|q| !q.was_visible);
            // Esc: 입력칸에 포커스가 있든 없든 바로 닫는다 (입력칸이 Esc를 먹어 포커스만 잃던 문제)
            let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            let close = esc || std::mem::take(&mut self.quick_close);
            if close && to_tray {
                // 숨기는 경우: 이번 프레임도 빠른 검색창을 그대로 그린 뒤 숨긴다 (큰 창 화면이나 빈 화면을 남기지 않는다)
                self.quick_ui(ui);
                self.leave_quick(&ctx, false);
                return;
            } else if close {
                // 큰 창으로 돌아가는 경우: 아래에서 큰 창을 그린다
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
        use crate::ui::theme;
        if self.shot.as_ref().is_some_and(|s| s.overlay) {
            // 화면 확인용: --lend-demo면 빌려주기 칸, --shot-indexing이면 인덱싱 칸도
            let lend = self.lend_status_arc();
            let progress = (self.indexer.is_active() || lend.is_none()).then(|| self.indexer.progress.clone());
            crate::ui::overlay::overlay_ui(ui, progress.as_ref(), lend.as_ref(), &self.overlay_tx);
            return;
        }
        self.sync_titlebar(ui.visuals().dark_mode);
        // 벽: 창 전체에 깔리는 색 번짐. 메뉴·카드는 그 위에 떠 있는 유리다.
        crate::ui::glass::paint_wall(ui);
        // 본문의 여백. 아래 여백은 두지 않는다 (스크롤되는 내용이 창 끝까지 이어지게).
        let body = |left: i8| {
            egui::Frame::NONE.inner_margin(egui::Margin {
                left,
                right: theme::EDGE,
                top: theme::EDGE,
                bottom: 0,
            })
        };
        // 토스트·대화상자는 숨김 상태의 이른 반환 뒤에만 그린다 (숨긴 창에서 즉시형 창을 띄우면 패닉)
        if self.setup_open {
            egui::CentralPanel::default().frame(body(20)).show(ui, |ui| self.setup_ui(ui));
            self.data_move_dialog(&ctx);
            self.show_toast(&ctx);
            return;
        }
        // 창이 좁으면 메뉴를 아이콘만 남긴다. 메뉴는 창 가장자리에서 떼어 띄운다.
        let compact = ctx.content_rect().width() < 900.0;
        let edge = theme::EDGE;
        egui::Panel::left("nav")
            .exact_size(edge as f32 + if compact { theme::NAV_W_COMPACT } else { theme::NAV_W })
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(NAV_PAD).outer_margin(egui::Margin {
                left: edge,
                right: 0,
                top: edge,
                bottom: edge,
            }))
            .show(ui, |ui| self.nav_panel(ui, compact));
        let body = body(theme::GAP);
        egui::CentralPanel::default().frame(body).show(ui, |ui| {
            if self.show_gallery {
                return widgets::gallery(ui);
            }
            match self.tab {
                Tab::Search => self.search_tab(ui),
                Tab::Index => self.index_tab(ui),
                Tab::Server => self.server_tab(ui),
                Tab::Settings => self.settings_tab(ui),
            }
        });
        self.rebuild_dialog(&ctx);
        self.data_move_dialog(&ctx);
        self.show_toast(&ctx);
    }
}

impl Drop for GemmaApp {
    fn drop(&mut self) {
        self.indexer.stop();
        self.server.stop();
    }
}

/// 앱에 넣은 화면 글꼴: Pretendard 가변 글꼴 v1.3.9 (SIL OFL 1.1 — assets/fonts/Pretendard-LICENSE.txt).
/// 어느 PC에서나 같은 모양으로 보이도록 exe에 포함한다. 파일 하나로 굵기를 달리해 쓴다.
static PRETENDARD: &[u8] = include_bytes!("../assets/fonts/PretendardVariable.ttf");

/// 화면 글꼴을 정한다: Pretendard를 굵기별 패밀리로 등록하고, 거기에 없는 글자는 Windows 글꼴로 대체한다.
fn setup_fonts(ctx: &egui::Context) {
    use crate::ui::theme;
    use egui::epaint::text::VariationCoords;
    use egui::FontFamily;

    let mut fonts = egui::FontDefinitions::default();
    let face = |fonts: &mut egui::FontDefinitions, name: &str, wght: f32| {
        let mut data = egui::FontData::from_static(PRETENDARD);
        data.tweak.coords = VariationCoords::new([(b"wght", wght)]);
        fonts.font_data.insert(name.to_string(), Arc::new(data));
    };
    face(&mut fonts, "pretendard", 400.0);
    // 아이콘 글꼴 (Lucide). 본문 글꼴 목록에는 넣지 않고 `ICONS` 패밀리로만 쓴다: Pretendard가 같은 번호에
    // 다른 글자를 갖고 있고, 아이콘 글꼴에는 숫자·영문 자리에 빈 글자가 있어 섞으면 서로 가린다.
    fonts.font_data.insert(
        "lucide".to_string(),
        Arc::new(egui::FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES)),
    );
    fonts
        .families
        .insert(FontFamily::Name(theme::ICONS.into()), vec!["lucide".to_string()]);

    // Pretendard에 없는 글자(한자 등)는 맑은 고딕, 기호는 Segoe UI Symbol
    let system = |fonts: &mut egui::FontDefinitions, name: &str, path: &str| -> bool {
        let Ok(bytes) = std::fs::read(path) else { return false };
        fonts
            .font_data
            .insert(name.to_string(), Arc::new(egui::FontData::from_owned(bytes)));
        true
    };
    let has_malgun = system(&mut fonts, "malgun", crate::platform::TEXT_FALLBACK_FONT);
    let has_symbols = system(&mut fonts, "segoe-symbol", crate::platform::SYMBOL_FALLBACK_FONT);

    // 본문: Pretendard → 맑은 고딕 → egui 기본 글꼴(이모지 등) → 기호
    let prop = fonts.families.entry(FontFamily::Proportional).or_default();
    let defaults = std::mem::take(prop);
    prop.push("pretendard".to_string());
    if has_malgun {
        prop.push("malgun".to_string());
    }
    prop.extend(defaults);
    if has_symbols {
        prop.push("segoe-symbol".to_string());
    }
    let prop_list = prop.clone();

    // 고정폭(로그): 기본 Hack 뒤에 한글·기호 대체 글꼴
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.push("pretendard".to_string());
    if has_malgun {
        mono.push("malgun".to_string());
    }
    if has_symbols {
        mono.push("segoe-symbol".to_string());
    }

    // 굵기별 패밀리: 버튼(500), 강조 라벨(600), 제목·숫자(700). 그 굵기에 없는 글자는 본문 글꼴 목록으로 대체된다.
    for (family, name, wght) in [
        (theme::MEDIUM, "pretendard-500", 500.0),
        (theme::SEMIBOLD, "pretendard-600", 600.0),
        (theme::BOLD, "pretendard-700", 700.0),
    ] {
        face(&mut fonts, name, wght);
        let mut list = vec![name.to_string()];
        list.extend(prop_list.iter().cloned());
        fonts.families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(fonts);
}
