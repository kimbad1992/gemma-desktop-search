//! 일감 대기열 (메시 1단계 ③, `MESH-DESIGN.md` 8.1·8.5)
//!
//! 인덱싱 일꾼은 파일을 읽고 잘라 임베딩할 입력을 만든 뒤, 그 묶음(일감)을 대기열에 넣는다.
//! 계산 제공자가 대기열에서 일감을 가져가 임베딩하고 결과를 돌려준다.
//!
//! - 계산 제공자는 지금 하나다: 설정의 임베딩 대상(이 기기의 llama-server 또는 `--embed-server`).
//!   서버의 슬롯 수만큼 스레드를 띄워 동시에 가져간다. 다른 기기가 일감을 가져가는 일(④ `lend`)과
//!   마감·다시 맡기기는 이 위에 더한다.
//! - 일꾼은 일감을 넣고(`submit`) 결과를 기다리지 않은 채 다음 파일로 넘어간다(B단계). 결과는 인덱서의
//!   결과 모으기 스레드가 받는다. 묶음을 만드는 규칙(파일마다, 같은 종류끼리)은 예전 그대로다.
//! - 대기열의 칸 수에 한도가 있다. 긴 영상 하나가 일감 수백 개를 한꺼번에 밀어 넣어 메모리가 불어나지 않게,
//!   칸이 차면 일감을 넣는 쪽이 기다린다.
//!
//! 지키는 것
//! - 연결이 끊기면(`EmbedError::Transport`) 그 사실을 결과로 그대로 돌려준다. 다시 맡기지 않는다:
//!   계산 제공자가 하나뿐일 때 다시 맡기면 서버가 죽은 것을 감추게 된다.
//! - 처음 연결이 끊긴 순간 서버에서 처리 중이던 일감의 파일들을 남긴다 (`take_crash`, 충돌 의심 파일).
//! - 묶음이 실패하면 하나씩 다시 해 보는 것도 예전과 같다 (`run_work`).
//! - 일시정지면 계산 제공자도 새 일감을 보내지 않고 기다린다. 중지면 일감을 버리고, 결과를 기다리던 쪽은
//!   결과 채널이 닫혀 "중지"로 끝난다.

use crate::embed::client::{EmbedClient, EmbedError, EmbedInput};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// 입력마다의 결과. 묶음 전체가 연결 끊김으로 실패하면 `Err(까닭)`.
pub type WorkReply = Result<Vec<Result<Vec<f32>, EmbedError>>, String>;

/// 일감: 임베딩할 입력 한 묶음과 결과를 돌려줄 곳
pub struct Work {
    id: u64,
    /// 어느 파일의 일감인지 (충돌 의심 파일을 고를 때 쓴다)
    tag: String,
    pub inputs: Vec<EmbedInput>,
    reply: Sender<WorkReply>,
}

impl Work {
    /// 결과를 돌려준다 (기다리던 쪽이 이미 없으면 버린다)
    pub fn finish(self, reply: WorkReply) {
        let _ = self.reply.send(reply);
    }
}

/// 일감 대기열. 일꾼들이 나눠 쓴다.
pub struct WorkQueue {
    tx: Mutex<Option<Sender<Work>>>,
    next_id: AtomicU64,
    /// 지금 서버에서 처리 중인 일감: id → 파일
    executing: Mutex<HashMap<u64, String>>,
    /// 처음 연결이 끊긴 순간 처리 중이던 파일들
    crash: Mutex<Option<Vec<String>>>,
}

impl WorkQueue {
    /// 대기열(칸 `capacity`개)과, 계산 제공자가 일감을 가져갈 쪽
    pub fn new(capacity: usize) -> (Arc<Self>, Receiver<Work>) {
        let (tx, rx) = bounded(capacity.max(1));
        let queue = Self {
            tx: Mutex::new(Some(tx)),
            next_id: AtomicU64::new(0),
            executing: Mutex::new(HashMap::new()),
            crash: Mutex::new(None),
        };
        (Arc::new(queue), rx)
    }

    /// 일감을 넣고, 결과가 올 곳을 돌려준다. 칸이 차 있으면 빌 때까지 기다린다.
    /// 대기열이 닫혔으면 `None`. 결과 채널이 결과 없이 닫히면 일감이 버려진 것이다(중지).
    pub fn submit(&self, tag: &str, inputs: Vec<EmbedInput>) -> Option<Receiver<WorkReply>> {
        // 칸이 차서 기다리는 동안 잠금을 쥐고 있지 않도록, 보내는 쪽을 복사해 잠금을 놓은 뒤 보낸다
        let tx = self.tx.lock().clone()?;
        let (reply, result) = bounded(1);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        tx.send(Work {
            id,
            tag: tag.to_string(),
            inputs,
            reply,
        })
        .ok()?;
        Some(result)
    }

    /// 일감을 넣고 결과를 기다린다. 대기열이 닫혔거나 일감이 처리되지 않고 버려지면(중지) `None`.
    #[cfg(test)]
    pub fn embed(&self, tag: &str, inputs: Vec<EmbedInput>) -> Option<WorkReply> {
        self.submit(tag, inputs)?.recv().ok()
    }

    /// 더는 일감을 받지 않는다. 남은 일감을 다 가져가면 계산 제공자들이 끝난다.
    pub fn close(&self) {
        self.tx.lock().take();
    }

    /// 처음 연결이 끊긴 순간 서버에서 처리 중이던 일감의 파일들 (중복 없이)
    pub fn take_crash(&self) -> Option<Vec<String>> {
        self.crash.lock().take()
    }

    fn begin(&self, work: &Work) {
        self.executing.lock().insert(work.id, work.tag.clone());
    }

    /// 일감을 끝냈다. 연결이 끊겨 실패했으면, 처음 한 번 그때 처리 중이던 파일들(자기 포함)을 남긴다.
    fn end(&self, work: &Work, lost_connection: bool) {
        let mut executing = self.executing.lock();
        if lost_connection {
            let mut crash = self.crash.lock();
            if crash.is_none() {
                let mut files: Vec<String> = executing.values().cloned().collect();
                files.sort();
                files.dedup();
                *crash = Some(files);
            }
        }
        executing.remove(&work.id);
    }
}

/// 연결이 끊겨 실패한 결과인지 (묶음 전체든, 하나씩 다시 할 때든)
fn lost_connection(reply: &WorkReply) -> bool {
    match reply {
        Err(_) => true,
        Ok(each) => each.iter().any(|r| matches!(r, Err(EmbedError::Transport(_)))),
    }
}

/// 한 묶음을 임베딩한다. 묶음이 실패하면(연결 끊김 말고) 하나씩 다시 해 본다.
pub fn run_work(client: &EmbedClient, inputs: &[EmbedInput]) -> WorkReply {
    match client.embed(inputs) {
        Ok(vecs) => Ok(vecs.into_iter().map(Ok).collect()),
        Err(EmbedError::Transport(m)) => Err(m),
        Err(e) if inputs.len() == 1 => Ok(vec![Err(e)]),
        Err(_) => Ok(inputs
            .iter()
            .map(|inp| client.embed(std::slice::from_ref(inp)).map(|mut v| v.remove(0)))
            .collect()),
    }
}

/// 이 기기에서 도는 계산 제공자: 설정의 임베딩 대상에 일감을 보낸다. 스레드 `n`개가 동시에 가져간다.
/// 버려지면(drop) 대기열을 닫고 스레드가 모두 끝날 때까지 기다린다.
pub struct LocalProvider {
    queue: Arc<WorkQueue>,
    threads: Vec<JoinHandle<()>>,
}

impl LocalProvider {
    /// `go`: 일시정지면 기다리고, 중지면 false (인덱서의 `Control::checkpoint`)
    pub fn start(
        queue: Arc<WorkQueue>,
        rx: Receiver<Work>,
        client: Arc<EmbedClient>,
        n: usize,
        go: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> std::io::Result<Self> {
        let mut threads = Vec::new();
        for i in 0..n.max(1) {
            let (rx, client, go, q) = (rx.clone(), client.clone(), go.clone(), queue.clone());
            let spawned = std::thread::Builder::new()
                .name(format!("embed-provider-{i}"))
                .spawn(move || {
                    for work in rx.iter() {
                        if !go() {
                            // 중지: 일감을 버린다 → 기다리던 쪽은 결과 채널이 닫혀 "중지"로 끝난다.
                            // 버리면서도 계속 꺼내므로 일감을 넣으려고 기다리던 쪽이 막히지 않는다.
                            continue;
                        }
                        q.begin(&work);
                        let reply = run_work(&client, &work.inputs);
                        q.end(&work, lost_connection(&reply));
                        work.finish(reply);
                    }
                });
            match spawned {
                Ok(t) => threads.push(t),
                Err(e) => {
                    // 이미 띄운 스레드를 정리하고 실패를 알린다
                    queue.close();
                    for t in threads {
                        let _ = t.join();
                    }
                    return Err(e);
                }
            }
        }
        Ok(Self { queue, threads })
    }
}

impl Drop for LocalProvider {
    fn drop(&mut self) {
        self.queue.close();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Duration;

    /// 시험용 계산 제공자: 입력 글자 길이를 벡터로 돌려준다
    fn fake_provider(rx: Receiver<Work>, go: Arc<AtomicBool>, seen: Arc<AtomicUsize>) -> JoinHandle<()> {
        std::thread::spawn(move || {
            for work in rx.iter() {
                if !go.load(Ordering::SeqCst) {
                    continue;
                }
                seen.fetch_add(1, Ordering::SeqCst);
                let out = work
                    .inputs
                    .iter()
                    .map(|i| match i {
                        EmbedInput::Text(t) => Ok(vec![t.len() as f32]),
                        _ => Err(EmbedError::Transport("x".into())),
                    })
                    .collect();
                work.finish(Ok(out));
            }
        })
    }

    #[test]
    fn work_comes_back_to_the_submitter_in_order() {
        let (q, rx) = WorkQueue::new(4);
        let seen = Arc::new(AtomicUsize::new(0));
        let t = fake_provider(rx, Arc::new(AtomicBool::new(true)), seen.clone());
        let r = q
            .embed("f", vec![EmbedInput::Text("ab".into()), EmbedInput::Text("abcd".into())])
            .unwrap()
            .unwrap();
        let v: Vec<f32> = r.into_iter().map(|x| x.unwrap()[0]).collect();
        assert_eq!(v, vec![2.0, 4.0]);
        q.close();
        t.join().unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 1);
        // 닫힌 뒤에는 받지 않는다
        assert!(q.embed("f", vec![EmbedInput::Text("a".into())]).is_none());
    }

    #[test]
    fn many_submitters_share_one_provider() {
        let (q, rx) = WorkQueue::new(2);
        let seen = Arc::new(AtomicUsize::new(0));
        let t = fake_provider(rx, Arc::new(AtomicBool::new(true)), seen.clone());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let q = q.clone();
                std::thread::spawn(move || {
                    let s = "x".repeat(i + 1);
                    q.embed("f", vec![EmbedInput::Text(s)]).unwrap().unwrap()[0].as_ref().unwrap()[0]
                })
            })
            .collect();
        let mut got: Vec<f32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        got.sort_by(f32::total_cmp);
        assert_eq!(got, (1..=8).map(|n| n as f32).collect::<Vec<_>>());
        q.close();
        t.join().unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn submit_does_not_wait_for_the_result() {
        // 계산 제공자가 아직 없어도, 칸이 남아 있으면 넣고 바로 돌아온다
        let (q, rx) = WorkQueue::new(3);
        let r1 = q.submit("a", vec![EmbedInput::Text("x".into())]).unwrap();
        let r2 = q.submit("b", vec![EmbedInput::Text("yy".into())]).unwrap();
        let t = fake_provider(rx, Arc::new(AtomicBool::new(true)), Arc::new(AtomicUsize::new(0)));
        assert_eq!(r2.recv().unwrap().unwrap()[0].as_ref().unwrap()[0], 2.0);
        assert_eq!(r1.recv().unwrap().unwrap()[0].as_ref().unwrap()[0], 1.0);
        q.close();
        t.join().unwrap();
    }

    #[test]
    fn stopped_provider_drops_work_and_submitter_sees_none() {
        let (q, rx) = WorkQueue::new(1);
        let t = fake_provider(rx, Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        // 칸이 하나뿐이어도, 버리면서 계속 꺼내므로 여러 개를 넣는 쪽이 막히지 않는다
        let pending: Vec<_> = (0..5)
            .map(|_| q.submit("a", vec![EmbedInput::Text("a".into())]).unwrap())
            .collect();
        for r in pending {
            assert!(r.recv().is_err());
        }
        q.close();
        t.join().unwrap();
    }

    #[test]
    fn local_provider_closes_and_joins_on_drop() {
        let (q, rx) = WorkQueue::new(4);
        // 닿지 않는 주소: 일감을 하나도 넣지 않으므로 보내지 않는다
        let target = crate::embed::client::EmbedTarget::local(1);
        let client = Arc::new(EmbedClient::new(&target));
        let p = LocalProvider::start(q.clone(), rx, client, 3, Arc::new(|| true)).unwrap();
        let t0 = std::time::Instant::now();
        drop(p);
        assert!(t0.elapsed() < Duration::from_secs(5));
        assert!(q.embed("a", vec![EmbedInput::Text("a".into())]).is_none());
    }

    #[test]
    fn lost_connection_names_the_files_at_the_server() {
        // 닿지 않는 주소로 보내면 연결 끊김으로 실패하고, 그 일감의 파일이 충돌 의심 목록에 남는다
        let (q, rx) = WorkQueue::new(4);
        let target = crate::embed::client::EmbedTarget::local(1);
        let client = Arc::new(EmbedClient::new(&target));
        let p = LocalProvider::start(q.clone(), rx, client, 1, Arc::new(|| true)).unwrap();
        let r = q.embed("C:\\a.txt", vec![EmbedInput::Text("a".into())]).unwrap();
        assert!(r.is_err());
        drop(p);
        assert_eq!(q.take_crash(), Some(vec!["C:\\a.txt".to_string()]));
        assert_eq!(q.take_crash(), None);
    }
}
