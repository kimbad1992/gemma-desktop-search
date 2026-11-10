//! 새 버전 확인과 업데이트.
//!
//! - 확인: 하루 한 번(설정에서 끌 수 있음) 또는 "업데이트"를 눌렀을 때 GitHub의 최신 릴리스를 본다.
//!   파일 정보는 보내지 않는다.
//! - 업데이트: 릴리스의 zip과 `.sha256`을 받아 해시를 대조하고, zip 안의 exe를 꺼내 지금 exe와 바꾼다.
//!   실행 중인 exe는 덮어쓸 수 없지만 이름은 바꿀 수 있으므로, 지금 exe를 `gemma-search.old.exe`로 옮기고
//!   새 exe를 그 자리에 둔다. 다시 시작하면 새 버전이 뜬다. `data\` 폴더는 건드리지 않는다.
//! - 해시 확인은 받다가 깨진 파일을 걸러 준다. exe에 코드 서명이 없으므로, 사용자가 눌렀을 때만 바꾼다.

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const REPO: &str = "kimbad1992/gemma-desktop-search";
const EXE_NAME: &str = "gemma-search.exe";
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

/// 릴리스에 붙은 Windows용 zip과 그 체크섬 파일
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipAsset {
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
    /// 받아서 바꿀 수 있는 파일. zip이나 체크섬 파일이 없으면 None (그때는 페이지를 안내한다)
    pub zip: Option<ZipAsset>,
}

fn release_from_json(v: &serde_json::Value) -> Release {
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
    let zip = assets
        .iter()
        .find(|(n, _, _)| n.ends_with("-windows-x64.zip") && !n.contains(['/', '\\']))
        .and_then(|(name, url, size)| {
            let sha_name = format!("{name}.sha256");
            let sha_url = assets.iter().find(|(n, _, _)| *n == sha_name)?.1.clone();
            (ours(url) && ours(&sha_url)).then(|| ZipAsset {
                name: name.clone(),
                url: url.clone(),
                size: *size,
                sha_url,
            })
        });
    Release {
        tag: s(&v["tag_name"]),
        page: s(&v["html_url"]),
        zip,
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
    let Some(zip) = rel.zip.clone() else {
        return Err(fail(format!(
            "{} 릴리스에 자동으로 받을 파일이 없습니다. 릴리스 페이지에서 받아 주세요.",
            rel.tag
        )));
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
    let result = install(&rel.tag, &zip, &exe, &work, report);
    // 임시 폴더는 우리가 만든 파일만 지운다 (install이 지움). 비었으면 폴더도 지운다.
    let _ = std::fs::remove_dir(&work);
    match result {
        Ok(old) => Ok(Outcome::Updated { tag: rel.tag, old }),
        Err(m) => Err(fail(m)),
    }
}

fn install(tag: &str, zip: &ZipAsset, exe: &Path, work: &Path, report: &dyn Fn(Stage)) -> Result<PathBuf, String> {
    let zip_path = work.join(&zip.name);
    let new_exe = work.join("gemma-search.new.exe");
    let result = (|| {
        let want = fetch_sha(&zip.sha_url)?;
        download(tag, zip, &zip_path, &want, report)?;
        report(Stage::Installing { tag: tag.to_string() });
        extract_exe(&zip_path, &new_exe)?;
        swap_exe(exe, &new_exe)
    })();
    let _ = std::fs::remove_file(&zip_path);
    let _ = std::fs::remove_file(zip_path.with_extension("zip.part"));
    let _ = std::fs::remove_file(&new_exe);
    result
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

/// zip을 받아 해시와 크기를 대조한다. 맞지 않으면 받은 파일을 지우고 실패.
fn download(tag: &str, zip: &ZipAsset, dest: &Path, want_sha: &str, report: &dyn Fn(Stage)) -> Result<(), String> {
    let part = dest.with_extension("zip.part");
    let io = |e: std::io::Error| format!("받은 파일을 쓰지 못했습니다: {e}");
    let mut resp = download_agent()
        .get(&zip.url)
        .header("User-Agent", "gemma-desktop-search")
        .call()
        .map_err(|e| format!("{}을 받지 못했습니다: {e}", zip.name))?;
    let mut out = std::fs::File::create(&part).map_err(io)?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let progress = |done: u64| {
        report(Stage::Downloading {
            tag: tag.to_string(),
            done,
            total: zip.size,
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
    if got != want_sha || (zip.size > 0 && done != zip.size) {
        let _ = std::fs::remove_file(&part);
        return Err("받은 파일의 체크섬이 맞지 않습니다. 받은 파일을 지웠습니다.".to_string());
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&part, dest).map_err(io)
}

/// zip에서 실행 파일만 꺼낸다 (README·라이선스는 두고)
fn extract_exe(zip_path: &Path, dest: &Path) -> Result<(), String> {
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

/// 지금 exe를 `….old.exe`로 옮기고 새 exe를 그 자리에 둔다. 이전 exe를 옮겨 둔 곳을 돌려준다.
/// 새 exe를 놓지 못하면 이전 exe를 제자리로 되돌린다.
pub fn swap_exe(current: &Path, new: &Path) -> Result<PathBuf, String> {
    let old = current.with_extension("old.exe");
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

/// 창 없이 업데이트 (`gemma-search.exe --update`): 새 버전이 있으면 받아서 바꾼다.
pub fn run_cli() -> i32 {
    println!("지금 버전 v{}", env!("CARGO_PKG_VERSION"));
    let report = |s: Stage| match s {
        Stage::Checking => println!("새 버전 확인 중…"),
        Stage::Downloading { tag, done: 0, total } => {
            println!("{tag} 받는 중 ({:.1}MB)…", total as f64 / 1048576.0)
        }
        Stage::Installing { tag } => println!("체크섬 확인 완료. {tag} 버전으로 바꾸는 중…"),
        _ => {}
    };
    match run(&report) {
        Ok(Outcome::UpToDate) => {
            println!("새 버전 없음");
            0
        }
        Ok(Outcome::Updated { tag, old }) => {
            println!("{tag} 버전으로 바꿨습니다. 다음에 실행하면 새 버전이 뜹니다.");
            println!("이전 실행 파일: {}", old.display());
            0
        }
        Err(f) => {
            println!("실패: {}", f.message);
            if let Some(p) = f.page {
                println!("직접 받기: {p}");
            }
            1
        }
    }
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

    #[test]
    fn picks_zip_and_checksum_from_release() {
        let base = format!("https://github.com/{REPO}/releases/download/v9.9.9");
        let v = serde_json::json!({
            "tag_name": "v9.9.9",
            "html_url": format!("https://github.com/{REPO}/releases/tag/v9.9.9"),
            "assets": [
                { "name": "gemma-desktop-search-v9.9.9-windows-x64.zip.sha256", "size": 110,
                  "browser_download_url": format!("{base}/gemma-desktop-search-v9.9.9-windows-x64.zip.sha256") },
                { "name": "gemma-desktop-search-v9.9.9-windows-x64.zip", "size": 14103601,
                  "browser_download_url": format!("{base}/gemma-desktop-search-v9.9.9-windows-x64.zip") },
            ],
        });
        let r = release_from_json(&v);
        assert_eq!(r.tag, "v9.9.9");
        let zip = r.zip.expect("zip");
        assert_eq!(zip.name, "gemma-desktop-search-v9.9.9-windows-x64.zip");
        assert_eq!(zip.size, 14103601);
        assert!(zip.url.ends_with("x64.zip"));
        assert!(zip.sha_url.ends_with("x64.zip.sha256"));

        // 체크섬 파일이 없거나, 다른 곳을 가리키는 주소면 자동으로 받지 않는다
        let mut no_sha = v.clone();
        no_sha["assets"].as_array_mut().unwrap().remove(0);
        assert_eq!(release_from_json(&no_sha).zip, None);
        let mut elsewhere = v.clone();
        elsewhere["assets"][1]["browser_download_url"] = "https://example.com/x-windows-x64.zip".into();
        assert_eq!(release_from_json(&elsewhere).zip, None);
        assert_eq!(release_from_json(&serde_json::json!({ "tag_name": "v1.0.0" })).zip, None);
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
        let current = dir.join("gemma-search.exe");
        let new = dir.join("gemma-search.new.exe");
        std::fs::write(&current, b"old version").unwrap();
        std::fs::write(&new, b"new version").unwrap();
        // 예전 백업이 남아 있어도 된다
        std::fs::write(dir.join("gemma-search.old.exe"), b"older").unwrap();

        let old = swap_exe(&current, &new).unwrap();
        assert_eq!(old, dir.join("gemma-search.old.exe"));
        assert_eq!(std::fs::read(&current).unwrap(), b"new version");
        assert_eq!(std::fs::read(&old).unwrap(), b"old version");
        assert!(!new.exists());

        // 새 파일이 없으면 실패하고, 지금 exe는 제자리에 그대로 있다
        assert!(swap_exe(&current, &dir.join("missing.exe")).is_err());
        assert_eq!(std::fs::read(&current).unwrap(), b"new version");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
