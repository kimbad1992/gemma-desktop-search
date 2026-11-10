//! Linux(와 그 밖의 Unix) 구현. 지금은 창 없는 서비스판에 필요한 것만 있다:
//! 창·탐색기·클립보드처럼 화면이 있어야 쓰는 기능은 아직 없다.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

// ── 자식 프로세스 ──

/// 실행 파일 이름: 확장자가 붙지 않는다
pub fn exe_name(base: &str) -> String {
    base.to_string()
}

/// 콘솔 창이 따로 뜨지 않는 OS라 할 일이 없다
pub fn no_console(_cmd: &mut Command) {}

/// 이 프로세스가 죽으면 자식도 끝나게 한다 (Linux). 자식을 만든 스레드가 끝날 때 신호가 가므로,
/// 오래 사는 스레드에서 만들어야 한다.
pub fn die_with_parent(cmd: &mut Command) {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cmd;
}

/// Windows의 오류 창 숨기기에 해당하는 것이 없다
pub struct QuietErrors;

impl QuietErrors {
    pub fn new() -> Self {
        Self
    }
}

/// 프로세스가 끝날 때까지 기다린다 (최대 `timeout`)
pub fn wait_for_process_exit(pid: u32, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        // 신호 0: 보내지 않고 있는지만 확인한다
        if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// PATH에 없을 때 더 찾아볼 흔한 설치 위치
pub fn find_installed_tool(exe: &str) -> Option<PathBuf> {
    ["/usr/local/bin", "/usr/bin", "/snap/bin", "/opt/homebrew/bin"]
        .iter()
        .map(|d| Path::new(d).join(exe))
        .find(|p| p.is_file())
}

// ── 시각 · PC 사양 ──

/// 지금 로컬 시각: [년, 월, 일, 시, 분, 초]
pub fn local_time() -> [u16; 6] {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        [
            (tm.tm_year + 1900) as u16,
            (tm.tm_mon + 1) as u16,
            tm.tm_mday as u16,
            tm.tm_hour as u16,
            tm.tm_min as u16,
            tm.tm_sec as u16,
        ]
    }
}

/// 설치된 메모리 (MB). 알 수 없으면 0.
pub fn total_memory_mb() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(0)
}

/// `dir`이 있는 파일 시스템의 남은 공간 (MB). 폴더가 아직 없으면 있는 윗폴더 기준. 알 수 없으면 0.
pub fn free_disk_mb(dir: &Path) -> u64 {
    use std::os::unix::ffi::OsStrExt;
    let mut p = dir.to_path_buf();
    while !p.exists() {
        match p.parent() {
            Some(q) => p = q.to_path_buf(),
            None => break,
        }
    }
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return 0;
    };
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return 0;
        }
        (s.f_bavail as u64).saturating_mul(s.f_frsize as u64) / 1_048_576
    }
}

/// OS 이름과 그래픽카드 목록
pub struct OsInfo {
    /// 화면에 보여 줄 이름 ("Ubuntu 26.04 LTS (커널 6.8.0)")
    pub name: String,
    pub build: u32,
    /// 그래픽카드: (이름, VRAM 바이트). 여기서는 찾지 않는다: NVIDIA는 nvidia-smi가 따로 찾는다.
    pub adapters: Vec<(String, u64)>,
}

pub fn os_and_adapters() -> OsInfo {
    let pretty = std::fs::read_to_string("/etc/os-release").ok().and_then(|t| {
        t.lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim().trim_matches('"').to_string())
    });
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let name = match (pretty, kernel.is_empty()) {
        (Some(p), false) => format!("{p} (커널 {kernel})"),
        (Some(p), true) => p,
        (None, _) => std::env::consts::OS.to_string(),
    };
    OsInfo {
        name,
        build: 0,
        adapters: Vec::new(),
    }
}

// ── 경로 ──

/// 변경 알림을 믿기 어려운 파일 시스템: 네트워크로 붙인 것, 그리고 다른 OS가 바꾸는 것(WSL의 Windows 드라이브)
fn fs_is_remote(fstype: &str) -> bool {
    matches!(
        fstype,
        "nfs" | "nfs4" | "cifs" | "smb3" | "smbfs" | "9p" | "drvfs" | "afs" | "ceph" | "glusterfs" | "davfs"
            | "fuse.sshfs" | "fuse.rclone" | "fuse.gvfsd-fuse"
    )
}

/// `/proc/mounts` 내용에서 `path`가 놓인 마운트의 파일 시스템 종류 (가장 길게 맞는 마운트 지점)
fn fs_type_of<'a>(path: &str, mounts: &'a str) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for line in mounts.lines() {
        let mut f = line.split_whitespace();
        let (Some(_dev), Some(mp), Some(fstype)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        // 마운트 지점의 공백은 \040으로 적힌다
        let mp = mp.replace("\\040", " ");
        let inside = path == mp || mp == "/" || path.starts_with(&format!("{}/", mp.trim_end_matches('/')));
        if inside && best.is_none_or(|(n, _)| mp.len() > n) {
            best = Some((mp.len(), fstype));
        }
    }
    best.map(|(_, t)| t)
}

/// 네트워크 위치인지 (변경 알림이 오지 않거나 빠질 수 있는 곳)
pub fn is_network_path(path: &str) -> bool {
    let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
        return false;
    };
    fs_type_of(path.trim(), &mounts).is_some_and(fs_is_remote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_mount_a_path_lives_on() {
        let mounts = "\
/dev/sda1 / ext4 rw 0 0
//nas/share /mnt/nas cifs rw 0 0
server:/export /mnt/nfs\\040data nfs4 rw 0 0
C:\\134 /mnt/c 9p rw 0 0
";
        assert_eq!(fs_type_of("/home/me/docs", mounts), Some("ext4"));
        assert_eq!(fs_type_of("/mnt/nas/photos/a.jpg", mounts), Some("cifs"));
        assert_eq!(fs_type_of("/mnt/nas", mounts), Some("cifs"));
        assert_eq!(fs_type_of("/mnt/nfs data/x", mounts), Some("nfs4"));
        assert_eq!(fs_type_of("/mnt/nas2/x", mounts), Some("ext4"));
        assert!(fs_is_remote("cifs") && fs_is_remote("nfs4") && fs_is_remote("9p"));
        assert!(!fs_is_remote("ext4") && !fs_is_remote("btrfs"));
    }

    #[test]
    fn exe_names() {
        assert_eq!(exe_name("ffmpeg"), "ffmpeg");
    }
}
