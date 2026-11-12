//! 인덱싱 파이프라인
//!
//! 탐색 스레드 ─(FileJob)→ 워커 N개(추출+임베딩) ─(FileRecord)→ 기록(이 스레드: DB + 메모리 저장소)
//! 일시정지/중지는 모든 단계의 checkpoint에서 반영된다.

use super::compat;
use super::db::{self, ChunkRecord, Db, FileRecord, FileRow};
use super::lend::{Gate, LendHub, Needs, RunSpec};
use super::progress::{Progress, RunState};
use super::queue::{LocalProvider, WorkQueue, WorkReply};
use crate::config::Config;
use crate::embed::client::{EmbedClient, EmbedError, EmbedInput, EmbedTarget};
use crate::embed::prompt;
use crate::embed::selftest::Capabilities;
use crate::extract::{self, media, Kind, Modality};
use crate::paths::{norm, under};
use crate::search::keyword;
use crate::search::store::{Entry, FileInfo, VectorStore};
use crate::util::{fmt_ts, truncate_chars};
use anyhow::{anyhow, bail, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::{Condvar, Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, UNIX_EPOCH};
use walkdir::WalkDir;

const RUN: u8 = 0;
const PAUSE: u8 = 1;
const STOP: u8 = 2;
const MB: i64 = 1024 * 1024;
/// 이보다 작은 이미지(긴 변 기준)는 이름만
const MIN_IMAGE_SIDE: u32 = 256;
/// 이보다 짧은 오디오는 이름만 (초)
const MIN_AUDIO_SECS: f64 = 2.0;

#[derive(Default)]
pub struct Control {
    flag: AtomicU8,
    m: Mutex<()>,
    cv: Condvar,
}

impl Control {
    fn set(&self, v: u8) {
        self.flag.store(v, Ordering::SeqCst);
        self.cv.notify_all();
    }
    fn stopped(&self) -> bool {
        self.flag.load(Ordering::SeqCst) == STOP
    }
    /// 기다리지 않고 지금 상태만 (빌려주는 기기에 일감을 맡길지 정할 때)
    fn gate(&self) -> Gate {
        match self.flag.load(Ordering::SeqCst) {
            RUN => Gate::Go,
            STOP => Gate::Stop,
            _ => Gate::Pause,
        }
    }
    /// 일시정지 중이면 대기. 중지 요청이면 false.
    fn checkpoint(&self) -> bool {
        loop {
            match self.flag.load(Ordering::SeqCst) {
                RUN => return true,
                STOP => return false,
                _ => {
                    let mut g = self.m.lock();
                    self.cv.wait_for(&mut g, Duration::from_millis(200));
                }
            }
        }
    }
}

pub struct IndexParams {
    pub cfg: Config,
    pub caps: Capabilities,
    /// 임베딩을 맡길 곳
    pub target: EmbedTarget,
    /// 인덱스에 기록할 llama.cpp 서명 (`ServerManager::signature`). 바뀌면 벡터가 그대로인지 확인한다.
    pub server_sig: String,
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    pub store: Arc<RwLock<VectorStore>>,
    pub full_rebuild: bool,
    /// 바뀐 경로만 처리한다 (폴더 감시). 파일이면 그 파일, 폴더면 그 아래 전체, 없는 경로면 지워진 것으로 본다.
    /// None이면 대상 폴더 전체를 훑는다.
    pub only: Option<Vec<String>>,
    /// 빌려주는 기기(`gemma-search lend`)에도 일감을 맡긴다 (서비스판). None이면 이 기기만 계산한다.
    pub lend: Option<Arc<LendHub>>,
}

pub struct Indexer {
    pub progress: Arc<Mutex<Progress>>,
    control: Arc<Control>,
    handle: Option<JoinHandle<()>>,
}

impl Default for Indexer {
    fn default() -> Self {
        Self {
            progress: Arc::new(Mutex::new(Progress::default())),
            control: Arc::new(Control::default()),
            handle: None,
        }
    }
}

impl Indexer {
    pub fn is_active(&self) -> bool {
        self.handle.as_ref().map(|h| !h.is_finished()).unwrap_or(false)
    }

    pub fn start(&mut self, p: IndexParams) -> Result<()> {
        if self.is_active() {
            bail!("이미 인덱싱이 진행 중입니다");
        }
        if p.cfg.roots.iter().all(|r| r.trim().is_empty()) {
            bail!("인덱싱할 폴더가 없습니다. 설정 탭에서 폴더를 추가하세요.");
        }
        if !p.caps.text.usable() {
            bail!("서버 자가진단(텍스트)이 통과하지 않았습니다");
        }
        {
            let mut g = self.progress.lock();
            g.reset_for_run(p.full_rebuild);
            g.targeted = p.only.is_some();
        }
        self.control.set(RUN);
        let prog = self.progress.clone();
        let ctl = self.control.clone();
        self.handle = Some(
            std::thread::Builder::new()
                .name("indexer".into())
                .spawn(move || run(p, prog, ctl))?,
        );
        Ok(())
    }

    pub fn pause(&self) {
        let mut g = self.progress.lock();
        if g.state == RunState::Running {
            self.control.set(PAUSE);
            g.state = RunState::Paused;
            g.paused_since = Some(Instant::now());
        }
    }

    pub fn resume(&self) {
        let mut g = self.progress.lock();
        if g.state == RunState::Paused {
            if let Some(p) = g.paused_since.take() {
                g.paused_total += p.elapsed();
            }
            g.state = RunState::Running;
            self.control.set(RUN);
        }
    }

    pub fn stop(&self) {
        let mut g = self.progress.lock();
        if g.state.is_active() {
            if let Some(p) = g.paused_since.take() {
                g.paused_total += p.elapsed();
            }
            g.state = RunState::Stopping;
        }
        drop(g);
        self.control.set(STOP);
    }
}

enum Outcome {
    Completed,
    Stopped,
}

fn run(p: IndexParams, prog: Arc<Mutex<Progress>>, ctl: Arc<Control>) {
    let result = run_inner(&p, &prog, &ctl);
    let mut g = prog.lock();
    if let Some(ps) = g.paused_since.take() {
        g.paused_total += ps.elapsed();
    }
    g.finished = Some(Instant::now());
    g.scanning = false;
    g.current.clear();
    match result {
        Ok(Outcome::Completed) => g.state = RunState::Completed,
        Ok(Outcome::Stopped) => g.state = RunState::Stopped,
        Err(e) => {
            g.state = RunState::Failed;
            g.message = e.to_string();
        }
    }
}

struct FileJob {
    path: String,
    size: i64,
    mtime: i64,
    kind: Kind,
    /// 옮기거나 이름을 바꾼 파일로 보이면: 인덱스에 있던 항목의 (id, 옛 경로)
    moved_from: Option<(i64, String)>,
}

struct WalkOutcome {
    seen: HashSet<String>,
    complete: bool,
    error_dirs: Vec<String>,
    /// 바뀐 경로만 처리할 때: 지금은 없는 경로 (지워졌거나 옮겨 감). 소문자.
    missing: Vec<String>,
    /// 바뀐 경로만 처리할 때: 아래를 전부 훑은 폴더. 소문자.
    walked_dirs: Vec<String>,
}

/// 워커가 기록 스레드로 넘기는 결과
enum Done {
    File(FileRecord),
    /// 옮기거나 이름을 바꾼 파일: 내용 벡터는 그대로 두고 경로와 이름 벡터만 바꾼다
    Moved {
        id: i64,
        path: String,
        kind: Kind,
        name: ChunkRecord,
    },
}



struct WorkerCtx {
    cfg: Config,
    caps: Capabilities,
    /// 토큰 세기 등 임베딩 말고 서버에 묻는 일에 쓴다. 임베딩은 `queue`로 한다.
    client: EmbedClient,
    /// 일감 대기열: 임베딩할 입력 묶음을 넣으면 계산 제공자가 처리해 돌려준다
    queue: Arc<WorkQueue>,
    file_map: Arc<HashMap<String, FileRow>>,
    enabled: [bool; 6],
    full_rebuild: bool,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
    max_tokens: usize,
    prog: Arc<Mutex<Progress>>,
    ctl: Arc<Control>,
    fatal: Arc<Mutex<Option<String>>>,
    /// 지금 처리 중인 파일들 (서버 충돌 시 의심 파일로 기록)
    inflight: Mutex<HashSet<String>>,
    /// 첫 치명적 오류 시점의 처리 중 파일 목록
    crash_inflight: Mutex<Option<Vec<String>>>,
    /// 이전 충돌 의심 횟수
    suspects: HashMap<String, i64>,
    /// 서버가 영상 입력(input_video)을 받는지 — 아니면 프레임 이미지 방식으로 대체
    video_native: bool,
    /// 예전(프레임) 방식으로 인덱싱된 영상을 새 방식으로 다시 처리
    reindex_video: bool,
    /// 이미 새 방식(영상 구간 벡터)으로 처리된 영상 — 다시 처리하지 않음
    video_done: HashSet<String>,
    /// 텍스트·문서 처리 방식이 바뀜 (키워드 색인 본문 등) → 다시 처리
    reindex_text: bool,
}

/// 영상 인덱싱 방식 버전 (2 = 구간을 영상 그대로 임베딩)
const VIDEO_INDEX_VERSION: &str = "2";
/// 텍스트·문서 인덱싱 방식 버전 (2 = 본문을 키워드 색인에도 넣음)
const TEXT_INDEX_VERSION: &str = "2";

fn run_inner(p: &IndexParams, prog: &Arc<Mutex<Progress>>, ctl: &Arc<Control>) -> Result<Outcome> {
    let mut db = Db::open(&p.db_path)?;
    let sig = p.cfg.model_signature();
    let server_sig = p.server_sig.clone();
    if p.full_rebuild {
        db.clear_all()?;
        p.store.write().clear();
    } else {
        // 모델 파일(BF16 ↔ Q8 등)이나 llama.cpp 빌드가 바뀌었으면 기존 항목 몇 개를 다시 임베딩해 비교한다.
        // 벡터가 사실상 같으면 인덱스를 그대로 쓰고, 달라졌으면 재구축을 요구한다 (섞이면 검색이 틀어짐).
        let old_model = db.meta_get("model_signature");
        let old_server = db.meta_get("server_signature");
        let model_changed = old_model.as_deref().is_some_and(|o| o != sig);
        // 서버 서명이 없는 예전 인덱스는 지금 빌드로 만든 것으로 본다
        let server_changed = old_server.as_deref().is_some_and(|o| o != server_sig);
        if (model_changed || server_changed) && db.chunk_count()? > 0 {
            let what = if model_changed { "모델 파일" } else { "llama.cpp 빌드" };
            prog.lock().current = format!("{what}이 바뀌어 기존 인덱스와 벡터가 같은지 확인하는 중…");
            let report = compat::check(&db, &p.cfg, &EmbedClient::new(&p.target), compat::Budget::quick())?;
            if !report.compatible() {
                bail!(
                    "{what}이 바뀌어 벡터가 기존 인덱스와 다릅니다 ({}). '전체 재구축'을 실행하세요.",
                    if report.compared() == 0 { "비교할 표본 없음".to_string() } else { report.summary() }
                );
            }
            prog.lock().add_warning(
                "호환성 확인",
                format!("{what}이 바뀌었지만 벡터가 같아 기존 인덱스를 그대로 씁니다 — {}", report.summary()),
            );
        }
    }
    db.meta_set("model_signature", &sig)?;
    db.meta_set("server_signature", &server_sig)?;
    db.register_roots(&p.cfg.roots)?;
    // 키워드 색인 이전 인덱스: 파일 이름은 DB만으로 채운다
    db.backfill_name_fts()?;
    let file_map = Arc::new(db.load_file_map()?);
    let suspects = db.crash_suspects().unwrap_or_default();

    let ffmpeg = p.cfg.ffmpeg();
    let ffprobe = p.cfg.ffprobe();
    let media_ok = ffmpeg.is_some() && ffprobe.is_some();
    let mut enabled = [false; 6];
    enabled[Kind::Text.idx()] = p.cfg.index_text;
    enabled[Kind::Doc.idx()] = p.cfg.index_docs;
    enabled[Kind::Image.idx()] = p.cfg.index_images && p.caps.image.usable();
    enabled[Kind::Video.idx()] = p.cfg.index_video && p.caps.image.usable() && media_ok;
    enabled[Kind::Audio.idx()] = p.cfg.index_audio && p.caps.audio.usable() && media_ok;

    // 서버가 영상 입력을 받는지 2초짜리 시험 영상으로 한 번 확인
    let client = EmbedClient::new(&p.target);
    let video_native = if enabled[Kind::Video.idx()] {
        match media::test_clip(ffmpeg.as_deref().unwrap()) {
            Ok(clip) => match client.embed(&[EmbedInput::Video(clip)]) {
                Ok(_) => true,
                Err(EmbedError::Transport(m)) => bail!("서버 연결이 끊겼습니다: {m}"),
                Err(e) => {
                    prog.lock().add_warning(
                        "영상 입력 확인",
                        format!("서버가 영상 입력을 받지 않아 프레임 이미지 방식으로 대체합니다: {e}"),
                    );
                    false
                }
            },
            Err(e) => {
                prog.lock().add_warning("영상 입력 확인", format!("시험 영상 생성 실패, 프레임 방식 사용: {e}"));
                false
            }
        }
    } else {
        false
    };
    let reindex_video = video_native
        && db.meta_get("video_index_version").as_deref() != Some(VIDEO_INDEX_VERSION);
    let video_done = if reindex_video {
        db.paths_with_modality(Modality::Video).unwrap_or_default()
    } else {
        HashSet::new()
    };
    // 빈 인덱스에서 시작했으면, 이번에 넣는 것은 모두 새 방식이다 (바뀐 경로만 처리하는 실행이어도)
    let was_empty = db.chunk_count()? == 0;
    let reindex_text = db.meta_get("text_index_version").as_deref() != Some(TEXT_INDEX_VERSION) && !was_empty;

    let n_workers = p.cfg.workers.clamp(1, 16);
    // 계산 제공자가 서버에 동시에 보내는 수: 서버의 슬롯 수. 모르면 설정의 parallel.
    let slots = match client.props() {
        Ok(props) if props.slots > 0 => props.slots as usize,
        _ => p.cfg.parallel as usize,
    }
    .clamp(1, 32);
    // 대기열 칸: 슬롯의 두 배 (서버가 쉬지 않을 만큼만 쌓고, 긴 영상이 일감을 한꺼번에 밀어 넣지 못하게)
    let (queue, work_rx) = WorkQueue::new(slots * 2);
    let ctx = Arc::new(WorkerCtx {
        cfg: p.cfg.clone(),
        caps: p.caps.clone(),
        client,
        queue: queue.clone(),
        file_map: file_map.clone(),
        enabled,
        full_rebuild: p.full_rebuild,
        ffmpeg,
        ffprobe,
        max_tokens: p.cfg.max_input_tokens(),
        prog: prog.clone(),
        ctl: ctl.clone(),
        fatal: Arc::new(Mutex::new(None)),
        inflight: Mutex::new(HashSet::new()),
        crash_inflight: Mutex::new(None),
        suspects,
        video_native,
        reindex_video,
        video_done,
        reindex_text,
    });

    let (job_tx, job_rx) = bounded::<FileJob>(512);
    let (res_tx, res_rx) = bounded::<Done>(32);

    let walker = {
        let roots = p.cfg.roots.clone();
        let exclude = p.cfg.exclude_dirs.clone();
        let data_dir = p.data_dir.clone();
        let only = p.only.clone();
        let file_map = file_map.clone();
        // 다시 처리해야 하는 실행에서는 옮긴 파일로 보지 않고 새로 처리한다
        let allow_moves = !p.full_rebuild && !reindex_video && !reindex_text;
        let prog = prog.clone();
        let ctl = ctl.clone();
        std::thread::Builder::new().name("walker".into()).spawn(move || {
            walk(&roots, &exclude, &data_dir, only.as_deref(), &file_map, allow_moves, job_tx, &prog, &ctl)
        })?
    };

    // 빌려주는 기기도 같은 대기열에서 가져간다 (이 기기의 계산 제공자에 넘기기 전에 받는 쪽을 복사해 둔다)
    let lend_rx = work_rx.clone();
    // 계산 제공자: 설정의 임베딩 대상 (이 기기의 서버 또는 외부 서버). 슬롯 수만큼 동시에 보낸다.
    let provider = {
        let ctl = ctl.clone();
        let client = Arc::new(EmbedClient::new(&p.target));
        LocalProvider::start(queue, work_rx, client, slots, Arc::new(move || ctl.checkpoint()))?
    };
    // 빌려주는 기기(④ lend): 서비스의 허브에 이 실행을 붙인다. 버리면(끝나면) 떨어진다.
    let lend_attached = p.lend.as_ref().map(|hub| {
        let ctl = ctl.clone();
        let video = enabled[Kind::Video.idx()];
        hub.attach(RunSpec {
            queue: ctx.queue.clone(),
            rx: lend_rx,
            signature: server_sig.clone(),
            max_input: ctx.max_tokens as u32,
            needs: Needs {
                // 영상을 프레임 그림으로 대신 처리하면 그림이 필요하다
                image: enabled[Kind::Image.idx()] || (video && !video_native),
                // 영상의 소리 트랙도 소리 인덱싱이 켜져 있을 때만 넣는다
                audio: enabled[Kind::Audio.idx()],
                video: video && video_native,
            },
            gate: Arc::new(move || ctl.gate()),
        })
    });
    // 일꾼은 파일을 추출해 일감을 넣고, 결과를 기다리지 않은 채 그 파일을 결과 모으기 쪽으로 넘긴다.
    // 넘기는 칸을 일꾼 수로 묶어, 메모리에 올라와 있는 파일 수에 한도를 둔다.
    let (pend_tx, pend_rx) = bounded::<PendingFile>(n_workers);
    let mut workers = Vec::new();
    for i in 0..n_workers {
        let (rx, tx, ptx, ctx) = (job_rx.clone(), res_tx.clone(), pend_tx.clone(), ctx.clone());
        workers.push(
            std::thread::Builder::new()
                .name(format!("index-worker-{i}"))
                .spawn(move || worker(rx, tx, ptx, ctx))?,
        );
    }
    // 결과 모으기: 넘겨받은 파일의 일감 결과를 기다려 기록을 만든다 (파일마다 기다리므로 일꾼 수만큼)
    let mut collectors = Vec::new();
    for i in 0..n_workers {
        let (rx, tx, ctx) = (pend_rx.clone(), res_tx.clone(), ctx.clone());
        collectors.push(
            std::thread::Builder::new()
                .name(format!("index-collect-{i}"))
                .spawn(move || collect(rx, tx, ctx))?,
        );
    }
    // 여기서 쥐고 있던 끝을 놓아야, 일꾼·결과 모으기가 끝날 때 채널이 닫힌다
    drop(res_tx);
    drop(job_rx);
    drop(pend_tx);
    drop(pend_rx);

    // 기록 루프: 일꾼과 결과 모으기가 모두 끝나면 채널이 닫힌다
    for done in res_rx.iter() {
        match done {
            Done::File(rec) => write_record(&mut db, &p.store, prog, rec),
            Done::Moved { id, path, kind, name } => write_move(&mut db, &p.store, prog, id, path, kind, &name),
        }
    }
    // 끝내는 순서: 일꾼 → 결과 모으기 → 계산 제공자 (결과 모으기는 계산 제공자의 결과를 기다린다)
    for w in workers {
        let _ = w.join();
    }
    for c in collectors {
        let _ = c.join();
    }
    drop(lend_attached);
    drop(provider);
    let walk = walker.join().map_err(|_| anyhow!("탐색 스레드 오류"))?;

    if let Some(msg) = ctx.fatal.lock().take() {
        // 충돌 의심 파일: 연결이 끊긴 순간 서버에서 처리 중이던 일감의 파일들 +
        // 추출하다(토큰 세기 등) 서버에 묻는 중 끊긴 일꾼이 처리 중이던 파일들
        let mut list = ctx.queue.take_crash().unwrap_or_default();
        if let Some(l) = ctx.crash_inflight.lock().take() {
            list.extend(l);
        }
        list.sort();
        list.dedup();
        if !list.is_empty() {
            let _ = db.add_crash_suspects(&list);
        }
        bail!("{msg}");
    }
    if ctl.stopped() {
        return Ok(Outcome::Stopped);
    }

    // 사라진 파일 정리 (탐색이 끝까지 된 경우만)
    if walk.complete {
        prog.lock().current = "삭제된 파일 정리 중…".into();
        let roots: Vec<String> = p
            .cfg
            .roots
            .iter()
            .map(|r| norm(r))
            .filter(|r| !r.is_empty())
            .collect();
        let err_dirs: Vec<String> = walk.error_dirs.iter().map(|d| d.to_lowercase()).collect();
        let targeted = p.only.is_some();
        let stale: Vec<i64> = file_map
            .iter()
            .filter(|(path, _)| !walk.seen.contains(*path))
            .filter(|(path, _)| {
                let pl = path.to_lowercase();
                // 바뀐 경로만 처리했으면: 없어진 경로(와 그 아래), 다시 훑은 폴더 아래에서 사라진 것만 지운다
                let in_scope = if targeted {
                    walk.missing.iter().any(|m| pl == *m || under(&pl, m))
                        || walk.walked_dirs.iter().any(|d| under(&pl, d))
                } else {
                    true
                };
                in_scope && roots.iter().any(|r| under(&pl, r)) && !err_dirs.iter().any(|d| under(&pl, d))
            })
            .map(|(_, row)| row.id)
            .collect();
        if !stale.is_empty() {
            db.delete_files(&stale)?;
            let mut st = p.store.write();
            for id in &stale {
                st.remove_file(*id);
            }
        }
        prog.lock().removed = stale.len() as u64;
    }
    // "인덱스 전체가 새 방식이다"라는 표시는 대상 폴더 전체를 훑었거나 빈 인덱스에서 시작한 실행에서만 남긴다.
    // (바뀐 경로만 처리한 실행은 나머지 파일이 예전 방식일 수 있다)
    let all_new = p.only.is_none() || was_empty;
    if ctx.video_native && walk.complete && all_new {
        db.meta_set("video_index_version", VIDEO_INDEX_VERSION)?;
    }
    if walk.complete && all_new {
        db.meta_set("text_index_version", TEXT_INDEX_VERSION)?;
    }
    Ok(Outcome::Completed)
}

fn write_record(db: &mut Db, store: &RwLock<VectorStore>, prog: &Mutex<Progress>, rec: FileRecord) {
    let result = db.write_file(&rec);
    let FileRecord {
        path,
        kind,
        status,
        chunks,
        ..
    } = rec;
    let mut g = prog.lock();
    g.processed += 1;
    match result {
        Ok((file_id, ids)) => {
            g.chunks += chunks.len() as u64;
            if status != db::ST_ERROR {
                g.indexed_by_kind[kind.idx()] += 1;
            }
            if status == db::ST_NAME_ONLY {
                g.name_only += 1;
            }
            g.sample();
            drop(g);
            let entries = chunks
                .into_iter()
                .zip(ids)
                .map(|(c, id)| {
                    (
                        Entry {
                            chunk_id: id,
                            file_id,
                            modality: c.modality,
                            locator: c.locator,
                        },
                        c.vec,
                    )
                })
                .collect();
            store
                .write()
                .replace_file(file_id, FileInfo::new(path, kind), entries);
        }
        Err(e) => g.add_error(&path, format!("DB 기록 실패: {e}")),
    }
}

/// 옮기거나 이름을 바꾼 파일을 기록한다: DB의 경로와 이름 청크를 바꾸고, 검색용 저장소의 그 파일 항목을 다시 채운다
fn write_move(
    db: &mut Db,
    store: &RwLock<VectorStore>,
    prog: &Mutex<Progress>,
    id: i64,
    path: String,
    kind: Kind,
    name: &ChunkRecord,
) {
    let result = db.move_file(id, &path, name).and_then(|_| db.file_vectors(id));
    let mut g = prog.lock();
    g.processed += 1;
    match result {
        Ok(rows) => {
            g.moved += 1;
            g.sample();
            drop(g);
            let entries = rows
                .into_iter()
                .map(|(chunk_id, modality, locator, vec)| {
                    (
                        Entry {
                            chunk_id,
                            file_id: id,
                            modality,
                            locator,
                        },
                        vec,
                    )
                })
                .collect();
            store.write().replace_file(id, FileInfo::new(path, kind), entries);
        }
        Err(e) => g.add_error(&path, format!("옮긴 파일 기록 실패: {e}")),
    }
}

/// 탐색 상태: 파일을 찾아 작업으로 보낸다
struct Walker<'a> {
    /// 대상 폴더 (소문자, 끝에 \ 없음)
    roots: Vec<String>,
    excl: Vec<String>,
    data_lower: String,
    tx: Sender<FileJob>,
    prog: &'a Mutex<Progress>,
    ctl: &'a Control,
    file_map: &'a HashMap<String, FileRow>,
    allow_moves: bool,
    /// (크기, 수정 시각) → 인덱스에 있는 파일들 (경로, id). 새 파일이 처음 나왔을 때 만든다.
    by_sig: Option<HashMap<(i64, i64), Vec<(String, i64)>>>,
    /// 이미 옮긴 것으로 짝지은 항목
    claimed: HashSet<i64>,
    out: WalkOutcome,
}

impl Walker<'_> {
    /// 파일 하나를 작업으로 보낸다. 받는 쪽이 닫혔으면 false.
    fn emit(&mut self, path: &Path, md: &std::fs::Metadata) -> bool {
        let path_s = path.to_string_lossy().to_string();
        let size = md.len() as i64;
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let kind = extract::classify(path);
        {
            let mut g = self.prog.lock();
            g.discovered += 1;
            g.discovered_by_kind[kind.idx()] += 1;
        }
        let moved_from = if self.allow_moves && !self.file_map.contains_key(&path_s) {
            self.find_move(path, size, mtime)
        } else {
            None
        };
        if let Some((_, old)) = &moved_from {
            // 옛 경로의 항목은 지우지 않는다 (그 항목을 새 경로로 옮겨 쓴다)
            self.out.seen.insert(old.clone());
        }
        self.out.seen.insert(path_s.clone());
        self.tx
            .send(FileJob {
                path: path_s,
                size,
                mtime,
                kind,
                moved_from,
            })
            .is_ok()
    }

    /// 인덱스에 없는 새 경로가 옮기거나 이름을 바꾼 파일인지 본다. 크기와 수정 시각이 같고 옛 자리에는 더 없는
    /// 항목 가운데, 이름이 같거나(다른 폴더로 옮김 · 폴더 이름 변경) 같은 폴더에서 확장자가 같은 것(이름 변경)을 고른다.
    /// 복사한 파일은 옛 자리에 그대로 있으므로 해당하지 않는다.
    fn find_move(&mut self, path: &Path, size: i64, mtime: i64) -> Option<(i64, String)> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        let dir = path.parent().map(|d| d.to_string_lossy().to_lowercase());
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
        let file_map = self.file_map;
        let by_sig = self.by_sig.get_or_insert_with(|| {
            let mut m: HashMap<(i64, i64), Vec<(String, i64)>> = HashMap::new();
            for (p, row) in file_map {
                m.entry((row.size, row.mtime)).or_default().push((p.clone(), row.id));
            }
            m
        });
        let claimed = &self.claimed;
        let cands = by_sig.get(&(size, mtime))?;
        let free = |old: &str, id: &i64| !claimed.contains(id) && !Path::new(old).exists();
        let same_name = |old: &str| {
            Path::new(old)
                .file_name()
                .is_some_and(|n| n.to_string_lossy().to_lowercase() == name)
        };
        // 같은 폴더에서 이름만 바꾼 것: 빈 파일은 크기·시각이 우연히 같기 쉬우므로 뺀다
        let renamed_here = |old: &str| {
            let o = Path::new(old);
            size > 0
                && o.parent().map(|d| d.to_string_lossy().to_lowercase()) == dir
                && o.extension().map(|e| e.to_string_lossy().to_lowercase()) == ext
        };
        let (old, id) = cands
            .iter()
            .find(|(old, id)| same_name(old) && free(old, id))
            .or_else(|| cands.iter().find(|(old, id)| renamed_here(old) && free(old, id)))?
            .clone();
        self.claimed.insert(id);
        Some((id, old))
    }

    /// 폴더 아래를 모두 훑는다. 중지됐거나 받는 쪽이 닫혔으면 false.
    fn walk_dir(&mut self, dir: &Path) -> bool {
        let excl = self.excl.clone();
        let data_lower = self.data_lower.clone();
        let it = WalkDir::new(dir).follow_links(false).into_iter().filter_entry(move |e| {
            if e.depth() == 0 || !e.file_type().is_dir() {
                return true;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if excl.contains(&name) {
                return false;
            }
            !e.path().to_string_lossy().to_lowercase().starts_with(&data_lower)
        });
        for entry in it {
            if !self.ctl.checkpoint() {
                self.out.complete = false;
                return false;
            }
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    self.prog.lock().inaccessible += 1;
                    if let Some(p) = e.path() {
                        self.out.error_dirs.push(p.to_string_lossy().to_string());
                    }
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let md = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    self.prog.lock().inaccessible += 1;
                    continue;
                }
            };
            if !self.emit(entry.path(), &md) {
                self.out.complete = false;
                return false;
            }
        }
        true
    }

    /// 바뀐 경로 하나: 대상 폴더 밖이거나 제외 폴더·데이터 폴더 아래면 건너뛴다.
    /// 파일이면 그 파일, 폴더면 그 아래 전체, 없으면 지워진 것으로 적어 둔다. 중지됐으면 false.
    fn target(&mut self, t: &str) -> bool {
        if !self.ctl.checkpoint() {
            self.out.complete = false;
            return false;
        }
        let lower = norm(t);
        if lower.starts_with(&self.data_lower) {
            return true;
        }
        let Some(root) = self.roots.iter().find(|r| lower == **r || under(&lower, r)) else {
            return true;
        };
        // 대상 폴더 아래의 폴더 이름 가운데 제외 목록에 있는 것이 있는지 (마지막 이름은 폴더일 때만 본다)
        let parts: Vec<&str> = crate::paths::parts(&lower[root.len()..]).collect();
        let (last, dirs) = match parts.split_last() {
            Some((last, dirs)) => (Some(*last), dirs),
            None => (None, &parts[..]),
        };
        if dirs.iter().any(|d| self.excl.iter().any(|e| e == d)) {
            return true;
        }
        let path = Path::new(t.trim());
        match std::fs::metadata(path) {
            Ok(md) if md.is_file() => {
                if !self.emit(path, &md) {
                    self.out.complete = false;
                    return false;
                }
                true
            }
            Ok(md) if md.is_dir() => {
                if last.is_some_and(|l| self.excl.iter().any(|e| e == l)) {
                    return true;
                }
                self.out.walked_dirs.push(lower);
                self.walk_dir(path)
            }
            _ => {
                self.out.missing.push(lower);
                true
            }
        }
    }
}

/// 탐색: 대상 폴더 전체(`only`가 None) 또는 바뀐 경로들만(`only`) 훑어 작업으로 보낸다.
#[allow(clippy::too_many_arguments)]
fn walk(
    roots: &[String],
    exclude: &[String],
    data_dir: &Path,
    only: Option<&[String]>,
    file_map: &HashMap<String, FileRow>,
    allow_moves: bool,
    tx: Sender<FileJob>,
    prog: &Mutex<Progress>,
    ctl: &Control,
) -> WalkOutcome {
    let mut w = Walker {
        roots: roots.iter().map(|r| norm(r)).filter(|r| !r.is_empty()).collect(),
        excl: exclude
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        data_lower: data_dir.to_string_lossy().to_lowercase(),
        tx,
        prog,
        ctl,
        file_map,
        allow_moves,
        by_sig: None,
        claimed: HashSet::new(),
        out: WalkOutcome {
            seen: HashSet::new(),
            complete: true,
            error_dirs: Vec::new(),
            missing: Vec::new(),
            walked_dirs: Vec::new(),
        },
    };
    match only {
        Some(targets) => {
            for t in targets {
                if !w.target(t) {
                    break;
                }
            }
        }
        None => {
            for root in roots {
                let root = root.trim();
                if root.is_empty() {
                    continue;
                }
                if !Path::new(root).is_dir() {
                    prog.lock().add_error(root, "폴더가 존재하지 않습니다");
                    w.out.error_dirs.push(root.to_string());
                    continue;
                }
                if !w.walk_dir(Path::new(root)) {
                    break;
                }
            }
        }
    }
    prog.lock().scanning = false;
    w.out
}

enum WorkErr {
    /// 서버 연결 끊김 등 — 인덱싱 전체 중단
    Fatal(String),
    /// 사용자가 중지 — 이 파일은 기록하지 않음(다음 실행에서 다시 처리)
    Stopped,
}

/// 일꾼이 한 파일을 처리한 결과
enum Step {
    /// 다 끝나 바로 기록할 것 (옮긴 파일)
    Done(Done),
    /// 임베딩을 맡겨 두었다: 결과 모으기 쪽이 마저 끝낸다
    Pending(PendingFile),
}

fn worker(rx: Receiver<FileJob>, tx: Sender<Done>, pending: Sender<PendingFile>, ctx: Arc<WorkerCtx>) {
    while let Ok(job) = rx.recv() {
        if !ctx.ctl.checkpoint() {
            break;
        }
        let force = (ctx.reindex_video && job.kind == Kind::Video && !ctx.video_done.contains(&job.path))
            || (ctx.reindex_text && matches!(job.kind, Kind::Text | Kind::Doc));
        if !ctx.full_rebuild && !force {
            if let Some(row) = ctx.file_map.get(&job.path) {
                if row.size == job.size && row.mtime == job.mtime {
                    let up_to_date = match row.status {
                        db::ST_OK => true,
                        // 서버를 반복해서 죽인 파일은 다시 시도하지 않음
                        db::ST_ERROR
                            if ctx.suspects.get(&job.path).copied().unwrap_or(0)
                                >= db::CRASH_SUSPECT_LIMIT =>
                        {
                            true
                        }
                        // 그 밖의 오류 파일은 다시 시도 (보통 몇 개뿐이고, 일시적 문제였을 수 있음)
                        db::ST_ERROR => false,
                        // 예전엔 이름만 넣었는데 이제 내용 인덱싱이 가능해졌으면 다시 처리
                        db::ST_NAME_ONLY => !ctx.enabled[job.kind.idx()],
                        _ => false,
                    };
                    if up_to_date {
                        let mut g = ctx.prog.lock();
                        g.processed += 1;
                        g.unchanged += 1;
                        g.sample();
                        continue;
                    }
                }
            }
        }
        ctx.prog.lock().current = job.path.clone();
        ctx.inflight.lock().insert(job.path.clone());
        // 옮기거나 이름만 바꾼 파일은 내용을 다시 읽지 않는다 (다시 처리해야 하는 종류는 새로 처리).
        // 옮긴 파일은 이름 벡터 하나라 결과를 기다려 바로 끝낸다. 그 밖의 파일은 일감을 넣고 넘긴다.
        let result = match &job.moved_from {
            Some((id, _)) if !ctx.full_rebuild && !force => process_move(&ctx, &job, *id).map(Step::Done),
            _ => prepare_file(&ctx, &job).map(Step::Pending),
        };
        if let Err(WorkErr::Fatal(_)) = &result {
            // 추출하다 서버에 묻는 중(토큰 세기 등) 끊겼다: 이때 처리 중이던 파일들도 의심 파일로 남긴다 (자기 자신 포함)
            let snapshot: Vec<String> = ctx.inflight.lock().iter().cloned().collect();
            ctx.crash_inflight.lock().get_or_insert(snapshot);
        }
        ctx.inflight.lock().remove(&job.path);
        match result {
            Ok(Step::Done(rec)) => {
                if tx.send(rec).is_err() {
                    break;
                }
            }
            Ok(Step::Pending(p)) => {
                if pending.send(p).is_err() {
                    break;
                }
            }
            Err(WorkErr::Stopped) => break,
            Err(WorkErr::Fatal(msg)) => {
                ctx.fatal.lock().get_or_insert(msg);
                ctx.ctl.set(STOP);
                break;
            }
        }
    }
}

struct Piece {
    modality: Modality,
    locator: i64,
    snippet: String,
    input: EmbedInput,
    thumb: Option<Vec<u8>>,
    /// 키워드 색인 본문
    text: Option<String>,
}

enum Content {
    Pieces(Vec<Piece>),
    NameOnly(String),
}

fn disabled_reason(ctx: &WorkerCtx, kind: Kind) -> String {
    let media_ok = ctx.ffmpeg.is_some() && ctx.ffprobe.is_some();
    match kind {
        Kind::Other => "미지원 형식 (이름만)".into(),
        Kind::Text | Kind::Doc => "설정에서 꺼짐".into(),
        Kind::Image if !ctx.cfg.index_images => "설정에서 꺼짐".into(),
        Kind::Image => "이미지 임베딩 사용 불가 (자가진단)".into(),
        Kind::Video if !ctx.cfg.index_video => "설정에서 꺼짐".into(),
        Kind::Video if !media_ok => "ffmpeg/ffprobe 없음".into(),
        Kind::Video => "이미지 임베딩 사용 불가 (자가진단)".into(),
        Kind::Audio if !ctx.cfg.index_audio => "설정에서 꺼짐".into(),
        Kind::Audio if !media_ok => "ffmpeg/ffprobe 없음".into(),
        Kind::Audio => "오디오 임베딩 사용 불가 (자가진단)".into(),
    }
}

/// 파일 이름(과 폴더 이름)으로 찾기 위한 조각
fn name_piece(path: &str) -> Piece {
    let file_name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    Piece {
        modality: Modality::Name,
        locator: -1,
        snippet: String::new(),
        input: EmbedInput::Text(prompt::document(&file_name, path)),
        thumb: None,
        text: Some(keyword::name_body(path)),
    }
}

/// 옮기거나 이름을 바꾼 파일: 이름 벡터만 새로 만든다 (내용 벡터는 그대로 쓴다)
fn process_move(ctx: &WorkerCtx, job: &FileJob, id: i64) -> Result<Done, WorkErr> {
    let (mut chunks, _failures) = embed_pieces(ctx, &job.path, vec![name_piece(&job.path)])?;
    match chunks.pop() {
        Some(name) => Ok(Done::Moved {
            id,
            path: job.path.clone(),
            kind: job.kind,
            name,
        }),
        // 이름 벡터를 만들지 못했으면 새 파일로 처리한다 (옛 항목은 다음에 전체를 훑을 때 정리된다)
        None => process_file(ctx, job).map(Done::File),
    }
}

/// 임베딩을 맡겨 두고 결과를 기다리는 파일 (일꾼 → 결과 모으기)
struct PendingFile {
    path: String,
    size: i64,
    mtime: i64,
    kind: Kind,
    status: i64,
    error: Option<String>,
    /// 이름 조각을 뺀 내용 조각 수
    content_pieces: usize,
    groups: Vec<PendingGroup>,
}

/// 일감 하나로 맡긴 조각들과, 그 결과가 올 곳
struct PendingGroup {
    pieces: Vec<PieceMeta>,
    reply: Receiver<WorkReply>,
}

/// 벡터를 받으면 기록에 쓰는 조각 정보 (입력은 일감으로 넘어갔다)
struct PieceMeta {
    modality: Modality,
    locator: i64,
    snippet: String,
    thumb: Option<Vec<u8>>,
    text: Option<String>,
}

/// 파일 하나를 끝까지 처리한다 (결과를 기다린다). 옮긴 파일의 이름 벡터를 만들지 못했을 때만 쓴다.
fn process_file(ctx: &WorkerCtx, job: &FileJob) -> Result<FileRecord, WorkErr> {
    finish_file(ctx, prepare_file(ctx, job)?)
}

/// 결과 모으기 스레드: 넘겨받은 파일의 결과를 기다려 기록을 만든다.
/// 중지·연결 끊김이어도 남은 파일을 계속 꺼내 비운다 (일꾼이 넘기다 막히지 않게).
fn collect(rx: Receiver<PendingFile>, tx: Sender<Done>, ctx: Arc<WorkerCtx>) {
    for p in rx.iter() {
        match finish_file(&ctx, p) {
            Ok(rec) => {
                let _ = tx.send(Done::File(rec));
            }
            // 이 파일은 기록하지 않는다 (다음 실행에서 다시 처리)
            Err(WorkErr::Stopped) => {}
            Err(WorkErr::Fatal(msg)) => {
                ctx.fatal.lock().get_or_insert(msg);
                ctx.ctl.set(STOP);
            }
        }
    }
}

/// 파일을 읽고 잘라 임베딩할 조각을 만들고, 일감으로 넣는다 (결과는 기다리지 않는다)
fn prepare_file(ctx: &WorkerCtx, job: &FileJob) -> Result<PendingFile, WorkErr> {
    let path = Path::new(&job.path);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| job.path.clone());

    let mut pieces = vec![name_piece(&job.path)];
    let mut status = db::ST_OK;
    let mut error: Option<String> = None;
    let crash_count = ctx.suspects.get(&job.path).copied().unwrap_or(0);

    if crash_count >= db::CRASH_SUSPECT_LIMIT {
        status = db::ST_ERROR;
        error = Some(format!(
            "이 파일을 처리하던 중 llama-server가 {crash_count}번 충돌해 내용 인덱싱을 건너뜀 (이름만)"
        ));
    } else if ctx.enabled[job.kind.idx()] {
        match extract_content(ctx, job, path, &file_name) {
            Ok(Content::Pieces(v)) if !v.is_empty() => pieces.extend(v),
            Ok(Content::Pieces(_)) => {
                status = db::ST_NAME_ONLY;
                error = Some("추출된 내용 없음".into());
            }
            Ok(Content::NameOnly(reason)) => {
                status = db::ST_NAME_ONLY;
                error = Some(reason);
            }
            Err(e) => {
                if ctx.ctl.stopped() {
                    return Err(WorkErr::Stopped);
                }
                if let Some(EmbedError::Transport(m)) = e.downcast_ref::<EmbedError>() {
                    return Err(WorkErr::Fatal(format!("서버 연결이 끊겼습니다: {m}")));
                }
                status = db::ST_ERROR;
                error = Some(e.to_string());
            }
        }
    } else {
        status = db::ST_NAME_ONLY;
        error = Some(disabled_reason(ctx, job.kind));
    }

    let content_pieces = pieces.len() - 1;
    let groups = submit_pieces(ctx, &job.path, pieces)?;
    Ok(PendingFile {
        path: job.path.clone(),
        size: job.size,
        mtime: job.mtime,
        kind: job.kind,
        status,
        error,
        content_pieces,
        groups,
    })
}

/// 맡긴 일감의 결과를 모아 파일 기록을 만든다. 일부 조각이 실패했을 때의 상태 규칙은 예전과 같다.
fn finish_file(ctx: &WorkerCtx, p: PendingFile) -> Result<FileRecord, WorkErr> {
    let PendingFile {
        path,
        size,
        mtime,
        kind,
        mut status,
        mut error,
        content_pieces,
        groups,
    } = p;
    let (chunks, failures) = gather(groups)?;
    let content_ok = chunks.iter().filter(|c| c.modality != Modality::Name).count();
    if !failures.is_empty() {
        if content_pieces > 0 && content_ok == 0 || chunks.is_empty() {
            status = db::ST_ERROR;
            error = Some(failures[0].clone());
        } else if status == db::ST_OK {
            let msg = format!("일부 구간 실패 {}개: {}", failures.len(), failures[0]);
            ctx.prog.lock().add_warning(&path, msg.clone());
            error = Some(msg);
        }
    }
    if status == db::ST_ERROR {
        ctx.prog.lock().add_error(&path, error.clone().unwrap_or_default());
    }
    Ok(FileRecord {
        path,
        size,
        mtime,
        kind,
        status,
        error,
        chunks,
    })
}

fn extract_content(ctx: &WorkerCtx, job: &FileJob, path: &Path, title: &str) -> Result<Content> {
    let cfg = &ctx.cfg;
    if job.size == 0 {
        return Ok(Content::NameOnly("빈 파일 (0바이트)".into()));
    }
    match job.kind {
        Kind::Text => {
            if job.size > cfg.max_text_mb as i64 * MB {
                return Ok(Content::NameOnly(format!("크기 제한 초과 ({}MB)", cfg.max_text_mb)));
            }
            let bytes = std::fs::read(path)?;
            let Some(text) = extract::text::decode_text(&bytes) else {
                return Ok(Content::NameOnly("바이너리 또는 알 수 없는 인코딩".into()));
            };
            text_pieces(ctx, title, &text)
        }
        Kind::Doc => {
            if job.size > 300 * MB {
                return Ok(Content::NameOnly("문서가 너무 큼 (300MB 초과)".into()));
            }
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if ext == "pdf" {
                let pages = match extract::pdf::extract_pdf_pages(path, Duration::from_secs(90)) {
                    Ok(p) => p,
                    Err(e) if extract::pdf::is_encrypted_error(&e) => {
                        return Ok(Content::NameOnly("암호화된 PDF (텍스트 추출 불가)".into()));
                    }
                    Err(e) => return Err(e),
                };
                if pages.iter().all(|p| p.trim().is_empty()) {
                    return Ok(Content::NameOnly("텍스트 없음 (스캔 문서일 수 있음)".into()));
                }
                return pdf_pieces(ctx, title, &pages);
            }
            let text = extract::docx::extract_docx(path)?;
            if text.trim().is_empty() {
                return Ok(Content::NameOnly("텍스트 없음".into()));
            }
            text_pieces(ctx, title, &text)
        }
        Kind::Image => {
            if job.size > cfg.max_media_mb as i64 * MB {
                return Ok(Content::NameOnly("크기 제한 초과".into()));
            }
            let img = extract::image::load_image(path, ctx.ffmpeg.as_deref())?;
            let (w, h) = (img.width(), img.height());
            if w.max(h) < MIN_IMAGE_SIDE {
                // 아이콘·작은 썸네일은 검색 결과만 어지럽힌다 (DigUp도 256px 미만은 건너뜀)
                return Ok(Content::NameOnly(format!("작은 이미지 ({w}×{h}, {MIN_IMAGE_SIDE}px 미만)")));
            }
            let (jpeg, thumb) = extract::image::model_and_thumb(&img)?;
            Ok(Content::Pieces(vec![Piece {
                modality: Modality::Image,
                locator: 0,
                snippet: format!("{w}×{h}"),
                input: EmbedInput::Image(jpeg),
                thumb: Some(thumb),
                text: None,
            }]))
        }
        Kind::Video => {
            if job.size > cfg.max_media_mb as i64 * MB {
                return Ok(Content::NameOnly("크기 제한 초과".into()));
            }
            let (ff, fp) = (ctx.ffmpeg.as_deref().unwrap(), ctx.ffprobe.as_deref().unwrap());
            let info = media::probe(fp, path)?;
            let mut pieces = Vec::new();
            if info.has_video && ctx.video_native {
                // 모델 카드 방식: 구간을 영상 그대로 넣어 구간마다 하나의 벡터 (서버가 1fps로 샘플링)
                let seg = cfg.video_window_secs() as f64;
                let mut last_err = None;
                for (start, len) in media::segments(info.duration, seg, cfg.video_max_segments) {
                    if !ctx.ctl.checkpoint() {
                        bail!("중지됨");
                    }
                    match media::video_segment(ff, path, start, len) {
                        Ok(mp4) => {
                            let thumb = media::frame_at(ff, path, start + len / 2.0)
                                .ok()
                                .and_then(|j| thumb_from_jpeg(&j));
                            let (s_ms, e_ms) = ((start * 1000.0) as i64, ((start + len) * 1000.0) as i64);
                            pieces.push(Piece {
                                modality: Modality::Video,
                                locator: s_ms,
                                snippet: format!("{}–{} 구간", fmt_ts(s_ms), fmt_ts(e_ms)),
                                input: EmbedInput::Video(mp4),
                                thumb,
                                text: None,
                            });
                        }
                        Err(e) => last_err = Some(e),
                    }
                }
                if pieces.is_empty() {
                    if let Some(e) = last_err {
                        return Err(e);
                    }
                }
            } else if info.has_video {
                // 대체 방식: 서버가 영상 입력을 받지 못할 때만 프레임을 이미지로
                let n = if info.duration > 0.0 {
                    cfg.video_frames
                        .max(1)
                        .min(((info.duration / 2.0).ceil() as usize).max(1))
                } else {
                    1
                };
                let mut last_err = None;
                for t in media::spread(info.duration, n) {
                    if !ctx.ctl.checkpoint() {
                        bail!("중지됨");
                    }
                    match media::frame_at(ff, path, t) {
                        Ok(jpeg) => {
                            let thumb = thumb_from_jpeg(&jpeg);
                            let ms = (t * 1000.0) as i64;
                            pieces.push(Piece {
                                modality: Modality::Image,
                                locator: ms,
                                snippet: format!("{} 장면", fmt_ts(ms)),
                                input: EmbedInput::Image(jpeg),
                                thumb,
                                text: None,
                            });
                        }
                        Err(e) => last_err = Some(e),
                    }
                }
                if pieces.is_empty() {
                    if let Some(e) = last_err {
                        return Err(e);
                    }
                }
            }
            if info.has_audio && ctx.enabled[Kind::Audio.idx()] && !(info.duration > 0.0 && info.duration < MIN_AUDIO_SECS) {
                pieces.extend(audio_pieces(ctx, ff, path, info.duration)?);
            }
            Ok(Content::Pieces(pieces))
        }
        Kind::Audio => {
            if job.size > cfg.max_media_mb as i64 * MB {
                return Ok(Content::NameOnly("크기 제한 초과".into()));
            }
            let (ff, fp) = (ctx.ffmpeg.as_deref().unwrap(), ctx.ffprobe.as_deref().unwrap());
            let info = media::probe(fp, path)?;
            if !info.has_audio {
                return Ok(Content::NameOnly("오디오 스트림 없음".into()));
            }
            if info.duration > 0.0 && info.duration < MIN_AUDIO_SECS {
                return Ok(Content::NameOnly(format!("짧은 소리 ({:.1}초)", info.duration)));
            }
            Ok(Content::Pieces(audio_pieces(ctx, ff, path, info.duration)?))
        }
        Kind::Other => Ok(Content::NameOnly("미지원 형식 (이름만)".into())),
    }
}

fn thumb_from_jpeg(jpeg: &[u8]) -> Option<Vec<u8>> {
    image::load_from_memory(jpeg)
        .ok()
        .map(|img| img.thumbnail(extract::image::THUMB_SIDE, extract::image::THUMB_SIDE))
        .and_then(|t| extract::image::encode_jpeg(&t, 80).ok())
}

fn audio_pieces(ctx: &WorkerCtx, ff: &Path, path: &Path, dur: f64) -> Result<Vec<Piece>> {
    let seg = ctx.cfg.audio_segment_secs.max(5) as f64;
    // 창을 겹쳐서(30초 창이면 25초마다) 문장이 두 창 사이에서 끊기지 않게
    let starts = media::audio_windows(dur.max(0.0), seg, seg * 5.0 / 6.0, ctx.cfg.audio_max_segments);
    let mut out = Vec::new();
    let mut last_err = None;
    for s in starts {
        if !ctx.ctl.checkpoint() {
            bail!("중지됨");
        }
        match media::audio_segment(ff, path, s, seg) {
            Ok(Some(wav)) => {
                let end = if dur > 0.0 { (s + seg).min(dur) } else { s + seg };
                let ms = (s * 1000.0) as i64;
                out.push(Piece {
                    modality: Modality::Audio,
                    locator: ms,
                    snippet: format!("{}–{} 구간", fmt_ts(ms), fmt_ts((end * 1000.0) as i64)),
                    input: EmbedInput::Audio(wav),
                    thumb: None,
                    text: None,
                });
            }
            Ok(None) => {}
            Err(e) => last_err = Some(e),
        }
    }
    if out.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(out)
}

/// PDF: 페이지마다 청크를 나눠 locator에 페이지를 담는다 (결과에서 "N페이지", 그 페이지로 열기)
fn pdf_pieces(ctx: &WorkerCtx, title: &str, pages: &[String]) -> Result<Content> {
    // 긴 PDF(책·보고서)는 일반 문서보다 청크 한도를 넉넉히
    let cap = ctx.cfg.max_chunks_per_file.max(1) * 5;
    let mut pieces = Vec::new();
    'pages: for (i, page) in pages.iter().enumerate() {
        if page.trim().is_empty() {
            continue;
        }
        let page_no = i as i64 + 1;
        for (sub, (chunk, input)) in text_chunks(&ctx.cfg, &ctx.client, ctx.max_tokens, title, page)?
            .into_iter()
            .enumerate()
        {
            if pieces.len() >= cap {
                break 'pages;
            }
            pieces.push(Piece {
                modality: Modality::Text,
                locator: page_no * extract::pdf::PAGE_LOCATOR + sub as i64,
                snippet: truncate_chars(&chunk.replace(['\n', '\t'], " "), 240),
                input: EmbedInput::Text(input),
                thumb: None,
                text: Some(chunk),
            });
        }
    }
    Ok(Content::Pieces(pieces))
}

fn text_pieces(ctx: &WorkerCtx, title: &str, text: &str) -> Result<Content> {
    let pieces = text_chunks(&ctx.cfg, &ctx.client, ctx.max_tokens, title, text)?
        .into_iter()
        .enumerate()
        .map(|(idx, (chunk, input))| Piece {
            modality: Modality::Text,
            locator: idx as i64,
            snippet: truncate_chars(&chunk.replace(['\n', '\t'], " "), 240),
            input: EmbedInput::Text(input),
            thumb: None,
            text: Some(chunk),
        })
        .collect();
    Ok(Content::Pieces(pieces))
}

/// 텍스트를 청크로 나누고 모델 입력(접두사 포함)을 만든다. 반환: (청크 원문, 입력) — 순서가 locator.
/// 인덱서와 호환성 검사(compat)가 같은 입력을 만들도록 한 곳에 둔다.
pub(crate) fn text_chunks(
    cfg: &Config,
    client: &EmbedClient,
    max_tokens: usize,
    title: &str,
    text: &str,
) -> Result<Vec<(String, String)>> {
    let chunks = extract::text::chunk_text(text, cfg.chunk_chars, cfg.chunk_overlap, cfg.max_chunks_per_file.max(1));
    let mut out = Vec::new();
    for chunk in chunks {
        let mut stack = vec![(chunk, 0u8)];
        while let Some((c, depth)) = stack.pop() {
            let input = prompt::document(title, &c);
            // 한글·코드·기호·드문 문자는 글자당 토큰이 2~3개까지 갈 수 있어 실제 토큰 수 확인
            if c.chars().count() * 3 > max_tokens && depth < 4 {
                let n = client.count_tokens(&input)?;
                if n > max_tokens {
                    let (a, b) = extract::text::split_half(&c);
                    stack.push((b, depth + 1));
                    stack.push((a, depth + 1));
                    continue;
                }
            }
            if c.trim().is_empty() {
                continue;
            }
            out.push((c, input));
        }
    }
    Ok(out)
}

/// 같은 종류끼리 묶어 배치 임베딩하고 결과를 기다린다 (옮긴 파일의 이름 벡터처럼 바로 끝내야 할 때)
fn embed_pieces(ctx: &WorkerCtx, tag: &str, pieces: Vec<Piece>) -> Result<(Vec<ChunkRecord>, Vec<String>), WorkErr> {
    gather(submit_pieces(ctx, tag, pieces)?)
}

/// 같은 종류끼리 묶어 일감으로 넣는다 (묶음 규칙: 글 16, 그림 4(배치 가능할 때), 영상·소리 1).
/// 결과는 기다리지 않는다. 묶음이 실패하면 하나씩 다시 해 보는 것은 계산 제공자가 한다 (queue::run_work).
/// `tag`는 이 일감이 어느 파일의 것인지 (서버가 죽었을 때 의심 파일로 남긴다).
fn submit_pieces(ctx: &WorkerCtx, tag: &str, pieces: Vec<Piece>) -> Result<Vec<PendingGroup>, WorkErr> {
    let mut groups = Vec::new();
    let class = |p: &Piece| match p.input {
        EmbedInput::Text(_) => 0u8,
        EmbedInput::Image(_) => 1,
        EmbedInput::Audio(_) => 2,
        EmbedInput::Video(_) => 3,
    };
    let batch_limit = |c: u8| match c {
        0 => 16,
        1 if ctx.caps.image_batch => 4,
        _ => 1,
    };
    let mut iter = pieces.into_iter().peekable();
    while let Some(first) = iter.next() {
        let c = class(&first);
        let mut group = vec![first];
        while group.len() < batch_limit(c) && iter.peek().map(|p| class(p) == c).unwrap_or(false) {
            group.push(iter.next().unwrap());
        }
        if !ctx.ctl.checkpoint() {
            return Err(WorkErr::Stopped);
        }
        let mut inputs = Vec::with_capacity(group.len());
        let mut metas = Vec::with_capacity(group.len());
        for p in group {
            inputs.push(p.input);
            metas.push(PieceMeta {
                modality: p.modality,
                locator: p.locator,
                snippet: p.snippet,
                thumb: p.thumb,
                text: p.text,
            });
        }
        // 대기열이 닫혔다 = 중지
        let reply = ctx.queue.submit(tag, inputs).ok_or(WorkErr::Stopped)?;
        groups.push(PendingGroup { pieces: metas, reply });
    }
    Ok(groups)
}

/// 맡긴 일감들의 결과를 차례로 받아 조각 기록과 실패 까닭을 모은다.
/// 결과 없이 채널이 닫히면 중지(일감이 버려졌다), 연결이 끊겼으면 인덱싱 전체를 멈춘다.
fn gather(groups: Vec<PendingGroup>) -> Result<(Vec<ChunkRecord>, Vec<String>), WorkErr> {
    let mut out = Vec::new();
    let mut fails = Vec::new();
    for g in groups {
        let results: Vec<Result<Vec<f32>, EmbedError>> = match g.reply.recv() {
            Ok(Ok(r)) => r,
            Ok(Err(m)) => return Err(WorkErr::Fatal(format!("서버 연결이 끊겼습니다: {m}"))),
            Err(_) => return Err(WorkErr::Stopped),
        };
        for (p, r) in g.pieces.into_iter().zip(results) {
            match r {
                Ok(vec) => out.push(ChunkRecord {
                    modality: p.modality,
                    locator: p.locator,
                    snippet: p.snippet,
                    vec,
                    thumb: p.thumb,
                    text: p.text,
                }),
                Err(EmbedError::Transport(m)) => {
                    return Err(WorkErr::Fatal(format!("서버 연결이 끊겼습니다: {m}")))
                }
                Err(e) => fails.push(e.to_string()),
            }
        }
    }
    Ok((out, fails))
}
