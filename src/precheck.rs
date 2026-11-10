//! 준비물(llama.cpp, 모델 파일)을 쓰기 전에 하는 간단한 검사. 직접 받아 둔 것을 지정했을 때 쓴다.
//!
//! - llama.cpp: `llama-server --version`을 실제로 실행해 본다 (DLL이 빠졌으면 여기서 걸린다).
//! - 모델: GGUF 머리 부분의 메타데이터에서 종류를 읽는다.
//!
//! 모델을 실제로 올려 임베딩을 만들어 보는 것은 마법사의 "동작 확인"과 서버 탭의 기능 점검이 한다.

use crate::sysinfo::Level;
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// EmbeddingGemma 2(`gemma-embedding2`)를 지원하는 첫 llama.cpp 빌드
pub const MIN_LLAMA_BUILD: u32 = 11457;
/// EmbeddingGemma 2 GGUF의 `general.architecture`
const MODEL_ARCH: &str = "gemma-embedding2";

/// 검사 결과: 좋음(초록 ✔) / 쓸 수는 있지만 확인 필요 / 쓸 수 없음
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub level: Level,
    pub text: String,
}

impl Verdict {
    pub(crate) fn ok(text: impl Into<String>) -> Self {
        Self { level: Level::Ok, text: text.into() }
    }

    pub(crate) fn warn(text: impl Into<String>) -> Self {
        Self { level: Level::Warn, text: text.into() }
    }

    pub(crate) fn bad(text: impl Into<String>) -> Self {
        Self { level: Level::Bad, text: text.into() }
    }

    /// 쓸 수 없는 상태가 아닌지
    pub fn usable(&self) -> bool {
        self.level != Level::Bad
    }
}

// ── llama.cpp ──

/// `--version` 출력에서 빌드 번호를 읽는다.
/// 새 형식 `version: 0.9.0-dev (build 12201, commit a2b301831)`, 예전 형식 `version: 6123 (abcdef0)`.
pub fn parse_llama_build(text: &str) -> Option<u32> {
    let leading_number = |s: &str| -> Option<(u32, usize)> {
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok().map(|n| (n, digits.len()))
    };
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("version:") else { continue };
        let rest = rest.trim();
        if let Some(i) = rest.find("build ") {
            if let Some((n, _)) = leading_number(&rest[i + 6..]) {
                return Some(n);
            }
        }
        if let Some((n, len)) = leading_number(rest) {
            let tail = &rest[len..];
            if tail.is_empty() || tail.starts_with(' ') {
                return Some(n);
            }
        }
    }
    None
}

/// 프로그램을 시작도 못 하고 끝났을 때(종료 코드가 NTSTATUS)의 이유
fn loader_reason(code: u32) -> String {
    match code {
        0xC000_0135 => "필요한 DLL을 찾지 못해 실행되지 않습니다. NVIDIA용이면 cudart 묶음(cudart-llama-bin-win-cuda-*.zip)을 같은 폴더에 풀었는지 확인하세요".into(),
        0xC000_007B => "실행 파일이나 DLL이 이 Windows와 맞지 않습니다 (64비트용인지 확인하세요)".into(),
        0xC000_001D => "이 PC의 CPU가 지원하지 않는 명령을 씁니다. 다른 판을 받아 보세요".into(),
        _ => format!("실행하자마자 오류로 끝났습니다 (0x{code:08X})"),
    }
}

/// llama.cpp가 이 PC에서 실제로 실행되는지 본다. 몇 초 걸릴 수 있으니 UI 스레드에서 부르지 않는다.
pub fn check_engine(llama_path: &str, extra_dll_dir: &str) -> Verdict {
    let (exe, _serve) = match crate::server::resolve_binary(llama_path) {
        Ok(v) => v,
        Err(e) => return Verdict::bad(e.to_string()),
    };
    // 서버를 켤 때와 같은 PATH (실행 파일 폴더, 추가 DLL 경로)
    let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut path_env = std::ffi::OsString::new();
    path_env.push(exe_dir.as_os_str());
    if !extra_dll_dir.trim().is_empty() {
        path_env.push(sep);
        path_env.push(extra_dll_dir.trim());
    }
    if let Some(old) = std::env::var_os("PATH") {
        path_env.push(sep);
        path_env.push(old);
    }
    let mut cmd = crate::util::hidden_command(&exe);
    cmd.arg("--version")
        .current_dir(&exe_dir)
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let spawned = {
        // DLL이 빠진 폴더를 실행해도 Windows의 "DLL을 찾을 수 없습니다" 창이 뜨지 않게
        let _quiet = crate::platform::QuietErrors::new();
        cmd.spawn()
    };
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return Verdict::bad(format!("실행하지 못했습니다: {e}")),
    };
    let deadline = Instant::now() + Duration::from_secs(8);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(40)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Verdict::bad("실행했지만 8초 안에 응답하지 않았습니다");
            }
            Err(e) => return Verdict::bad(format!("실행 상태를 확인하지 못했습니다: {e}")),
        }
    };
    // 버전 줄은 짧아서 끝난 뒤에 읽어도 파이프가 막히지 않는다
    let mut raw = Vec::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_end(&mut raw);
    }
    raw.push(b'\n');
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_end(&mut raw);
    }
    let out = String::from_utf8_lossy(&raw);

    if !status.success() {
        let code = status.code().unwrap_or(-1) as u32;
        if code >= 0xC000_0000 {
            return Verdict::bad(loader_reason(code));
        }
        return Verdict::warn(format!("실행은 되지만 버전을 확인하지 못했습니다 (종료 코드 {code})"));
    }
    match parse_llama_build(&out) {
        Some(b) if b < MIN_LLAMA_BUILD => Verdict::warn(format!(
            "실행은 되지만 버전이 낮습니다 (b{b}). EmbeddingGemma 2에는 b{MIN_LLAMA_BUILD} 이상이 필요합니다"
        )),
        Some(b) => Verdict::ok(format!("실행 확인 (b{b})")),
        None => Verdict::ok("실행 확인"),
    }
}

// ── 모델 (GGUF) ──

/// GGUF 머리 부분에서 읽은 것
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GgufInfo {
    /// `general.architecture` (EmbeddingGemma 2는 `gemma-embedding2`, mmproj는 `clip`)
    pub architecture: Option<String>,
    /// `general.type` (`model` / `mmproj`)
    pub kind: Option<String>,
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.buf.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn string(&mut self) -> Option<String> {
        let len = usize::try_from(self.u64()?).ok()?;
        Some(String::from_utf8_lossy(self.take(len)?).into_owned())
    }
}

/// GGUF 머리 부분(메타데이터 앞쪽)을 읽는다. GGUF가 아니면 None.
/// `general.architecture`와 `general.type`은 맨 앞에 있어서, 배열 값이 나오면 거기서 멈춘다.
pub fn parse_gguf_head(buf: &[u8]) -> Option<GgufInfo> {
    let mut c = Cursor { buf, pos: 0 };
    if c.take(4)? != b"GGUF" {
        return None;
    }
    let mut info = GgufInfo::default();
    let version = c.u32()?;
    if version < 2 {
        // v1은 길이 필드 크기가 달라 읽지 않는다 (GGUF인 것만 알린다)
        return Some(info);
    }
    let (_tensors, kv_count) = (c.u64()?, c.u64()?);
    for _ in 0..kv_count.min(64) {
        let Some(key) = c.string() else { break };
        let Some(ty) = c.u32() else { break };
        let value = match ty {
            8 => c.string(),
            // 값은 건너뛴다: 1바이트(u8·i8·bool), 2바이트, 4바이트, 8바이트
            0 | 1 | 7 => c.take(1).map(|_| String::new()),
            2 | 3 => c.take(2).map(|_| String::new()),
            4..=6 => c.take(4).map(|_| String::new()),
            10..=12 => c.take(8).map(|_| String::new()),
            _ => None, // 배열(9)이나 모르는 형식
        };
        let Some(value) = value else { break };
        match key.as_str() {
            "general.architecture" => info.architecture = Some(value),
            "general.type" => info.kind = Some(value),
            _ => {}
        }
        if info.architecture.is_some() && info.kind.is_some() {
            break;
        }
    }
    Some(info)
}

/// 모델 파일이 EmbeddingGemma 2 GGUF인지 본다 (파일 앞 64KB만 읽는다)
pub fn check_model(model_path: &str) -> Verdict {
    let trimmed = model_path.trim();
    if trimmed.is_empty() {
        return Verdict::bad("모델 파일이 지정되지 않았습니다");
    }
    let p = Path::new(trimmed);
    if p.is_dir() {
        return Verdict::bad("폴더가 아니라 모델 파일(.gguf)을 골라 주세요");
    }
    if !p.is_file() {
        return Verdict::bad("파일을 찾을 수 없습니다");
    }
    let mut head = Vec::new();
    if let Err(e) = std::fs::File::open(p).and_then(|f| f.take(64 * 1024).read_to_end(&mut head)) {
        return Verdict::bad(format!("파일을 읽지 못했습니다: {e}"));
    }
    let Some(info) = parse_gguf_head(&head) else {
        return Verdict::bad("GGUF 모델 파일이 아닙니다");
    };
    if info.kind.as_deref() == Some("mmproj") {
        return Verdict::bad("사진·소리용 보조 파일(mmproj)입니다. 모델 파일(embeddinggemma-2-….gguf)을 골라 주세요");
    }
    match info.architecture.as_deref() {
        Some(MODEL_ARCH) => Verdict::ok("EmbeddingGemma 2 모델 확인"),
        Some(other) => Verdict::warn(format!("GGUF 파일이지만 EmbeddingGemma 2가 아닌 것 같습니다 (종류: {other})")),
        None => Verdict::warn("GGUF 파일이지만 모델 종류를 확인하지 못했습니다"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_llama_build_number() {
        let new = "version: 0.9.0-dev (build 12201, commit a2b301831)\nbuilt with Clang 20.1.8 for Windows x86_64";
        assert_eq!(parse_llama_build(new), Some(12201));
        assert_eq!(parse_llama_build("version: 6123 (abcdef0)\nbuilt with MSVC"), Some(6123));
        assert_eq!(parse_llama_build("version: 0.9.0-dev"), None);
        assert_eq!(parse_llama_build("error: unknown argument"), None);
    }

    fn gguf(kvs: &[(&str, &str)]) -> Vec<u8> {
        let mut b = b"GGUF".to_vec();
        b.extend(3u32.to_le_bytes());
        b.extend(0u64.to_le_bytes());
        b.extend((kvs.len() as u64).to_le_bytes());
        for (k, v) in kvs {
            b.extend((k.len() as u64).to_le_bytes());
            b.extend(k.as_bytes());
            b.extend(8u32.to_le_bytes());
            b.extend((v.len() as u64).to_le_bytes());
            b.extend(v.as_bytes());
        }
        b
    }

    #[test]
    fn reads_gguf_architecture_and_type() {
        let model = gguf(&[("general.architecture", "gemma-embedding2"), ("general.type", "model")]);
        let info = parse_gguf_head(&model).unwrap();
        assert_eq!(info.architecture.as_deref(), Some("gemma-embedding2"));
        assert_eq!(info.kind.as_deref(), Some("model"));
        // GGUF가 아니거나 잘린 파일
        assert_eq!(parse_gguf_head(b"PK\x03\x04 not a model"), None);
        assert_eq!(parse_gguf_head(&model[..30]), Some(GgufInfo::default()));
    }

    #[test]
    fn judges_model_files() {
        let dir = std::env::temp_dir().join(format!("gemma-precheck-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p.display().to_string()
        };
        let model = write("m.gguf", &gguf(&[("general.architecture", "gemma-embedding2"), ("general.type", "model")]));
        let mmproj = write("mmproj.gguf", &gguf(&[("general.architecture", "clip"), ("general.type", "mmproj")]));
        let other = write("other.gguf", &gguf(&[("general.architecture", "llama"), ("general.type", "model")]));
        let junk = write("junk.gguf", b"hello");

        assert_eq!(check_model(&model).level, Level::Ok);
        assert_eq!(check_model(&mmproj).level, Level::Bad);
        assert_eq!(check_model(&other).level, Level::Warn);
        assert_eq!(check_model(&junk).level, Level::Bad);
        assert_eq!(check_model(&dir.display().to_string()).level, Level::Bad);
        assert_eq!(check_model("").level, Level::Bad);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
