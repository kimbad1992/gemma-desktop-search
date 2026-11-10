//! llama-server 실행/중지/상태 감시.
//!
//! 상태: 중지 → 시작 중 → 모델 로딩(/health 503) → 준비됨(200) → 오류/종료
//! 준비되면 자가진단을 돌려 사용 가능한 모달리티를 판별한다.

pub mod job;

use crate::config::Config;
use crate::embed::client::EmbedClient;
use crate::embed::selftest::{self, Capabilities};
use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_LOG_LINES: usize = 3000;

#[derive(Clone, Debug, PartialEq)]
pub enum ServerState {
    Stopped,
    Starting,
    Loading,
    Ready,
    /// 앱이 띄우지 않은, 이미 실행 중인 서버에 연결
    External,
    Failed(String),
}

impl ServerState {
    pub fn is_ready(&self) -> bool {
        matches!(self, ServerState::Ready | ServerState::External)
    }
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            ServerState::Starting | ServerState::Loading | ServerState::Ready
        )
    }
    pub fn label(&self) -> &'static str {
        match self {
            ServerState::Stopped => "중지됨",
            ServerState::Starting => "시작 중",
            ServerState::Loading => "모델 로딩 중",
            ServerState::Ready => "준비됨",
            ServerState::External => "외부 서버 연결됨",
            ServerState::Failed(_) => "오류",
        }
    }
}

#[derive(Clone, Debug)]
pub enum SelfTest {
    NotRun,
    Running,
    Done(Capabilities),
}

pub struct Shared {
    pub state: ServerState,
    pub pid: Option<u32>,
    pub started_at: Option<Instant>,
    pub ready_at: Option<Instant>,
    pub port: u16,
    pub command_line: String,
    pub selftest: SelfTest,
    /// start/stop 할 때마다 증가 — 이전 감시 스레드가 스스로 종료하도록
    generation: u64,
    has_mmproj: bool,
}

#[derive(Clone)]
pub struct ServerManager {
    pub shared: Arc<Mutex<Shared>>,
    pub logs: Arc<Mutex<VecDeque<String>>>,
    /// 서버 로그를 파일로도 남긴다 (충돌 원인 확인용, data\logs\llama-server.log)
    log_file: Arc<Mutex<Option<std::fs::File>>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Default for ServerManager {
    fn default() -> Self {
        Self {
            shared: Arc::new(Mutex::new(Shared {
                state: ServerState::Stopped,
                pid: None,
                started_at: None,
                ready_at: None,
                port: 0,
                command_line: String::new(),
                selftest: SelfTest::NotRun,
                generation: 0,
                has_mmproj: false,
            })),
            logs: Arc::new(Mutex::new(VecDeque::new())),
            log_file: Arc::new(Mutex::new(None)),
            child: Arc::new(Mutex::new(None)),
        }
    }
}

const MAX_LOG_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// 사용자가 지정한 경로(폴더 또는 exe)에서 실행 파일을 찾는다.
/// 반환: (exe 경로, `llama.exe serve` 형태인지)
pub fn resolve_binary(path: &str) -> Result<(PathBuf, bool)> {
    let p = PathBuf::from(path.trim());
    if path.trim().is_empty() {
        bail!("llama.cpp 위치가 지정되지 않았습니다");
    }
    let (server_exe, llama_exe) = (crate::platform::exe_name("llama-server"), crate::platform::exe_name("llama"));
    let is_llama_exe = |f: &Path| {
        f.file_name()
            .map(|n| n.to_string_lossy().eq_ignore_ascii_case(&llama_exe))
            .unwrap_or(false)
    };
    if p.is_file() {
        let serve = is_llama_exe(&p);
        return Ok((p, serve));
    }
    if p.is_dir() {
        let subdirs: [&[&str]; 4] = [&[], &["build", "bin"], &["bin"], &["build", "bin", "Release"]];
        let dir = |sub: &[&str]| sub.iter().fold(p.clone(), |d, s| d.join(s));
        for sub in subdirs {
            let c = dir(sub).join(&server_exe);
            if c.is_file() {
                return Ok((c, false));
            }
        }
        for sub in subdirs {
            let c = dir(sub).join(&llama_exe);
            if c.is_file() {
                return Ok((c, true));
            }
        }
        bail!("{}에서 {server_exe} / {llama_exe}를 찾지 못했습니다", p.display());
    }
    bail!("경로가 존재하지 않습니다: {}", p.display())
}

pub fn build_args(cfg: &Config, serve_mode: bool) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    if serve_mode {
        a.push("serve".into());
    }
    a.extend(["-m".into(), cfg.model_path.trim().to_string()]);
    if cfg.has_mmproj() {
        a.extend(["--mmproj".into(), cfg.mmproj_path.trim().to_string()]);
        // 모델 카드 기본값: 영상은 초당 1프레임으로 샘플링 (llama-server 기본은 4)
        a.extend(["--video-fps".into(), "1".into()]);
        // 서버가 영상을 직접 디코딩하므로 ffmpeg 위치를 알려준다
        if let Some(dir) = cfg.ffmpeg().as_deref().and_then(Path::parent) {
            a.extend(["--video-ffmpeg-dir".into(), dir.display().to_string()]);
        }
    }
    let s = |v: &dyn ToString| v.to_string();
    a.extend([
        "--embeddings".into(),
        "--pooling".into(),
        "mean".into(),
        "-c".into(),
        s(&cfg.ctx_size),
        "-np".into(),
        s(&cfg.parallel.max(1)),
        "-b".into(),
        s(&cfg.batch_size),
        "-ub".into(),
        s(&cfg.ubatch_size),
        "-ngl".into(),
        s(&cfg.n_gpu_layers),
        "-fa".into(),
        match cfg.flash_attn.trim() {
            "on" => "on".into(),
            "auto" => "auto".into(),
            _ => "off".into(),
        },
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        s(&cfg.port),
        "--no-webui".into(),
    ]);
    a.extend(crate::util::split_args(&cfg.extra_args));
    a
}

fn quote(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

impl ServerManager {
    pub fn state(&self) -> ServerState {
        self.shared.lock().state.clone()
    }

    pub fn capabilities(&self) -> Option<Capabilities> {
        match &self.shared.lock().selftest {
            SelfTest::Done(c) => Some(c.clone()),
            _ => None,
        }
    }

    pub fn port(&self) -> u16 {
        self.shared.lock().port
    }

    pub fn log(&self, line: impl Into<String>) {
        push_log(&self.logs, &self.log_file, line.into());
    }

    /// 로그 파일 위치 지정. 20MB를 넘으면 .old.log로 돌린다.
    pub fn set_log_path(&self, path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) > MAX_LOG_FILE_BYTES {
            let _ = std::fs::rename(path, path.with_extension("old.log"));
        }
        *self.log_file.lock() = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok();
    }

    pub fn start(&self, cfg: &Config) -> Result<()> {
        if self.state().is_running() {
            bail!("서버가 이미 실행 중입니다");
        }
        let (exe, serve_mode) = resolve_binary(&cfg.llama_path)?;
        if !Path::new(cfg.model_path.trim()).is_file() {
            bail!("모델 파일이 없습니다: {}", cfg.model_path);
        }
        if cfg.has_mmproj() && !Path::new(cfg.mmproj_path.trim()).is_file() {
            bail!("mmproj 파일이 없습니다: {}", cfg.mmproj_path);
        }
        // 포트 확인. Hyper-V/WSL이 재부팅 때마다 포트 범위를 예약해(예: 8067~8566) 설정한 포트를 못 쓰게 되는
        // 경우가 있다 → 다른 서버가 응답 중인 게 아니면 빈 포트를 골라 쓴다.
        let mut cfg = cfg.clone();
        if std::net::TcpListener::bind(("127.0.0.1", cfg.port)).is_err() {
            if EmbedClient::new(cfg.port).health().is_ok() {
                bail!(
                    "포트 {}에서 이미 다른 서버가 응답 중입니다. '외부 서버 연결'을 쓰거나 포트를 바꾸세요.",
                    cfg.port
                );
            }
            let Some(free) = free_port(cfg.port) else {
                bail!("포트 {}를 사용할 수 없고 빈 포트도 찾지 못했습니다.", cfg.port);
            };
            self.log(format!(
                "[앱] 포트 {}를 쓸 수 없어(다른 프로그램이 점유했거나 Windows 예약 범위) 포트 {free}를 사용합니다",
                cfg.port
            ));
            cfg.port = free;
        }
        let cfg = &cfg;

        let args = build_args(cfg, serve_mode);
        let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
        // 실행 파일 폴더(와 추가 DLL 폴더)를 PATH 앞에 둔다
        let sep = if cfg!(windows) { ";" } else { ":" };
        let mut path_env = std::ffi::OsString::new();
        path_env.push(exe_dir.as_os_str());
        if !cfg.extra_dll_dir.trim().is_empty() {
            path_env.push(sep);
            path_env.push(cfg.extra_dll_dir.trim());
        }
        // Linux: 실행 파일과 같은 폴더의 공유 라이브러리를 찾게 한다
        let mut lib_env = path_env.clone();
        if let Some(old) = std::env::var_os("LD_LIBRARY_PATH") {
            lib_env.push(sep);
            lib_env.push(old);
        }
        if let Some(old) = std::env::var_os("PATH") {
            path_env.push(sep);
            path_env.push(old);
        }

        let mut cmd = crate::util::hidden_command(&exe);
        crate::platform::die_with_parent(&mut cmd);
        if cfg!(unix) {
            cmd.env("LD_LIBRARY_PATH", lib_env);
        }
        cmd.args(&args)
            .current_dir(&exe_dir)
            .env("PATH", path_env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let command_line = std::iter::once(quote(&exe.display().to_string()))
            .chain(args.iter().map(|a| quote(a)))
            .collect::<Vec<_>>()
            .join(" ");
        self.log(format!("[앱] ===== 서버 시작 {} =====", crate::util::now_local_string()));
        self.log(format!("[앱] 실행: {command_line}"));

        let mut child = cmd
            .spawn()
            .with_context(|| format!("실행 실패: {}", exe.display()))?;
        if !job::adopt(&child) {
            self.log("[앱] 경고: Job Object 등록 실패 — 앱이 비정상 종료되면 서버가 남을 수 있습니다");
        }
        if let Some(out) = child.stdout.take() {
            spawn_log_reader(out, self.logs.clone(), self.log_file.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_reader(err, self.logs.clone(), self.log_file.clone());
        }
        let pid = child.id();
        *self.child.lock() = Some(child);

        let gen = {
            let mut s = self.shared.lock();
            s.generation += 1;
            s.state = ServerState::Starting;
            s.pid = Some(pid);
            s.started_at = Some(Instant::now());
            s.ready_at = None;
            s.port = cfg.port;
            s.command_line = command_line;
            s.selftest = SelfTest::NotRun;
            s.has_mmproj = cfg.has_mmproj();
            s.generation
        };
        let me = self.clone();
        std::thread::Builder::new()
            .name("server-monitor".into())
            .spawn(move || me.monitor(gen))?;
        Ok(())
    }

    /// 이미 떠 있는 서버(같은 포트)에 연결해서 사용
    pub fn connect_external(&self, cfg: &Config) -> Result<()> {
        if self.state().is_running() {
            bail!("앱이 띄운 서버가 실행 중입니다");
        }
        let client = EmbedClient::new(cfg.port);
        match client.health() {
            Ok(200) => {}
            Ok(code) => bail!("포트 {}의 서버가 아직 준비되지 않았습니다 (HTTP {code})", cfg.port),
            Err(e) => bail!("포트 {}에 연결할 수 없습니다: {e}", cfg.port),
        }
        let gen = {
            let mut s = self.shared.lock();
            s.generation += 1;
            s.state = ServerState::External;
            s.pid = None;
            s.started_at = Some(Instant::now());
            s.ready_at = Some(Instant::now());
            s.port = cfg.port;
            s.command_line = format!("(외부 서버) http://127.0.0.1:{}", cfg.port);
            s.selftest = SelfTest::NotRun;
            s.has_mmproj = true; // 외부 서버는 mmproj 여부를 알 수 없으니 진단으로 판별
            s.generation
        };
        self.log(format!("[앱] 외부 서버에 연결: 포트 {}", cfg.port));
        self.spawn_selftest();
        let me = self.clone();
        std::thread::spawn(move || me.monitor_external(gen));
        Ok(())
    }

    pub fn stop(&self) {
        {
            let mut s = self.shared.lock();
            s.generation += 1;
            s.state = ServerState::Stopped;
            s.pid = None;
            s.ready_at = None;
            s.selftest = SelfTest::NotRun;
        }
        if let Some(mut child) = self.child.lock().take() {
            let _ = child.kill();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(50))
                    }
                    _ => break,
                }
            }
            self.log("[앱] 서버를 중지했습니다");
        }
    }

    pub fn rerun_selftest(&self) {
        if self.state().is_ready() {
            self.spawn_selftest();
        }
    }

    fn spawn_selftest(&self) {
        let (port, has_mmproj) = {
            let mut s = self.shared.lock();
            s.selftest = SelfTest::Running;
            (s.port, s.has_mmproj)
        };
        let me = self.clone();
        std::thread::spawn(move || {
            let client = EmbedClient::new(port);
            let caps = selftest::run(&client, has_mmproj);
            me.log(format!(
                "[앱] 자가진단 ({}ms): 텍스트 {:?} / 이미지 {:?} / 오디오 {:?}",
                caps.elapsed_ms, caps.text, caps.image, caps.audio
            ));
            let mut s = me.shared.lock();
            if s.state.is_ready() {
                s.selftest = SelfTest::Done(caps);
            }
        });
    }

    fn monitor(&self, gen: u64) {
        let port = self.port();
        let client = EmbedClient::new(port);
        loop {
            if self.shared.lock().generation != gen {
                return;
            }
            // 프로세스 종료 감지
            let exited: Option<ExitStatus> = {
                let mut guard = self.child.lock();
                match guard.as_mut().map(|c| c.try_wait()) {
                    Some(Ok(Some(status))) => {
                        guard.take();
                        Some(status)
                    }
                    _ => None,
                }
            };
            if let Some(status) = exited {
                // 로그 리더가 마지막 줄을 받을 시간
                std::thread::sleep(Duration::from_millis(300));
                let msg = explain_exit(status, &self.logs.lock());
                self.log(format!("[앱] 서버 종료: {msg}"));
                let mut s = self.shared.lock();
                if s.generation == gen {
                    s.state = ServerState::Failed(msg);
                    s.pid = None;
                    s.ready_at = None;
                }
                return;
            }
            let state = self.state();
            let next = match client.health() {
                Ok(200) => ServerState::Ready,
                Ok(503) => ServerState::Loading,
                Ok(_) => ServerState::Loading,
                Err(_) => {
                    if state == ServerState::Ready {
                        ServerState::Ready // 일시적 지연은 무시 (프로세스 종료는 위에서 감지)
                    } else {
                        ServerState::Starting
                    }
                }
            };
            let became_ready = {
                let mut s = self.shared.lock();
                if s.generation != gen {
                    return;
                }
                let became = next == ServerState::Ready && s.state != ServerState::Ready;
                if became {
                    s.ready_at = Some(Instant::now());
                }
                s.state = next;
                became
            };
            if became_ready {
                self.log("[앱] 서버 준비 완료 — 자가진단 시작");
                self.spawn_selftest();
            }
            let wait = if self.state() == ServerState::Ready { 1000 } else { 400 };
            std::thread::sleep(Duration::from_millis(wait));
        }
    }

    fn monitor_external(&self, gen: u64) {
        let client = EmbedClient::new(self.port());
        let mut failures = 0;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            if self.shared.lock().generation != gen {
                return;
            }
            match client.health() {
                Ok(200) => failures = 0,
                _ => failures += 1,
            }
            if failures >= 3 {
                let mut s = self.shared.lock();
                if s.generation == gen {
                    s.state = ServerState::Failed("외부 서버가 응답하지 않습니다".into());
                    s.selftest = SelfTest::NotRun;
                }
                return;
            }
        }
    }
}

fn push_log(logs: &Mutex<VecDeque<String>>, file: &Mutex<Option<std::fs::File>>, line: String) {
    if let Some(f) = file.lock().as_mut() {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
    let mut l = logs.lock();
    l.push_back(line);
    while l.len() > MAX_LOG_LINES {
        l.pop_front();
    }
}

fn spawn_log_reader(
    stream: impl Read + Send + 'static,
    logs: Arc<Mutex<VecDeque<String>>>,
    file: Arc<Mutex<Option<std::fs::File>>>,
) {
    std::thread::spawn(move || {
        let mut r = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match r.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                    if !line.is_empty() {
                        push_log(&logs, &file, line);
                    }
                }
            }
        }
    });
}

/// 종료 코드와 마지막 로그를 사람이 읽을 수 있는 설명으로
/// 쓸 수 있는 포트: 원래 포트+10000 근처 → 그래도 없으면 OS가 고른 포트
fn free_port(preferred: u16) -> Option<u16> {
    let ok = |p: u16| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok();
    let base = preferred.checked_add(10_000).unwrap_or(18_090);
    (0..50u16)
        .filter_map(|i| base.checked_add(i))
        .find(|p| ok(*p))
        .or_else(|| {
            std::net::TcpListener::bind(("127.0.0.1", 0))
                .ok()
                .and_then(|l| l.local_addr().ok())
                .map(|a| a.port())
        })
}

pub fn explain_exit(status: ExitStatus, logs: &VecDeque<String>) -> String {
    let code = status.code().map(|c| c as u32);
    let mut msg = match code {
        Some(0) => "정상 종료".to_string(),
        Some(0xC000_0135) => "필요한 DLL을 찾지 못했습니다 (0xC0000135). CUDA 런타임(cudart/cublas) DLL이 llama 폴더에 있는지 확인하거나 설정의 '추가 DLL 경로'를 지정하세요".to_string(),
        Some(0xC000_007B) => "DLL 아키텍처가 맞지 않습니다 (0xC000007B)".to_string(),
        Some(0xC000_0005) => "접근 위반으로 종료되었습니다 (0xC0000005)".to_string(),
        Some(0xC000_0409) => "비정상 종료 (0xC0000409)".to_string(),
        Some(c) => format!("종료 코드 {} (0x{c:08X})", c as i32),
        None => "알 수 없는 이유로 종료".to_string(),
    };
    let tail: Vec<&String> = logs.iter().rev().take(40).collect();
    let has = |pat: &str| tail.iter().any(|l| l.to_lowercase().contains(pat));
    if has("unknown model architecture") {
        msg.push_str(" — 이 llama.cpp 빌드가 모델 아키텍처를 지원하지 않습니다. 더 최신 빌드가 필요합니다");
    } else if has("out of memory") || has("failed to allocate") {
        msg.push_str(" — 메모리(VRAM) 부족: -ngl 값을 줄이거나 컨텍스트를 줄이세요");
    } else if has("couldn't bind") || has("address already in use") {
        msg.push_str(" — 포트가 이미 사용 중입니다");
    } else if has("failed to load model") || has("error loading model") {
        msg.push_str(" — 모델 파일을 불러오지 못했습니다 (경로/파일 손상 확인)");
    } else if has("mmproj") && has("failed") {
        msg.push_str(" — mmproj 파일을 불러오지 못했습니다");
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::process::ExitStatusExt;

    #[test]
    fn dll_not_found_is_explained() {
        let st = ExitStatus::from_raw(0xC000_0135);
        let m = explain_exit(st, &VecDeque::new());
        assert!(m.contains("DLL"), "{m}");
    }

    #[test]
    fn unknown_arch_hint() {
        let st = ExitStatus::from_raw(1);
        let logs: VecDeque<String> =
            ["llama_model_load: error loading model: unknown model architecture: 'gemma-embedding2'".to_string()]
                .into();
        let m = explain_exit(st, &logs);
        assert!(m.contains("최신 빌드"), "{m}");
    }

    #[test]
    fn args_contain_embedding_flags() {
        let cfg = Config::default();
        let a = build_args(&cfg, false);
        assert!(a.contains(&"--embeddings".to_string()));
        assert!(a.windows(2).any(|w| w[0] == "--pooling" && w[1] == "mean"));
        assert!(a.windows(2).any(|w| w[0] == "--port" && w[1] == "8090"));
        assert!(a.windows(2).any(|w| w[0] == "-fa" && w[1] == "off"), "FA 기본 off");
        let s = build_args(&cfg, true);
        assert_eq!(s[0], "serve");
    }
}
