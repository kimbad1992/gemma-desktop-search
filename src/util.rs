//! 공용 유틸: 콘솔 창 없는 자식 프로세스 실행, 경로 탐색, 포맷팅

use anyhow::{bail, Result};
use std::ffi::OsStr;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// 백그라운드 작업이 상태를 바꿨을 때 부르는 함수. 화면이 있는 앱은 다시 그리게 하고, 서비스판은 아무것도 하지 않는다.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// 모든 자식 프로세스(llama-server, ffmpeg, ffprobe)는 이 헬퍼로 만든다 (콘솔 창이 뜨지 않게).
pub fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    crate::platform::no_console(&mut c);
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

/// PATH와 그 OS의 흔한 설치 위치에서 실행 파일을 찾는다.
pub fn find_tool(exe: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    crate::platform::find_installed_tool(exe)
}

/// 현재 로컬 시각 "2026-11-08 22:04:43"
pub fn now_local_string() -> String {
    let [y, mo, d, h, mi, s] = crate::platform::local_time();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
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
