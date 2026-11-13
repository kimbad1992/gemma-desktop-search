//! 접속 기기 쪽: 메시 기기들의 상태를 주기적으로 묻는다 (스레드 하나)
//!
//! - 닿는지(화면의 "연결 안 됨"), 닿은 주소(다음 검색이 그 주소부터 시도), 소유 기기의 인덱싱 진행(원격 진행 표시,
//!   추가한 기기의 계산 빌려주기 시작·끝), 라이브러리 목록을 안다.
//! - 인덱싱 중인 기기가 있거나 방금 라이브러리를 붙였으면 자주(3초), 아니면 가끔(30초) 묻는다.

use super::client::{libraries, CallError, Owner};
use super::{LibraryInfo, MeshDevice};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const FAST: Duration = Duration::from_secs(3);
const SLOW: Duration = Duration::from_secs(30);
/// 라이브러리 목록은 이만큼마다만 다시 묻는다
const LIBRARIES_EVERY: Duration = Duration::from_secs(60);

/// 소유 기기의 인덱싱 진행 (`/api/status`의 `indexing`)
#[derive(Clone, Debug, Default)]
pub struct Indexing {
    pub active: bool,
    /// 받아 두고 아직 시작하지 않은 요청이 있다
    pub pending: bool,
    pub processed: u64,
    pub discovered: u64,
    pub fraction: f32,
    pub scanning: bool,
}

impl Indexing {
    /// 인덱싱 중이거나 곧 시작한다
    pub fn busy(&self) -> bool {
        self.active || self.pending
    }
}

/// 기기 하나의 상태
#[derive(Clone, Debug, Default)]
pub struct DeviceState {
    /// None = 아직 묻지 않음
    pub reachable: Option<bool>,
    /// 마지막으로 닿은 주소
    pub base: Option<String>,
    pub error: String,
    pub version: String,
    pub files: u64,
    pub indexing: Indexing,
    /// 인덱싱이 끝난 것을 본 때 (빌려주기를 끝내는 데 쓴다)
    pub idle_since: Option<Instant>,
    pub libraries: Vec<LibraryInfo>,
    pub checked: Option<Instant>,
}

struct Shared {
    devices: Mutex<Vec<MeshDevice>>,
    states: Mutex<HashMap<String, DeviceState>>,
    /// 이때까지는 자주 묻는다 (방금 붙였을 때)
    fast_until: Mutex<Option<Instant>>,
    poke: AtomicBool,
    stop: AtomicBool,
}

/// 상태를 묻는 스레드. 버리면 끝난다.
pub struct Monitor {
    shared: Arc<Shared>,
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Monitor {
    /// `changed`: 상태가 바뀌면 부른다 (화면 다시 그리기)
    pub fn start(changed: crate::util::Waker) -> Self {
        let shared = Arc::new(Shared {
            devices: Mutex::new(Vec::new()),
            states: Mutex::new(HashMap::new()),
            fast_until: Mutex::new(None),
            poke: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let s = shared.clone();
        let _ = std::thread::Builder::new().name("mesh-monitor".into()).spawn(move || run(s, changed));
        Self { shared }
    }

    /// 물을 기기들 (설정이 바뀌면 다시 준다). 바뀌었으면 바로 묻는다.
    pub fn set_devices(&self, devices: &[MeshDevice]) {
        let mut d = self.shared.devices.lock();
        if *d != devices {
            *d = devices.to_vec();
            self.shared.states.lock().retain(|id, _| devices.iter().any(|x| &x.id == id));
            self.shared.poke.store(true, Ordering::SeqCst);
        }
    }

    /// 지금 바로 묻고, `for_secs` 동안 자주 묻는다 (라이브러리를 막 붙였을 때)
    pub fn poke(&self, for_secs: u64) {
        *self.shared.fast_until.lock() = Some(Instant::now() + Duration::from_secs(for_secs));
        self.shared.poke.store(true, Ordering::SeqCst);
    }

    pub fn state(&self, device_id: &str) -> Option<DeviceState> {
        self.shared.states.lock().get(device_id).cloned()
    }

    /// 마지막으로 닿은 주소
    pub fn base(&self, device_id: &str) -> Option<String> {
        self.shared.states.lock().get(device_id).and_then(|s| s.base.clone())
    }

    /// 그 기기가 곧 인덱싱한다 (방금 라이브러리를 만들게 했다): 다음 확인 전에 "끝남"으로 보지 않게
    pub fn expect_indexing(&self, device_id: &str) {
        let mut st = self.shared.states.lock();
        let s = st.entry(device_id.to_string()).or_default();
        s.indexing.pending = true;
        s.idle_since = None;
    }

}

fn indexing_of(v: &serde_json::Value) -> Indexing {
    let i = &v["indexing"];
    Indexing {
        active: i["active"].as_bool().unwrap_or(false),
        pending: !i["pending"].is_null(),
        processed: i["processed"].as_u64().unwrap_or(0),
        discovered: i["discovered"].as_u64().unwrap_or(0),
        fraction: i["fraction"].as_f64().unwrap_or(0.0) as f32,
        scanning: i["scanning"].as_bool().unwrap_or(false),
    }
}

fn run(s: Arc<Shared>, changed: crate::util::Waker) {
    let mut next = Instant::now();
    while !s.stop.load(Ordering::SeqCst) {
        let poked = s.poke.swap(false, Ordering::SeqCst);
        if !poked && Instant::now() < next {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let devices = s.devices.lock().clone();
        let mut any_busy = false;
        for d in &devices {
            if s.stop.load(Ordering::SeqCst) {
                return;
            }
            let prev = s.states.lock().get(&d.id).cloned().unwrap_or_default();
            let owner = Owner::new(d, prev.base.as_deref(), Duration::from_secs(8));
            let mut st = prev.clone();
            st.checked = Some(Instant::now());
            match owner.json("GET", "/api/status", None) {
                Ok((base, v)) => {
                    st.reachable = Some(true);
                    st.base = Some(base);
                    st.error.clear();
                    st.version = v["version"].as_str().unwrap_or("").to_string();
                    st.files = v["index"]["files"].as_u64().unwrap_or(0);
                    st.indexing = indexing_of(&v);
                    if st.indexing.busy() {
                        st.idle_since = None;
                    } else if st.idle_since.is_none() {
                        st.idle_since = Some(Instant::now());
                    }
                    let stale = prev.libraries.is_empty()
                        || poked
                        || prev.checked.is_none_or(|t| t.elapsed() > LIBRARIES_EVERY)
                        || prev.indexing.busy() != st.indexing.busy();
                    if stale {
                        if let Ok(l) = libraries(&owner) {
                            st.libraries = l;
                        }
                    }
                }
                Err(e) => {
                    st.reachable = Some(false);
                    st.error = match &e {
                        CallError::Unreachable(_) => "연결 안 됨".to_string(),
                        e => e.to_string(),
                    };
                }
            }
            any_busy |= st.indexing.busy() && st.reachable == Some(true);
            let differs = st.reachable != prev.reachable
                || st.indexing.processed != prev.indexing.processed
                || st.indexing.busy() != prev.indexing.busy()
                || st.error != prev.error
                || st.files != prev.files;
            s.states.lock().insert(d.id.clone(), st);
            if differs {
                changed();
            }
        }
        let fast = any_busy || s.fast_until.lock().is_some_and(|t| Instant::now() < t);
        next = Instant::now() + if fast { FAST } else { SLOW };
    }
}
