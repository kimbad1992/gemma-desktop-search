//! 상주 기능: 트레이 아이콘, 전역 단축키(Ctrl+Shift+Space), 단일 실행.
//!
//! 전용 스레드가 숨은 Win32 창 하나를 만들고 메시지 루프를 돈다. 앱 창이 숨겨져 있어도 동작하고,
//! 이벤트는 채널로 앱에 넘긴 뒤 화면 갱신을 요청한다 (eframe은 숨긴 창에서도 update를 부른다).
//!
//! 트레이 아이콘과 단축키 등록은 실패할 수 있다 (로그인 직후 작업 표시줄이 아직 없음, 다른 프로그램이
//! 같은 단축키를 씀, 탐색기가 이 앱보다 높은 권한으로 실행 중). 실패하면 `ShellStatus`에 남기고
//! 될 때까지 주기적으로 다시 시도한다. 앱은 이 상태를 보고 "트레이로 숨기기"를 할지 정한다.

use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const HOTKEY_LABEL: &str = "Ctrl+Shift+Space";
const CLASS: &str = "GemmaSearchShellWindow";

const WM_TRAY: u32 = WM_APP + 1;
const WM_REFRESH: u32 = WM_APP + 2;
/// 두 번째로 실행된 인스턴스가 보내는 "창을 보여줘"
const WM_ACTIVATE_APP: u32 = WM_APP + 3;
const WM_BALLOON: u32 = WM_APP + 4;
/// 두 번째로 실행된 `gemma-search.exe lend …`가 보내는 "빌려주기를 시작해"
const WM_LEND_START: u32 = WM_APP + 5;

const ID_QUICK: usize = 100;
const ID_OPEN: usize = 101;
const ID_INDEX: usize = 102;
const ID_SERVER: usize = 103;
const ID_QUIT: usize = 104;
const ID_LEND: usize = 105;
const ID_LEND_SHOW: usize = 106;

const HOTKEY_ID: i32 = 1;
/// 실패한 등록(트레이 아이콘·단축키)을 다시 시도하는 타이머
const TIMER_RETRY: usize = 1;
const RETRY_MS: u32 = 5000;

/// GetLastError 값
const ERR_ACCESS_DENIED: u32 = 5;
const ERR_HOTKEY_TAKEN: u32 = 1409;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// 단축키 또는 메뉴의 "검색": 빠른 검색창
    QuickSearch,
    /// 트레이 클릭, 메뉴의 "창 열기", 두 번째 실행
    Open,
    ToggleIndex,
    ToggleServer,
    /// 트레이 메뉴의 "계산 빌려주기"
    ToggleLend,
    /// 두 번째로 실행된 `lend` 명령: 설정에 저장된 주소로 빌려주기를 시작한다
    StartLend,
    /// 트레이 메뉴의 "빌려주기 상태 창 보이기" (오버레이를 숨겼을 때)
    ShowLendOverlay,
    Quit,
    /// 트레이 아이콘·단축키 등록 상태가 바뀜 (`Shell::status`로 읽는다)
    Status,
}

/// 트레이 아이콘·전역 단축키가 실제로 등록됐는지
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellStatus {
    pub tray_ok: bool,
    /// 마지막 트레이 등록 실패의 오류 코드 (GetLastError)
    pub tray_error: u32,
    /// 설정에서 단축키를 켰는지
    pub hotkey_wanted: bool,
    pub hotkey_ok: bool,
    pub hotkey_error: u32,
}

impl ShellStatus {
    /// 작업 표시줄(탐색기)이 이 앱보다 높은 권한으로 돌고 있어 등록을 거부당한 상태.
    /// UAC를 끈 PC에서 '보통' 무결성 레이블이 붙은 폴더의 exe를 실행하면 이렇게 된다:
    /// 탐색기는 '높음', 앱은 '보통'이라 창 메시지가 막힌다 (다시 시도해도 풀리지 않는다).
    pub fn below_shell(&self) -> bool {
        !self.tray_ok && self.tray_error == ERR_ACCESS_DENIED
    }

    /// 트레이 아이콘이 없는 이유 (있으면 None)
    pub fn tray_problem(&self) -> Option<String> {
        if self.tray_ok {
            return None;
        }
        Some(if self.below_shell() {
            "트레이 아이콘을 만들 수 없습니다: 작업 표시줄(탐색기)이 이 앱보다 높은 권한으로 실행 중입니다. \
             UAC를 끈 PC에서 권한이 제한된 폴더에 프로그램을 두면 생깁니다. 프로그램 폴더를 다른 드라이브로 옮겨 보세요."
                .to_string()
        } else {
            format!(
                "트레이 아이콘을 만들지 못했습니다 (오류 {}). {}초마다 다시 시도합니다.",
                self.tray_error,
                RETRY_MS / 1000
            )
        })
    }

    /// 단축키를 켰는데 못 쓰는 이유 (껐거나 등록됐으면 None)
    pub fn hotkey_problem(&self) -> Option<String> {
        if !self.hotkey_wanted || self.hotkey_ok {
            return None;
        }
        Some(if self.hotkey_error == ERR_HOTKEY_TAKEN {
            format!("{HOTKEY_LABEL}를 다른 프로그램이 쓰고 있습니다. 그 프로그램이 놓으면 자동으로 등록합니다.")
        } else {
            format!(
                "{HOTKEY_LABEL}를 등록하지 못했습니다 (오류 {}). {}초마다 다시 시도합니다.",
                self.hotkey_error,
                RETRY_MS / 1000
            )
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrayIcon {
    #[default]
    Idle,
    Busy,
    Paused,
}

/// 트레이 메뉴·툴팁에 쓰는 앱 상태
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayState {
    pub tip: String,
    pub icon: TrayIcon,
    /// None = 인덱싱 안 함, Some(false) = 진행 중, Some(true) = 일시정지
    pub indexing: Option<bool>,
    pub server_running: bool,
    pub hotkey: bool,
    /// None = 빌려주지 않음, Some(false) = 빌려주는 중, Some(true) = 일시정지
    pub lending: Option<bool>,
    /// 빌려주는 중인데 오른쪽 아래 상태 창을 숨겼다 (다시 보이기 메뉴를 둔다)
    pub lend_hidden: bool,
}

struct Shared {
    tx: Sender<ShellEvent>,
    ctx: egui::Context,
    state: Arc<Mutex<TrayState>>,
    status: Mutex<ShellStatus>,
    balloon: Mutex<(String, String)>,
    icons: [usize; 3],
    taskbar_created: u32,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

pub struct Shell {
    hwnd: AtomicUsize,
    pub rx: Receiver<ShellEvent>,
    state: Arc<Mutex<TrayState>>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn copy_wide(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

fn send(ev: ShellEvent) {
    if let Some(s) = SHARED.get() {
        let _ = s.tx.send(ev);
        s.ctx.request_repaint();
    }
}

unsafe fn notify_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

/// 트레이 아이콘을 넣거나(add) 바꾼다. 결과는 `ShellStatus`에 남기고 성공 여부를 돌려준다.
unsafe fn tray_set(hwnd: HWND, add: bool) -> bool {
    let Some(sh) = SHARED.get() else { return false };
    let st = sh.state.lock().clone();
    let mut nid = notify_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = sh.icons[st.icon as usize] as HICON;
    copy_wide(&mut nid.szTip, if st.tip.is_empty() { "Gemma Search" } else { &st.tip });
    SetLastError(0);
    let mut ok = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &nid) != 0;
    let err = GetLastError();
    // 이미 들어 있는 아이콘을 다시 넣으려 하면 실패한다 → 바꾸기로 한 번 더
    if !ok && add {
        ok = Shell_NotifyIconW(NIM_MODIFY, &nid) != 0;
    }
    let mut s = sh.status.lock();
    s.tray_ok = ok;
    s.tray_error = if ok { 0 } else { err };
    ok
}

/// 단축키 등록을 시도한다 (설정에서 켰고 아직 등록되지 않았을 때만)
unsafe fn hotkey_try(hwnd: HWND) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_SPACE,
    };
    let Some(sh) = SHARED.get() else { return };
    {
        let s = sh.status.lock();
        if !s.hotkey_wanted || s.hotkey_ok {
            return;
        }
    }
    SetLastError(0);
    let ok = RegisterHotKey(hwnd, HOTKEY_ID, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, VK_SPACE as u32) != 0;
    let err = GetLastError();
    {
        let mut s = sh.status.lock();
        s.hotkey_ok = ok;
        s.hotkey_error = if ok { 0 } else { err };
    }
    sh.state.lock().hotkey = ok;
}

/// 아직 안 된 등록(트레이 아이콘·단축키)을 시도하고, 남은 게 있으면 재시도 타이머를 건다.
/// 상태가 바뀌었으면 앱에 알린다.
unsafe fn ensure_registered(hwnd: HWND) {
    let Some(sh) = SHARED.get() else { return };
    let before = *sh.status.lock();
    hotkey_try(hwnd);
    if !before.tray_ok {
        tray_set(hwnd, true);
    }
    let after = *sh.status.lock();
    if after.tray_ok && (!after.hotkey_wanted || after.hotkey_ok) {
        KillTimer(hwnd, TIMER_RETRY);
    } else {
        // 같은 ID로 다시 걸면 간격만 새로 잡힌다
        SetTimer(hwnd, TIMER_RETRY, RETRY_MS, None);
    }
    if after != before {
        send(ShellEvent::Status);
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let Some(sh) = SHARED.get() else { return };
    let st = sh.state.lock().clone();
    let menu = CreatePopupMenu();
    let item = |id: usize, text: &str| {
        let t = wide(text);
        AppendMenuW(menu, MF_STRING, id, t.as_ptr());
    };
    item(ID_QUICK, &if st.hotkey { format!("검색\t{HOTKEY_LABEL}") } else { "검색".to_string() });
    item(ID_OPEN, "창 열기");
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    item(
        ID_INDEX,
        match st.indexing {
            None => "인덱싱 시작 (변경분)",
            Some(false) => "인덱싱 일시정지",
            Some(true) => "인덱싱 재개",
        },
    );
    item(ID_SERVER, if st.server_running { "서버 중지" } else { "서버 시작" });
    item(ID_LEND, if st.lending.is_some() { "계산 빌려주기 끝내기" } else { "계산 빌려주기 시작" });
    if st.lending.is_some() && st.lend_hidden {
        item(ID_LEND_SHOW, "빌려주기 상태 창 보이기");
    }
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    item(ID_QUIT, "종료");
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // 이렇게 해야 메뉴 밖을 클릭했을 때 메뉴가 닫힌다
    SetForegroundWindow(hwnd);
    TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, std::ptr::null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let taskbar_created = SHARED.get().map(|s| s.taskbar_created).unwrap_or(0);
    match msg {
        WM_HOTKEY => send(ShellEvent::QuickSearch),
        WM_TRAY => match (lp as u32) & 0xFFFF {
            WM_LBUTTONUP => send(ShellEvent::Open),
            WM_RBUTTONUP => show_menu(hwnd),
            _ => {}
        },
        WM_COMMAND => match wp & 0xFFFF {
            ID_QUICK => send(ShellEvent::QuickSearch),
            ID_OPEN => send(ShellEvent::Open),
            ID_INDEX => send(ShellEvent::ToggleIndex),
            ID_SERVER => send(ShellEvent::ToggleServer),
            ID_LEND => send(ShellEvent::ToggleLend),
            ID_LEND_SHOW => send(ShellEvent::ShowLendOverlay),
            ID_QUIT => send(ShellEvent::Quit),
            _ => {}
        },
        WM_REFRESH => {
            // 아이콘이 아직 없으면 건드리지 않는다 (재시도 때 최신 상태로 들어간다)
            let had_icon = SHARED.get().is_some_and(|s| s.status.lock().tray_ok);
            if had_icon && !tray_set(hwnd, false) {
                // 있던 아이콘이 사라졌다 (탐색기가 알림 없이 다시 시작된 경우 등) → 다시 넣기
                ensure_registered(hwnd);
                send(ShellEvent::Status);
            }
        }
        WM_TIMER => {
            if wp == TIMER_RETRY {
                ensure_registered(hwnd);
            }
        }
        WM_ACTIVATE_APP => send(ShellEvent::Open),
        WM_LEND_START => send(ShellEvent::StartLend),
        WM_BALLOON => {
            if let Some(sh) = SHARED.get() {
                let (title, text) = sh.balloon.lock().clone();
                let mut nid = notify_data(hwnd);
                nid.uFlags = NIF_INFO;
                nid.dwInfoFlags = NIIF_INFO;
                copy_wide(&mut nid.szInfoTitle, &title);
                copy_wide(&mut nid.szInfo, &text);
                Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }
        WM_CLOSE => {
            KillTimer(hwnd, TIMER_RETRY);
            let nid = notify_data(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &nid);
            DestroyWindow(hwnd);
        }
        WM_DESTROY => PostQuitMessage(0),
        // 탐색기가 다시 시작되면 트레이 아이콘이 사라진다 → 다시 넣는다
        m if m == taskbar_created && m != 0 => {
            if let Some(sh) = SHARED.get() {
                sh.status.lock().tray_ok = false;
            }
            ensure_registered(hwnd);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

impl Shell {
    pub fn start(ctx: egui::Context, hotkey: bool) -> Result<Shell, String> {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let (tx, rx) = unbounded();
        let state = Arc::new(Mutex::new(TrayState::default()));
        let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
        let load = |id: u16| unsafe {
            LoadImageW(
                hinst,
                id as usize as *const u16,
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_DEFAULTCOLOR,
            ) as usize
        };
        let shared = Shared {
            tx,
            ctx,
            state: state.clone(),
            status: Mutex::new(ShellStatus {
                hotkey_wanted: hotkey,
                ..Default::default()
            }),
            balloon: Mutex::new(Default::default()),
            icons: [load(1), load(2), load(3)],
            taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        };
        SHARED.set(shared).map_err(|_| "셸이 이미 시작됨".to_string())?;

        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<(usize, Option<String>)>(1);
        // 핸들은 스레드로 넘길 수 있게 정수로
        let hinst_n = hinst as usize;
        std::thread::Builder::new()
            .name("shell".into())
            .spawn(move || unsafe {
                let hinst = hinst_n as windows_sys::Win32::Foundation::HINSTANCE;
                let class = wide(CLASS);
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: hinst,
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: class.as_ptr(),
                };
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    wide("Gemma Search").as_ptr(),
                    WS_OVERLAPPED,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    hinst,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    let _ = ready_tx.send((0, Some("셸 창을 만들지 못했습니다".into())));
                    return;
                }
                let _ = ready_tx.send((hwnd as usize, None));
                // 단축키·트레이 아이콘 등록. 안 되면 타이머로 계속 다시 시도한다.
                ensure_registered(hwnd);
                // 첫 등록 시도가 끝났음을 알린다 (앱이 시작할 때 결과를 바로 읽을 수 있게)
                let _ = ready_tx.send((hwnd as usize, None));
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .map_err(|e| e.to_string())?;
        let (hwnd, error) = ready_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .map_err(|_| "셸 스레드 응답 없음".to_string())?;
        if hwnd == 0 {
            return Err(error.unwrap_or_default());
        }
        // 첫 등록 시도가 끝나기를 잠깐 기다린다. 탐색기가 바쁘면 몇 초 걸릴 수 있는데, 그때는 더 기다리지
        // 않고 진행한다 (결과는 나중에 ShellEvent::Status로 온다).
        let _ = ready_rx.recv_timeout(std::time::Duration::from_secs(2));
        Ok(Shell {
            hwnd: AtomicUsize::new(hwnd),
            rx,
            state,
        })
    }

    /// 트레이 아이콘·단축키가 지금 등록돼 있는지
    pub fn status(&self) -> ShellStatus {
        SHARED.get().map(|s| *s.status.lock()).unwrap_or_default()
    }

    fn post(&self, msg: u32) {
        let h = self.hwnd.load(Ordering::SeqCst);
        if h != 0 {
            unsafe { PostMessageW(h as HWND, msg, 0, 0) };
        }
    }

    pub fn hotkey_registered(&self) -> bool {
        self.state.lock().hotkey
    }

    /// 앱 상태를 트레이에 반영 (바뀌었을 때만 아이콘·툴팁 갱신)
    pub fn update(
        &self,
        tip: String,
        icon: TrayIcon,
        indexing: Option<bool>,
        server_running: bool,
        lending: Option<bool>,
        lend_hidden: bool,
    ) {
        let changed = {
            let mut st = self.state.lock();
            let visual = st.tip != tip || st.icon != icon;
            st.tip = tip;
            st.icon = icon;
            st.indexing = indexing;
            st.server_running = server_running;
            st.lending = lending;
            st.lend_hidden = lend_hidden;
            visual
        };
        if changed {
            self.post(WM_REFRESH);
        }
    }

    /// 트레이 알림 풍선
    pub fn balloon(&self, title: &str, text: &str) {
        if let Some(sh) = SHARED.get() {
            *sh.balloon.lock() = (title.to_string(), text.to_string());
            self.post(WM_BALLOON);
        }
    }

    /// 트레이 아이콘을 지우고 셸 스레드를 끝낸다
    pub fn shutdown(&self) {
        self.post(WM_CLOSE);
        self.hwnd.store(0, Ordering::SeqCst);
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 같은 데이터 폴더로 이미 실행 중인 인스턴스가 있으면 그 창을 띄우라고 알리고 true.
/// 없으면 이 인스턴스가 주인이 된다 (뮤텍스는 프로세스가 끝날 때까지 유지).
/// 같은 데이터 폴더로 이미 떠 있으면 그 앱에 알리고 true. `lend`면 "빌려주기를 시작해", 아니면 "창을 보여줘".
pub fn already_running(data_dir: &std::path::Path, lend: bool) -> bool {
    use windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS;
    use windows_sys::Win32::System::Threading::CreateMutexW;
    // 경로를 이름에 쓸 수 있게 단순 해시
    let mut h: u64 = 1469598103934665603;
    for b in data_dir.to_string_lossy().to_lowercase().bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    let name = wide(&format!("Local\\GemmaSearch-{h:016x}"));
    unsafe {
        let m = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if !m.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
            let hwnd = FindWindowW(wide(CLASS).as_ptr(), std::ptr::null());
            if !hwnd.is_null() {
                PostMessageW(hwnd, if lend { WM_LEND_START } else { WM_ACTIVATE_APP }, 0, 0);
            }
            return true;
        }
        // 핸들은 일부러 닫지 않는다 (프로세스 수명 동안 유지)
        let _ = m;
    }
    false
}

/// 지금 알림을 띄우면 안 되는 상황인지: 전체 화면 앱(게임·영상), 발표 모드, 방해 금지 시간.
/// Windows가 알림 표시 여부를 판단할 때 쓰는 그 값을 그대로 쓴다.
pub fn user_busy() -> bool {
    use windows_sys::Win32::UI::Shell::{
        SHQueryUserNotificationState, QUNS_ACCEPTS_NOTIFICATIONS, QUNS_BUSY, QUNS_PRESENTATION_MODE,
        QUNS_QUIET_TIME, QUNS_RUNNING_D3D_FULL_SCREEN,
    };
    let mut st = QUNS_ACCEPTS_NOTIFICATIONS;
    if unsafe { SHQueryUserNotificationState(&mut st) } != 0 {
        return false;
    }
    match st {
        QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE | QUNS_QUIET_TIME => true,
        // 이 앱이 탐색기보다 낮은 권한으로 돌면 전체 화면 앱이 없어도 BUSY가 돌아온다 (실측).
        // 그대로 믿으면 오버레이가 영영 안 뜨므로, 그 상황에서만 맨 앞 창을 직접 본다.
        QUNS_BUSY => {
            let below_shell = SHARED.get().is_some_and(|s| s.status.lock().below_shell());
            !below_shell || foreground_fullscreen()
        }
        _ => false,
    }
}

/// 맨 앞 창이 모니터 하나를 통째로 덮는 테두리 없는 창인지 (전체 화면 게임·영상).
/// 다른 프로세스의 창이라도 위치·스타일은 권한과 상관없이 읽을 수 있다.
fn foreground_fullscreen() -> bool {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return false;
        }
        // 바탕 화면은 화면을 다 덮지만 전체 화면 앱이 아니다
        let mut buf = [0u16; 64];
        let n = GetClassNameW(fg, buf.as_mut_ptr(), buf.len() as i32).clamp(0, buf.len() as i32) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        if class == "Progman" || class == "WorkerW" {
            return false;
        }
        // 제목 표시줄이 있는 보통 창은 최대화해서 화면을 다 덮어도(작업 표시줄 자동 숨김) 전체 화면이 아니다
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        if (style & WS_CAPTION) == WS_CAPTION {
            return false;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(fg, &mut r) == 0 {
            return false;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// 주 모니터의 작업 영역 (물리 픽셀, 작업 표시줄 제외): (x, y, 너비, 높이)
pub fn primary_work_area() -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        let r = mi.rcWork;
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }
}

/// 마우스가 있는 모니터의 작업 영역 (물리 픽셀): (x, y, 너비, 높이)
pub fn cursor_work_area() -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return None;
        }
        let r = mi.rcWork;
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }
}
