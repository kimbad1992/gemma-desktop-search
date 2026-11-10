//! 폴더 감시: 검색 폴더의 파일이 바뀌면 그 파일만 다시 인덱싱하도록 바뀐 경로를 모아 둔다.
//!
//! - 변경 알림(`notify` 크레이트: Windows는 ReadDirectoryChangesW, macOS는 FSEvents, Linux는 inotify)을 받아
//!   바뀐 경로를 모은다. 복사·다운로드가 끝나 조용해질 때까지 기다렸다가 앱이 꺼내 간다.
//! - 알림만으로는 놓치는 것이 있다.
//!   - 앱이 꺼져 있던 동안 바뀐 파일
//!   - 네트워크 폴더: NAS에 따라 알림이 오지 않거나 빠진다
//!   - 알림이 한꺼번에 몰려 넘친 경우
//!
//!   그래서 켠 직후와 그 뒤 주기적으로 폴더를 훑어 인덱스와 비교한다 (`scan`). 서버 없이 크기와 수정 시각만
//!   보므로 그래픽카드를 쓰지 않고, 바뀐 것이 있을 때만 인덱싱이 시작된다.
//! - 실제 인덱싱(서버 켜기, 전체 화면 앱 중에는 미루기)은 앱이 한다 (`app.rs`의 `watch_tick`).

use crate::index::db::Db;
use notify::{EventKind, RecursiveMode, Watcher as _};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};
use walkdir::WalkDir;

/// 마지막 알림 뒤 이만큼 조용해야 꺼내 간다 (복사·저장이 끝나기를 기다린다)
pub const QUIET: Duration = Duration::from_secs(15);
/// 다른 파일의 알림이 계속 와도, 이만큼 기다린 경로가 있으면 조용해진 것부터 꺼내 간다
const MAX_WAIT: Duration = Duration::from_secs(300);
/// 같은 파일은 이 시간 안에 두 번까지만 처리한다 (계속 쓰이는 로그 파일 등이 인덱싱을 끝없이 돌리지 않게)
const COOLDOWN: Duration = Duration::from_secs(600);
const COOLDOWN_TAKES: u32 = 2;
/// 대기 중인 경로가 이보다 많아지면 하나씩 세지 않고 폴더를 훑어 확인한다
const MAX_PENDING: usize = 20_000;
/// 켠 뒤 처음 훑기까지 (꺼져 있던 동안의 변경을 찾는다)
const FIRST_SCAN: Duration = Duration::from_secs(30);

/// 네트워크 위치인지: `\\서버\공유` 또는 네트워크 드라이브에 연결한 드라이브 문자
pub fn is_network(path: &str) -> bool {
    let p = path.trim();
    if p.starts_with(r"\\") {
        return true;
    }
    #[cfg(windows)]
    {
        let b = p.as_bytes();
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            const DRIVE_REMOTE: u32 = 4;
            let root: Vec<u16> = format!("{}:\\", b[0] as char).encode_utf16().chain(Some(0)).collect();
            return unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE;
        }
    }
    false
}

fn norm(path: &str) -> String {
    path.trim().trim_end_matches('\\').to_lowercase()
}

fn under(path: &str, base: &str) -> bool {
    path.starts_with(base) && path[base.len()..].starts_with('\\')
}

/// 인덱싱 대상인 경로만 남기는 규칙 (인덱서의 탐색과 같은 기준)
struct PathFilter {
    /// 대상 폴더 (소문자, 끝에 \ 없음)
    roots: Vec<String>,
    excl: Vec<String>,
    data_lower: String,
}

impl PathFilter {
    fn new(roots: &[String], exclude: &[String], data_dir: &Path) -> Self {
        Self {
            roots: roots.iter().map(|r| norm(r)).filter(|r| !r.is_empty()).collect(),
            excl: exclude
                .iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            data_lower: data_dir.to_string_lossy().to_lowercase(),
        }
    }

    /// 무시할 경로: 대상 폴더 밖, 제외 폴더 아래, 데이터 폴더 아래, 저장 중에 잠깐 생기는 임시 파일
    fn ignores(&self, path: &str) -> bool {
        let lower = norm(path);
        if lower.starts_with(&self.data_lower) {
            return true;
        }
        let Some(root) = self.roots.iter().find(|r| under(&lower, r)) else {
            return true;
        };
        let mut parts = lower[root.len()..].split('\\').filter(|s| !s.is_empty()).peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                // 폴더 이름
                if self.excl.iter().any(|e| e == part) {
                    return true;
                }
            } else if is_temp_name(part) {
                return true;
            }
        }
        false
    }
}

/// 프로그램이 저장·다운로드 중에 잠깐 만드는 파일 이름
fn is_temp_name(name_lower: &str) -> bool {
    name_lower.starts_with("~$")
        || [".tmp", ".part", ".crdownload", ".partial", ".download"]
            .iter()
            .any(|ext| name_lower.ends_with(ext))
}

/// 폴더 하나의 감시 상태
#[derive(Clone, Debug)]
pub struct RootStatus {
    pub path: String,
    /// 네트워크 위치: 변경 알림이 오지 않거나 빠질 수 있어 주기적으로 훑는 것에 기댄다
    pub network: bool,
    /// 변경 알림을 받지 못하는 이유 (받는 중이면 None)
    pub error: Option<String>,
    /// 알림 등록이 아직 끝나지 않았다 (꺼져 있는 NAS는 응답을 기다리느라 오래 걸린다)
    pub registering: bool,
}

/// 화면에 보여 줄 지금 상태
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub roots: Vec<RootStatus>,
    /// 반영을 기다리는 바뀐 경로 수
    pub pending: usize,
    /// 폴더를 훑어 확인하는 중
    pub scanning: bool,
    /// 마지막으로 훑은 것: (끝난 시각, 찾은 변경 수)
    pub last_scan: Option<(Instant, usize)>,
    /// 다음에 훑을 시각 (주기적으로 훑지 않으면 None)
    pub next_scan: Option<Instant>,
    /// 훑다가 난 문제 (NAS에 연결할 수 없음 등)
    pub scan_note: Option<String>,
}

#[derive(Default)]
struct Inner {
    /// 바뀐 경로 → 마지막 알림 시각
    pending: HashMap<String, Instant>,
    /// 최근에 꺼내 간 경로 → (처음 꺼낸 시각, 그 뒤로 꺼낸 횟수)
    recent: HashMap<String, (Instant, u32)>,
    /// 알림이 넘쳤다: 폴더를 훑어 확인해야 한다
    overflow: bool,
    scanning: bool,
    last_scan: Option<(Instant, usize)>,
    next_scan: Option<Instant>,
    scan_note: Option<String>,
}

/// 감시를 다시 시작해야 하는지 판단하는 키 (설정이 같으면 그대로 둔다)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchKey {
    roots: Vec<String>,
    exclude: Vec<String>,
    rescan_hours: u32,
}

impl WatchKey {
    pub fn new(roots: &[String], exclude: &[String], rescan_hours: u32) -> Self {
        Self {
            roots: roots.to_vec(),
            exclude: exclude.to_vec(),
            rescan_hours,
        }
    }
}

pub struct Watcher {
    key: WatchKey,
    inner: Arc<Mutex<Inner>>,
    roots: Arc<Mutex<Vec<RootStatus>>>,
    filter: Arc<PathFilter>,
    rescan_every: Option<Duration>,
    /// 살아 있는 동안 알림을 받는다 (떨어뜨리면 감시가 끝난다). 등록이 끝나면 채워진다.
    _watcher: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
}

impl Watcher {
    /// 감시를 시작한다. 알림 등록은 백그라운드에서 한다: 꺼져 있는 NAS는 응답을 기다리느라 수십 초가 걸릴 수
    /// 있어, 화면이 멈추지 않게 한다. 폴더별로 등록에 실패해도(없는 폴더, 알림을 지원하지 않는 위치) 나머지는
    /// 감시하고, 실패한 폴더는 주기적으로 훑는 것으로만 확인한다.
    pub fn start(key: WatchKey, data_dir: &Path) -> Self {
        let filter = Arc::new(PathFilter::new(&key.roots, &key.exclude, data_dir));
        let inner = Arc::new(Mutex::new(Inner {
            next_scan: Some(Instant::now() + FIRST_SCAN),
            ..Default::default()
        }));
        let roots: Vec<RootStatus> = key
            .roots
            .iter()
            .map(|r| r.trim())
            .filter(|r| !r.is_empty())
            .map(|r| RootStatus {
                path: r.to_string(),
                network: is_network(r),
                error: None,
                registering: true,
            })
            .collect();
        let roots = Arc::new(Mutex::new(roots));
        let holder: Arc<Mutex<Option<notify::RecommendedWatcher>>> = Arc::new(Mutex::new(None));
        let spawned = std::thread::Builder::new().name("watch-register".into()).spawn({
            let (inner, filter, roots, holder) = (inner.clone(), filter.clone(), roots.clone(), holder.clone());
            move || {
                let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                    on_event(&inner, &filter, res)
                });
                let start_error = watcher.as_ref().err().map(|e| e.to_string());
                let n = roots.lock().len();
                for i in 0..n {
                    let path = roots.lock()[i].path.clone();
                    let error = match &mut watcher {
                        Err(_) => Some(format!(
                            "변경 알림을 시작하지 못했습니다: {}",
                            start_error.clone().unwrap_or_default()
                        )),
                        Ok(_) if !Path::new(&path).is_dir() => Some("폴더를 찾을 수 없습니다".to_string()),
                        Ok(w) => w
                            .watch(Path::new(&path), RecursiveMode::Recursive)
                            .err()
                            .map(|e| format!("변경 알림을 받을 수 없습니다: {e}")),
                    };
                    let mut g = roots.lock();
                    g[i].error = error;
                    g[i].registering = false;
                }
                *holder.lock() = watcher.ok();
            }
        });
        if spawned.is_err() {
            for r in roots.lock().iter_mut() {
                r.error = Some("변경 알림을 시작하지 못했습니다".to_string());
                r.registering = false;
            }
        }
        let rescan_every = (key.rescan_hours > 0).then(|| Duration::from_secs(key.rescan_hours as u64 * 3600));
        Self {
            key,
            inner,
            roots,
            filter,
            rescan_every,
            _watcher: holder,
        }
    }

    pub fn key(&self) -> &WatchKey {
        &self.key
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.inner.lock();
        Snapshot {
            roots: self.roots.lock().clone(),
            pending: g.pending.len(),
            scanning: g.scanning,
            last_scan: g.last_scan,
            next_scan: g.next_scan,
            scan_note: g.scan_note.clone(),
        }
    }

    /// 반영할 준비가 된 경로를 꺼낸다: 알림이 `QUIET`만큼 조용해진 뒤에 한꺼번에.
    /// 다른 파일의 알림이 끊이지 않으면, 오래 기다린 것이 생겼을 때 조용해진 경로부터 꺼낸다.
    pub fn take_ready(&self) -> Vec<String> {
        let mut g = self.inner.lock();
        let now = Instant::now();
        g.recent.retain(|_, (t, _)| now.duration_since(*t) < COOLDOWN);
        let (Some(newest), Some(oldest)) = (g.pending.values().max().copied(), g.pending.values().min().copied())
        else {
            return Vec::new();
        };
        if now.duration_since(newest) < QUIET && now.duration_since(oldest) < MAX_WAIT {
            return Vec::new();
        }
        let ready: Vec<String> = g
            .pending
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= QUIET)
            // 계속 바뀌는 파일은 잠시 미룬다. 지워진 것은 미루지 않는다 (한 번이면 끝난다).
            .filter(|(p, _)| {
                g.recent.get(*p).is_none_or(|(_, n)| *n < COOLDOWN_TAKES) || !Path::new(p.as_str()).exists()
            })
            .map(|(p, _)| p.clone())
            .collect();
        for p in &ready {
            g.pending.remove(p);
            g.recent.entry(p.clone()).or_insert((now, 0)).1 += 1;
        }
        ready
    }

    /// 꺼내 갔지만 인덱싱을 시작하지 못한 경로를 되돌려 놓는다
    pub fn requeue(&self, paths: Vec<String>) {
        let mut g = self.inner.lock();
        let ready = Instant::now().checked_sub(QUIET).unwrap_or_else(Instant::now);
        for p in paths {
            // 처리하지 못했으니 꺼낸 횟수에서 뺀다
            if let Some((_, n)) = g.recent.get_mut(&p) {
                *n = n.saturating_sub(1);
            }
            g.pending.entry(p).or_insert(ready);
        }
    }

    /// 폴더를 훑어 확인할 때가 됐는지: 주기가 됐거나, 알림이 넘쳤거나
    pub fn scan_due(&self) -> bool {
        let g = self.inner.lock();
        !g.scanning && (g.overflow || g.next_scan.is_some_and(|t| Instant::now() >= t))
    }

    /// 다음 확인을 지금으로 당긴다 ("지금 확인" 버튼)
    pub fn scan_now(&self) {
        self.inner.lock().next_scan = Some(Instant::now());
    }

    /// 폴더를 훑어 인덱스와 비교한다 (백그라운드). 바뀐 경로는 대기 목록에 들어간다.
    pub fn start_scan(&self, db_path: PathBuf, ctx: eframe::egui::Context) {
        {
            let mut g = self.inner.lock();
            if g.scanning {
                return;
            }
            g.scanning = true;
            g.overflow = false;
            g.next_scan = None;
        }
        let inner = self.inner.clone();
        let filter = self.filter.clone();
        let roots: Vec<String> = self.roots.lock().iter().map(|r| r.path.clone()).collect();
        let every = self.rescan_every;
        let spawned = std::thread::Builder::new().name("watch-scan".into()).spawn({
            let inner = inner.clone();
            move || {
                let result = scan(&roots, &filter, &db_path);
                let mut g = inner.lock();
                // 훑어서 찾은 것은 이미 다 쓰인 파일이므로 기다리지 않고 바로 꺼내 갈 수 있게 한다
                let ready = Instant::now().checked_sub(QUIET).unwrap_or_else(Instant::now);
                let (found, note) = match result {
                    Ok((paths, note)) => (paths, note),
                    Err(e) => (Vec::new(), Some(format!("폴더를 확인하지 못했습니다: {e}"))),
                };
                let n = found.len();
                for p in found {
                    g.pending.entry(p).or_insert(ready);
                }
                g.scanning = false;
                g.last_scan = Some((Instant::now(), n));
                g.next_scan = every.map(|d| Instant::now() + d);
                g.scan_note = note;
                drop(g);
                ctx.request_repaint();
            }
        });
        if spawned.is_err() {
            let mut g = inner.lock();
            g.scanning = false;
            g.next_scan = every.map(|d| Instant::now() + d);
        }
    }
}

fn on_event(inner: &Mutex<Inner>, filter: &PathFilter, res: notify::Result<notify::Event>) {
    let ev = match res {
        Ok(ev) => ev,
        Err(_) => {
            // 알림을 읽다가 오류: 놓친 것이 있을 수 있으니 폴더를 훑어 확인한다
            inner.lock().overflow = true;
            return;
        }
    };
    if matches!(ev.kind, EventKind::Access(_)) {
        return;
    }
    // 폴더 안의 파일이 바뀌면 그 폴더 자체도 "바뀜"으로 온다. 파일 알림이 따로 오므로 폴더 쪽은 버린다
    // (폴더를 대상으로 넣으면 그 아래를 전부 훑게 된다). 폴더가 생기거나 없어지거나 이름이 바뀐 것만 남긴다.
    let content_change = matches!(
        ev.kind,
        EventKind::Modify(notify::event::ModifyKind::Data(_))
            | EventKind::Modify(notify::event::ModifyKind::Metadata(_))
            | EventKind::Modify(notify::event::ModifyKind::Any)
            | EventKind::Modify(notify::event::ModifyKind::Other)
    );
    let now = Instant::now();
    let rescan = ev.need_rescan();
    let paths: Vec<String> = ev
        .paths
        .iter()
        .filter(|p| !(content_change && p.is_dir()))
        .map(|p| p.to_string_lossy().to_string())
        .filter(|p| !filter.ignores(p))
        .collect();
    let mut g = inner.lock();
    if rescan {
        g.overflow = true;
    }
    for p in paths {
        g.pending.insert(p, now);
    }
    if g.pending.len() > MAX_PENDING {
        g.pending.clear();
        g.overflow = true;
    }
}

/// 폴더를 훑어 인덱스(DB)와 비교한다: 새 파일, 크기나 수정 시각이 달라진 파일, 없어진 파일의 경로를 돌려준다.
/// 연결할 수 없는 폴더는 건너뛰고(그 아래 파일을 지워진 것으로 보지 않는다) 그 사실을 함께 돌려준다.
fn scan(roots: &[String], filter: &PathFilter, db_path: &Path) -> Result<(Vec<String>, Option<String>), String> {
    let map = Db::open(db_path)
        .and_then(|db| db.load_file_map())
        .map_err(|e| e.to_string())?;
    let mut changed = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // 끝까지 훑은 폴더 / 읽지 못한 폴더 (소문자)
    let mut scanned: Vec<String> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut offline: Vec<String> = Vec::new();
    for root in roots {
        if !Path::new(root).is_dir() {
            offline.push(root.clone());
            continue;
        }
        let excl = filter.excl.clone();
        let data_lower = filter.data_lower.clone();
        let it = WalkDir::new(root).follow_links(false).into_iter().filter_entry(move |e| {
            if e.depth() == 0 || !e.file_type().is_dir() {
                return true;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            !excl.contains(&name) && !e.path().to_string_lossy().to_lowercase().starts_with(&data_lower)
        });
        for entry in it {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    if let Some(p) = e.path() {
                        unreadable.push(norm(&p.to_string_lossy()));
                    }
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().to_string_lossy().to_string();
            let Ok(md) = entry.metadata() else { continue };
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let same = map
                .get(&path)
                .is_some_and(|row| row.size == md.len() as i64 && row.mtime == mtime);
            if !same && !entry.file_name().to_str().is_some_and(|n| is_temp_name(&n.to_lowercase())) {
                changed.push(path.clone());
            }
            seen.insert(path);
        }
        scanned.push(norm(root));
    }
    // 인덱스에는 있는데 폴더에는 없는 파일 (읽지 못한 폴더 아래는 제외)
    for path in map.keys() {
        if seen.contains(path) {
            continue;
        }
        let lower = path.to_lowercase();
        if scanned.iter().any(|r| under(&lower, r)) && !unreadable.iter().any(|d| lower == *d || under(&lower, d)) {
            changed.push(path.clone());
        }
    }
    let note = (!offline.is_empty()).then(|| format!("연결할 수 없어 확인하지 못한 폴더: {}", offline.join(", ")));
    Ok((changed, note))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> PathFilter {
        PathFilter::new(
            &[r"C:\Docs".to_string(), r"\\nas\share\Photos\".to_string()],
            &["node_modules".to_string(), ".git".to_string()],
            Path::new(r"C:\Docs\app\data"),
        )
    }

    #[test]
    fn keeps_only_paths_the_indexer_would_index() {
        let f = filter();
        assert!(!f.ignores(r"C:\Docs\report.txt"));
        assert!(!f.ignores(r"c:\docs\Sub\Report.TXT"));
        assert!(!f.ignores(r"\\NAS\share\Photos\2024\IMG_0001.jpg"));
        // 대상 폴더 밖, 대상 폴더 자체
        assert!(f.ignores(r"C:\Other\report.txt"));
        assert!(f.ignores(r"C:\Docs"));
        assert!(f.ignores(r"C:\Docs2\report.txt"));
        // 제외 폴더 아래 (폴더 이름일 때만: 같은 이름의 파일은 대상)
        assert!(f.ignores(r"C:\Docs\proj\node_modules\x\index.js"));
        assert!(!f.ignores(r"C:\Docs\proj\node_modules"));
        // 데이터 폴더 아래
        assert!(f.ignores(r"C:\Docs\app\data\index.db-wal"));
        // 저장·다운로드 중의 임시 파일
        assert!(f.ignores(r"C:\Docs\~$report.docx"));
        assert!(f.ignores(r"C:\Docs\movie.mp4.crdownload"));
        assert!(f.ignores(r"C:\Docs\a1b2.tmp"));
    }

    #[test]
    fn network_paths() {
        assert!(is_network(r"\\192.168.0.100\storage\Media"));
        assert!(!is_network(r"C:\Users"));
        assert!(!is_network("relative"));
    }

    #[test]
    fn waits_until_quiet_then_hands_out_once() {
        let key = WatchKey::new(&[], &[], 0);
        let w = Watcher::start(key, Path::new(r"C:\nowhere\data"));
        let now = Instant::now();
        let old = now.checked_sub(QUIET + Duration::from_secs(1)).unwrap();
        {
            let mut g = w.inner.lock();
            g.pending.insert(r"C:\Docs\done.txt".into(), old);
            g.pending.insert(r"C:\Docs\still-copying.bin".into(), now);
        }
        // 알림이 아직 이어지고 있으면 아무것도 꺼내지 않는다
        assert!(w.take_ready().is_empty());
        // 조용해지면 꺼낸다
        w.inner.lock().pending.insert(r"C:\Docs\still-copying.bin".into(), old);
        let mut ready = w.take_ready();
        ready.sort();
        assert_eq!(ready, vec![r"C:\Docs\done.txt".to_string(), r"C:\Docs\still-copying.bin".to_string()]);
        assert_eq!(w.snapshot().pending, 0);
        // 시작하지 못해 되돌려 놓은 것은 바로 다시 꺼낼 수 있다
        w.requeue(vec![r"C:\Docs\done.txt".to_string()]);
        assert_eq!(w.take_ready(), vec![r"C:\Docs\done.txt".to_string()]);

        // 계속 바뀌는 파일: 두 번까지는 바로 꺼내고, 그 뒤로는 한동안 미룬다. 지워지면 미루지 않는다.
        let file = std::env::temp_dir().join(format!("gemma-watch-test-{}.log", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let path = file.to_string_lossy().to_string();
        for _ in 0..COOLDOWN_TAKES {
            w.inner.lock().pending.insert(path.clone(), old);
            assert_eq!(w.take_ready(), vec![path.clone()]);
        }
        w.inner.lock().pending.insert(path.clone(), old);
        assert!(w.take_ready().is_empty());
        assert_eq!(w.snapshot().pending, 1);
        std::fs::remove_file(&file).unwrap();
        assert_eq!(w.take_ready(), vec![path]);
    }
}
