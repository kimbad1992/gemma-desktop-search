//! 새 버전 확인과 업데이트.
//!
//! - 확인: 하루 한 번(설정에서 끌 수 있음) 또는 "업데이트"를 눌렀을 때 GitHub의 최신 릴리스를 본다.
//!   파일 정보는 보내지 않는다.
//! - 업데이트: 릴리스에서 이 OS용 묶음과 `.sha256`을 받아 해시를 대조하고, 묶음 안의 실행 파일을 꺼내 지금 것과 바꾼다.
//!   - Windows: `…-windows-x64.zip` 안의 `gemma-search.exe`
//!   - Linux x64: `…-linux-x64.tar.gz` 안의 `gemma-search` (릴리스를 올리면 GitHub Actions가 빌드해 붙인다)
//!   실행 중인 파일은 덮어쓸 수 없어도 이름은 바꿀 수 있으므로, 지금 것을 `.old`(Windows는 `.old.exe`)로 옮기고
//!   새 것을 그 자리에 둔다. 다시 시작하면 새 버전이 뜬다. 데이터 폴더와 준비물은 건드리지 않는다.
//!   모두 실행 파일이 있는 폴더 안의 일이라 관리자 권한이 필요 없다.
//! - 해시 확인은 받다가 깨진 파일을 걸러 준다. 실행 파일에 코드 서명이 없으므로, 사용자가 시켰을 때만 바꾼다.

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const REPO: &str = "kimbad1992/gemma-desktop-search";
const EXE_NAME: &str = if cfg!(windows) { "gemma-search.exe" } else { "gemma-search" };
/// 이 OS용 릴리스 묶음 이름의 끝. 없으면 이 OS는 자동으로 바꾸지 않는다 (릴리스 페이지를 안내).
const PACKAGE_SUFFIX: Option<&str> = if cfg!(windows) {
    Some("-windows-x64.zip")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("-linux-x64.tar.gz")
} else {
    None
};
/// 받는 동안 쓰는 임시 폴더 (프로그램 폴더 아래)
const WORK_DIR: &str = "update-tmp";

/// "v0.4.0" → (0, 4, 0)
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut it = s.split(['.', '-']).map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

/// 지금 버전보다 새 태그인지
pub fn is_newer(tag: &str) -> bool {
    match (parse_version(tag), parse_version(env!("CARGO_PKG_VERSION"))) {
        (Some(latest), Some(mine)) => latest > mine,
        _ => false,
    }
}

/// 릴리스에 붙은 이 OS용 묶음(zip, tar.gz)과 그 체크섬 파일
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha_url: String,
}

/// 최신 릴리스
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    /// 릴리스 페이지 주소
    pub page: String,
    /// 받아서 바꿀 수 있는 묶음. 이 OS용 묶음이나 체크섬 파일이 없으면 None (그때는 페이지를 안내한다)
    pub package: Option<Package>,
}

fn release_from_json(v: &serde_json::Value) -> Release {
    release_for(v, PACKAGE_SUFFIX)
}

/// `suffix`로 끝나는 묶음을 고른다 (시험에서 OS와 상관없이 부를 수 있게 나눠 둠)
fn release_for(v: &serde_json::Value, suffix: Option<&str>) -> Release {
    let s = |x: &serde_json::Value| x.as_str().unwrap_or_default().to_string();
    let assets: Vec<(String, String, u64)> = v["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| (s(&x["name"]), s(&x["browser_download_url"]), x["size"].as_u64().unwrap_or(0)))
                .collect()
        })
        .unwrap_or_default();
    // 우리 저장소의 릴리스 파일만 받는다
    let ours = |url: &str| url.starts_with(&format!("https://github.com/{REPO}/releases/download/"));
    let package = suffix.and_then(|suffix| {
        assets
            .iter()
            .find(|(n, _, _)| n.ends_with(suffix) && !n.contains(['/', '\\']))
            .and_then(|(name, url, size)| {
                let sha_name = format!("{name}.sha256");
                let sha_url = assets.iter().find(|(n, _, _)| *n == sha_name)?.1.clone();
                (ours(url) && ours(&sha_url)).then(|| Package {
                    name: name.clone(),
                    url: url.clone(),
                    size: *size,
                    sha_url,
                })
            })
    });
    Release {
        tag: s(&v["tag_name"]),
        page: s(&v["html_url"]),
        package,
    }
}

/// GitHub의 최신 릴리스
pub fn latest() -> Result<Release, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let v: serde_json::Value = agent
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", "gemma-desktop-search")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    Ok(release_from_json(&v))
}

/// 지금 버전보다 새 릴리스가 있으면 (태그, 릴리스 페이지 주소)
pub fn check() -> Result<Option<(String, String)>, String> {
    let r = latest()?;
    Ok(is_newer(&r.tag).then_some((r.tag, r.page)))
}

// ── 업데이트 ──

/// 업데이트가 어디까지 왔는지 (화면에 보여 준다)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Idle,
    Checking,
    /// 이미 최신
    UpToDate,
    Downloading {
        tag: String,
        done: u64,
        total: u64,
    },
    /// 해시 확인이 끝나 exe를 바꾸는 중
    Installing {
        tag: String,
    },
    /// 바꿨다. 다시 시작하면 적용된다.
    Ready {
        tag: String,
    },
    Failed {
        message: String,
        /// 직접 받을 수 있는 릴리스 페이지 (알 때만)
        page: Option<String>,
    },
}

impl Stage {
    /// 진행 중인지 (그동안은 다시 시작할 수 없다)
    pub fn busy(&self) -> bool {
        matches!(self, Stage::Checking | Stage::Downloading { .. } | Stage::Installing { .. })
    }

    /// 한 줄 설명
    pub fn text(&self) -> String {
        match self {
            Stage::Idle => String::new(),
            Stage::Checking => "새 버전을 확인하는 중…".to_string(),
            Stage::UpToDate => format!("최신 버전입니다 (v{})", env!("CARGO_PKG_VERSION")),
            Stage::Downloading { tag, done, total } => {
                let pct = if *total > 0 { *done as f64 / *total as f64 * 100.0 } else { 0.0 };
                format!("{tag} 받는 중 {pct:.0}%")
            }
            Stage::Installing { tag } => format!("{tag} 버전으로 바꾸는 중…"),
            Stage::Ready { tag } => format!("{tag} 버전으로 바꿨습니다. 다시 시작하면 적용됩니다"),
            Stage::Failed { message, .. } => message.clone(),
        }
    }
}

pub enum Outcome {
    UpToDate,
    /// 바꿨다 (새 태그, 이전 exe를 옮겨 둔 곳)
    Updated { tag: String, old: PathBuf },
}

pub struct Failure {
    pub message: String,
    pub page: Option<String>,
}

/// 이 프로세스의 exe 위치. exe를 바꾸기 전에 한 번 불러 기억해 둔다 (다시 시작할 때 같은 자리의 새 exe를 띄운다).
pub fn exe_path() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| std::env::current_exe().unwrap_or_else(|_| PathBuf::from(EXE_NAME)))
        .clone()
}

/// 확인 → 받기 → 해시 대조 → exe 교체. 단계가 바뀔 때마다 `report`를 부른다.
pub fn run(report: &dyn Fn(Stage)) -> Result<Outcome, Failure> {
    report(Stage::Checking);
    let rel = latest().map_err(|e| Failure {
        message: format!("새 버전을 확인하지 못했습니다: {e}"),
        page: None,
    })?;
    if !is_newer(&rel.tag) {
        return Ok(Outcome::UpToDate);
    }
    let page = (!rel.page.is_empty()).then(|| rel.page.clone());
    let fail = |message: String| Failure {
        message,
        page: page.clone(),
    };
    let Some(pkg) = rel.package.clone() else {
        return Err(fail(match PACKAGE_SUFFIX {
            None => format!("이 OS용 자동 업데이트는 없습니다. {} 릴리스 페이지에서 받아 주세요.", rel.tag),
            Some(_) if !cfg!(windows) => format!(
                "{} 릴리스에 아직 이 OS용 파일이 없습니다. Linux판은 릴리스가 올라간 뒤 몇 분 안에 붙으니 잠시 뒤 다시 해 보세요.",
                rel.tag
            ),
            Some(_) => format!("{} 릴리스에 자동으로 받을 파일이 없습니다. 릴리스 페이지에서 받아 주세요.", rel.tag),
        }));
    };
    let exe = exe_path();
    let Some(dir) = exe.parent().map(Path::to_path_buf) else {
        return Err(fail("실행 파일이 있는 폴더를 알 수 없습니다.".to_string()));
    };
    // 개발 중인 빌드(cargo의 target 폴더)는 바꾸지 않는다
    if crate::config::app_dir() != dir {
        return Err(fail("개발 빌드(target 폴더)에서는 업데이트하지 않습니다.".to_string()));
    }
    let work = dir.join(WORK_DIR);
    if let Err(e) = std::fs::create_dir_all(&work) {
        return Err(fail(format!(
            "프로그램 폴더({})에 쓸 수 없어 자동으로 바꾸지 못했습니다: {e}",
            dir.display()
        )));
    }
    let result = install(&rel.tag, &pkg, &exe, &work, report);
    // 임시 폴더는 우리가 만든 파일만 지운다 (install이 지움). 비었으면 폴더도 지운다.
    let _ = std::fs::remove_dir(&work);
    match result {
        Ok(old) => Ok(Outcome::Updated { tag: rel.tag, old }),
        Err(m) => Err(fail(m)),
    }
}

fn install(tag: &str, pkg: &Package, exe: &Path, work: &Path, report: &dyn Fn(Stage)) -> Result<PathBuf, String> {
    let pkg_path = work.join(&pkg.name);
    let new_exe = work.join(if cfg!(windows) { "gemma-search.new.exe" } else { "gemma-search.new" });
    let result = (|| {
        let want = fetch_sha(&pkg.sha_url)?;
        download(tag, pkg, &pkg_path, &want, report)?;
        report(Stage::Installing { tag: tag.to_string() });
        extract_exe(&pkg_path, &new_exe)?;
        swap_exe(exe, &new_exe)
    })();
    let _ = std::fs::remove_file(&pkg_path);
    let _ = std::fs::remove_file(part_path(&pkg_path));
    let _ = std::fs::remove_file(&new_exe);
    result
}

/// 받는 동안 쓰는 파일: 이름 뒤에 `.part`
fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .build()
        .into()
}

/// 체크섬 파일의 첫 낱말이 SHA-256(16진수 64자)이면 그것을 소문자로
fn parse_sha(text: &str) -> Option<String> {
    let h = text.split_whitespace().next()?.to_ascii_lowercase();
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some(h)
}

fn fetch_sha(url: &str) -> Result<String, String> {
    let text = download_agent()
        .get(url)
        .header("User-Agent", "gemma-desktop-search")
        .call()
        .map_err(|e| format!("체크섬 파일을 받지 못했습니다: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("체크섬 파일을 읽지 못했습니다: {e}"))?;
    parse_sha(&text).ok_or_else(|| "체크섬 파일의 형식이 올바르지 않습니다.".to_string())
}

/// 묶음을 받아 해시와 크기를 대조한다. 맞지 않으면 받은 파일을 지우고 실패.
fn download(tag: &str, pkg: &Package, dest: &Path, want_sha: &str, report: &dyn Fn(Stage)) -> Result<(), String> {
    let part = part_path(dest);
    let io = |e: std::io::Error| format!("받은 파일을 쓰지 못했습니다: {e}");
    let mut resp = download_agent()
        .get(&pkg.url)
        .header("User-Agent", "gemma-desktop-search")
        .call()
        .map_err(|e| format!("{}을 받지 못했습니다: {e}", pkg.name))?;
    let mut out = std::fs::File::create(&part).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let progress = |done: u64| {
        report(Stage::Downloading {
            tag: tag.to_string(),
            done,
            total: pkg.size,
        })
    };
    progress(0);
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 18];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("받는 중에 끊겼습니다: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(io)?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done);
    }
    out.flush().map_err(io)?;
    drop(out);
    let got = format!("{:x}", hasher.finalize());
    if got != want_sha || (pkg.size > 0 && done != pkg.size) {
        let _ = std::fs::remove_file(&part);
        return Err("받은 파일의 체크섬이 맞지 않습니다. 받은 파일을 지웠습니다.".to_string());
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&part, dest).map_err(io)
}

/// 묶음에서 실행 파일만 꺼낸다 (README·라이선스는 두고): Windows는 zip, Linux는 tar.gz
fn extract_exe(pkg_path: &Path, dest: &Path) -> Result<(), String> {
    if pkg_path.to_string_lossy().ends_with(".tar.gz") {
        extract_from_tar_gz(pkg_path, dest)
    } else {
        extract_from_zip(pkg_path, dest)
    }
}

/// tar.gz에서 `gemma-search`를 꺼낸다. Linux 실행 파일(ELF)인지 보고, 이 계정만 쓰고 모두 실행할 수 있는 권한(755)으로 둔다.
#[cfg(unix)]
fn extract_from_tar_gz(pkg_path: &Path, dest: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let bad = |e: String| format!("받은 tar.gz를 풀지 못했습니다: {e}");
    let f = std::io::BufReader::new(std::fs::File::open(pkg_path).map_err(|e| bad(e.to_string()))?);
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for entry in ar.entries().map_err(|e| bad(e.to_string()))? {
        let mut entry = entry.map_err(|e| bad(e.to_string()))?;
        let is_ours = entry.header().entry_type().is_file()
            && entry
                .path()
                .ok()
                .and_then(|p| p.file_name().map(|n| n == EXE_NAME))
                .unwrap_or(false);
        if !is_ours {
            continue;
        }
        let mut out = std::fs::File::create(dest).map_err(|e| bad(e.to_string()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| bad(e.to_string()))?;
        out.flush().map_err(|e| bad(e.to_string()))?;
        drop(out);
        let mut head = [0u8; 4];
        let ok = std::fs::File::open(dest)
            .and_then(|mut f| f.read_exact(&mut head))
            .is_ok()
            && &head == b"\x7fELF"
            && std::fs::metadata(dest).map(|m| m.len() > 1 << 20).unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_file(dest);
            return Err("묶음 안의 실행 파일이 올바르지 않습니다.".to_string());
        }
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755)).map_err(|e| bad(e.to_string()))?;
        return Ok(());
    }
    Err(format!("묶음 안에 {EXE_NAME}가 없습니다."))
}

#[cfg(not(unix))]
fn extract_from_tar_gz(_pkg_path: &Path, _dest: &Path) -> Result<(), String> {
    Err("이 OS에서는 tar.gz 묶음을 풀지 않습니다.".to_string())
}

fn extract_from_zip(zip_path: &Path, dest: &Path) -> Result<(), String> {
    let bad = |e: String| format!("받은 zip을 풀지 못했습니다: {e}");
    let f = std::fs::File::open(zip_path).map_err(|e| bad(e.to_string()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| bad(e.to_string()))?;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| bad(e.to_string()))?;
        // 이름을 읽을 수 없는 항목은 건너뛴다
        let Ok(name) = entry.name().map(|n| n.replace('\\', "/")) else { continue };
        if !name.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case(EXE_NAME)) {
            continue;
        }
        let mut out = std::fs::File::create(dest).map_err(|e| bad(e.to_string()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| bad(e.to_string()))?;
        out.flush().map_err(|e| bad(e.to_string()))?;
        drop(out);
        // 최소한의 확인: Windows 실행 파일이고 크기가 말이 되는지
        let mut head = [0u8; 2];
        let ok = std::fs::File::open(dest)
            .and_then(|mut f| f.read_exact(&mut head))
            .is_ok()
            && &head == b"MZ"
            && std::fs::metadata(dest).map(|m| m.len() > 1 << 20).unwrap_or(false);
        if !ok {
            let _ = std::fs::remove_file(dest);
            return Err("zip 안의 실행 파일이 올바르지 않습니다.".to_string());
        }
        return Ok(());
    }
    Err(format!("zip 안에 {EXE_NAME}가 없습니다."))
}

/// 지금 실행 파일을 `….old.exe`(Linux는 `….old`)로 옮기고 새 것을 그 자리에 둔다. 이전 것을 옮겨 둔 곳을 돌려준다.
/// 새 것을 놓지 못하면 이전 것을 제자리로 되돌린다.
pub fn swap_exe(current: &Path, new: &Path) -> Result<PathBuf, String> {
    let old = if cfg!(windows) {
        current.with_extension("old.exe")
    } else {
        let mut s = current.as_os_str().to_os_string();
        s.push(".old");
        PathBuf::from(s)
    };
    if old.exists() {
        std::fs::remove_file(&old).map_err(|e| format!("이전 백업({})을 지울 수 없습니다: {e}", old.display()))?;
    }
    // 실행 중인 exe도 이름은 바꿀 수 있다
    std::fs::rename(current, &old)
        .map_err(|e| format!("지금 실행 파일을 옮길 수 없습니다 (폴더에 쓰기 권한이 있는지 확인하세요): {e}"))?;
    let placed = std::fs::rename(new, current).or_else(|_| std::fs::copy(new, current).map(|_| ()));
    match placed {
        Ok(()) => {
            let _ = std::fs::remove_file(new);
            Ok(old)
        }
        Err(e) => {
            let _ = std::fs::rename(&old, current);
            Err(format!("새 실행 파일을 놓지 못했습니다: {e}"))
        }
    }
}

pub type Shared = Arc<Mutex<Stage>>;

/// 백그라운드로 업데이트를 시작한다 (확인부터). 이미 진행 중이면 아무 일도 하지 않는다.
pub fn start(state: Shared, wake: crate::util::Waker) {
    {
        let mut g = state.lock();
        if g.busy() {
            return;
        }
        *g = Stage::Checking;
    }
    let spawned = std::thread::Builder::new().name("self-update".into()).spawn({
        let state = state.clone();
        move || {
            let report = |s: Stage| {
                *state.lock() = s;
                wake();
            };
            let end = match run(&report) {
                Ok(Outcome::UpToDate) => Stage::UpToDate,
                Ok(Outcome::Updated { tag, .. }) => Stage::Ready { tag },
                Err(f) => Stage::Failed {
                    message: f.message,
                    page: f.page,
                },
            };
            report(end);
        }
    });
    if let Err(e) = spawned {
        *state.lock() = Stage::Failed {
            message: format!("업데이트를 시작하지 못했습니다: {e}"),
            page: None,
        };
    }
}

/// 새 exe로 다시 시작한다. 새 프로세스는 이 프로세스가 끝나기를 기다렸다가 뜬다 (`--after-update <pid>`, main.rs).
/// 같은 데이터 폴더로는 앱이 하나만 뜨므로, 기다리지 않으면 "이미 실행 중"으로 보고 바로 끝난다.
/// `background`면 창 없이 트레이로 뜬다.
pub fn spawn_restart(background: bool) -> Result<(), String> {
    let mut cmd = std::process::Command::new(exe_path());
    cmd.arg("--after-update").arg(std::process::id().to_string());
    if background {
        cmd.arg("--background");
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// 명령줄 업데이트: `gemma-search update [--check] [--restart] [--unit <이름>]`
/// (예전 이름 `--update`, `--check-update`도 받는다)
/// - 새 버전이 있으면 받아서 실행 파일을 바꾼다. 관리자 권한은 쓰지 않는다.
/// - Linux에서 서비스가 떠 있으면 다시 띄우는 방법을 알려 준다. `--restart`면 systemd 사용자 서비스(`--unit`, 기본
///   gemma-search)를 다시 띄운다.
pub fn cli(args: &[String]) -> i32 {
    if crate::cli::wants_help(args) {
        println!("{}", crate::service::USAGE);
        return 0;
    }
    let a = match crate::cli::parse_args(args, &["--unit"], &["--check", "--restart"], false) {
        Ok(a) => a,
        Err(e) => return crate::cli::usage_error(&e),
    };
    let unit = a.value("--unit").unwrap_or("gemma-search").trim().to_string();
    println!("지금 버전 v{}", env!("CARGO_PKG_VERSION"));
    if a.flag("--check") {
        return match check() {
            Ok(Some((tag, page))) => {
                println!("새 버전 {tag}이 있습니다: {page}");
                println!("바꾸려면: {} update", me());
                0
            }
            Ok(None) => {
                println!("최신 버전입니다.");
                0
            }
            Err(e) => {
                eprintln!("새 버전을 확인하지 못했습니다: {e}");
                1
            }
        };
    }
    let report = |s: Stage| match s {
        Stage::Checking => println!("새 버전 확인 중…"),
        Stage::Downloading { tag, done: 0, total } => {
            println!("{tag} 받는 중 ({:.1}MB)…", total as f64 / 1_000_000.0)
        }
        Stage::Installing { tag } => println!("체크섬 확인 완료. {tag} 버전으로 바꾸는 중…"),
        _ => {}
    };
    match run(&report) {
        Ok(Outcome::UpToDate) => {
            println!("최신 버전입니다.");
            0
        }
        Ok(Outcome::Updated { tag, old }) => {
            println!("{tag} 버전으로 바꿨습니다. 이전 실행 파일: {}", old.display());
            if cfg!(windows) {
                println!("다음에 실행하면 새 버전이 뜹니다.");
                0
            } else {
                after_update(&tag, a.flag("--restart"), &unit)
            }
        }
        Err(f) => {
            eprintln!("실패: {}", f.message);
            if let Some(p) = f.page {
                eprintln!("직접 받기: {p}");
            }
            1
        }
    }
}

/// 안내에 쓸 이 프로그램의 이름: 경로로 실행했으면 그 경로, 아니면 gemma-search
fn me() -> String {
    std::env::args()
        .next()
        .filter(|a| a.contains('/') || a.contains('\\'))
        .unwrap_or_else(|| "gemma-search".to_string())
}

/// systemd 사용자 서비스가 떠 있는지 (`systemctl --user is-active`)
fn user_unit_active(unit: &str) -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .status()
        .is_ok_and(|s| s.success())
}

/// Linux: 바꾼 뒤 서비스가 새 버전으로 돌게 한다 (`restart`면 systemd 사용자 서비스를 다시 띄우고, 아니면 방법을 알려 준다)
fn after_update(tag: &str, restart: bool, unit: &str) -> i32 {
    let unit_active = user_unit_active(unit);
    let data = crate::config::data_dir();
    let locked = data.is_dir()
        && crate::config::instance_lock(&data).is_err_and(|e| e.starts_with(crate::config::LOCK_BUSY));
    if !unit_active && !locked {
        println!("떠 있는 서비스가 없습니다. 다음에 serve를 띄우면 {tag} 버전이 돕니다.");
        return 0;
    }
    if restart {
        if !unit_active {
            eprintln!(
                "systemd 사용자 서비스 {unit}이(가) 떠 있지 않아 다시 띄우지 못했습니다. 떠 있는 serve를 끄고 다시 실행하세요."
            );
            return 1;
        }
        let ok = std::process::Command::new("systemctl")
            .args(["--user", "restart", unit])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            println!("서비스 {unit}을(를) 다시 띄웠습니다. 이제 {tag} 버전이 돕니다.");
            return 0;
        }
        eprintln!("서비스 {unit}을(를) 다시 띄우지 못했습니다. `systemctl --user status {unit}`으로 확인하세요.");
        return 1;
    }
    println!("서비스가 아직 예전 버전으로 돌고 있습니다. 다시 띄워야 {tag} 버전이 돕니다:");
    if unit_active {
        println!("  systemctl --user restart {unit}     (또는 {} update --restart)", me());
    } else {
        println!("  떠 있는 serve를 끄고(Ctrl+C) 다시 실행하세요.");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.4.0"), Some((0, 4, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert!(parse_version("v0.10.0") > parse_version("v0.9.3"));
        assert_eq!(parse_version("latest"), None);
        assert!(is_newer("v999.0.0"));
        assert!(!is_newer("v0.0.1"));
        assert!(!is_newer(&format!("v{}", env!("CARGO_PKG_VERSION"))));
        assert!(!is_newer("latest"));
    }

    const WIN: Option<&str> = Some("-windows-x64.zip");
    const LINUX: Option<&str> = Some("-linux-x64.tar.gz");

    #[test]
    fn picks_this_os_package_and_checksum_from_release() {
        let base = format!("https://github.com/{REPO}/releases/download/v9.9.9");
        let asset = |name: &str, size: u64| {
            serde_json::json!({ "name": name, "size": size, "browser_download_url": format!("{base}/{name}") })
        };
        let v = serde_json::json!({
            "tag_name": "v9.9.9",
            "html_url": format!("https://github.com/{REPO}/releases/tag/v9.9.9"),
            "assets": [
                asset("gemma-desktop-search-v9.9.9-windows-x64.zip.sha256", 110),
                asset("gemma-desktop-search-v9.9.9-windows-x64.zip", 14103601),
                asset("gemma-desktop-search-v9.9.9-linux-x64.tar.gz", 6200000),
                asset("gemma-desktop-search-v9.9.9-linux-x64.tar.gz.sha256", 112),
            ],
        });
        let r = release_for(&v, WIN);
        assert_eq!(r.tag, "v9.9.9");
        let zip = r.package.expect("zip");
        assert_eq!(zip.name, "gemma-desktop-search-v9.9.9-windows-x64.zip");
        assert_eq!(zip.size, 14103601);
        assert!(zip.url.ends_with("x64.zip"));
        assert!(zip.sha_url.ends_with("x64.zip.sha256"));
        let tgz = release_for(&v, LINUX).package.expect("tar.gz");
        assert_eq!(tgz.name, "gemma-desktop-search-v9.9.9-linux-x64.tar.gz");
        assert!(tgz.sha_url.ends_with("linux-x64.tar.gz.sha256"));
        // 이 OS용 묶음이 정해져 있지 않으면 받지 않는다
        assert_eq!(release_for(&v, None).package, None);

        // 체크섬 파일이 없거나, 다른 곳을 가리키는 주소면 자동으로 받지 않는다
        let mut no_sha = v.clone();
        no_sha["assets"].as_array_mut().unwrap().remove(0);
        assert_eq!(release_for(&no_sha, WIN).package, None);
        let mut elsewhere = v.clone();
        elsewhere["assets"][2]["browser_download_url"] = "https://example.com/x-linux-x64.tar.gz".into();
        assert_eq!(release_for(&elsewhere, LINUX).package, None);
        assert_eq!(release_for(&serde_json::json!({ "tag_name": "v1.0.0" }), WIN).package, None);
        // Linux 묶음만 있는 릴리스(Actions가 아직 안 붙였거나 반대)
        let mut only_win = v.clone();
        only_win["assets"].as_array_mut().unwrap().truncate(2);
        assert_eq!(release_for(&only_win, LINUX).package, None);
    }

    #[test]
    fn reads_checksum_file() {
        let h = "4d2bdfeaf027f51a1512af61385f981be157a3f556d397d1d860167fecd64f13";
        assert_eq!(parse_sha(&format!("{h}  some-file.zip\n")).as_deref(), Some(h));
        assert_eq!(parse_sha(&h.to_uppercase()).as_deref(), Some(h));
        assert_eq!(parse_sha("not a hash"), None);
        assert_eq!(parse_sha(""), None);
    }

    #[test]
    fn swaps_exe_and_keeps_the_old_one() {
        let dir = std::env::temp_dir().join(format!("gemma-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join(EXE_NAME);
        let new = dir.join("gemma-search.new");
        let backup = dir.join(if cfg!(windows) { "gemma-search.old.exe" } else { "gemma-search.old" });
        std::fs::write(&current, b"old version").unwrap();
        std::fs::write(&new, b"new version").unwrap();
        // 예전 백업이 남아 있어도 된다
        std::fs::write(&backup, b"older").unwrap();

        let old = swap_exe(&current, &new).unwrap();
        assert_eq!(old, backup);
        assert_eq!(std::fs::read(&current).unwrap(), b"new version");
        assert_eq!(std::fs::read(&old).unwrap(), b"old version");
        assert!(!new.exists());

        // 새 파일이 없으면 실패하고, 지금 exe는 제자리에 그대로 있다
        assert!(swap_exe(&current, &dir.join("missing.exe")).is_err());
        assert_eq!(std::fs::read(&current).unwrap(), b"new version");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 로컬 HTTP 서버로 Linux 묶음을 내주고 실제로 받아서 바꿔 본다 (받기 → 체크섬 → 풀기 → 바꾸기)
    #[cfg(unix)]
    #[test]
    fn installs_linux_package_end_to_end() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gemma-update-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("work")).unwrap();
        // 가짜 새 실행 파일 (ELF 머리 + 2MB)
        let mut elf = b"\x7fELF".to_vec();
        elf.resize(2 << 20, 7);
        let tgz = {
            let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
            for (name, data) in [("README.md", &b"readme"[..]), ("gemma-search", &elf[..])] {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o4755);
                b.append_data(&mut h, name, data).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap()
        };
        let sha = format!("{:x}", Sha256::digest(&tgz));
        // 요청 n개에 답하는 서버
        let serve = |files: Vec<(&'static str, Vec<u8>)>, n: usize| {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let port = server.server_addr().to_ip().unwrap().port();
            std::thread::spawn(move || {
                for _ in 0..n {
                    let Ok(req) = server.recv() else { return };
                    let body = files.iter().find(|(p, _)| *p == req.url()).map(|(_, b)| b.clone()).unwrap_or_default();
                    let _ = req.respond(tiny_http::Response::from_data(body));
                }
            });
            port
        };
        let pkg = |port: u16, size: u64| Package {
            name: "gemma-desktop-search-v9.9.9-linux-x64.tar.gz".into(),
            url: format!("http://127.0.0.1:{port}/p.tar.gz"),
            size,
            sha_url: format!("http://127.0.0.1:{port}/p.tar.gz.sha256"),
        };
        let current = dir.join("gemma-search");
        std::fs::write(&current, b"old version").unwrap();

        // 체크섬이 맞지 않으면 바꾸지 않는다
        let port = serve(vec![("/p.tar.gz.sha256", format!("{}  x\n", "0".repeat(64)).into_bytes()), ("/p.tar.gz", tgz.clone())], 2);
        assert!(install("v9.9.9", &pkg(port, tgz.len() as u64), &current, &dir.join("work"), &|_| {}).is_err());
        assert_eq!(std::fs::read(&current).unwrap(), b"old version");

        // 맞으면 바꾸고, 이전 것은 .old로 남긴다. 권한은 755 (setuid 등은 버린다)
        let port = serve(vec![("/p.tar.gz.sha256", format!("{sha}  x\n").into_bytes()), ("/p.tar.gz", tgz.clone())], 2);
        let old = install("v9.9.9", &pkg(port, tgz.len() as u64), &current, &dir.join("work"), &|_| {}).unwrap();
        assert_eq!(old, dir.join("gemma-search.old"));
        assert_eq!(std::fs::read(&current).unwrap(), elf);
        assert_eq!(std::fs::read(&old).unwrap(), b"old version");
        assert_eq!(std::fs::metadata(&current).unwrap().permissions().mode() & 0o7777, 0o755);
        // 임시 파일은 남지 않는다
        assert_eq!(std::fs::read_dir(dir.join("work")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
