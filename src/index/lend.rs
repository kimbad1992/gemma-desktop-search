//! 계산 빌려주기: 소유 기기 쪽 (메시 1단계 ④, `MESH-DESIGN.md` 8.1·8.5)
//!
//! 빌려주는 기기(`gemma-search lend`)는 서비스에 나가는 접속만 한다. `POST /api/lend/claim`으로 일감을 달라고 하면
//! 이 허브가 인덱서의 일감 대기열에서 일감을 꺼내 맡기고, `POST /api/lend/result`로 결과가 오면 인덱서에 돌려준다.
//!
//! - 맡기는 조건: 빌려주는 기기의 서버 서명(빌드·모델)이 이번 인덱싱의 서명과 같고, 입력 한계(슬롯 하나에 들어가는
//!   토큰 수)가 이 기기 설정 이상이고, 이번 인덱싱에 필요한 입력 종류(그림·소리·영상)를 받을 수 있어야 한다.
//! - 빌려주는 기기는 일하는 동안에도 몇 초마다 소식을 준다(일감 요청·결과·신호). `LENDER_SILENT` 동안 소식이
//!   없으면 빠진 것으로 보고 그 기기에 맡긴 일감을 바로 다시 대기열에 넣는다(`WorkQueue::requeue`). 결과 모으기가
//!   그 일감을 기다리느라 인덱싱이 멈춰 있지 않게 하려는 것이다.
//! - 일감마다 마감도 있다(살아는 있는데 멈춘 기기에 대한 마지막 안전장치). 넘기거나 그 기기가 실패를 알려도 다시
//!   맡긴다. 늦게 온 결과는 버린다(먼저 온 결과를 쓴다).
//! - 빌려주는 기기가 빠지는 것은 이 기기 서버가 죽는 것과 다르다: 인덱싱을 멈추지 않고, 충돌 의심 파일로 남기지
//!   않는다. 같은 일감이 빌려준 기기에서 여러 번 실패하면 그 일감은 이 기기에서만 처리한다.
//! - 일시정지 중에는 새 일감을 맡기지 않는다(맡긴 것의 결과는 받는다). 중지하면 맡긴 일감을 버린다.

use super::queue::{Next, Work, WorkQueue, WorkReply};
use crate::embed::client::{EmbedError, EmbedInput};
use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 같은 일감이 빌려준 기기에서 이만큼 실패하면 이 기기에서만 처리한다
pub const REMOTE_FAILURE_LIMIT: u8 = 2;
/// 한 번의 요청이 일감을 기다리는 최대 시간
pub const MAX_WAIT: Duration = Duration::from_secs(20);
/// 기다리는 동안 상태를 다시 보는 간격
const TICK: Duration = Duration::from_millis(250);
/// 빌려주는 기기에서 이만큼 소식이 없으면 빠진 것으로 본다 (빌려주는 기기는 5초마다 신호를 보낸다)
pub const LENDER_SILENT: Duration = Duration::from_secs(15);

/// 인덱싱의 진행 신호 (인덱서의 `Control`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    Go,
    Pause,
    Stop,
}

/// 이번 인덱싱에 필요한 입력 종류
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Needs {
    pub image: bool,
    pub audio: bool,
    pub video: bool,
}

/// 빌려주는 기기가 일감을 달라고 할 때 알리는 것
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LenderInfo {
    /// 이 실행의 이름표 (같은 기기에서 다시 띄우면 바뀐다)
    pub id: String,
    /// 기기 이름 (화면 표시용)
    pub name: String,
    /// 그 기기 llama-server의 서명 (`ServerProps::signature`)
    pub signature: String,
    /// 동시에 처리할 수 있는 일감 수 (서버 슬롯)
    pub slots: u32,
    /// 입력 하나에 들어가는 최대 토큰 수
    pub max_input: u32,
    #[serde(default)]
    pub image: bool,
    #[serde(default)]
    pub audio: bool,
    #[serde(default)]
    pub video: bool,
}

/// 인덱서가 붙일 때 주는 것
pub struct RunSpec {
    pub queue: Arc<WorkQueue>,
    pub rx: Receiver<Work>,
    /// 이번 인덱싱의 서버 서명 (빌려주는 기기도 같아야 한다)
    pub signature: String,
    /// 이 기기 설정의 입력 한계 (`Config::max_input_tokens`)
    pub max_input: u32,
    pub needs: Needs,
    pub gate: Arc<dyn Fn() -> Gate + Send + Sync>,
}

struct Lent {
    work: Work,
    deadline: Instant,
    /// 맡은 기기의 이름표
    lender: String,
}

struct Run {
    spec: RunSpec,
    lent: HashMap<u64, Lent>,
}

/// 빌려주는 기기마다의 기록 (화면·status용)
#[derive(Clone, Debug)]
pub struct LenderStat {
    pub name: String,
    pub last_seen: Instant,
    /// 맡아 끝낸 일감 수와 입력 수
    pub jobs: u64,
    pub inputs: u64,
    /// 실패를 알렸거나 마감을 넘긴 일감 수
    pub failures: u64,
    /// 그 기기가 알린 슬롯 수 (기다리는 일감 수만 묻는 요청은 0)
    pub slots: u32,
    /// 맡길 수 없는 까닭 (서명이 다르다 등). 맡길 수 있으면 None
    pub rejected: Option<String>,
}

/// 결과 하나: 벡터, 또는 그 입력의 오류
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum JobResult {
    #[serde(rename = "v")]
    Vector(Vec<f32>),
    #[serde(rename = "e")]
    Error(String),
}

/// 일감을 달라는 요청에 대한 답
#[derive(Debug, Default)]
pub struct ClaimReply {
    /// 맡긴 일감: {"id", "inputs": [llama-server 입력…], "deadline_secs"}
    pub jobs: Vec<Value>,
    /// 꺼내 가기를 기다리는 일감 수
    pub pending: usize,
    /// "indexing", "paused", "stopping", "idle"
    pub state: &'static str,
    /// 맡길 수 없는 까닭이 있으면
    pub rejected: Option<String>,
}

#[derive(Default)]
pub struct LendHub {
    run: Mutex<Option<Run>>,
    /// 인덱싱마다 하나씩 늘어난다 (마감 감시 스레드가 자기 실행이 끝났는지 안다)
    generation: AtomicU64,
    /// 이름표 → 기록
    lenders: Mutex<HashMap<String, LenderStat>>,
    /// 마지막으로 끝난 인덱싱의 처리 몫
    last_share: Mutex<Vec<(String, u64)>>,
}

/// 인덱싱이 끝나면(버리면) 허브에서 떨어진다
pub struct Attached {
    hub: Arc<LendHub>,
    generation: u64,
}

impl Drop for Attached {
    fn drop(&mut self) {
        self.hub.detach(self.generation);
    }
}

/// 일감 하나의 마감: 넉넉히 잡는다 (느린 기기에서 영상 구간이 오래 걸릴 수 있다)
pub fn deadline_for(inputs: &[EmbedInput]) -> Duration {
    let per: u64 = inputs
        .iter()
        .map(|i| match i {
            EmbedInput::Text(_) => 2,
            EmbedInput::Image(_) => 5,
            EmbedInput::Audio(_) => 15,
            EmbedInput::Video(_) => 30,
        })
        .sum();
    Duration::from_secs(60 + per)
}

impl LendHub {
    /// 인덱싱을 붙인다. 돌려받은 것을 버리면 떨어진다.
    pub fn attach(self: &Arc<Self>, spec: RunSpec) -> Attached {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.run.lock() = Some(Run { spec, lent: HashMap::new() });
        // 마감 감시: 일감을 달라는 요청이 끊겨도 마감을 넘긴 일감을 다시 맡기고, 중지하면 맡긴 일감을 버린다
        let hub = self.clone();
        let _ = std::thread::Builder::new().name("lend-sweep".into()).spawn(move || {
            while hub.generation.load(Ordering::SeqCst) == generation && hub.run.lock().is_some() {
                hub.sweep();
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        Attached { hub: self.clone(), generation }
    }

    fn detach(&self, generation: u64) {
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if let Some(run) = self.run.lock().take() {
            // 정상이면 맡긴 일감은 이미 다 돌아왔다. 남은 것(중지 등)은 버린다.
            // 이 기기의 계산 제공자가 남은 일감을 비우고 끝나도록 더는 비켜 주지 않게 한다.
            run.spec.queue.set_remote_slots(0);
            *self.last_share.lock() = run.spec.queue.share();
        }
    }

    /// 마감을 넘겼거나 맡은 기기가 빠진(소식이 끊긴) 일감을 다시 맡기고, 중지면 맡긴 일감을 버린다
    pub fn sweep(&self) {
        let mut g = self.run.lock();
        let Some(run) = g.as_mut() else { return };
        let gate = (run.spec.gate)();
        if gate == Gate::Stop {
            // 버리면 결과를 기다리던 쪽은 결과 채널이 닫혀 "중지"로 끝난다
            run.lent.clear();
            return;
        }
        let now = Instant::now();
        let (silent, remote_slots) = {
            let l = self.lenders.lock();
            let silent: std::collections::HashSet<String> = l
                .iter()
                .filter(|(_, s)| s.last_seen.elapsed() > LENDER_SILENT)
                .map(|(id, _)| id.clone())
                .collect();
            // 붙어 있는(소식이 있고 맡길 수 있는) 빌려주는 기기들의 슬롯 합: 이 기기의 계산 제공자가 그만큼 비켜 준다
            let slots: usize = l
                .iter()
                .filter(|(id, s)| !silent.contains(*id) && s.rejected.is_none())
                .map(|(_, s)| s.slots as usize)
                .sum();
            (silent, slots)
        };
        run.spec.queue.set_remote_slots(remote_slots);
        let expired: Vec<u64> = run
            .lent
            .iter()
            .filter(|(_, l)| l.deadline <= now || silent.contains(&l.lender))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(mut l) = run.lent.remove(&id) {
                l.work.count_remote_failure();
                if let Some(s) = self.lenders.lock().get_mut(&l.lender) {
                    s.failures += 1;
                }
                run.spec.queue.requeue(l.work);
            }
        }
    }

    /// 빌려주는 기기가 이번 인덱싱의 일감을 맡을 수 있는지
    fn check(spec: &RunSpec, who: &LenderInfo) -> Result<(), String> {
        if who.signature != spec.signature {
            return Err(format!(
                "서버 서명이 다릅니다: 이 기기 {} · 빌려주는 기기 {}. 같은 llama.cpp 빌드와 모델 파일로 띄워야 합니다",
                spec.signature, who.signature
            ));
        }
        if who.max_input > 0 && who.max_input < spec.max_input {
            return Err(format!(
                "입력 한계가 작습니다: 빌려주는 기기 {}토큰 · 이 기기 설정 {}토큰. 그 기기의 -c·-np·-ub를 이 기기와 같게 하세요",
                who.max_input, spec.max_input
            ));
        }
        let missing: Vec<&str> = [
            (spec.needs.image && !who.image, "그림"),
            (spec.needs.audio && !who.audio, "소리"),
            (spec.needs.video && !who.video, "영상"),
        ]
        .into_iter()
        .filter_map(|(m, name)| m.then_some(name))
        .collect();
        if !missing.is_empty() {
            return Err(format!(
                "빌려주는 기기의 서버가 {} 입력을 받지 못합니다 (mmproj, ffmpeg를 확인하세요)",
                missing.join("·")
            ));
        }
        Ok(())
    }

    fn seen(&self, who: &LenderInfo, rejected: Option<String>) {
        let mut l = self.lenders.lock();
        let s = l.entry(who.id.clone()).or_insert_with(|| LenderStat {
            name: who.name.clone(),
            last_seen: Instant::now(),
            jobs: 0,
            inputs: 0,
            failures: 0,
            slots: 0,
            rejected: None,
        });
        s.last_seen = Instant::now();
        s.name = who.name.clone();
        s.rejected = rejected;
        s.slots = who.slots;
    }

    /// 일감을 달라는 요청. `want`개까지 맡긴다(0이면 기다리는 일감 수만 알린다). 맡길 일감이 없으면 `wait`까지 기다린다.
    pub fn claim(&self, who: &LenderInfo, want: usize, wait: Duration) -> ClaimReply {
        let wait = wait.min(MAX_WAIT);
        let t0 = Instant::now();
        loop {
            self.sweep();
            let left = wait.saturating_sub(t0.elapsed());
            // 잠금 밖에서 기다릴 수 있도록 필요한 것만 복사한다
            let snap = {
                let g = self.run.lock();
                g.as_ref().map(|r| {
                    (r.spec.queue.clone(), r.spec.rx.clone(), r.spec.gate.clone(), Self::check(&r.spec, who))
                })
            };
            let Some((queue, rx, gate, ok)) = snap else {
                self.seen(who, None);
                if left.is_zero() {
                    return ClaimReply { state: "idle", ..Default::default() };
                }
                std::thread::sleep(TICK.min(left));
                continue;
            };
            // 기다리는 일감 수만 묻는 요청(want 0)은 서명을 보지 않는다: 빌려주는 기기가 서버를 꺼 둔 채 묻는다
            match ok {
                Err(why) if want > 0 => {
                    self.seen(who, Some(why.clone()));
                    return ClaimReply { state: "indexing", rejected: Some(why), ..Default::default() };
                }
                Err(_) => {}
                Ok(()) => self.seen(who, None),
            }
            let pending = queue.pending(&rx);
            let state = match gate() {
                Gate::Stop => return ClaimReply { state: "stopping", pending, ..Default::default() },
                Gate::Pause => "paused",
                Gate::Go => "indexing",
            };
            if state == "paused" || want == 0 {
                if left.is_zero() || (want == 0 && pending > 0) {
                    return ClaimReply { state, pending, ..Default::default() };
                }
                std::thread::sleep(TICK.min(left));
                continue;
            }
            // 첫 일감은 조금씩 기다리며(상태가 바뀌었는지 보려고), 나머지는 있는 만큼만
            let mut got = Vec::new();
            let mut local_only = Vec::new();
            let mut take = |w: Work, got: &mut Vec<Work>| {
                if w.remote_failures() >= REMOTE_FAILURE_LIMIT {
                    local_only.push(w);
                } else {
                    got.push(w);
                }
            };
            match queue.next(&rx, Some(TICK.min(left.max(Duration::from_millis(1))))) {
                Next::Work(w) => take(w, &mut got),
                Next::Empty | Next::Closed => {}
            }
            while !got.is_empty() && got.len() < want {
                match queue.next(&rx, Some(Duration::ZERO)) {
                    Next::Work(w) => take(w, &mut got),
                    _ => break,
                }
            }
            // 이 기기에서만 처리할 일감은 되돌린다 (이 기기의 계산 제공자가 가져간다)
            let only_local = got.is_empty() && !local_only.is_empty();
            for w in local_only {
                queue.requeue(w);
            }
            if got.is_empty() {
                if left.is_zero() {
                    return ClaimReply { state, pending: queue.pending(&rx), ..Default::default() };
                }
                if only_local {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
            return self.lend_out(who, got, &queue, &rx, state);
        }
    }

    /// 꺼낸 일감을 맡긴다고 적고, 보낼 모양으로 만든다
    fn lend_out(&self, who: &LenderInfo, works: Vec<Work>, queue: &Arc<WorkQueue>, rx: &Receiver<Work>, state: &'static str) -> ClaimReply {
        let mut g = self.run.lock();
        let Some(run) = g.as_mut() else {
            // 그사이 인덱싱이 끝났다: 버린다(기다리던 쪽은 이미 없다)
            return ClaimReply { state: "idle", ..Default::default() };
        };
        let mut jobs = Vec::with_capacity(works.len());
        for work in works {
            let limit = deadline_for(&work.inputs);
            jobs.push(json!({
                "id": work.id(),
                "inputs": work.inputs.iter().map(EmbedInput::to_json).collect::<Vec<_>>(),
                "deadline_secs": limit.as_secs(),
            }));
            run.lent.insert(work.id(), Lent { work, deadline: Instant::now() + limit, lender: who.id.clone() });
        }
        ClaimReply { jobs, pending: queue.pending(rx), state, rejected: None }
    }

    /// 빌려주는 기기가 아직 시작하지 않은 일감을 돌려준다(일시정지·쉬는 조건). 실패로 세지 않고 바로 다시 맡긴다.
    pub fn give_back(&self, lender_id: &str, id: u64) -> bool {
        let mut g = self.run.lock();
        let Some(run) = g.as_mut() else { return false };
        let Some(lent) = run.lent.remove(&id) else { return false };
        if let Some(s) = self.lenders.lock().get_mut(lender_id) {
            s.last_seen = Instant::now();
        }
        run.spec.queue.requeue(lent.work);
        true
    }

    /// 결과가 왔다. 받아들였으면 true (마감을 넘겨 다시 맡겼거나 이미 다른 기기가 냈으면 false).
    /// `outcome`이 Err이면 그 기기가 일감을 처리하지 못한 것이다(자기 서버가 죽음 등): 다시 맡긴다.
    pub fn result(&self, lender_id: &str, id: u64, outcome: Result<Vec<JobResult>, String>) -> bool {
        let mut g = self.run.lock();
        let Some(run) = g.as_mut() else { return false };
        let Some(lent) = run.lent.remove(&id) else { return false };
        let name = {
            let mut l = self.lenders.lock();
            match l.get_mut(lender_id) {
                Some(s) => {
                    s.last_seen = Instant::now();
                    s.name.clone()
                }
                None => lender_id.to_string(),
            }
        };
        let n = lent.work.inputs.len();
        match outcome {
            Ok(results) if results.len() == n => {
                if let Some(s) = self.lenders.lock().get_mut(lender_id) {
                    s.jobs += 1;
                    s.inputs += n as u64;
                }
                run.spec.queue.record(&name, n);
                let reply: WorkReply = Ok(results
                    .into_iter()
                    .map(|r| match r {
                        JobResult::Vector(v) if !v.is_empty() && v.iter().all(|x| x.is_finite()) => Ok(v),
                        JobResult::Vector(_) => Err(EmbedError::Invalid("빌려준 기기의 결과가 비었거나 NaN".into())),
                        JobResult::Error(m) => Err(EmbedError::Server(format!("{m} (빌려준 기기 {name})"))),
                    })
                    .collect());
                lent.work.finish(reply);
                true
            }
            other => {
                // 결과 수가 맞지 않거나 처리하지 못했다: 다시 맡긴다
                let _ = other;
                let mut work = lent.work;
                work.count_remote_failure();
                if let Some(s) = self.lenders.lock().get_mut(lender_id) {
                    s.failures += 1;
                }
                run.spec.queue.requeue(work);
                false
            }
        }
    }

    /// status용: 지금 인덱싱의 처리 몫(없으면 마지막 인덱싱), 맡긴 일감 수, 빌려주는 기기들
    pub fn status(&self) -> Value {
        let (active, share, lent, pending) = {
            let g = self.run.lock();
            match g.as_ref() {
                Some(r) => (true, r.spec.queue.share(), r.lent.len(), r.spec.queue.pending(&r.spec.rx)),
                None => (false, self.last_share.lock().clone(), 0, 0),
            }
        };
        let total: u64 = share.iter().map(|(_, n)| n).sum();
        let mut lenders: Vec<LenderStat> = self.lenders.lock().values().cloned().collect();
        lenders.sort_by(|a, b| b.inputs.cmp(&a.inputs).then(a.name.cmp(&b.name)));
        json!({
            "active": active,
            "lent": lent,
            "pending": pending,
            "share": share.iter().map(|(name, n)| json!({
                "name": name,
                "inputs": n,
                "percent": if total > 0 { (*n as f64 * 100.0 / total as f64).round() } else { 0.0 },
            })).collect::<Vec<_>>(),
            "lenders": lenders.iter().map(|s| json!({
                "name": s.name,
                "last_seen_secs": s.last_seen.elapsed().as_secs(),
                "jobs": s.jobs,
                "inputs": s.inputs,
                "failures": s.failures,
                "rejected": s.rejected,
            })).collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU8;

    fn lender(sig: &str) -> LenderInfo {
        LenderInfo {
            id: "L1".into(),
            name: "desk".into(),
            signature: sig.into(),
            slots: 4,
            max_input: 8000,
            image: true,
            audio: true,
            video: true,
        }
    }

    struct Fixture {
        hub: Arc<LendHub>,
        queue: Arc<WorkQueue>,
        gate: Arc<AtomicU8>,
        _attached: Attached,
    }

    fn fixture(cap: usize) -> Fixture {
        let hub = Arc::new(LendHub::default());
        let (queue, rx) = WorkQueue::new(cap);
        let gate = Arc::new(AtomicU8::new(0));
        let g = gate.clone();
        let attached = hub.attach(RunSpec {
            queue: queue.clone(),
            rx,
            signature: "llama:b1|m.gguf".into(),
            max_input: 4000,
            needs: Needs { image: true, audio: false, video: false },
            gate: Arc::new(move || match g.load(Ordering::SeqCst) {
                0 => Gate::Go,
                1 => Gate::Pause,
                _ => Gate::Stop,
            }),
        });
        Fixture { hub, queue, gate, _attached: attached }
    }

    fn text(s: &str) -> Vec<EmbedInput> {
        vec![EmbedInput::Text(s.into())]
    }

    #[test]
    fn lends_work_and_takes_the_result_back() {
        let f = fixture(4);
        let r = f.queue.submit("a", vec![EmbedInput::Text("x".into()), EmbedInput::Text("yy".into())]).unwrap();
        let reply = f.hub.claim(&lender("llama:b1|m.gguf"), 4, Duration::from_millis(500));
        assert_eq!(reply.state, "indexing");
        assert_eq!(reply.jobs.len(), 1);
        let id = reply.jobs[0]["id"].as_u64().unwrap();
        assert_eq!(reply.jobs[0]["inputs"][1], json!("yy"));
        assert!(f.hub.result("L1", id, Ok(vec![JobResult::Vector(vec![1.0]), JobResult::Error("bad".into())])));
        let got = r.recv().unwrap().unwrap();
        assert_eq!(got[0].as_ref().unwrap(), &vec![1.0]);
        assert!(got[1].as_ref().unwrap_err().to_string().contains("bad"));
        // 같은 결과가 또 오면(늦은 결과) 버린다
        assert!(!f.hub.result("L1", id, Ok(vec![JobResult::Vector(vec![1.0]), JobResult::Vector(vec![2.0])])));
        let st = f.hub.status();
        assert_eq!(st["share"][0]["name"], "desk");
        assert_eq!(st["share"][0]["inputs"], 2);
    }

    #[test]
    fn rejects_other_builds_small_limits_and_missing_inputs() {
        let f = fixture(4);
        let r = f.hub.claim(&lender("llama:b2|m.gguf"), 1, Duration::ZERO);
        assert!(r.rejected.unwrap().contains("서버 서명"));
        let mut small = lender("llama:b1|m.gguf");
        small.max_input = 1000;
        assert!(f.hub.claim(&small, 1, Duration::ZERO).rejected.unwrap().contains("입력 한계"));
        let mut no_image = lender("llama:b1|m.gguf");
        no_image.image = false;
        assert!(f.hub.claim(&no_image, 1, Duration::ZERO).rejected.unwrap().contains("그림"));
        // 소리는 이번 인덱싱에 필요 없으므로 받지 못해도 된다
        let mut no_audio = lender("llama:b1|m.gguf");
        no_audio.audio = false;
        assert!(f.hub.claim(&no_audio, 1, Duration::ZERO).rejected.is_none());
    }

    #[test]
    fn failure_or_deadline_requeues_and_repeated_failures_stay_local() {
        let f = fixture(4);
        let r = f.queue.submit("a", text("x")).unwrap();
        let who = lender("llama:b1|m.gguf");
        let id = f.hub.claim(&who, 1, Duration::from_millis(500)).jobs[0]["id"].as_u64().unwrap();
        // 그 기기가 처리하지 못했다고 알림 → 다시 대기열로, 다시 맡길 수 있다
        assert!(!f.hub.result("L1", id, Err("server died".into())));
        let again = f.hub.claim(&who, 1, Duration::from_millis(500));
        assert_eq!(again.jobs[0]["id"].as_u64(), Some(id));
        // 맡은 기기의 소식이 끊긴 것으로 만든다 (마감 전이어도 다시 맡긴다)
        f.hub.lenders.lock().get_mut("L1").unwrap().last_seen = Instant::now() - LENDER_SILENT - Duration::from_secs(1);
        f.hub.sweep();
        assert_eq!(f.hub.run.lock().as_ref().unwrap().lent.len(), 0);
        // 두 번 실패한 일감은 빌려주는 기기에 다시 주지 않는다
        let third = f.hub.claim(&who, 1, Duration::from_millis(300));
        assert!(third.jobs.is_empty());
        // 이 기기의 계산 제공자는 가져간다
        let rx = f.hub.run.lock().as_ref().unwrap().spec.rx.clone();
        match f.queue.next(&rx, Some(Duration::from_millis(100))) {
            Next::Work(w) => {
                assert_eq!(w.id(), id);
                w.finish(Ok(vec![Ok(vec![3.0])]));
            }
            _ => panic!("다시 맡긴 일감이 없다"),
        }
        assert_eq!(r.recv().unwrap().unwrap()[0].as_ref().unwrap(), &vec![3.0]);
    }

    #[test]
    fn given_back_work_is_lent_again_without_counting_a_failure() {
        let f = fixture(4);
        let _r = f.queue.submit("a", text("x")).unwrap();
        let who = lender("llama:b1|m.gguf");
        for _ in 0..3 {
            // 여러 번 돌려받아도 이 기기 전용이 되지 않는다 (실패가 아니므로)
            let id = f.hub.claim(&who, 1, Duration::from_millis(500)).jobs[0]["id"].as_u64().unwrap();
            assert!(f.hub.give_back("L1", id));
        }
        assert_eq!(f.hub.lenders.lock()["L1"].failures, 0);
        assert_eq!(f.hub.claim(&who, 1, Duration::from_millis(500)).jobs.len(), 1);
    }

    #[test]
    fn pause_gives_nothing_and_stop_drops_lent_work() {
        let f = fixture(4);
        let r = f.queue.submit("a", text("x")).unwrap();
        f.gate.store(1, Ordering::SeqCst);
        let p = f.hub.claim(&lender("llama:b1|m.gguf"), 1, Duration::from_millis(300));
        assert_eq!(p.state, "paused");
        assert!(p.jobs.is_empty());
        assert_eq!(p.pending, 1);
        f.gate.store(0, Ordering::SeqCst);
        let id = f.hub.claim(&lender("llama:b1|m.gguf"), 1, Duration::from_millis(300)).jobs[0]["id"].as_u64().unwrap();
        f.gate.store(2, Ordering::SeqCst);
        f.hub.sweep();
        // 버려졌으니 기다리던 쪽은 "중지"(채널 닫힘)로 끝나고, 늦게 온 결과는 받지 않는다
        assert!(r.recv().is_err());
        assert!(!f.hub.result("L1", id, Ok(vec![JobResult::Vector(vec![1.0])])));
    }

    #[test]
    fn want_zero_only_reports_pending_and_idle_without_a_run() {
        let hub = Arc::new(LendHub::default());
        let r = hub.claim(&lender("x"), 1, Duration::from_millis(100));
        assert_eq!(r.state, "idle");
        let f = fixture(4);
        let _r = f.queue.submit("a", text("x")).unwrap();
        let probe = f.hub.claim(&lender("llama:b1|m.gguf"), 0, Duration::from_millis(300));
        assert!(probe.jobs.is_empty());
        assert_eq!(probe.pending, 1);
    }

    #[test]
    fn results_serialize_compactly() {
        let v = serde_json::to_value(vec![JobResult::Vector(vec![0.5]), JobResult::Error("x".into())]).unwrap();
        assert_eq!(v, json!([{ "v": [0.5] }, { "e": "x" }]));
    }
}
