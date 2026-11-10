//! 포터블 준비물 받기: EmbeddingGemma 2 Q8 모델과 llama.cpp(b12201 고정)를 프로그램 폴더 아래로.
//!
//! - 받는 중에는 `.part` 파일에 쓰고, 끝나면 SHA-256을 확인한 뒤 이름을 바꾼다.
//! - 끊겼던 `.part`가 있으면 이어서 받는다 (Range 요청, 서버가 지원하지 않으면 처음부터).
//! - llama.cpp는 검증한 빌드(b12201)로 고정한다. 최신판은 gemma-embedding2 지원이 바뀌거나 충돌이 다시 생길 수 있다.

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const LLAMA_BUILD: &str = "b12201";

pub struct Asset {
    pub name: &'static str,
    pub url: String,
    pub sha256: &'static str,
    pub size: u64,
    /// zip이면 풀고 지운다
    pub unzip: bool,
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

    /// 받을 폴더 (프로그램 폴더 기준)
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
            unzip: false,
        };
        let gh = |name: &'static str, sha256: &'static str, size: u64| Asset {
            name,
            url: format!("https://github.com/ggml-org/llama.cpp/releases/download/{LLAMA_BUILD}/{name}"),
            sha256,
            size,
            unzip: true,
        };
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

fn unzip(zip_path: &Path, dest: &Path) -> Result<()> {
    let f = std::fs::File::open(zip_path)?;
    let mut z = zip::ZipArchive::new(f)?;
    z.extract(dest)?;
    Ok(())
}

pub fn run(pack: Pack, app_dir: &Path, st: &Mutex<DlState>, cancel: &AtomicBool) -> Result<String> {
    let dest = pack.dest(app_dir);
    for a in pack.assets() {
        let path = fetch(&a, &dest, st, cancel)?;
        if a.unzip {
            st.lock().file = format!("{} 푸는 중…", a.name);
            unzip(&path, &dest)?;
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(dest.display().to_string())
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
            unzip: false,
        };
        let st = Mutex::new(DlState::default());
        assert_eq!(fetch(&a, &dir, &st, &AtomicBool::new(false)).unwrap(), p);
        let _ = std::fs::remove_dir_all(dir);
    }
}
