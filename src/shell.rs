//! 상주 기능: 트레이 아이콘, 전역 단축키(Ctrl+Shift+Space), 단일 실행.
//!
//! 전용 스레드가 숨은 Win32 창 하나를 만들고 메시지 루프를 돈다. 앱 창이 숨겨져 있어도 동작하고,
//! 이벤트는 채널로 앱에 넘긴 뒤 화면 갱신을 요청한다 (eframe은 숨긴 창에서도 update를 부른다).

use crossbeam_channel::{unbounded, Receiver, Sender};
use eframe::egui;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
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

const ID_QUICK: usize = 100;
const ID_OPEN: usize = 101;
const ID_INDEX: usize = 102;
const ID_SERVER: usize = 103;
const ID_QUIT: usize = 104;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    /// 단축키 또는 메뉴의 "검색": 빠른 검색창
    QuickSearch,
    /// 트레이 클릭, 메뉴의 "창 열기", 두 번째 실행
    Open,
    ToggleIndex,
    ToggleServer,
    Quit,
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
}

struct Shared {
    tx: Sender<ShellEvent>,
    ctx: egui::Context,
    state: Arc<Mutex<TrayState>>,
    balloon: Mutex<(String, String)>,
    icons: [usize; 3],
    taskbar_created: u32,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

pub struct Shell {
    hwnd: AtomicUsize,
    pub rx: Receiver<ShellEvent>,
    state: Arc<Mutex<TrayState>>,
    pub hotkey_error: Option<String>,
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

unsafe fn tray_set(hwnd: HWND, add: bool) {
    let Some(sh) = SHARED.get() else { return };
    let st = sh.state.lock().clone();
    let mut nid = notify_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = sh.icons[st.icon as usize] as HICON;
    copy_wide(&mut nid.szTip, if st.tip.is_empty() { "Gemma Search" } else { &st.tip });
    Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &nid);
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
            ID_QUIT => send(ShellEvent::Quit),
            _ => {}
        },
        WM_REFRESH => tray_set(hwnd, false),
        WM_ACTIVATE_APP => send(ShellEvent::Open),
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
            let nid = notify_data(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &nid);
            DestroyWindow(hwnd);
        }
        WM_DESTROY => PostQuitMessage(0),
        // 탐색기가 다시 시작되면 트레이 아이콘을 다시 넣는다
        m if m == taskbar_created && m != 0 => tray_set(hwnd, true),
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
                let mut hotkey_err = None;
                if hotkey {
                    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                        RegisterHotKey, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VK_SPACE,
                    };
                    let ok = RegisterHotKey(hwnd, 1, MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT, VK_SPACE as u32) != 0;
                    if ok {
                        if let Some(sh) = SHARED.get() {
                            sh.state.lock().hotkey = true;
                        }
                    } else {
                        hotkey_err = Some(format!("{HOTKEY_LABEL}를 다른 프로그램이 이미 쓰고 있습니다"));
                    }
                }
                tray_set(hwnd, true);
                let _ = ready_tx.send((hwnd as usize, hotkey_err));
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            })
            .map_err(|e| e.to_string())?;
        let (hwnd, hotkey_error) = ready_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .map_err(|_| "셸 스레드 응답 없음".to_string())?;
        if hwnd == 0 {
            return Err(hotkey_error.unwrap_or_default());
        }
        Ok(Shell {
            hwnd: AtomicUsize::new(hwnd),
            rx,
            state,
            hotkey_error,
        })
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
    pub fn update(&self, tip: String, icon: TrayIcon, indexing: Option<bool>, server_running: bool) {
        let changed = {
            let mut st = self.state.lock();
            let visual = st.tip != tip || st.icon != icon;
            st.tip = tip;
            st.icon = icon;
            st.indexing = indexing;
            st.server_running = server_running;
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
pub fn already_running(data_dir: &std::path::Path) -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
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
                PostMessageW(hwnd, WM_ACTIVATE_APP, 0, 0);
            }
            return true;
        }
        // 핸들은 일부러 닫지 않는다 (프로세스 수명 동안 유지)
        let _ = m;
    }
    false
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
