//! 계산 빌려주기: 빌려주는 쪽 (메시 1단계 ④, `MESH-DESIGN.md` 8.2)
//!
//!   gemma-search lend --server <http://주소:포트> [--token <토큰>] [--name <이름>]
//!
//! 다른 기기의 서비스(소유 기기)가 인덱싱할 때, 이 기기의 llama-server로 임베딩을 나눠 맡는다.
//! - 이 기기는 나가는 접속만 한다: 소유 기기에 "일감 주세요"(`/api/lend/claim`)를 보내 받아 오고, 결과를
//!   돌려준다(`/api/lend/result`). 파일에는 닿지 않고, 받은 입력은 계산에만 쓰고 저장하지 않는다.
//! - 소유 기기는 서버 서명(llama.cpp 빌드와 모델 파일)이 같은 기기에만 일감을 준다. 다르면 까닭을 알리고 끝낸다.
//! - 이 기기 서버가 일감을 처리하다 죽으면 그 일감을 "처리하지 못함"으로 돌려준다(소유 기기가 다시 맡긴다).
//!   서버를 다시 켜 보고, 짧은 사이에 여러 번 죽으면 끝낸다.
//!
//! 빌려주는 일은 `LendSession`(스레드 하나)이 한다. 두 곳에서 쓴다.
//! - 명령줄 `gemma-search lend`: 세션이 llama-server를 직접 켜고, 한동안(`idle_stop_minutes`) 일감이 없으면 끄고,
//!   소유 기기에 기다리는 일감이 생기면 다시 켠다. 진행은 콘솔에 찍는다.
//! - 창 있는 앱(Windows): 앱의 llama-server를 같이 쓴다(서버가 둘 뜨지 않게). 서버가 필요하면 앱에 알리고, 끄는 것은
//!   앱의 자동 끄기에 맡긴다. 진행은 오른쪽 아래 오버레이에 보인다(`ui/lend.rs`, `ui/overlay.rs`).

use crate::config::Config;
use crate::embed::client::{EmbedClient, EmbedError, EmbedInput};
use crate::index::lend::{JobResult, LenderInfo};
use crate::server::{ServerManager, ServerState};
use crate::util::fmt_num;
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const USAGE: &str = "\
gemma-search lend --server <http://주소:포트>
  다른 기기의 서비스(gemma-search serve)가 인덱싱할 때, 이 기기의 그래픽으로 임베딩을 나눠 맡는다.
  이 기기는 나가는 접속만 한다(포트를 열지 않는다). Ctrl+C로 끝낸다.

  --server <주소>     그 서비스의 주소 (환경 변수 GEMMA_SEARCH_URL)
  --token <토큰>      그 서비스의 접속 토큰 (환경 변수 GEMMA_SEARCH_TOKEN, 서비스 쪽 설정의 service_token)
  --name <이름>       그 서비스의 status에 보일 이 기기의 이름 (기본: 컴퓨터 이름)

  llama-server는 이 기기의 설정(llama.cpp·모델 위치)으로 켜고, 한동안 일감이 없으면 끈다.
  두 기기의 llama.cpp 빌드와 모델 파일이 같아야 한다 (다르면 서비스가 까닭을 알려 준다).
  Windows의 창 있는 앱에서는 이 명령이 앱(트레이)에서 빌려주기를 시작하고, 진행을 화면 오른쪽 아래에 보인다.
  (--console을 주면 창 있는 빌드에서도 콘솔 방식으로 돈다)";

/// 일감 하나를 기다리는 시간 (소유 기기가 이만큼 붙잡고 있다가 없으면 빈 답을 준다)
const CLAIM_WAIT: Duration = Duration::from_secs(10);
/// 짧은 사이에 서버가 이만큼 죽으면 끝낸다
const MAX_SERVER_RESTARTS: usize = 3;
/// 슬롯이 다 차서 일감을 달라고 하지 않는 동안에도 이만큼마다 소식을 준다 (소유 기기는 15초 소식이 없으면
/// 이 기기가 빠진 것으로 보고 맡긴 일감을 다른 곳에 다시 맡긴다)
const HEARTBEAT: Duration = Duration::from_secs(5);
/// 서버가 준비되기를 기다리는 최대 시간
const SERVER_READY_WAIT: Duration = Duration::from_secs(300);

/// 빌려주기의 지금 단계
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Connecting,
    StartingServer,
    /// 일감을 받아 처리하는 중
    Working,
    /// 소유 기기는 인덱싱 중인데 지금 맡을 일감이 없다
    Waiting,
    /// 소유 기기가 인덱싱하고 있지 않다
    OwnerIdle,
    OwnerPaused,
    /// 이 기기에서 일시정지 (새 일감을 받지 않음)
    Paused,
    /// 쉬는 조건 (전체 화면 앱 등): 새 일감을 받지 않음
    Resting,
    /// 일감이 없어 서버를 껐다 (일감이 생기면 다시 켠다)
    ServerOff,
    Stopping,
    Stopped,
    Failed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Connecting => "소유 기기에 연결하는 중",
            Phase::StartingServer => "llama-server를 켜는 중",
            Phase::Working => "계산 빌려주는 중",
            Phase::Waiting => "맡을 일감을 기다리는 중",
            Phase::OwnerIdle => "소유 기기가 인덱싱하고 있지 않음",
            Phase::OwnerPaused => "소유 기기가 인덱싱을 일시정지함",
            Phase::Paused => "빌려주기 일시정지",
            Phase::Resting => "빌려주기 쉬는 중",
            Phase::ServerOff => "일감이 없어 서버를 껐음",
            Phase::Stopping => "빌려주기 끝내는 중",
            Phase::Stopped => "빌려주기 끝남",
            Phase::Failed => "빌려주기 멈춤",
        }
    }

    /// 아직 돌고 있는지
    pub fn is_live(self) -> bool {
        !matches!(self, Phase::Stopped | Phase::Failed)
    }

    /// 이 기기의 그래픽을 쓰는 중인지
    pub fn is_busy(self) -> bool {
        matches!(self, Phase::Working | Phase::StartingServer)
    }
}

/// 화면(오버레이·설정)과 명령줄에 보여 줄 상태
#[derive(Clone, Debug, Default)]
pub struct LendStatus {
    pub phase: Phase,
    /// 소유 기기의 주소
    pub owner: String,
    /// 이 기기 이름 (소유 기기의 status에 보이는 이름)
    pub name: String,
    pub signature: String,
    pub slots: u32,
    /// 맡아서 아직 결과를 보내지 않은 일감 수
    pub busy: usize,
    pub jobs: u64,
    pub inputs: u64,
    /// 처리하지 못해 돌려준 일감 수
    pub failed: u64,
    /// 최근 처리 속도 (입력/초, 약 20초 평균)
    pub rate: f64,
    pub last_job: Option<Instant>,
    /// 지금 단계의 까닭이나 마지막 문제 (쉬는 까닭, 오류 등)
    pub note: String,
}

impl LendStatus {
    /// 소유 기기 주소를 짧게 (http:// 뗀 것)
    pub fn owner_short(&self) -> &str {
        self.owner.split_once("://").map(|(_, r)| r).unwrap_or(&self.owner)
    }
}

/// 세션을 시작할 때 주는 것
pub struct LendParams {
    pub base: String,
    pub token: Option<String>,
    pub name: String,
    pub cfg: Config,
}

/// 세션이 바깥에 맡기는 일
#[derive(Clone)]
pub struct LendHooks {
    /// 서버가 필요하다: 켜 달라고 한다 (명령줄: 직접 켬, 앱: 앱이 켜도록 알림). 켜지 못하면 Err.
    pub want_server: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
    /// 쉬어야 하는지. Some(까닭)이면 새 일감을 받지 않는다 (앱: 전체 화면 앱 등)
    pub rest: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    /// 한동안 일감이 없으면 세션이 서버를 끈다 (명령줄). 앱에서는 앱의 자동 끄기에 맡긴다.
    pub stop_server_when_idle: bool,
    /// 끝날 때 서버를 끈다 (명령줄)
    pub stop_server_at_end: bool,
    /// 진행을 남긴다 (명령줄: 콘솔, 앱: 서버 로그)
    pub log: Arc<dyn Fn(&str) + Send + Sync>,
    /// 상태가 바뀌었다 (앱: 화면을 다시 그리게)
    pub changed: Arc<dyn Fn() + Send + Sync>,
}

/// 빌려주기 하나 (스레드 하나). 버리면 끝내라고 알리기만 하고 기다리지 않는다.
pub struct LendSession {
    pub status: Arc<Mutex<LendStatus>>,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    thread: Option<JoinHandle<i32>>,
}

impl LendSession {
    pub fn start(p: LendParams, server: ServerManager, hooks: LendHooks) -> Self {
        let status = Arc::new(Mutex::new(LendStatus {
            owner: p.base.clone(),
            name: p.name.clone(),
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let (st, sp, pa) = (status.clone(), stop.clone(), pause.clone());
        let thread = std::thread::Builder::new()
            .name("lend".into())
            .spawn(move || run(p, server, hooks, st, sp, pa))
            .ok();
        if thread.is_none() {
            let mut s = status.lock();
            s.phase = Phase::Failed;
            s.note = "빌려주기 스레드를 띄우지 못했습니다".into();
        }
        Self { status, stop, pause, thread }
    }

    pub fn status(&self) -> LendStatus {
        self.status.lock().clone()
    }

    /// 일시정지: 새 일감을 받지 않고, 아직 시작하지 않은 일감은 돌려준다 (처리 중인 것은 마저 끝낸다).
    /// 누르자마자 화면에 보이도록 상태도 바로 바꾼다.
    pub fn set_paused(&self, on: bool) {
        self.pause.store(on, Ordering::SeqCst);
        let mut s = self.status.lock();
        if s.phase.is_live() && s.phase != Phase::Stopping {
            s.phase = if on { Phase::Paused } else { Phase::Waiting };
            s.note.clear();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    /// 끝내라고 알린다 (받은 일감을 마저 처리하고 끝난다)
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// 끝나기를 기다려 종료 코드를 돌려준다
    pub fn join(mut self) -> i32 {
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or(1)
    }
}

impl Drop for LendSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 소유 기기의 서비스
struct Owner {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

enum Reply {
    Ok(Value),
    /// 서비스가 오류로 답함: (HTTP 상태, 까닭)
    Refused(u16, String),
}

impl Owner {
    fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply, String> {
        let url = format!("{}{path}", self.base);
        let auth = self.token.as_ref().map(|t| format!("Bearer {t}"));
        let r = if method == "GET" {
            let mut req = self.agent.get(&url);
            if let Some(a) = &auth {
                req = req.header("Authorization", a);
            }
            req.call()
        } else {
            let mut req = self.agent.post(&url);
            if let Some(a) = &auth {
                req = req.header("Authorization", a);
            }
            req.send_json(body.cloned().unwrap_or(Value::Null))
        };
        let mut r = r.map_err(|e| format!("{}에 닿지 않습니다: {e}", self.base))?;
        let status = r.status().as_u16();
        let v: Value = r
            .body_mut()
            .with_config()
            .limit(512 * 1024 * 1024)
            .read_json()
            .map_err(|e| format!("서비스의 응답을 읽지 못했습니다 (HTTP {status}): {e}"))?;
        if status != 200 {
            let why = v["error"].as_str().unwrap_or("").to_string();
            return Ok(Reply::Refused(status, why));
        }
        Ok(Reply::Ok(v))
    }
}

/// 받은 일감
struct Job {
    id: u64,
    inputs: Vec<Value>,
}

/// 서버가 준비된 동안의 상태: 일감을 처리하는 스레드들
struct Pool {
    info: LenderInfo,
    tx: Option<Sender<Job>>,
    /// 아직 시작하지 않은 일감을 꺼내 돌려줄 때 쓴다
    rx: Receiver<Job>,
    threads: Vec<JoinHandle<()>>,
    /// 맡아서 아직 결과를 보내지 않은 일감 수
    busy: Arc<AtomicUsize>,
}

impl Pool {
    fn free(&self) -> usize {
        (self.info.slots as usize).saturating_sub(self.busy.load(Ordering::SeqCst))
    }

    /// 아직 시작하지 않은 일감을 꺼낸다 (일시정지·쉬는 조건에서 소유 기기에 돌려준다)
    fn take_queued(&self) -> Vec<Job> {
        let mut out = Vec::new();
        while let Ok(j) = self.rx.try_recv() {
            self.busy.fetch_sub(1, Ordering::SeqCst);
            out.push(j);
        }
        out
    }

    /// 받은 일감을 다 처리하고 끝낸다
    fn finish(mut self) {
        self.tx.take();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// 처리한 양
#[derive(Default)]
struct Counters {
    jobs: AtomicU64,
    inputs: AtomicU64,
    failed: AtomicU64,
}

/// 이 기기의 이름: 컴퓨터 이름, 없으면 "이름 없는 기기".
/// Linux는 지금 쓰는 호스트 이름(커널)을 `/etc/hostname`보다 먼저 본다. 둘이 다를 수 있고(`lee_ubuntu`처럼
/// 호스트 이름에 쓸 수 없는 글자가 든 파일), 다른 기기는 지금 이름으로 이 기기를 부른다(`\\leeubuntu\…`).
pub fn machine_name() -> String {
    let from_env = ["COMPUTERNAME", "HOSTNAME"].iter().find_map(|k| std::env::var(k).ok());
    let from_kernel = || std::fs::read_to_string("/proc/sys/kernel/hostname").ok();
    let from_file = || std::fs::read_to_string("/etc/hostname").ok();
    [from_env, from_kernel(), from_file()]
        .into_iter()
        .flatten()
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "이름 없는 기기".to_string())
}

/// 이 실행의 이름표 (표준 라이브러리의 해시 키가 OS 난수로 정해지는 것을 쓴다)
fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:016x}", h.finish())
}

/// 오류의 속 내용만 (소유 기기가 "빌려준 기기" 표시를 붙인다)
fn inner(e: EmbedError) -> String {
    match e {
        EmbedError::Transport(m) | EmbedError::Server(m) | EmbedError::Invalid(m) => m,
    }
}

/// 일감 하나를 이 기기 서버로 임베딩한다. 묶음이 실패하면(연결 끊김 말고) 하나씩 다시 해 본다.
/// 이 기기 서버에 닿지 않으면 Err: 소유 기기가 다른 곳에 다시 맡긴다.
fn run_job(client: &EmbedClient, inputs: &[Value]) -> Result<Vec<JobResult>, String> {
    match client.embed_values(inputs) {
        Ok(v) => Ok(v.into_iter().map(JobResult::Vector).collect()),
        Err(EmbedError::Transport(m)) => Err(format!("빌려주는 기기의 서버에 닿지 않습니다: {m}")),
        Err(e) if inputs.len() == 1 => Ok(vec![JobResult::Error(inner(e))]),
        Err(_) => {
            let mut out = Vec::with_capacity(inputs.len());
            for one in inputs {
                match client.embed_values(std::slice::from_ref(one)) {
                    Ok(mut v) => out.push(JobResult::Vector(v.remove(0))),
                    Err(EmbedError::Transport(m)) => return Err(format!("빌려주는 기기의 서버에 닿지 않습니다: {m}")),
                    Err(e) => out.push(JobResult::Error(inner(e))),
                }
            }
            Ok(out)
        }
    }
}

/// 결과를 돌려준다. 소유 기기에 닿지 않으면 몇 번 다시 해 보고 버린다(소유 기기가 다시 맡긴다).
fn send_result(owner: &Owner, lender: &str, id: u64, outcome: &Result<Vec<JobResult>, String>, log: &dyn Fn(&str)) {
    let body = match outcome {
        Ok(results) => json!({ "lender": lender, "id": id, "results": results }),
        Err(why) => json!({ "lender": lender, "id": id, "error": why }),
    };
    for attempt in 0..3 {
        match owner.send("POST", "/api/lend/result", Some(&body)) {
            Ok(_) => return,
            Err(e) if attempt == 2 => log(&format!("결과를 돌려주지 못했습니다 (소유 기기가 다시 맡깁니다): {e}")),
            Err(_) => std::thread::sleep(Duration::from_secs(1)),
        }
    }
}

fn start_pool(info: LenderInfo, client: Arc<EmbedClient>, owner: Arc<Owner>, counters: Arc<Counters>, hooks: &LendHooks) -> Pool {
    let n = info.slots.max(1) as usize;
    let (tx, rx): (Sender<Job>, Receiver<Job>) = bounded(n);
    let busy = Arc::new(AtomicUsize::new(0));
    let mut threads = Vec::new();
    for i in 0..n {
        let (rx, client, owner, busy, counters, lender, log) = (
            rx.clone(),
            client.clone(),
            owner.clone(),
            busy.clone(),
            counters.clone(),
            info.id.clone(),
            hooks.log.clone(),
        );
        if let Ok(t) = std::thread::Builder::new().name(format!("lend-{i}")).spawn(move || {
            for job in rx.iter() {
                let outcome = run_job(&client, &job.inputs);
                match &outcome {
                    Ok(_) => {
                        counters.jobs.fetch_add(1, Ordering::Relaxed);
                        counters.inputs.fetch_add(job.inputs.len() as u64, Ordering::Relaxed);
                    }
                    Err(_) => {
                        counters.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
                send_result(&owner, &lender, job.id, &outcome, &*log);
                busy.fetch_sub(1, Ordering::SeqCst);
            }
        }) {
            threads.push(t);
        }
    }
    Pool { info, tx: Some(tx), rx, threads, busy }
}

/// 시작하지 않은 일감을 돌려준다 (소유 기기가 실패로 세지 않고 바로 다시 맡긴다)
fn give_back(owner: &Owner, lender: &str, jobs: Vec<Job>) {
    for j in jobs {
        let body = json!({ "lender": lender, "id": j.id, "give_back": true });
        let _ = owner.send("POST", "/api/lend/result", Some(&body));
    }
}

/// 서버를 쓸 수 있게 하고(필요하면 켜 달라고 하고) 준비되면, 소유 기기에 알릴 것을 만든다
fn bring_up(
    server: &ServerManager,
    cfg: &Config,
    hooks: &LendHooks,
    stop: &AtomicBool,
    id: &str,
    name: &str,
) -> Result<(LenderInfo, Arc<EmbedClient>), String> {
    if !server.state().is_ready() {
        (hooks.want_server)().map_err(|e| format!("llama-server를 켜지 못했습니다: {e}"))?;
    }
    let t0 = Instant::now();
    let caps = loop {
        if let Some(c) = server.capabilities() {
            break c;
        }
        if let ServerState::Failed(m) = server.state() {
            return Err(format!("llama-server가 멈췄습니다: {m}"));
        }
        if stop.load(Ordering::SeqCst) {
            return Err("끝내라는 요청을 받았습니다".to_string());
        }
        if t0.elapsed() > SERVER_READY_WAIT {
            return Err("llama-server가 5분 안에 준비되지 않았습니다".to_string());
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    if !caps.text.usable() {
        return Err("llama-server 자가진단(텍스트)이 통과하지 않았습니다".to_string());
    }
    let client = Arc::new(EmbedClient::new(&server.target()));
    let props = server.props().unwrap_or_default();
    // 입력 한계: 이 기기 설정으로 켠 서버면 그 설정의 한계, 슬롯 컨텍스트가 더 작으면 그쪽
    let mut max_input = cfg.max_input_tokens() as u32;
    if props.slot_ctx > 16 {
        max_input = max_input.min(props.slot_ctx - 16);
    }
    // 영상 입력: 2초짜리 시험 영상을 실제로 보내 본다 (서버가 ffmpeg로 풀 수 있어야 한다)
    let video = caps.image.usable()
        && cfg.ffmpeg().is_some_and(|ff| {
            crate::extract::media::test_clip(&ff).is_ok_and(|clip| client.embed(&[EmbedInput::Video(clip)]).is_ok())
        });
    let info = LenderInfo {
        id: id.to_string(),
        name: name.to_string(),
        signature: server.signature(cfg),
        slots: if props.slots > 0 { props.slots } else { cfg.parallel.max(1) },
        max_input,
        image: caps.image.usable(),
        audio: caps.audio.usable(),
        video,
    };
    Ok((info, client))
}

/// 빌려주기 본체 (세션 스레드). 종료 코드: 0 정상, 1 실패.
fn run(
    p: LendParams,
    server: ServerManager,
    hooks: LendHooks,
    status: Arc<Mutex<LendStatus>>,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
) -> i32 {
    let log = |m: &str| (hooks.log)(m);
    let set = |f: &dyn Fn(&mut LendStatus)| {
        f(&mut status.lock());
        (hooks.changed)();
    };
    let fail = |why: String| -> i32 {
        log(&why);
        set(&|s| {
            s.phase = Phase::Failed;
            s.note = why.clone();
        });
        1
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(CLAIM_WAIT + Duration::from_secs(50)))
        .http_status_as_error(false)
        .build()
        .into();
    let owner = Arc::new(Owner { base: p.base.clone(), token: p.token.clone(), agent });
    let cfg = p.cfg;
    let (id, name) = (new_id(), p.name.clone());

    // 소유 기기에 닿는지, 토큰이 맞는지 먼저 본다
    set(&|s| s.phase = Phase::Connecting);
    match owner.send("GET", "/api/status", None) {
        Ok(Reply::Ok(v)) => log(&format!(
            "소유 기기: {} (Gemma Search v{} · 파일 {}개) · 이 기기 이름: {name}",
            p.base,
            v["version"].as_str().unwrap_or("?"),
            fmt_num(v["index"]["files"].as_u64().unwrap_or(0))
        )),
        Ok(Reply::Refused(401, _)) => {
            return fail(format!(
                "{}가 접속 토큰을 받지 않습니다. 그 서비스의 service_token을 주세요 (--token, GEMMA_SEARCH_TOKEN, 앱 설정)",
                p.base
            ))
        }
        Ok(Reply::Refused(code, why)) => return fail(format!("{}가 HTTP {code}로 답했습니다: {why}", p.base)),
        Err(e) => {
            return fail(format!(
                "{e}. 그 기기에서 서비스(gemma-search serve)가 떠 있는지, 이 기기에서 닿는 주소로 열려 있는지(--bind) 확인하세요"
            ))
        }
    }

    let counters = Arc::new(Counters::default());
    let idle = Duration::from_secs(cfg.idle_stop_minutes as u64 * 60);
    let mut pool: Option<Pool> = None;
    let mut last_job = Instant::now();
    let mut restarts: Vec<Instant> = Vec::new();
    let mut backoff = Duration::from_secs(2);
    let mut last_report = Instant::now();
    let mut reported = (0u64, 0u64, 0u64);
    let mut announced = Phase::Connecting;
    let mut last_contact = Instant::now();
    let mut samples: VecDeque<(Instant, u64)> = VecDeque::new();
    // 처음에는 바로 켜서, 서명이 맞는지 일찍 알 수 있게 한다
    let mut want_server = true;
    let mut code = 0;
    // 단계가 바뀌면 남긴다 (같은 단계를 되풀이해 찍지 않게)
    let phase_to = |ph: Phase, note: &str, announced: &mut Phase| {
        if *announced != ph {
            log(&if note.is_empty() { ph.label().to_string() } else { format!("{} ({note})", ph.label()) });
            *announced = ph;
        }
        let note = note.to_string();
        set(&|s| {
            s.phase = ph;
            s.note = note.clone();
        });
    };

    while !stop.load(Ordering::SeqCst) {
        // 숫자와 최근 속도
        let now = (
            counters.jobs.load(Ordering::Relaxed),
            counters.inputs.load(Ordering::Relaxed),
            counters.failed.load(Ordering::Relaxed),
        );
        samples.push_back((Instant::now(), now.1));
        while samples.front().is_some_and(|(t, _)| t.elapsed() > Duration::from_secs(20)) {
            samples.pop_front();
        }
        let rate = match (samples.front(), samples.back()) {
            (Some((t0, n0)), Some((t1, n1))) if *t1 > *t0 => (n1 - n0) as f64 / (*t1 - *t0).as_secs_f64(),
            _ => 0.0,
        };
        let busy_now = pool.as_ref().map(|p| p.busy.load(Ordering::SeqCst)).unwrap_or(0);
        {
            let mut s = status.lock();
            s.jobs = now.0;
            s.inputs = now.1;
            s.failed = now.2;
            s.rate = rate;
            s.busy = busy_now;
            if busy_now > 0 {
                s.last_job = Some(Instant::now());
            }
        }
        // 1분마다 처리한 양
        if last_report.elapsed() >= Duration::from_secs(60) {
            if now != reported {
                let secs = last_report.elapsed().as_secs_f64();
                log(&format!(
                    "지난 {:.0}초: 일감 {}개 · 입력 {}개 (초당 {:.1}){}",
                    secs,
                    fmt_num(now.0 - reported.0),
                    fmt_num(now.1 - reported.1),
                    (now.1 - reported.1) as f64 / secs,
                    if now.2 > reported.2 { format!(" · 처리하지 못함 {}개", now.2 - reported.2) } else { String::new() }
                ));
            }
            reported = now;
            last_report = Instant::now();
        }
        // 서버가 죽었거나(다시 켠다) 꺼졌으면(앱이 쓰지 않을 때 끔: 일감이 생기면 다시 켠다) 정리
        if pool.is_some() {
            match server.state() {
                ServerState::Failed(m) => {
                    log(&format!("llama-server가 멈췄습니다: {m}"));
                    if let Some(p) = pool.take() {
                        p.finish();
                    }
                    restarts.retain(|t| t.elapsed() < Duration::from_secs(600));
                    restarts.push(Instant::now());
                    if restarts.len() > MAX_SERVER_RESTARTS {
                        code = fail(format!("llama-server가 10분 안에 {MAX_SERVER_RESTARTS}번 넘게 멈춰 빌려주기를 끝냅니다"));
                        break;
                    }
                    want_server = true;
                }
                ServerState::Stopped => {
                    if let Some(p) = pool.take() {
                        p.finish();
                    }
                    phase_to(Phase::ServerOff, "", &mut announced);
                }
                _ => {}
            }
        }
        // 일시정지·쉬는 조건: 새 일감을 받지 않는다 (받은 일감은 마저 처리하고, 처리하는 동안 신호를 보낸다)
        let hold = if pause.load(Ordering::SeqCst) {
            Some((Phase::Paused, String::new()))
        } else {
            (hooks.rest)().map(|why| (Phase::Resting, why))
        };
        if let Some((ph, why)) = hold {
            // 아직 시작하지 않은 일감은 바로 돌려준다 (처리 중인 것만 마저 끝낸다)
            if let Some(p) = &pool {
                let queued = p.take_queued();
                if !queued.is_empty() {
                    log(&format!("시작하지 않은 일감 {}개를 소유 기기에 돌려줍니다", queued.len()));
                    give_back(&owner, &p.info.id, queued);
                }
            }
            let busy_left = pool.as_ref().map(|p| p.busy.load(Ordering::SeqCst)).unwrap_or(0);
            let why = match (why.is_empty(), busy_left) {
                (_, 0) => why,
                (true, n) => format!("받은 일감 {n}개를 마저 처리하는 중"),
                (false, n) => format!("{why} · 받은 일감 {n}개를 마저 처리하는 중"),
            };
            phase_to(ph, &why, &mut announced);
            if let Some(p) = &pool {
                if p.busy.load(Ordering::SeqCst) > 0 && last_contact.elapsed() >= HEARTBEAT {
                    let beat = json!({ "lender": p.info, "want": 0, "wait_ms": 0 });
                    if owner.send("POST", "/api/lend/claim", Some(&beat)).is_ok() {
                        last_contact = Instant::now();
                    }
                }
            }
            // 쉬는 동안은 다시 시작할 때 처음부터 일감이 없던 것으로 보지 않게
            last_job = Instant::now();
            std::thread::sleep(Duration::from_millis(300));
            continue;
        }
        let Some(p) = &pool else {
            if want_server {
                phase_to(Phase::StartingServer, "", &mut announced);
                match bring_up(&server, &cfg, &hooks, &stop, &id, &name) {
                    Ok((info, client)) => {
                        log(&format!(
                            "준비됨: {} · 슬롯 {}개 · 입력 한계 {}토큰 · 그림 {} · 소리 {} · 영상 {}",
                            info.signature,
                            info.slots,
                            info.max_input,
                            if info.image { "O" } else { "X" },
                            if info.audio { "O" } else { "X" },
                            if info.video { "O" } else { "X" }
                        ));
                        let (sig, slots) = (info.signature.clone(), info.slots);
                        set(&|s| {
                            s.signature = sig.clone();
                            s.slots = slots;
                        });
                        pool = Some(start_pool(info, client, owner.clone(), counters.clone(), &hooks));
                        last_job = Instant::now();
                        want_server = false;
                    }
                    Err(e) if stop.load(Ordering::SeqCst) => {
                        log(&e);
                        break;
                    }
                    Err(e) => {
                        code = fail(e);
                        break;
                    }
                }
                continue;
            }
            // 서버를 꺼 둔 동안: 소유 기기에 기다리는 일감이 있는지만 묻는다
            let probe = LenderInfo { id: id.clone(), name: name.clone(), ..Default::default() };
            let body = json!({ "lender": probe, "want": 0, "wait_ms": CLAIM_WAIT.as_millis() as u64 });
            match owner.send("POST", "/api/lend/claim", Some(&body)) {
                Ok(Reply::Ok(v)) => {
                    backoff = Duration::from_secs(2);
                    if v["pending"].as_u64().unwrap_or(0) > 0 {
                        log("소유 기기에 일감이 생겼습니다");
                        want_server = true;
                    }
                }
                Ok(Reply::Refused(c, why)) => {
                    phase_to(Phase::ServerOff, &format!("소유 기기가 HTTP {c}로 답함: {why}"), &mut announced);
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
                Err(e) => {
                    phase_to(Phase::ServerOff, &format!("{e} (다시 해 봅니다)"), &mut announced);
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                }
            }
            continue;
        };
        let free = p.free();
        if free == 0 {
            // 일하는 중이라는 신호: 일감 수만 묻고 바로 돌아오는 요청
            if last_contact.elapsed() >= HEARTBEAT {
                let beat = json!({ "lender": p.info, "want": 0, "wait_ms": 0 });
                if owner.send("POST", "/api/lend/claim", Some(&beat)).is_ok() {
                    last_contact = Instant::now();
                }
            }
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        let body = json!({ "lender": p.info, "want": free, "wait_ms": CLAIM_WAIT.as_millis() as u64 });
        match owner.send("POST", "/api/lend/claim", Some(&body)) {
            Ok(Reply::Ok(v)) => {
                backoff = Duration::from_secs(2);
                last_contact = Instant::now();
                let state = v["state"].as_str().unwrap_or("").to_string();
                let jobs = v["jobs"].as_array().cloned().unwrap_or_default();
                if !jobs.is_empty() {
                    phase_to(Phase::Working, "", &mut announced);
                    last_job = Instant::now();
                    for j in jobs {
                        let job = Job {
                            id: j["id"].as_u64().unwrap_or(u64::MAX),
                            inputs: j["inputs"].as_array().cloned().unwrap_or_default(),
                        };
                        p.busy.fetch_add(1, Ordering::SeqCst);
                        if let Some(tx) = &p.tx {
                            if tx.send(job).is_err() {
                                p.busy.fetch_sub(1, Ordering::SeqCst);
                            }
                        }
                    }
                } else {
                    if p.busy.load(Ordering::SeqCst) == 0 {
                        let ph = match state.as_str() {
                            "idle" => Phase::OwnerIdle,
                            "paused" => Phase::OwnerPaused,
                            _ => Phase::Waiting,
                        };
                        phase_to(ph, "", &mut announced);
                    }
                    // 한동안 일감이 없으면 서버를 꺼서 그래픽 메모리를 돌려준다 (명령줄. 앱은 앱이 끈다)
                    if hooks.stop_server_when_idle
                        && !idle.is_zero()
                        && p.busy.load(Ordering::SeqCst) == 0
                        && last_job.elapsed() > idle
                    {
                        log(&format!("{}분 동안 일감이 없어 llama-server를 끕니다 (일감이 생기면 다시 켭니다)", cfg.idle_stop_minutes));
                        if let Some(p) = pool.take() {
                            p.finish();
                        }
                        server.stop();
                        phase_to(Phase::ServerOff, "", &mut announced);
                    }
                }
            }
            Ok(Reply::Refused(409, why)) => {
                code = fail(format!("소유 기기가 이 기기에 일감을 줄 수 없다고 합니다: {why}"));
                break;
            }
            Ok(Reply::Refused(c, why)) => {
                set(&|s| s.note = format!("소유 기기가 HTTP {c}로 답함: {why}"));
                log(&format!("소유 기기가 HTTP {c}로 답했습니다: {why}"));
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
            Err(e) => {
                set(&|s| s.note = format!("{e} (다시 해 봅니다)"));
                log(&format!("{e} (다시 해 봅니다)"));
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
    if stop.load(Ordering::SeqCst) && code == 0 {
        set(&|s| s.phase = Phase::Stopping);
        log("끝내는 중: 받은 일감을 마저 처리합니다");
    }
    if let Some(p) = pool.take() {
        p.finish();
    }
    if hooks.stop_server_at_end {
        server.stop();
    }
    let (jobs, inputs) = (counters.jobs.load(Ordering::Relaxed), counters.inputs.load(Ordering::Relaxed));
    log(&format!("끝: 일감 {}개 · 입력 {}개 처리", fmt_num(jobs), fmt_num(inputs)));
    set(&|s| {
        s.jobs = jobs;
        s.inputs = inputs;
        s.busy = 0;
        s.rate = 0.0;
        if s.phase != Phase::Failed {
            s.phase = Phase::Stopped;
        }
    });
    code
}

/// 명령줄 인자에서 소유 기기 주소·토큰·이름을 읽는다. 주소가 없으면 None.
pub fn params_from_args(args: &[String], cfg: &Config) -> Option<(String, Option<String>, String)> {
    let value = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).cloned();
    let env = |k: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let given = value("--server").or_else(|| value("--url")).or_else(|| env("GEMMA_SEARCH_URL"))?;
    let base = crate::cli::base_url(&given);
    let token = crate::cli::pick_token(
        value("--token").as_deref(),
        env("GEMMA_SEARCH_TOKEN").as_deref(),
        &cfg.service_token,
        crate::cli::is_local(&base),
    );
    let name = value("--name").unwrap_or_else(machine_name);
    Some((base, token, name))
}

/// 명령줄 `gemma-search lend` (콘솔에 진행을 찍는다)
pub fn lend(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let data_dir = crate::config::data_dir();
    let cfg = Config::load(&data_dir);
    let Some((base, token, name)) = params_from_args(args, &cfg) else {
        eprintln!("소유 기기 서비스의 주소가 필요합니다: --server <http://주소:포트>\n\n{USAGE}");
        return 2;
    };
    println!("Gemma Search v{} 계산 빌려주기", env!("CARGO_PKG_VERSION"));
    println!("데이터 폴더: {}", data_dir.display());
    let server = ServerManager::default();
    server.set_log_path(&lend_log_path(&data_dir));
    let stop_requested = Arc::new(AtomicBool::new(false));
    {
        let s = stop_requested.clone();
        if let Err(e) = ctrlc::set_handler(move || s.store(true, Ordering::SeqCst)) {
            eprintln!("종료 신호를 받을 수 없습니다: {e}");
        }
    }
    let srv = server.clone();
    let start_cfg = cfg.clone();
    let hooks = LendHooks {
        want_server: Arc::new(move || {
            if srv.state().is_running() || srv.state().is_ready() {
                return Ok(());
            }
            srv.bring_up(&start_cfg).map_err(|e| e.to_string())
        }),
        rest: Arc::new(|| None),
        stop_server_when_idle: true,
        stop_server_at_end: true,
        log: Arc::new(|m: &str| println!("[{}] {m}", crate::util::now_local_string())),
        changed: Arc::new(|| {}),
    };
    let session = LendSession::start(LendParams { base, token, name, cfg }, server, hooks);
    while !session.is_finished() {
        if stop_requested.load(Ordering::SeqCst) {
            session.stop();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let failed = session.status().phase == Phase::Failed;
    let code = session.join();
    if failed {
        1
    } else {
        code
    }
}

/// 빌려줄 때 쓰는 llama-server의 로그 (명령줄)
pub fn lend_log_path(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("logs").join("llama-server-lend.log")
}
