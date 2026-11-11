//! Windows 구현.

use crate::util::{hidden_command, run_capture};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

// ── 자식 프로세스 ──

/// 실행 파일 이름: "ffmpeg" → "ffmpeg.exe"
pub fn exe_name(base: &str) -> String {
    format!("{base}.exe")
}

/// GUI 앱에서 만든 자식 프로세스가 콘솔 창을 띄우지 않게 한다 (CREATE_NO_WINDOW)
pub fn no_console(cmd: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[link(name = "kernel32")]
extern "system" {
    fn SetErrorMode(mode: u32) -> u32;
}

/// 살아 있는 동안 만든 자식 프로세스는 DLL이 없어도 Windows 오류 창을 띄우지 않는다
/// (오류 모드는 프로세스를 만들 때 물려받는다). 사라지면 원래대로 돌린다.
pub struct QuietErrors(u32);

impl QuietErrors {
    pub fn new() -> Self {
        const SEM_FAILCRITICALERRORS: u32 = 0x0001;
        let old = unsafe { SetErrorMode(SEM_FAILCRITICALERRORS) };
        unsafe { SetErrorMode(old | SEM_FAILCRITICALERRORS) };
        Self(old)
    }
}

impl Drop for QuietErrors {
    fn drop(&mut self) {
        unsafe { SetErrorMode(self.0) };
    }
}

/// 앱이 죽으면 자식도 끝나게 하는 일은 Windows에서는 Job Object가 한다 (`server/job.rs`)
pub fn die_with_parent(_cmd: &mut Command) {}

/// 프로세스가 끝날 때까지 기다린다 (최대 `timeout`). 이미 끝났거나 열 수 없으면 바로 돌아온다.
pub fn wait_for_process_exit(pid: u32, timeout: Duration) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    unsafe {
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, timeout.as_millis().min(u32::MAX as u128) as u32);
            CloseHandle(h);
        }
    }
}

// ── 데이터 위치 (location.rs) ──

/// 데이터의 고정 위치: `%LOCALAPPDATA%\GemmaSearch`. 인덱스가 크고 PC마다 다르므로 Roaming이 아닌 Local에 둔다.
/// 알 수 없으면 None (그때는 실행 파일 옆을 쓴다).
pub fn data_home() -> Option<PathBuf> {
    let local = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    local.is_absolute().then(|| local.join("GemmaSearch"))
}

/// 두 경로가 같은 드라이브에 있는지 (같으면 폴더 이름만 바꿔 옮길 수 있다). 모르면 false.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    use std::path::{Component, Prefix};
    fn volume(p: &Path) -> Option<String> {
        let p = std::path::absolute(p).ok()?;
        let Some(Component::Prefix(pre)) = p.components().next() else { return None };
        Some(match pre.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => (d as char).to_ascii_lowercase().to_string(),
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                format!(r"\\{}\{}", server.to_string_lossy(), share.to_string_lossy()).to_lowercase()
            }
            _ => pre.as_os_str().to_string_lossy().to_lowercase(),
        })
    }
    matches!((volume(a), volume(b)), (Some(x), Some(y)) if x == y)
}

/// PATH에 없을 때 더 찾아볼 설치 위치: winget(Gyan.FFmpeg 등)의 기본 위치
pub fn find_installed_tool(exe: &str) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let links = Path::new(&local).join(r"Microsoft\WinGet\Links").join(exe);
    if links.is_file() {
        return Some(links);
    }
    let pkgs = Path::new(&local).join(r"Microsoft\WinGet\Packages");
    for pkg in std::fs::read_dir(&pkgs).ok()?.flatten() {
        if let Ok(inner) = std::fs::read_dir(pkg.path()) {
            for sub in inner.flatten() {
                let p = sub.path().join("bin").join(exe);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// ffmpeg 설치 창을 띄운다 (명령 창에서 winget으로 설치)
#[cfg(feature = "gui")]
pub fn install_ffmpeg() {
    let _ = Command::new("cmd")
        .args(["/c", "start", "ffmpeg 설치", "cmd", "/k", "winget install --id Gyan.FFmpeg -e"])
        .spawn();
}

// ── 파일 관리자 · 클립보드 · 기본 앱 ──

/// 기본 프로그램으로 파일 열기
#[cfg(feature = "gui")]
pub fn open_with_default_app(path: &str) {
    let _ = Command::new("explorer.exe").arg(path).spawn();
}

/// 탐색기에서 파일을 선택한 채로 폴더 열기
#[cfg(feature = "gui")]
pub fn reveal_in_file_manager(path: &str) {
    let _ = Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn();
}

/// 파일 자체를 클립보드에 (탐색기·메신저에 붙여넣기). PowerShell의 Set-Clipboard 사용.
#[cfg(feature = "gui")]
pub fn copy_file_to_clipboard(path: &str) -> anyhow::Result<()> {
    let quoted = path.replace('\'', "''");
    let mut cmd = hidden_command("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &format!("Set-Clipboard -LiteralPath '{quoted}'"),
    ]);
    let out = run_capture(cmd, Duration::from_secs(15))?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// 확장자(".mp4")에 연결된 기본 앱의 실행 파일
#[cfg(feature = "gui")]
pub fn default_app_for(ext: &str) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};
    let assoc = wide(std::ffi::OsStr::new(ext));
    let verb = wide(std::ffi::OsStr::new("open"));
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let hr = unsafe {
        AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, assoc.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len)
    };
    if hr != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    p.is_file().then_some(p)
}

// ── 창 ──

/// 앱 창의 핸들 (모르면 0). 아래 창 함수들에 넘긴다.
#[cfg(feature = "gui")]
pub fn native_window(cc: &eframe::CreationContext<'_>) -> usize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    cc.window_handle()
        .ok()
        .and_then(|h| match h.as_raw() {
            RawWindowHandle::Win32(w) => Some(w.hwnd.get() as usize),
            _ => None,
        })
        .unwrap_or(0)
}

/// 창을 화면 맨 앞으로 가져와 입력을 받게 한다
#[cfg(feature = "gui")]
pub fn bring_to_front(window: usize) {
    if window != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(window as _);
        }
    }
}

/// 제목 표시줄과 창 테두리의 색을 앱 바탕에 맞춘다 (Windows 11. Windows 10에서는 아무 일도 없다). 색은 [R, G, B].
#[cfg(feature = "gui")]
pub fn set_titlebar_colors(window: usize, dark: bool, caption: [u8; 3], border: [u8; 3]) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    if window == 0 {
        return;
    }
    let colorref = |c: [u8; 3]| c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16;
    let set = |attr: i32, value: u32| unsafe {
        DwmSetWindowAttribute(window as _, attr as u32, &value as *const u32 as *const _, 4);
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, dark as u32);
    set(DWMWA_CAPTION_COLOR, colorref(caption));
    set(DWMWA_BORDER_COLOR, colorref(border));
}

/// 창이 보이게 되더라도 포커스를 받지 않고 작업 표시줄에도 나오지 않게 한다 (화면 확인용 실행)
#[cfg(feature = "gui")]
pub fn make_window_passive(window: usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    if window == 0 {
        return;
    }
    unsafe {
        let ex = GetWindowLongPtrW(window as _, GWL_EXSTYLE);
        SetWindowLongPtrW(window as _, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize);
    }
}

/// 앱 글꼴(Pretendard)에 없는 글자를 그릴 때 쓰는 시스템 글꼴 파일: 글자(한자 등) / 기호
#[cfg(feature = "gui")]
pub const TEXT_FALLBACK_FONT: &str = r"C:\Windows\Fonts\malgun.ttf";
#[cfg(feature = "gui")]
pub const SYMBOL_FALLBACK_FONT: &str = r"C:\Windows\Fonts\seguisym.ttf";

// ── 시각 · PC 사양 ──

/// 지금 로컬 시각: [년, 월, 일, 시, 분, 초]
pub fn local_time() -> [u16; 6] {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    [t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond]
}

/// 설치된 메모리 (MB). 알 수 없으면 0.
pub fn total_memory_mb() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return 0;
    }
    m.ullTotalPhys / 1_048_576
}

/// `dir`이 있는 드라이브의 남은 공간 (MB). 폴더가 아직 없으면 있는 윗폴더 기준. 알 수 없으면 0.
pub fn free_disk_mb(dir: &Path) -> u64 {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut p = dir.to_path_buf();
    while !p.exists() {
        match p.parent() {
            Some(q) => p = q.to_path_buf(),
            None => break,
        }
    }
    let w = wide(p.as_os_str());
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok == 0 {
        0
    } else {
        free / 1_048_576
    }
}

/// OS 이름과 그래픽카드 목록
pub struct OsInfo {
    /// 화면에 보여 줄 이름 ("Windows 11 Pro 26H2 (빌드 26300)")
    pub name: String,
    pub build: u32,
    /// 그래픽카드: (이름, VRAM 바이트). 가상 어댑터도 섞여 있을 수 있다.
    pub adapters: Vec<(String, u64)>,
}

/// PowerShell 한 번으로 Windows 버전과 그래픽카드 목록(레지스트리의 정확한 VRAM)을 얻는다
pub fn os_and_adapters() -> OsInfo {
    let script = r#"
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
"OS`t$($cv.ProductName)`t$($cv.DisplayVersion)`t$($cv.CurrentBuildNumber)"
Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}' -ErrorAction SilentlyContinue |
  Where-Object { $_.PSChildName -match '^\d{4}$' } | ForEach-Object {
    $p = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
    $mem = $p.'HardwareInformation.qwMemorySize'
    if (-not $mem) { $mem = $p.'HardwareInformation.MemorySize' }
    if ($mem -is [byte[]]) { $mem = [BitConverter]::ToUInt32($mem, 0) }
    if ($p.DriverDesc) { "GPU`t$($p.DriverDesc)`t$([uint64]$mem)" }
  }
"#;
    let mut cmd = hidden_command("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    let mut info = OsInfo {
        name: String::from("Windows"),
        build: 0,
        adapters: Vec::new(),
    };
    if let Ok(out) = run_capture(cmd, Duration::from_secs(20)) {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let f: Vec<&str> = line.trim_end().split('\t').collect();
            match f.as_slice() {
                ["OS", name, ver, b] => {
                    info.build = b.trim().parse().unwrap_or(0);
                    // Windows 11도 ProductName은 "Windows 10 …"으로 남아 있다 → 빌드 번호로 판별
                    let name = if info.build >= 22000 {
                        name.replace("Windows 10", "Windows 11")
                    } else {
                        name.to_string()
                    };
                    info.name = format!("{name} {ver} (빌드 {})", info.build);
                }
                ["GPU", name, mem] => {
                    info.adapters
                        .push((name.to_string(), mem.trim().parse::<u64>().unwrap_or(0)));
                }
                _ => {}
            }
        }
    }
    info
}

// ── 경로 ──

/// 네트워크 위치인지: `\\서버\공유` 또는 네트워크 드라이브에 연결한 드라이브 문자
pub fn is_network_path(path: &str) -> bool {
    let p = path.trim();
    if p.starts_with(r"\\") {
        return true;
    }
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        const DRIVE_REMOTE: u32 = 4;
        let root: Vec<u16> = format!("{}:\\", b[0] as char).encode_utf16().chain(Some(0)).collect();
        return unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_paths() {
        assert!(is_network_path(r"\\192.168.0.100\storage\Media"));
        assert!(!is_network_path(r"C:\Users"));
        assert!(!is_network_path("relative"));
    }

    #[test]
    fn exe_names() {
        assert_eq!(exe_name("ffmpeg"), "ffmpeg.exe");
    }
}
