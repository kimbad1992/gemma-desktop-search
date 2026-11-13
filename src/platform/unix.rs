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

// ── 데이터 위치 (location.rs) ──

/// 데이터의 고정 위치. Linux는 `$XDG_DATA_HOME/gemma-search`(없으면 `~/.local/share/gemma-search`),
/// macOS는 `~/Library/Application Support/GemmaSearch`. 홈 폴더를 알 수 없으면 None (그때는 실행 파일 옆을 쓴다).
/// 상대 경로로 적힌 환경 변수는 무시한다 (XDG 규칙).
pub fn data_home() -> Option<PathBuf> {
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    let home = std::env::var_os("HOME").and_then(absolute);
    if cfg!(target_os = "macos") {
        return home.map(|h| h.join("Library").join("Application Support").join("GemmaSearch"));
    }
    std::env::var_os("XDG_DATA_HOME")
        .and_then(absolute)
        .or_else(|| home.map(|h| h.join(".local").join("share")))
        .map(|d| d.join("gemma-search"))
}

/// 두 경로가 같은 파일 시스템에 있는지 (같으면 폴더 이름만 바꿔 옮길 수 있다). 아직 없는 경로는 있는 윗폴더로 본다.
pub fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    fn device(p: &Path) -> Option<u64> {
        p.ancestors().find_map(|q| std::fs::metadata(q).ok()).map(|m| m.dev())
    }
    matches!((device(a), device(b)), (Some(x), Some(y)) if x == y)
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
    /// 그래픽카드: (이름, VRAM 바이트). Vulkan으로 쓸 수 있는 것만 넣는다 (`vulkan_gpu`).
    /// NVIDIA는 nvidia-smi가 따로 찾아 이름·VRAM·드라이버를 덮어쓴다.
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
        adapters: vulkan_gpu().ok().into_iter().collect(),
    }
}

/// Vulkan으로 쓸 수 있는 그래픽 장치. 아무것도 바꾸지 않고 보기만 한다.
/// 그래픽 장치(/dev/dri/renderD*)에 이 계정이 접근할 수 있고, Vulkan 라이브러리와 드라이버(ICD)가 깔려 있어야 한다.
/// 쓸 수 있으면 (이름, 쓸 수 있는 메모리 바이트), 아니면 쓸 수 없는 까닭들.
/// 메모리: 전용 메모리를 알려 주는 카드(AMD)는 그 값, 내장 그래픽은 시스템 메모리를 나눠 쓰므로 설치된 메모리의 절반.
pub fn vulkan_gpu() -> Result<(String, u64), Vec<String>> {
    let nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
        .map(|d| {
            let mut v: Vec<PathBuf> = d
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("renderD")))
                .collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    if nodes.is_empty() {
        return Err(vec!["그래픽 장치(/dev/dri/renderD*)가 보이지 않습니다.".to_string()]);
    }
    let usable = nodes.iter().find(|n| std::fs::OpenOptions::new().read(true).write(true).open(n).is_ok());
    let mut why = Vec::new();
    if usable.is_none() {
        let user = std::env::var("USER").unwrap_or_else(|_| "<계정>".into());
        why.push(format!(
            "이 계정이 그래픽 장치({})를 쓸 수 없습니다. 그래픽을 쓰려면 관리자가 이 계정을 render 그룹에 넣어야 합니다 \
             (예: sudo usermod -aG render {user}, 그다음 다시 로그인).",
            nodes[0].display()
        ));
    }
    let lib = [
        "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
        "/lib/x86_64-linux-gnu/libvulkan.so.1",
        "/usr/lib64/libvulkan.so.1",
        "/usr/lib/libvulkan.so.1",
    ]
    .iter()
    .any(|p| Path::new(p).exists());
    let icd = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d", "/usr/local/share/vulkan/icd.d"].iter().any(|d| {
        std::fs::read_dir(d)
            .map(|r| r.flatten().any(|e| e.path().extension().is_some_and(|x| x == "json")))
            .unwrap_or(false)
    });
    if !lib || !icd {
        why.push(
            "Vulkan 라이브러리나 그래픽 드라이버가 없습니다. 그래픽을 쓰려면 관리자가 설치해야 합니다 \
             (Ubuntu: sudo apt install libvulkan1 mesa-vulkan-drivers)."
                .to_string(),
        );
    }
    let Some(node) = usable.filter(|_| why.is_empty()) else {
        return Err(why);
    };
    let dev = Path::new("/sys/class/drm").join(node.file_name().unwrap_or_default()).join("device");
    let vendor = std::fs::read_to_string(dev.join("vendor")).unwrap_or_default();
    let name = match vendor.trim() {
        "0x8086" => "Intel 그래픽",
        "0x1002" => "AMD 그래픽",
        "0x10de" => "NVIDIA 그래픽",
        _ => "그래픽",
    };
    let mem = std::fs::read_to_string(dev.join("mem_info_vram_total"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|b| *b > 0)
        .unwrap_or(total_memory_mb() / 2 * 1_048_576);
    Ok((name.to_string(), mem))
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

// ── 메시: SMB 마운트, 주소 ──

/// `/proc/mounts`의 `\040` 같은 8진수 표기를 푼다
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `/proc/mounts` 내용에서 `path`가 놓인 SMB(cifs) 마운트: (원격 이름 `//서버/공유[/아래]`, 마운트 지점)
fn smb_mount_in(path: &str, mounts: &str) -> Option<(String, String)> {
    let mut best: Option<(String, String)> = None;
    for line in mounts.lines() {
        let mut f = line.split_whitespace();
        let (Some(dev), Some(mp), Some(fstype)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if !matches!(fstype, "cifs" | "smb3" | "smbfs") {
            continue;
        }
        let mp = unescape_mount(mp);
        let inside = path == mp || path.starts_with(&format!("{}/", mp.trim_end_matches('/')));
        if inside && best.as_ref().is_none_or(|(_, m)| mp.len() > m.len()) {
            best = Some((unescape_mount(dev), mp));
        }
    }
    best
}

/// `path`가 놓인 SMB(cifs) 마운트: (원격 이름 `//서버/공유[/아래]`, 마운트 지점). 아니면 None.
pub fn smb_mount_of(path: &str) -> Option<(String, String)> {
    let mounts = std::fs::read_to_string("/proc/mounts").ok()?;
    smb_mount_in(path.trim(), &mounts)
}

/// 이 기기 안에서만 쓰는 가상 네트워크 장치인지 (Docker·가상 머신·컨테이너 브리지). 다른 기기는 그 주소로 닿지
/// 못하므로 알리지 않는다. VPN(tailscale0, wg0, zt…)과 실제 장치는 남긴다.
fn internal_interface(name: &str) -> bool {
    const PREFIXES: [&str; 12] =
        ["docker", "br-", "veth", "virbr", "vnet", "cni", "flannel", "podman", "lxcbr", "lxdbr", "vmnet", "vboxnet"];
    PREFIXES.iter().any(|p| name.starts_with(p))
}

/// 이 기기의 IP 주소들 (다른 기기가 닿을 수 있는 네트워크 장치: 가상 브리지는 뺀다)
pub fn local_addresses() -> Vec<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        let name = if ifa.ifa_name.is_null() {
            String::new()
        } else {
            unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }.to_string_lossy().into_owned()
        };
        if !ifa.ifa_addr.is_null() && !internal_interface(&name) {
            let family = unsafe { (*ifa.ifa_addr).sa_family } as i32;
            if family == libc::AF_INET {
                let sa = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                out.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr))));
            } else if family == libc::AF_INET6 {
                let sa = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                out.push(IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr)));
            }
        }
        cur = ifa.ifa_next;
    }
    unsafe { libc::freeifaddrs(ifap) };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_smb_mount_of_a_path() {
        let mounts = "\
/dev/sda1 / ext4 rw 0 0
//leeubuntu/\\352\\263\\265\\354\\234\\240 /mnt/home\\040nas cifs rw 0 0
//nas/media/photos /mnt/photos smb3 rw 0 0
server:/export /mnt/nfs nfs4 rw 0 0
";
        assert_eq!(
            smb_mount_in("/mnt/home nas/사진/a.jpg", mounts),
            Some(("//leeubuntu/공유".to_string(), "/mnt/home nas".to_string()))
        );
        assert_eq!(
            smb_mount_in("/mnt/photos", mounts),
            Some(("//nas/media/photos".to_string(), "/mnt/photos".to_string()))
        );
        assert_eq!(smb_mount_in("/mnt/nfs/x", mounts), None);
        assert_eq!(smb_mount_in("/home/me", mounts), None);
        assert!(!local_addresses().is_empty() || std::env::var("CI").is_ok());
        // Docker·가상 머신 브리지는 알리지 않고, VPN과 실제 장치는 알린다
        for n in ["docker0", "br-127a87fa168c", "veth1a2b", "virbr0", "vboxnet0"] {
            assert!(internal_interface(n), "{n}");
        }
        for n in ["tailscale0", "lan0", "eth0", "enp3s0", "wlan0", "wg0", "zt5u4y6"] {
            assert!(!internal_interface(n), "{n}");
        }
    }

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
