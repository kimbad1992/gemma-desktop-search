//! 공용 유틸: 콘솔 창 없는 자식 프로세스 실행, 경로 탐색, 포맷팅

use anyhow::{bail, Result};
use std::ffi::OsStr;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// CREATE_NO_WINDOW — GUI 앱에서 자식 프로세스가 콘솔 창을 띄우지 않도록
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 모든 자식 프로세스(llama-server, ffmpeg, ffprobe)는 이 헬퍼로 만든다.
pub fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// 자식 프로세스를 실행해 stdout/stderr를 모은다. 시간 초과 시 강제 종료.
pub fn run_capture(mut cmd: Command, timeout: Duration) -> Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    crate::server::job::adopt(&child);
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("시간 초과 ({}초)", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    Ok(Output {
        status,
        stdout: t_out.join().unwrap_or_default(),
        stderr: t_err.join().unwrap_or_default(),
    })
}

/// PATH와 winget 설치 위치에서 실행 파일을 찾는다.
pub fn find_tool(exe: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // winget(Gyan.FFmpeg 등) 기본 설치 위치
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let links = Path::new(&local).join(r"Microsoft\WinGet\Links").join(exe);
        if links.is_file() {
            return Some(links);
        }
        let pkgs = Path::new(&local).join(r"Microsoft\WinGet\Packages");
        if let Ok(rd) = std::fs::read_dir(&pkgs) {
            for pkg in rd.flatten() {
                if let Ok(inner) = std::fs::read_dir(pkg.path()) {
                    for sub in inner.flatten() {
                        let p = sub.path().join("bin").join(exe);
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }
    }
    None
}

/// 기본 프로그램으로 파일 열기
pub fn open_path(path: &str) {
    let _ = Command::new("explorer.exe").arg(path).spawn();
}

/// 파일 자체를 클립보드에 (탐색기·메신저에 붙여넣기). PowerShell의 Set-Clipboard 사용.
pub fn copy_file_to_clipboard(path: &str) -> Result<()> {
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

/// 탐색기에서 파일 선택한 채로 폴더 열기
pub fn reveal_path(path: &str) {
    let _ = Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn();
}

/// 현재 로컬 시각 "2026-11-08 22:04:43"
pub fn now_local_string() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// 1234567 → "1,234,567"
pub fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 경과 시간 "1:02:03" / "02:03"
pub fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// 미디어 타임스탬프 (밀리초) → "1:02:03" / "02:03"
pub fn fmt_ts(ms: i64) -> String {
    fmt_dur(Duration::from_millis(ms.max(0) as u64))
}

pub fn truncate_chars(s: &str, max: usize) -> String {
    let mut it = s.char_indices();
    match it.nth(max) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_string(),
    }
}

/// 공백/따옴표를 고려한 간단한 인자 분리
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut has = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_q = !in_q;
                has = true;
            }
            c if c.is_whitespace() && !in_q => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_format() {
        assert_eq!(fmt_num(0), "0");
        assert_eq!(fmt_num(999), "999");
        assert_eq!(fmt_num(1000), "1,000");
        assert_eq!(fmt_num(1234567), "1,234,567");
    }

    #[test]
    fn args_split() {
        assert_eq!(
            split_args(r#"-fa on --alias "my model"  -t 8"#),
            vec!["-fa", "on", "--alias", "my model", "-t", "8"]
        );
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn dur_format() {
        assert_eq!(fmt_dur(Duration::from_secs(65)), "01:05");
        assert_eq!(fmt_dur(Duration::from_secs(3723)), "1:02:03");
    }
}
