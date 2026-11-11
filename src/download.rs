//! 준비물 받기: EmbeddingGemma 2 Q8 모델과 llama.cpp(b12201 고정)를 데이터 위치(`config::prep_dir`) 아래로.
//!
//! - 받는 중에는 `.part` 파일에 쓰고, 끝나면 SHA-256을 확인한 뒤 이름을 바꾼다.
//! - 끊겼던 `.part`가 있으면 이어서 받는다 (Range 요청, 서버가 지원하지 않으면 처음부터).
//! - llama.cpp는 검증한 빌드(b12201)로 고정한다. 최신판은 gemma-embedding2 지원이 바뀌거나 충돌이 다시 생길 수 있다.
//! - llama.cpp 묶음은 OS마다 다르다: Windows는 zip, Linux는 tar.gz(맨 위에 `llama-b12201/` 폴더가 하나 더 있다).
//!   그 밖의 OS용은 아직 없다 (`Pack::assets`가 빈 목록).
//! - 명령줄 받기(`install`)는 새 판을 옆 폴더에 다 푼 뒤에 바꿔 넣는다. 받다가 끊겨도 쓰던 llama.cpp는 그대로다.

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const LLAMA_BUILD: &str = "b12201";

/// 명령줄로 받아 넣은 llama.cpp 폴더에 남기는 표식: 어느 판인지 (다음에 같은 판이면 건너뛰고, 다른 판이면 바꿔 넣는다)
const MARKER: &str = ".gemma-search-pack";

pub struct Asset {
    pub name: &'static str,
    pub url: String,
    pub sha256: &'static str,
    pub size: u64,
    /// 묶음(zip, tar.gz)이면 풀고 지운다
    pub unpack: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pack {
    Model,
    LlamaCuda13,
    LlamaCuda12,
    LlamaVulkan,
    LlamaCpu,
}

impl Pack {
    pub fn label(self) -> &'static str {
        match self {
            Pack::Model => "EmbeddingGemma 2 Q8 모델 (865MB)",
            Pack::LlamaCuda13 => "llama.cpp CUDA 13 (NVIDIA, 최신 드라이버, 597MB)",
            Pack::LlamaCuda12 => "llama.cpp CUDA 12 (NVIDIA, 예전 드라이버, 680MB)",
            Pack::LlamaVulkan => "llama.cpp Vulkan (AMD·Intel·NVIDIA, 38MB)",
            Pack::LlamaCpu => "llama.cpp CPU 전용 (25MB, 느림)",
        }
    }

    /// 쉬운 말로 된 이름 (일반 설정, 처음 설정)
    pub fn plain_label(self) -> &'static str {
        match self {
            Pack::Model => "AI 모델 (865MB)",
            Pack::LlamaCuda13 => "llama.cpp · NVIDIA 그래픽카드용 (597MB)",
            Pack::LlamaCuda12 => "llama.cpp · NVIDIA 그래픽카드용, 예전 드라이버 (680MB)",
            Pack::LlamaVulkan => "llama.cpp · AMD·Intel 그래픽카드용 (38MB)",
            Pack::LlamaCpu => "llama.cpp · 그래픽카드 없이 (25MB, 느림)",
        }
    }

    /// 명령줄에서 쓰는 이름 (`gemma-search download --llama <이름>`)
    pub fn key(self) -> &'static str {
        match self {
            Pack::Model => "model",
            Pack::LlamaCuda13 => "cuda13",
            Pack::LlamaCuda12 => "cuda12",
            Pack::LlamaVulkan => "vulkan",
            Pack::LlamaCpu => "cpu",
        }
    }

    pub const LLAMAS: [Pack; 4] = [Pack::LlamaCpu, Pack::LlamaVulkan, Pack::LlamaCuda13, Pack::LlamaCuda12];

    /// 받을 크기 합계 (바이트)
    pub fn total_size(self) -> u64 {
        self.assets().iter().map(|a| a.size).sum()
    }

    /// 받을 폴더 (데이터 위치 기준: `config::prep_dir`)
    pub fn dest(self, app_dir: &Path) -> PathBuf {
        match self {
            Pack::Model => app_dir.join("models").join("embeddinggemma-2"),
            _ => app_dir.join("llama.cpp"),
        }
    }

    pub fn assets(self) -> Vec<Asset> {
        let hf = |name: &'static str, sha256: &'static str, size: u64| Asset {
            name,
            url: format!("https://huggingface.co/ggml-org/embeddinggemma-2-GGUF/resolve/main/{name}"),
            sha256,
            size,
            unpack: false,
        };
        let gh = |name: &'static str, sha256: &'static str, size: u64| Asset {
            name,
            url: format!("https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_BUILD}/{name}"),
            sha256,
            size,
            unpack: true,
        };
        if self != Pack::Model && !cfg!(windows) {
            return self.linux_assets(gh);
        }
        match self {
            Pack::Model => vec![
                hf(
                    "embeddinggemma-2-Q8_0.gguf",
                    "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135",
                    309_855_456,
                ),
                hf(
                    "mmproj-embeddinggemma-2-Q8_0.gguf",
                    "0531309472a6bc6f4b146c3736eee7795ec9db1903c25490d3c937490cef6dd8",
                    554_821_056,
                ),
            ],
            Pack::LlamaCuda13 => vec![
                gh(
                    "llama-b12201-bin-win-cuda-13.5-x64.zip",
                    "ccdac66ad785c8ca5dd1e901600ac0296818aee7cbec41c991fd4232cc3b1418",
                    170_781_135,
                ),
                gh(
                    "cudart-llama-bin-win-cuda-13.5-x64.zip",
                    "c2bf4cc202bfbaaa7a0b75f5082457c3811326947bb216059b72839d844e9cba",
                    426_314_199,
                ),
            ],
            Pack::LlamaCuda12 => vec![
                gh(
                    "llama-b12201-bin-win-cuda-12.4-x64.zip",
                    "7cacd54ef339fa7b8af0fd7bf9b1eb80731a40406a3e3a36531629241867e422",
                    288_732_513,
                ),
                gh(
                    "cudart-llama-bin-win-cuda-12.4-x64.zip",
                    "8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6",
                    391_443_627,
                ),
            ],
            Pack::LlamaVulkan => vec![gh(
                "llama-b12201-bin-win-vulkan-x64.zip",
                "ad79516a3c1f07ef5475bf72a88b4b74c96421ea0eba00b69ce15736f7745e1e",
                37_715_526,
            )],
            Pack::LlamaCpu => vec![gh(
                "llama-b12201-bin-win-cpu-x64.zip",
                "62a504f945ea97c9e1842cc2cb81fc64d86b7a668e29c2386cfd4cfe08ddd87d",
                24_758_934,
            )],
        }
    }

    /// Linux x64용 (Ubuntu에서 빌드한 판). 해시는 GitHub 릴리스의 digest 값.
    /// 그 밖의 OS(macOS 등)는 아직 없다.
    fn linux_assets(self, gh: impl Fn(&'static str, &'static str, u64) -> Asset) -> Vec<Asset> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Vec::new();
        }
        match self {
            Pack::Model => Vec::new(),
            Pack::LlamaCpu => vec![gh(
                "llama-b12201-bin-ubuntu-x64.tar.gz",
                "3fb1afbee5711414f00e59068c10602b1d34919aaea7d4253260387e0e6e4ccf",
                23_067_593,
            )],
            Pack::LlamaVulkan => vec![gh(
                "llama-b12201-bin-ubuntu-vulkan-x64.tar.gz",
                "94554990eec661b4f5750eb51cb96c8e506c040c9b8d31dd008b8027af3005c8",
                35_882_870,
            )],
            Pack::LlamaCuda13 => vec![
                gh(
                    "llama-b12201-bin-ubuntu-cuda-13.5-x64.tar.gz",
                    "849707a7e91264d763641b3880e8c44fc33f3f382507ee55971aba90c62b8351",
                    169_754_866,
                ),
                gh(
                    "cudart-llama-b12201-bin-ubuntu-cuda-13.5-x64.tar.gz",
                    "4f842dbc10ecd4f3881e1c3671e1db18a56657721f470d4ed62f0729500490d8",
                    444_078_696,
                ),
            ],
            Pack::LlamaCuda12 => vec![
                gh(
                    "llama-b12201-bin-ubuntu-cuda-12.8-x64.tar.gz",
                    "afbb0c9f796fd04c3696bcb72903d9509c1ab40df3a3e33d2dfa870787d3cf11",
                    188_149_329,
                ),
                gh(
                    "cudart-llama-b12201-bin-ubuntu-cuda-12.8-x64.tar.gz",
                    "db17380427c3431e290ed1654d9eafbf4f73f523f431cf1a869929a5161fa82a",
                    594_370_124,
                ),
            ],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DlState {
    pub running: bool,
    pub pack: Option<Pack>,
    /// 지금 받는 파일
    pub file: String,
    pub done: u64,
    pub total: u64,
    /// 끝난 결과 (성공 여부, 메시지) — 앱이 읽고 지운다
    pub finished: Option<(bool, String)>,
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

/// 파일 하나 받기 (이어받기 + 체크섬). 이미 같은 파일이 있으면 건너뛴다.
fn fetch(asset: &Asset, dest_dir: &Path, st: &Mutex<DlState>, cancel: &AtomicBool) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir)?;
    let target = dest_dir.join(asset.name);
    if target.is_file() && std::fs::metadata(&target)?.len() == asset.size && sha256_file(&target)? == asset.sha256 {
        return Ok(target);
    }
    let part = dest_dir.join(format!("{}.part", asset.name));
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > asset.size {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(60)))
        .build()
        .into();
    let mut req = agent.get(&asset.url).header("User-Agent", "gemma-desktop-search");
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let mut resp = req.call().map_err(|e| anyhow!("{}: {e}", asset.name))?;
    let resumed = have > 0 && resp.status().as_u16() == 206;
    let mut hasher = Sha256::new();
    let mut out = if resumed {
        // 이미 받은 부분도 체크섬에 넣는다
        let mut f = std::fs::File::open(&part)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        std::fs::OpenOptions::new().append(true).open(&part)?
    } else {
        have = 0;
        std::fs::File::create(&part)?
    };
    {
        let mut g = st.lock();
        g.file = asset.name.to_string();
        g.done = have;
        g.total = asset.size;
    }
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 18];
    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("취소됨 (다시 누르면 이어서 받습니다)");
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        have += n as u64;
        st.lock().done = have;
    }
    out.flush()?;
    drop(out);
    let got = format!("{:x}", hasher.finalize());
    if got != asset.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("{}: 체크섬이 맞지 않습니다 (받은 파일을 지웠습니다)", asset.name);
    }
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&part, &target)?;
    Ok(target)
}

/// 묶음을 `dest`에 푼다: 이름이 .zip이면 zip, .tar.gz면 tar.gz
fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(".zip") {
        let f = std::fs::File::open(archive)?;
        let mut z = zip::ZipArchive::new(f)?;
        z.extract(dest)?;
        Ok(())
    } else if name.ends_with(".tar.gz") {
        untar_gz(archive, dest)
    } else {
        bail!("푸는 방법을 모르는 묶음입니다: {name}")
    }
}

/// tar.gz 풀기. 묶음 밖으로 나가는 경로(.., 절대 경로)는 tar 크레이트가 막는다.
/// 권한은 실행 비트까지만 살리고 setuid·setgid·sticky와 소유자·확장 속성은 버린다 (받은 파일이 특수 권한을 갖지 않게).
#[cfg(unix)]
fn untar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let f = std::io::BufReader::new(std::fs::File::open(archive)?);
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    ar.set_preserve_permissions(false);
    ar.set_preserve_ownerships(false);
    ar.set_unpack_xattrs(false);
    ar.set_overwrite(true);
    std::fs::create_dir_all(dest)?;
    ar.unpack(dest).map_err(|e| anyhow!("{} 풀기 실패: {e}", archive.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn untar_gz(archive: &Path, _dest: &Path) -> Result<()> {
    bail!("이 OS에서는 tar.gz를 풀지 않습니다: {}", archive.display())
}

/// 창이 있는 앱의 받기: 받은 묶음을 그 자리(llama.cpp 폴더)에 푼다
pub fn run(pack: Pack, app_dir: &Path, st: &Mutex<DlState>, cancel: &AtomicBool) -> Result<String> {
    let dest = pack.dest(app_dir);
    for a in pack.assets() {
        let path = fetch(&a, &dest, st, cancel)?;
        if a.unpack {
            st.lock().file = format!("{} 푸는 중…", a.name);
            unpack(&path, &dest)?;
            let _ = std::fs::remove_file(&path);
            flatten_single_dir(&dest)?;
        }
    }
    Ok(dest.display().to_string())
}

/// 묶음이 맨 위에 폴더 하나만 둔 경우(Linux판의 `llama-b12201/`) 그 안의 것을 한 단계 위로 올린다
fn flatten_single_dir(dir: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
    let [only] = entries.as_slice() else { return Ok(()) };
    if !only.is_dir() || only.is_symlink() {
        return Ok(());
    }
    move_children(only, dir)?;
    std::fs::remove_dir(only)?;
    Ok(())
}

/// 묶음 하나를 `stage`에 합쳐 푼다: 따로 풀어 맨 위 폴더를 걷어 낸 뒤 옮긴다
/// (CUDA판은 `llama-b12201/`과 `cudart-llama-…-x64/`처럼 묶음마다 맨 위 폴더 이름이 다르다)
fn unpack_merged(archive: &Path, stage: &Path, i: usize) -> Result<()> {
    let part = stage.join(format!(".unpack-{i}"));
    unpack(archive, &part)?;
    flatten_single_dir(&part)?;
    move_children(&part, stage)?;
    std::fs::remove_dir(&part)?;
    Ok(())
}

/// `from` 안의 것을 모두 `to`로 옮긴다 (같은 이름이 있으면 바꿔 넣는다)
fn move_children(from: &Path, to: &Path) -> Result<()> {
    for e in std::fs::read_dir(from)?.flatten() {
        let dst = to.join(e.file_name());
        if dst.is_dir() && !dst.is_symlink() {
            std::fs::remove_dir_all(&dst)?;
        } else if dst.exists() || dst.is_symlink() {
            std::fs::remove_file(&dst)?;
        }
        std::fs::rename(e.path(), dst)?;
    }
    Ok(())
}

// ── 명령줄 받기 (gemma-search download) ──

/// `root` 아래 받을 곳: llama.cpp는 `root/llama.cpp`, 모델은 `root/models/embeddinggemma-2`
pub fn model_files(root: &Path) -> (PathBuf, PathBuf) {
    let d = Pack::Model.dest(root);
    (d.join("embeddinggemma-2-Q8_0.gguf"), d.join("mmproj-embeddinggemma-2-Q8_0.gguf"))
}

/// 이미 받아 둔 것: 모델은 파일 크기, llama.cpp는 표식(같은 판·같은 빌드)과 llama-server로 본다.
/// (모델의 체크섬은 받을 때 `fetch`가 다시 확인한다)
pub fn installed(pack: Pack, root: &Path) -> bool {
    let dest = pack.dest(root);
    match pack {
        Pack::Model => pack.assets().iter().all(|a| {
            std::fs::metadata(dest.join(a.name)).map(|m| m.len() == a.size).unwrap_or(false)
        }),
        _ => {
            std::fs::read_to_string(dest.join(MARKER)).is_ok_and(|m| m.trim() == marker_text(pack))
                && crate::server::resolve_binary(&dest.display().to_string()).is_ok()
        }
    }
}

fn marker_text(pack: Pack) -> String {
    format!("{} {LLAMA_BUILD}", pack.key())
}

/// `root/llama.cpp`에 명령줄로 받아 넣은 판: (판 이름, 빌드). 표식이 없으면 None.
pub fn installed_llama(root: &Path) -> Option<(String, String)> {
    let m = std::fs::read_to_string(Pack::LlamaCpu.dest(root).join(MARKER)).ok()?;
    let (key, build) = m.trim().split_once(' ')?;
    Some((key.to_string(), build.to_string()))
}

/// 받아 넣은 결과
pub struct Installed {
    pub dest: PathBuf,
    /// 원래 있던 llama.cpp 폴더를 지우지 않고 이 이름으로 옮겨 둔 경우 (우리가 받은 것이 아니었다)
    pub kept_old: Option<PathBuf>,
}

/// 명령줄 받기: 받아서 확인하고 넣는다. 진행은 `st`에 적는다.
/// - 묶음은 `root/.download`에 받는다 (끊기면 다음에 이어서 받는다).
/// - llama.cpp는 `root/llama.cpp.new`에 다 푼 뒤 원래 폴더와 바꾼다. 원래 폴더가 우리가 받은 것(표식 있음)이면 지우고,
///   아니면 `llama.cpp.old-<시각>`으로 옮겨 둔다 (남의 파일은 지우지 않는다).
pub fn install(pack: Pack, root: &Path, st: &Mutex<DlState>) -> Result<Installed> {
    let assets = pack.assets();
    if assets.is_empty() {
        bail!("이 OS용 {}는 아직 없습니다", pack.key());
    }
    let cancel = AtomicBool::new(false);
    let dest = pack.dest(root);
    if pack == Pack::Model {
        for a in &assets {
            {
                // 이미 있으면 fetch가 체크섬만 확인하고 끝낸다 (그동안 보여 줄 말)
                let mut g = st.lock();
                g.file = format!("{} 확인 중…", a.name);
                (g.done, g.total) = (0, 0);
            }
            fetch(a, &dest, st, &cancel)?;
        }
        return Ok(Installed { dest, kept_old: None });
    }
    let dl = root.join(".download");
    std::fs::create_dir_all(&dl)?;
    let stage = root.join("llama.cpp.new");
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    for (i, a) in assets.iter().enumerate() {
        let path = fetch(a, &dl, st, &cancel)?;
        st.lock().file = format!("{} 푸는 중…", a.name);
        unpack_merged(&path, &stage, i)?;
    }
    crate::server::resolve_binary(&stage.display().to_string())
        .map_err(|e| anyhow!("받은 묶음에서 llama-server를 찾지 못했습니다: {e}"))?;
    std::fs::write(stage.join(MARKER), marker_text(pack) + "\n")?;
    let mut kept_old = None;
    if dest.exists() {
        if dest.join(MARKER).is_file() {
            std::fs::remove_dir_all(&dest).map_err(|e| anyhow!("쓰던 llama.cpp 폴더를 지우지 못했습니다 ({}): {e}", dest.display()))?;
        } else {
            let [y, mo, d, h, mi, s] = crate::platform::local_time();
            let old = root.join(format!("llama.cpp.old-{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}"));
            std::fs::rename(&dest, &old)?;
            kept_old = Some(old);
        }
    }
    std::fs::rename(&stage, &dest)?;
    // 다 넣었으면 받은 묶음은 지운다 (끊겼을 때만 이어받기용으로 남는다)
    for a in &assets {
        let _ = std::fs::remove_file(dl.join(a.name));
    }
    let _ = std::fs::remove_dir(&dl);
    Ok(Installed { dest, kept_old })
}

/// 백그라운드로 받기 시작
pub fn start(pack: Pack, app_dir: PathBuf, st: Arc<Mutex<DlState>>, cancel: Arc<AtomicBool>, wake: crate::util::Waker) {
    {
        let mut g = st.lock();
        if g.running {
            return;
        }
        *g = DlState {
            running: true,
            pack: Some(pack),
            ..Default::default()
        };
    }
    cancel.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        let r = run(pack, &app_dir, &st, &cancel);
        let mut g = st.lock();
        g.running = false;
        g.finished = Some(match r {
            Ok(dir) => (true, dir),
            Err(e) => (false, e.to_string()),
        });
        drop(g);
        wake();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_and_existing_file_skip() {
        let dir = std::env::temp_dir().join(format!("gs-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.bin");
        std::fs::write(&p, b"hello").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        // 이미 받은 파일(크기·체크섬 일치)은 네트워크 없이 건너뛴다
        let a = Asset {
            name: "x.bin",
            url: "http://127.0.0.1:9/never".into(),
            sha256: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
            size: 5,
            unpack: false,
        };
        let st = Mutex::new(DlState::default());
        assert_eq!(fetch(&a, &dir, &st, &AtomicBool::new(false)).unwrap(), p);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("gs-dl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn flattens_only_a_single_top_folder() {
        // 여러 개가 있으면 그대로 둔다 (Windows zip)
        let d = temp("flat");
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::write(d.join("b.txt"), "").unwrap();
        flatten_single_dir(&d).unwrap();
        assert!(d.join("a").is_dir() && d.join("b.txt").is_file());
        // 폴더 하나뿐이면 안의 것을 올린다 (Linux tar.gz의 llama-b12201/)
        let d2 = temp("flat2");
        std::fs::create_dir_all(d2.join("llama-b12201").join("sub")).unwrap();
        std::fs::write(d2.join("llama-b12201").join("llama-server"), "x").unwrap();
        flatten_single_dir(&d2).unwrap();
        assert!(d2.join("llama-server").is_file() && d2.join("sub").is_dir() && !d2.join("llama-b12201").exists());
        let _ = (std::fs::remove_dir_all(d), std::fs::remove_dir_all(d2));
    }

    #[test]
    fn knows_what_was_installed() {
        let root = temp("inst");
        assert!(!installed(Pack::LlamaCpu, &root));
        let d = Pack::LlamaCpu.dest(&root);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(crate::platform::exe_name("llama-server")), "").unwrap();
        assert!(!installed(Pack::LlamaCpu, &root), "표식이 없으면 어느 판인지 모른다");
        std::fs::write(d.join(MARKER), marker_text(Pack::LlamaCpu) + "\n").unwrap();
        assert!(installed(Pack::LlamaCpu, &root));
        assert!(!installed(Pack::LlamaVulkan, &root));
        assert_eq!(installed_llama(&root), Some(("cpu".to_string(), LLAMA_BUILD.to_string())));
        let (m, p) = model_files(&root);
        assert!(m.ends_with("embeddinggemma-2-Q8_0.gguf") && p.ends_with("mmproj-embeddinggemma-2-Q8_0.gguf"));
        assert!(!installed(Pack::Model, &root));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn untar_keeps_exec_bit_drops_setuid_and_blocks_escapes() {
        use std::os::unix::fs::PermissionsExt;
        let d = temp("tar");
        let tgz = d.join("x.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(std::fs::File::create(&tgz).unwrap(), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            h.set_size(2);
            h.set_mode(0o4755);
            b.append_data(&mut h, "llama-b12201/llama-server", &b"hi"[..]).unwrap();
            let mut l = tar::Header::new_gnu();
            l.set_entry_type(tar::EntryType::Symlink);
            l.set_size(0);
            l.set_mode(0o777);
            b.append_link(&mut l, "llama-b12201/libx.so.0", "libx.so.0.1").unwrap();
            // 묶음 밖으로 나가려는 항목 (tar 크레이트의 set_path는 막으므로 이름 칸에 직접 쓴다)
            let mut e = tar::Header::new_old();
            let name = b"llama-b12201/../../evil.txt";
            e.as_old_mut().name[..name.len()].copy_from_slice(name);
            e.set_size(1);
            e.set_mode(0o644);
            e.set_cksum();
            b.append(&e, &b"!"[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let out = d.join("out");
        let _ = untar_gz(&tgz, &out);
        assert!(!d.join("evil.txt").exists(), "묶음 밖에 쓰지 않는다");
        flatten_single_dir(&out).unwrap();
        let mode = std::fs::metadata(out.join("llama-server")).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode & 0o7000, 0, "setuid·setgid·sticky는 버린다 (지금 {mode:o})");
        assert_eq!(mode & 0o111, 0o111, "실행 권한은 남는다 (지금 {mode:o})");
        assert_eq!(std::fs::read_link(out.join("libx.so.0")).unwrap(), PathBuf::from("libx.so.0.1"));
        let _ = std::fs::remove_dir_all(d);
    }

    /// CUDA판처럼 맨 위 폴더 이름이 다른 묶음 둘을 한 폴더에 합친다
    #[cfg(unix)]
    #[test]
    fn merges_archives_with_different_top_folders() {
        let d = temp("merge");
        let make = |file: &str, entries: &[&str]| {
            let p = d.join(file);
            let gz = flate2::write::GzEncoder::new(std::fs::File::create(&p).unwrap(), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            for e in entries {
                let mut h = tar::Header::new_gnu();
                h.set_size(1);
                h.set_mode(0o755);
                b.append_data(&mut h, e, &b"x"[..]).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
            p
        };
        let main = make("main.tar.gz", &["llama-b12201/llama-server", "llama-b12201/libggml.so"]);
        let rt = make("rt.tar.gz", &["cudart-llama-b12201-x64/libcudart.so.13"]);
        let stage = d.join("stage");
        unpack_merged(&main, &stage, 0).unwrap();
        unpack_merged(&rt, &stage, 1).unwrap();
        let mut names: Vec<String> =
            std::fs::read_dir(&stage).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["libcudart.so.13", "libggml.so", "llama-server"]);
        assert!(crate::server::resolve_binary(&stage.display().to_string()).is_ok());
        let _ = std::fs::remove_dir_all(d);
    }
}
