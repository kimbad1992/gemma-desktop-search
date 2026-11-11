//! 데이터 위치: 설정·인덱스(`data/`)와 앱이 받은 준비물(`llama.cpp/`, `models/`)을 두는 곳을 정하고, 옮긴다.
//!
//! 정하는 순서 (`resolve`)
//! 1. 실행 파일 옆에 `portable.txt`(또는 `portable`)가 있으면 포터블: 실행 파일 옆의 `data/`와 준비물을 쓴다.
//! 2. 아니면 고정 위치: OS별 사용자 폴더(`platform::data_home`)
//!    - Windows `%LOCALAPPDATA%\GemmaSearch`, Linux `~/.local/share/gemma-search`, macOS `~/Library/Application Support/GemmaSearch`
//!    - 그 안의 `location.txt`가 다른 폴더를 가리키면(설정에서 다른 드라이브로 옮긴 경우) 그 폴더
//! 3. 고정 위치에 인덱스가 아직 없고 실행 파일 옆에 예전 버전의 `data/index.db`가 있으면, 옮기기 전까지는 실행 파일 옆을
//!    그대로 쓴다 (`Kind::Legacy`). 창이 있는 앱은 처음에 옮길지 묻고, 서비스판은 옮기는 명령을 알려 준다.
//! 4. 환경 변수 `GEMMA_SEARCH_DATA`가 있으면 데이터 폴더만 그곳으로 바꾼다 (시험용). 준비물 폴더는 위에서 정한 곳.
//!
//! 위치를 정할 때는 아무것도 만들지 않는다. 폴더는 쓸 때 만든다.
//! 개발 빌드(cargo의 target 폴더)도 같은 고정 위치를 쓴다. 이때 "실행 파일 옆"은 저장소 루트다 (`config::app_dir`).
//!
//! 옮기기 (`plan` → `execute`): 같은 드라이브면 폴더 이름만 바꾸고, 다른 드라이브면 복사한 뒤 새 위치에서 인덱스가 열리고
//! 파일 수가 같은지 확인하고 나서 원래 것을 지운다. 복사는 `<이름>.partial`에 하고 다 된 뒤에 이름을 바꾸므로,
//! 중간에 끊겨도 반쯤 복사된 폴더를 데이터로 쓰는 일은 없다.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// 실행 파일 옆에 두면 포터블로 쓰는 표시 파일
pub const PORTABLE_MARKERS: [&str; 2] = ["portable.txt", "portable"];
/// 고정 위치를 다른 곳으로 옮겼을 때 기본 위치에 남기는 표시 파일 (옮긴 곳의 경로 한 줄)
pub const POINTER: &str = "location.txt";
const DATA: &str = "data";
const INDEX: &str = "index.db";
/// 앱이 받은 준비물 폴더 (`download::Pack::dest`)
const PREP_DIRS: [&str; 2] = ["llama.cpp", "models"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 고정 위치
    Home,
    /// 실행 파일 옆 (`portable.txt`가 있거나, 사용자 폴더를 알 수 없음)
    Portable,
    /// 고정 위치가 비어 있고 실행 파일 옆에 예전 데이터가 있다: 옮기기 전까지 실행 파일 옆을 쓴다
    Legacy,
    /// 환경 변수 `GEMMA_SEARCH_DATA`로 데이터 폴더를 정했다
    Env,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub kind: Kind,
    /// 설정·인덱스 폴더
    pub data: PathBuf,
    /// 받은 준비물(`llama.cpp/`, `models/`)을 두는 폴더
    pub root: PathBuf,
    /// 고정 위치 (옮겨 갈 곳). 사용자 폴더를 알 수 없으면 None
    pub home: Option<PathBuf>,
    /// 기본 고정 위치 (`location.txt`를 두는 곳). 사용자 폴더를 알 수 없으면 None
    pub base: Option<PathBuf>,
    /// 실행 파일 폴더 (개발 빌드면 저장소 루트)
    pub app_dir: PathBuf,
}

impl Location {
    /// 화면과 명령줄에 보여 줄 이름
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            Kind::Home => "고정 위치",
            Kind::Portable => "포터블 (실행 파일 옆)",
            Kind::Legacy => "실행 파일 옆 (옮기기 전)",
            Kind::Env => "환경 변수 GEMMA_SEARCH_DATA",
        }
    }
}

/// 위치를 정하는 데 쓰는 것 (시험에서는 임시 폴더를 넣는다)
pub struct Inputs {
    pub env_data: Option<PathBuf>,
    pub app_dir: PathBuf,
    pub base: Option<PathBuf>,
}

impl Inputs {
    pub fn from_env() -> Self {
        Self {
            env_data: std::env::var_os("GEMMA_SEARCH_DATA").filter(|v| !v.is_empty()).map(PathBuf::from),
            app_dir: crate::config::app_dir(),
            base: crate::platform::data_home(),
        }
    }
}

/// 위치를 정한다. 아무것도 만들지 않는다.
pub fn resolve(inp: &Inputs) -> Location {
    let home = inp.base.as_deref().map(|b| read_pointer(b).unwrap_or_else(|| b.to_path_buf()));
    let (kind, root) = match &home {
        Some(h) if !is_portable(&inp.app_dir) => {
            let legacy = !same_path(h, &inp.app_dir)
                && !has_index(&h.join(DATA))
                && inp.app_dir.join(DATA).join(INDEX).is_file();
            if legacy {
                (Kind::Legacy, inp.app_dir.clone())
            } else {
                (Kind::Home, h.clone())
            }
        }
        // 포터블이거나 사용자 폴더를 알 수 없다 (LOCALAPPDATA·HOME이 없는 서비스 계정 등)
        _ => (Kind::Portable, inp.app_dir.clone()),
    };
    let (kind, data) = match &inp.env_data {
        Some(d) => (Kind::Env, d.clone()),
        None => (kind, root.join(DATA)),
    };
    Location {
        kind,
        data,
        root,
        home,
        base: inp.base.clone(),
        app_dir: inp.app_dir.clone(),
    }
}

static CURRENT: Mutex<Option<Location>> = Mutex::new(None);

/// 이 프로세스가 쓰는 위치 (처음 한 번 정하고 기억한다)
pub fn current() -> Location {
    let mut g = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(|| resolve(&Inputs::from_env())).clone()
}

/// 옮긴 뒤 등: 위치를 다시 정한다
pub fn refresh() -> Location {
    let loc = resolve(&Inputs::from_env());
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(loc.clone());
    loc
}

/// 실행 파일 옆에 포터블 표시가 있는지
pub fn is_portable(app_dir: &Path) -> bool {
    PORTABLE_MARKERS.iter().any(|m| app_dir.join(m).is_file())
}

/// 실행 파일 옆에 포터블 표시를 남긴다 (예전 데이터를 옮기지 않고 그 자리에서 계속 쓰기로 했을 때)
pub fn mark_portable(app_dir: &Path) -> std::io::Result<()> {
    std::fs::write(
        app_dir.join(PORTABLE_MARKERS[0]),
        "이 파일이 있으면 Gemma Search가 설정·인덱스를 이 폴더의 data에 둡니다 (포터블).\n\
         지우면 사용자 폴더의 고정 위치를 씁니다. 설정 → 일반 → 데이터 위치에서 옮길 수 있습니다.\n",
    )
}

/// 기본 위치의 `location.txt`가 가리키는 폴더. 비었거나 상대 경로면 무시한다.
fn read_pointer(base: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(base.join(POINTER)).ok()?;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let p = PathBuf::from(line);
    p.is_absolute().then_some(p)
}

/// 데이터 폴더에 인덱싱한 파일이 하나라도 있는지. 빈 인덱스(처음 실행만 해 본 것)는 없는 것으로 본다.
/// 열어 보지 못하면(다른 프로그램의 잠금 등) 있는 것으로 본다: 모르는 데이터 위로 옮기지 않게.
pub fn has_index(data: &Path) -> bool {
    let db = data.join(INDEX);
    db.is_file() && file_count(&db).map_or(true, |n| n > 0)
}

/// 인덱스의 파일 수. 표가 아직 없으면 0.
fn file_count(db: &Path) -> rusqlite::Result<i64> {
    use rusqlite::OpenFlags;
    // 만들지는 않는다 (없는 파일이면 실패)
    let conn = rusqlite::Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let has_table: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'files')",
        [],
        |r| r.get(0),
    )?;
    if !has_table {
        return Ok(0);
    }
    conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))
}

/// 옮기기 전에: 쓰기 기록(WAL)을 본 파일에 합쳐 두고 파일 수를 센다. 옮긴 뒤 같은 수인지 확인한다.
fn checkpoint(db: &Path) -> Result<i64, String> {
    let fail = |e: rusqlite::Error| format!("인덱스를 열 수 없습니다 ({}): {e}", db.display());
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(fail)?;
    conn.busy_timeout(Duration::from_secs(5)).map_err(fail)?;
    let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    drop(conn);
    file_count(db).map_err(fail)
}

// ── 경로 비교 ──

/// Windows·macOS의 기본 파일 시스템은 대소문자를 구분하지 않는다
fn same_name(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

fn comps(p: &Path) -> Vec<Component<'_>> {
    p.components().filter(|c| !matches!(c, Component::CurDir)).collect()
}

/// `path`가 `base` 아래(또는 같은 곳)면 그 나머지 경로. 같은 곳이면 빈 경로.
fn rel_under(path: &Path, base: &Path) -> Option<PathBuf> {
    let (p, b) = (comps(path), comps(base));
    if b.len() > p.len() {
        return None;
    }
    let prefix_ok = p.iter().zip(&b).all(|(x, y)| match (x, y) {
        (Component::Normal(x), Component::Normal(y)) => same_name(x, y),
        (Component::Prefix(x), Component::Prefix(y)) => same_name(x.as_os_str(), y.as_os_str()),
        _ => x == y,
    });
    prefix_ok.then(|| p[b.len()..].iter().collect())
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    rel_under(a, b).is_some_and(|r| r.as_os_str().is_empty())
}

/// `path`가 `base` 안(같은 곳 제외)인지
fn inside(path: &Path, base: &Path) -> bool {
    rel_under(path, base).is_some_and(|r| !r.as_os_str().is_empty())
}

// ── 옮기기 ──

/// 옮기는 일
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// 예전 프로그램 폴더(또는 그 `data` 폴더)에서 데이터와, 설정이 가리키는 그 폴더의 준비물을 고정 위치로 가져온다.
    /// 실행 파일 옆(포터블·옮기기 전)에서 고정 위치로 옮길 때도 이것이다.
    Import(PathBuf),
    /// 고정 위치를 통째로 다른 폴더(예: 다른 드라이브)로 옮긴다
    MoveTo(PathBuf),
}

impl Job {
    /// 다시 시작한 프로세스에 넘기는 인자 (`--move-data <import|to> <경로>`, main.rs)
    pub fn to_args(&self) -> [String; 2] {
        match self {
            Job::Import(p) => ["import".into(), p.display().to_string()],
            Job::MoveTo(p) => ["to".into(), p.display().to_string()],
        }
    }

    pub fn from_args(kind: &str, path: &str) -> Option<Job> {
        let p = PathBuf::from(path);
        match kind {
            "import" => Some(Job::Import(p)),
            "to" => Some(Job::MoveTo(p)),
            _ => None,
        }
    }
}

/// 옮길 것 하나
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub from: PathBuf,
    pub to: PathBuf,
    pub bytes: u64,
}

/// 옮기기 전에 보여 줄 계획
#[derive(Debug, Clone)]
pub struct Plan {
    pub job: Job,
    /// 옮길 것. 첫째가 데이터 폴더다.
    pub items: Vec<Item>,
    /// 옮긴 뒤의 고정 위치
    pub home: PathBuf,
    /// 다른 드라이브라 복사한다 (같은 드라이브면 이름만 바꾼다)
    pub copy: bool,
    /// 옮겨 갈 곳에 이미 인덱스가 있으면 그 파일 수 (그 인덱스는 이름을 바꿔 남겨 둔다)
    pub replaces: Option<i64>,
    /// 설정의 경로를 바꿀 기준: 원래 폴더
    old_root: PathBuf,
}

impl Plan {
    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }

    /// 사람이 읽을 설명 (화면과 명령줄이 같이 쓴다)
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        for it in &self.items {
            out.push(format!("{} ({}): {} → {}", it.name, size(it.bytes), it.from.display(), it.to.display()));
        }
        out.push(if self.copy {
            "다른 드라이브라 복사합니다. 새 위치에서 인덱스가 열리고 파일 수가 같은지 확인한 뒤 원래 것을 지웁니다.".into()
        } else {
            "같은 드라이브라 폴더 이름만 바꿔 바로 옮깁니다.".into()
        });
        if let Some(n) = self.replaces {
            out.push(format!(
                "옮겨 갈 곳에 이미 인덱스(파일 {n}개)가 있습니다. 지우지 않고 data.before-import-<시각>으로 이름을 바꿔 남겨 둡니다."
            ));
        }
        out
    }
}

/// 진행: 지금 옮기는 것, 복사한 바이트 / 모두
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub item: String,
    pub done: u64,
    pub total: u64,
}

/// 옮긴 결과
#[derive(Debug, Clone)]
pub struct Moved {
    pub home: PathBuf,
    /// 알릴 것 (원래 폴더를 다 지우지 못함 등)
    pub notes: Vec<String>,
}

/// 할 일을 확인하고 계획을 세운다. 아무것도 바꾸지 않는다.
pub fn plan(job: &Job, loc: &Location) -> Result<Plan, String> {
    if loc.kind == Kind::Env {
        return Err("환경 변수 GEMMA_SEARCH_DATA로 데이터 폴더를 정해 두어 옮기지 않습니다.".into());
    }
    let home = loc
        .home
        .clone()
        .ok_or("사용자 폴더(LOCALAPPDATA·HOME)를 알 수 없어 고정 위치가 없습니다.")?;
    match job {
        Job::Import(from) => {
            let from = absolute(from)?;
            let (src_data, old_root) = find_data(&from).ok_or_else(|| {
                format!("{}에서 예전 데이터(data 폴더의 index.db)를 찾지 못했습니다.", from.display())
            })?;
            let dest_data = home.join(DATA);
            if same_path(&src_data, &dest_data) {
                return Err("이미 고정 위치의 데이터입니다.".into());
            }
            if inside(&home, &src_data) || inside(&dest_data, &src_data) {
                return Err("데이터 폴더 안으로는 옮길 수 없습니다.".into());
            }
            let mut items = vec![Item {
                name: "설정·인덱스 (data)".into(),
                bytes: tree_size(&src_data),
                from: src_data.clone(),
                to: dest_data.clone(),
            }];
            // 설정이 예전 폴더 안의 llama.cpp·models를 가리키면 함께 옮긴다 (직접 지정한 다른 곳의 경로는 그대로)
            let used = config_paths(&src_data.join("config.toml"));
            for name in PREP_DIRS {
                let dir = old_root.join(name);
                let in_use = used.iter().any(|p| rel_under(p, &dir).is_some());
                if in_use && dir.is_dir() && !home.join(name).exists() && !same_path(&dir, &home.join(name)) {
                    items.push(Item {
                        name: format!("준비물 ({name})"),
                        bytes: tree_size(&dir),
                        from: dir,
                        to: home.join(name),
                    });
                }
            }
            let replaces = (dest_data.join(INDEX).is_file() && has_index(&dest_data))
                .then(|| file_count(&dest_data.join(INDEX)).unwrap_or(-1));
            Ok(Plan {
                job: job.clone(),
                copy: !crate::platform::same_volume(&src_data, &home),
                items,
                home,
                replaces,
                old_root,
            })
        }
        Job::MoveTo(to) => {
            if loc.kind != Kind::Home {
                return Err("고정 위치를 쓰고 있을 때만 다른 곳으로 옮길 수 있습니다. 먼저 고정 위치로 옮기세요.".into());
            }
            let base = loc.base.clone().unwrap_or_else(|| home.clone());
            let from = loc.root.clone();
            if !from.join(DATA).is_dir() {
                return Err(format!("{}에 옮길 데이터가 없습니다.", from.display()));
            }
            let to = target_dir(&absolute(to)?, &base)?;
            if same_path(&to, &from) {
                return Err("지금 위치와 같습니다.".into());
            }
            if inside(&to, &from) {
                return Err("지금 위치 안의 폴더로는 옮길 수 없습니다.".into());
            }
            // 데이터가 먼저, 그다음 준비물 등 나머지 (기본 위치의 표시 파일은 두고)
            let mut names: Vec<String> = std::fs::read_dir(&from)
                .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
                .unwrap_or_default();
            names.retain(|n| n != POINTER && n != DATA);
            names.sort();
            names.insert(0, DATA.to_string());
            let items = names
                .into_iter()
                .filter(|n| n == DATA || from.join(n).exists())
                .map(|n| Item {
                    name: if n == DATA { "설정·인덱스 (data)".into() } else { n.clone() },
                    bytes: tree_size(&from.join(&n)),
                    from: from.join(&n),
                    to: to.join(&n),
                })
                .collect();
            Ok(Plan {
                job: job.clone(),
                copy: !crate::platform::same_volume(&from, &to),
                items,
                home: to,
                replaces: None,
                old_root: from,
            })
        }
    }
}

/// 계획대로 옮긴다. 다른 Gemma Search가 데이터를 쓰고 있으면 아무것도 하지 않는다.
/// 데이터 폴더를 옮기지 못하면 오류(바뀐 것 없음). 준비물을 옮기지 못하면 원래 자리의 것을 계속 쓰고 알린다.
pub fn execute(plan: &Plan, loc: &Location, report: &dyn Fn(Progress)) -> Result<Moved, String> {
    let data = plan.items.first().ok_or("옮길 것이 없습니다.")?;
    ensure_unlocked(&data.from)?;
    ensure_unlocked(&data.to)?;
    let mut notes = Vec::new();
    // 옮겨 갈 곳에 있던 데이터: 인덱스가 비어 있으면(처음 실행만 해 본 것) 지우고, 아니면 이름을 바꿔 남긴다
    if data.to.exists() {
        if has_index(&data.to) {
            let aside = unique_sibling(&data.to, "data.before-import");
            std::fs::rename(&data.to, &aside)
                .map_err(|e| format!("고정 위치에 있던 데이터({})를 비키지 못했습니다: {e}", data.to.display()))?;
            notes.push(format!("고정 위치에 있던 인덱스는 {}로 이름을 바꿔 두었습니다.", aside.display()));
        } else if let Err(e) = std::fs::remove_dir_all(&data.to) {
            return Err(format!("고정 위치의 빈 데이터 폴더({})를 지우지 못했습니다: {e}", data.to.display()));
        }
    }
    let index = data.from.join(INDEX);
    let count = if index.is_file() { Some(checkpoint(&index)?) } else { None };

    let total = plan.total_bytes();
    let done = std::cell::Cell::new(0u64);
    let step = |item: &Item, bytes: u64| {
        report(Progress {
            item: item.name.clone(),
            done: done.get() + bytes,
            total,
        })
    };
    // 1. 데이터: 실패하면 그대로 끝 (복사하다 실패했으면 반쯤 된 복사본은 지운다)
    let verify = |dir: &Path| -> Result<(), String> {
        match count {
            Some(n) => match file_count(&dir.join(INDEX)) {
                Ok(m) if m == n => Ok(()),
                Ok(m) => Err(format!("복사한 인덱스의 파일 수가 다릅니다 (원래 {n}개, 복사본 {m}개).")),
                Err(e) => Err(format!("복사한 인덱스를 열 수 없습니다: {e}")),
            },
            None => Ok(()),
        }
    };
    move_dir(&data.from, &data.to, &mut |b| step(data, b), &verify, &mut notes)?;
    done.set(done.get() + data.bytes);

    // 2. 어디를 쓸지 적는다: 고정 위치를 옮겼으면 기본 위치의 표시 파일, 실행 파일 옆에서 가져왔으면 포터블 표시를 지운다
    match &plan.job {
        Job::MoveTo(_) => {
            let base = loc.base.clone().unwrap_or_else(|| plan.old_root.clone());
            let r = if same_path(&plan.home, &base) {
                std::fs::remove_file(base.join(POINTER)).or_else(|e| match e.kind() {
                    std::io::ErrorKind::NotFound => Ok(()),
                    _ => Err(e),
                })
            } else {
                std::fs::create_dir_all(&base)
                    .and_then(|_| std::fs::write(base.join(POINTER), format!("{}\n", plan.home.display())))
            };
            if let Err(e) = r {
                notes.push(format!(
                    "새 위치를 {}에 적지 못했습니다 ({e}). 이 파일에 {} 한 줄을 적어 주세요.",
                    base.join(POINTER).display(),
                    plan.home.display()
                ));
            }
        }
        Job::Import(_) => {
            if same_path(&plan.old_root, &loc.app_dir) {
                for m in PORTABLE_MARKERS {
                    let _ = std::fs::remove_file(loc.app_dir.join(m));
                }
            }
        }
    }

    // 3. 준비물 등 나머지: 실패해도 원래 자리의 것을 그대로 쓴다
    for item in &plan.items[1..] {
        let r = if item.from.is_dir() {
            move_dir(&item.from, &item.to, &mut |b| step(item, b), &no_check, &mut notes)
        } else {
            move_file(&item.from, &item.to)
        };
        if let Err(e) = r {
            notes.push(format!("{}를 옮기지 못해 원래 자리의 것을 그대로 씁니다: {e}", item.name));
        }
        done.set(done.get() + item.bytes);
    }

    // 4. 설정의 경로(llama.cpp·모델)를 새 위치로
    if let Err(e) = retarget_config(&data.to.join("config.toml"), &plan.old_root, &plan.home) {
        notes.push(format!("설정의 llama.cpp·모델 경로를 고치지 못했습니다: {e}"));
    }
    // 5. 고정 위치를 옮겼으면 비게 된 예전 위치를 지운다 (기본 위치는 표시 파일이 있으니 둔다)
    if matches!(plan.job, Job::MoveTo(_)) && loc.base.as_deref().is_none_or(|b| !same_path(b, &plan.old_root)) {
        let _ = std::fs::remove_dir(&plan.old_root);
    }
    Ok(Moved {
        home: plan.home.clone(),
        notes,
    })
}

/// 다른 프로세스(앱·서비스)가 이 데이터 폴더를 쓰고 있으면 오류. 없는 폴더는 괜찮다.
fn ensure_unlocked(data: &Path) -> Result<(), String> {
    if !data.is_dir() {
        return Ok(());
    }
    match crate::config::instance_lock(data) {
        Ok(lock) => {
            drop(lock);
            Ok(())
        }
        Err(e) if e.starts_with(crate::config::LOCK_BUSY) => Err(format!(
            "{e}\n앱과 서비스(gemma-search serve)를 끈 뒤에 다시 해 주세요."
        )),
        // 쓸 수 없는 폴더 등은 옮기면서 드러난다
        Err(_) => Ok(()),
    }
}

/// 고른 곳이 비어 있으면 그곳, 아니면 그 아래 `GemmaSearch`(Linux는 `gemma-search`) 폴더
fn target_dir(picked: &Path, base: &Path) -> Result<PathBuf, String> {
    if empty_or_missing(picked) {
        return Ok(picked.to_path_buf());
    }
    let name = base.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| "GemmaSearch".into());
    let sub = picked.join(name);
    if empty_or_missing(&sub) {
        Ok(sub)
    } else {
        Err(format!("{}에 이미 파일이 있습니다. 빈 폴더를 골라 주세요.", sub.display()))
    }
}

/// 없거나 빈 폴더 (표시 파일 `location.txt`만 있는 기본 위치도 빈 것으로 본다)
fn empty_or_missing(p: &Path) -> bool {
    match std::fs::read_dir(p) {
        Ok(rd) => rd.flatten().all(|e| e.file_name() == POINTER),
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

/// 고른 폴더에서 데이터 폴더와 그 프로그램 폴더를 찾는다: 프로그램 폴더(`data/index.db`가 있는 곳)나 `data` 폴더 자체
fn find_data(from: &Path) -> Option<(PathBuf, PathBuf)> {
    if from.join(DATA).join(INDEX).is_file() {
        return Some((from.join(DATA), from.to_path_buf()));
    }
    if from.join(INDEX).is_file() {
        let root = from.parent().map(Path::to_path_buf).unwrap_or_else(|| from.to_path_buf());
        return Some((from.to_path_buf(), root));
    }
    None
}

fn absolute(p: &Path) -> Result<PathBuf, String> {
    std::path::absolute(p).map_err(|e| format!("경로가 잘못됐습니다 ({}): {e}", p.display()))
}

/// `dir` 옆에 겹치지 않는 이름 (`<prefix>-<시각>`)
fn unique_sibling(dir: &Path, prefix: &str) -> PathBuf {
    let [y, mo, d, h, mi, s] = crate::platform::local_time();
    let stamp = format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}");
    let parent = dir.parent().unwrap_or(dir);
    let mut p = parent.join(format!("{prefix}-{stamp}"));
    let mut n = 2;
    while p.exists() {
        p = parent.join(format!("{prefix}-{stamp}-{n}"));
        n += 1;
    }
    p
}

/// 폴더를 옮긴다: 이름 바꾸기가 되면 그것으로 끝. 안 되면(다른 드라이브) `<to>.partial`에 복사하고,
/// 크기 합과 `verify`로 확인한 뒤 이름을 바꾸고 원래 폴더를 지운다.
fn move_dir(
    from: &Path,
    to: &Path,
    progress: &mut dyn FnMut(u64),
    verify: &dyn Fn(&Path) -> Result<(), String>,
    notes: &mut Vec<String>,
) -> Result<(), String> {
    if to.exists() && !(empty_or_missing(to) && std::fs::remove_dir(to).is_ok()) {
        return Err(format!("{}에 이미 파일이 있습니다.", to.display()));
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}를 만들 수 없습니다: {e}", parent.display()))?;
    }
    if !force_copy() && std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let partial = {
        let mut name = to.file_name().unwrap_or_default().to_os_string();
        name.push(".partial");
        to.with_file_name(name)
    };
    if partial.exists() {
        std::fs::remove_dir_all(&partial)
            .map_err(|e| format!("예전에 복사하다 만 폴더({})를 지우지 못했습니다: {e}", partial.display()))?;
    }
    let mut copied = 0u64;
    let result = copy_tree(from, &partial, &mut |n| {
        copied += n;
        progress(copied)
    })
    .and_then(|_| {
        let (a, b) = (tree_size(from), tree_size(&partial));
        if a == b {
            Ok(())
        } else {
            Err(format!("복사한 크기가 다릅니다 (원래 {a}바이트, 복사본 {b}바이트)."))
        }
    })
    .and_then(|_| verify(&partial))
    .and_then(|_| std::fs::rename(&partial, to).map_err(|e| format!("복사본의 이름을 바꾸지 못했습니다: {e}")));
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(format!("{}를 {}로 옮기지 못했습니다: {e}", from.display(), to.display()));
    }
    // 새 위치에서 확인했으므로 원래 것을 지운다
    if let Err(e) = std::fs::remove_dir_all(from) {
        notes.push(format!("복사는 끝났지만 원래 폴더 {}를 다 지우지 못했습니다 ({e}). 직접 지워 주세요.", from.display()));
    }
    Ok(())
}

fn no_check(_: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// 시험용: 이름 바꾸기를 건너뛰고 복사하는 길(다른 드라이브)로 옮긴다
    static FORCE_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn force_copy() -> bool {
    #[cfg(test)]
    return FORCE_COPY.with(std::cell::Cell::get);
    #[cfg(not(test))]
    false
}

fn move_file(from: &Path, to: &Path) -> Result<(), String> {
    if to.exists() {
        return Err(format!("{}에 이미 파일이 있습니다.", to.display()));
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)
        .and_then(|_| std::fs::remove_file(from))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 폴더를 통째로 복사한다 (파일 권한도). 복사한 바이트를 `progress`로 알린다.
fn copy_tree(from: &Path, to: &Path, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    std::fs::create_dir_all(to).map_err(|e| err(to, e))?;
    for entry in std::fs::read_dir(from).map_err(|e| err(from, e))? {
        let entry = entry.map_err(|e| err(from, e))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let meta = std::fs::symlink_metadata(&src).map_err(|e| err(&src, e))?;
        #[cfg(unix)]
        if meta.file_type().is_symlink() {
            // Linux용 llama.cpp의 라이브러리 링크(libllama.so → …)는 링크 그대로
            let target = std::fs::read_link(&src).map_err(|e| err(&src, e))?;
            std::os::unix::fs::symlink(target, &dst).map_err(|e| err(&dst, e))?;
            continue;
        }
        if meta.is_dir() {
            copy_tree(&src, &dst, progress)?;
        } else {
            copy_file(&src, &dst, progress).map_err(|e| err(&src, e))?;
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path, progress: &mut dyn FnMut(u64)) -> std::io::Result<()> {
    use std::io::{Read, Write};
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::File::create(to)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])?;
        progress(n as u64);
    }
    dst.sync_all()?;
    drop(dst);
    std::fs::set_permissions(to, src.metadata()?.permissions())
}

/// 폴더(또는 파일)의 크기 합. 읽을 수 없는 것은 건너뛴다.
pub fn tree_size(p: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(p) else { return 0 };
    if meta.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| tree_size(&e.path())).sum())
            .unwrap_or(0)
    } else {
        meta.len()
    }
}

/// 설정 파일에 적힌 준비물 경로들
fn config_paths(config: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(config) else { return Vec::new() };
    let Ok(table) = text.parse::<toml::Table>() else { return Vec::new() };
    PATH_KEYS
        .iter()
        .filter_map(|k| table.get(*k)?.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// 옮기면 고쳐야 하는 설정 항목
const PATH_KEYS: [&str; 4] = ["llama_path", "extra_dll_dir", "model_path", "mmproj_path"];

/// 설정의 경로 가운데 `old_root` 아래를 가리키는 것을, `new_root` 아래 같은 자리에 그것이 있으면 그쪽으로 바꾼다.
/// 다른 항목은 건드리지 않는다.
fn retarget_config(config: &Path, old_root: &Path, new_root: &Path) -> Result<(), String> {
    let text = match std::fs::read_to_string(config) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let mut table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut changed = false;
    for key in PATH_KEYS {
        let Some(old) = table.get(key).and_then(|v| v.as_str()).map(|s| s.trim().to_string()) else { continue };
        if old.is_empty() {
            continue;
        }
        if let Some(rel) = rel_under(Path::new(&old), old_root) {
            let new = new_root.join(rel);
            if new.exists() {
                table.insert(key.to_string(), toml::Value::String(new.display().to_string()));
                changed = true;
            }
        }
    }
    if changed {
        let s = toml::to_string_pretty(&table).map_err(|e| e.to_string())?;
        std::fs::write(config, s).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 크기는 받기 화면과 같이 10진 MB로 적는다
pub fn size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        format!("{:.1}GB", mb / 1000.0)
    } else if mb >= 1.0 {
        format!("{mb:.0}MB")
    } else {
        format!("{}KB", bytes.div_ceil(1000))
    }
}

// ── 명령줄: gemma-search data ──

pub const USAGE: &str = "\
데이터 위치:
  gemma-search data               설정·인덱스와 받은 준비물이 있는 곳을 보여 준다
  gemma-search data move          실행 파일 옆의 예전 data/(와 거기 받은 준비물)를 고정 위치로 옮긴다
  gemma-search data move-to <폴더>
                                  고정 위치를 통째로 다른 곳(예: 다른 디스크)으로 옮긴다
  gemma-search data import <폴더> 예전 버전 폴더(또는 그 data 폴더)의 설정·인덱스를 가져온다
      -y, --yes                   묻지 않고 옮긴다
  옮기는 동안에는 앱과 서비스를 꺼 두세요. 다른 디스크로 옮길 때는 복사하고 새 위치에서 인덱스를 확인한 뒤
  원래 것을 지웁니다.";

pub fn cli(args: &[String]) -> i32 {
    use crate::cli::{parse_args, usage_error, wants_help};
    if wants_help(args) {
        println!("{USAGE}");
        return 0;
    }
    let a = match parse_args(args, &[], &["--yes"], true) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };
    let loc = current();
    let words: Vec<&str> = a.words.iter().map(String::as_str).collect();
    let job = match words.as_slice() {
        [] => {
            print_location(&loc);
            return 0;
        }
        ["move"] => match loc.kind {
            Kind::Legacy | Kind::Portable => Job::Import(loc.app_dir.clone()),
            Kind::Home => {
                println!("이미 고정 위치({})를 쓰고 있습니다.", loc.root.display());
                return 0;
            }
            Kind::Env => return usage_error("환경 변수 GEMMA_SEARCH_DATA로 데이터 폴더를 정해 두어 옮기지 않습니다."),
        },
        ["move-to", to] => Job::MoveTo(PathBuf::from(*to)),
        ["import", from] => {
            if loc.kind != Kind::Home {
                return usage_error(
                    "고정 위치를 쓰고 있을 때만 가져올 수 있습니다. 실행 파일 옆의 데이터는 `gemma-search data move`로 옮기세요.",
                );
            }
            Job::Import(PathBuf::from(*from))
        }
        _ => return usage_error(&format!("모르는 data 명령입니다: {}\n\n{USAGE}", words.join(" "))),
    };
    let plan = match plan(&job, &loc) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("옮길 것:");
    for line in plan.describe() {
        println!("  {line}");
    }
    if !a.flag("--yes") && !confirm("옮길까요? [Y/n] ") {
        println!("옮기지 않았습니다.");
        return 1;
    }
    let last = Mutex::new(std::time::Instant::now());
    let report = |p: Progress| {
        let mut l = last.lock().unwrap_or_else(|e| e.into_inner());
        if l.elapsed() >= Duration::from_secs(2) && p.total > 0 {
            *l = std::time::Instant::now();
            println!("  {} 복사 중… {} / {}", p.item, size(p.done), size(p.total));
        }
    };
    match execute(&plan, &loc, &report) {
        Ok(moved) => {
            println!("옮겼습니다: {}", moved.home.display());
            for n in &moved.notes {
                println!("참고: {n}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn print_location(loc: &Location) {
    println!("데이터 위치: {}", loc.kind_label());
    println!("  설정·인덱스: {}", loc.data.display());
    println!("  받은 준비물: {}", loc.root.display());
    match (&loc.kind, &loc.home) {
        (Kind::Legacy, Some(h)) => {
            println!("고정 위치({})가 비어 있어 실행 파일 옆의 예전 데이터를 그대로 씁니다.", h.display());
            println!("옮기려면 앱과 서비스를 끄고 `gemma-search data move`를 실행하세요.");
        }
        (Kind::Portable, Some(h)) if is_portable(&loc.app_dir) => {
            println!("실행 파일 옆의 portable.txt 때문에 포터블로 씁니다. 고정 위치({})로 옮기려면 `gemma-search data move`.", h.display());
        }
        (Kind::Portable, None) => println!("사용자 폴더(LOCALAPPDATA·HOME)를 알 수 없어 실행 파일 옆을 씁니다."),
        _ => {}
    }
}

fn confirm(prompt: &str) -> bool {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        return true;
    }
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut ans = String::new();
    let _ = std::io::stdin().read_line(&mut ans);
    let ans = ans.trim().to_lowercase();
    ans.is_empty() || ["y", "yes", "예", "네", "ㅇ"].contains(&ans.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험마다 따로 쓰는 임시 폴더 (끝나면 지운다)
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("gs-loc-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 파일 `n`개가 든 인덱스를 만든다 (앱처럼 WAL 모드)
    fn make_index(data: &Path, n: usize) {
        drop(open_index(data, n));
    }

    fn open_index(data: &Path, n: usize) -> rusqlite::Connection {
        std::fs::create_dir_all(data).unwrap();
        let conn = rusqlite::Connection::open(data.join(INDEX)).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE IF NOT EXISTS files(id INTEGER PRIMARY KEY, path TEXT);",
        )
        .unwrap();
        for i in 0..n {
            conn.execute("INSERT INTO files(path) VALUES (?1)", [format!("f{i}")]).unwrap();
        }
        conn
    }

    /// 쓰기 기록(WAL)을 본 파일에 합치지 않은 채 꺼진 인덱스: 연결이 열려 있을 때의 파일을 `data`에 복사해 둔다.
    /// (마지막 연결이 닫히면 SQLite가 기록을 합치고 지우므로, 그 전에 찍어 둔다)
    fn make_unmerged_index(data: &Path, n: usize) {
        let live = data.with_file_name("live-db");
        let conn = open_index(&live, n);
        std::fs::create_dir_all(data).unwrap();
        for f in [INDEX, "index.db-wal"] {
            std::fs::copy(live.join(f), data.join(f)).unwrap();
        }
        drop(conn);
        std::fs::remove_dir_all(&live).unwrap();
        assert!(std::fs::metadata(data.join("index.db-wal")).unwrap().len() > 0, "합치지 않은 기록이 남아 있어야 시험이 된다");
    }

    fn inputs(app: &Path, base: &Path) -> Inputs {
        Inputs {
            env_data: None,
            app_dir: app.to_path_buf(),
            base: Some(base.to_path_buf()),
        }
    }

    #[test]
    fn resolves_home_portable_legacy_and_env() {
        let t = Tmp::new("resolve");
        let (app, base) = (t.0.join("app"), t.0.join("local").join("GemmaSearch"));
        std::fs::create_dir_all(&app).unwrap();

        // 아무것도 없으면 고정 위치. 위치를 정하면서 아무것도 만들지 않는다.
        let loc = resolve(&inputs(&app, &base));
        assert_eq!((loc.kind, loc.root.clone(), loc.data.clone()), (Kind::Home, base.clone(), base.join("data")));
        assert!(!base.exists(), "위치를 정할 때는 폴더를 만들지 않는다");

        // 실행 파일 옆에 예전 인덱스가 있으면 옮기기 전까지 그쪽
        make_index(&app.join("data"), 3);
        let loc = resolve(&inputs(&app, &base));
        assert_eq!((loc.kind, loc.data.clone()), (Kind::Legacy, app.join("data")));
        assert_eq!(loc.home.as_deref(), Some(base.as_path()));

        // 고정 위치에 빈 인덱스(처음 실행만 해 본 것)만 있으면 여전히 옮기기 전
        make_index(&base.join("data"), 0);
        assert_eq!(resolve(&inputs(&app, &base)).kind, Kind::Legacy);
        // 고정 위치에 파일이 든 인덱스가 생기면 고정 위치
        make_index(&base.join("data"), 1);
        assert_eq!(resolve(&inputs(&app, &base)).kind, Kind::Home);

        // 포터블 표시가 있으면 실행 파일 옆
        mark_portable(&app).unwrap();
        let loc = resolve(&inputs(&app, &base));
        assert_eq!((loc.kind, loc.root.clone()), (Kind::Portable, app.clone()));
        std::fs::remove_file(app.join(PORTABLE_MARKERS[0])).unwrap();

        // 다른 곳으로 옮긴 고정 위치 (상대 경로는 무시)
        let moved = t.0.join("other");
        std::fs::write(base.join(POINTER), format!("\n  {}  \n", moved.display())).unwrap();
        assert_eq!(resolve(&inputs(&app, &base)).home, Some(moved.clone()));
        std::fs::write(base.join(POINTER), "relative/path\n").unwrap();
        assert_eq!(resolve(&inputs(&app, &base)).root, base);

        // 실행 파일 폴더가 곧 고정 위치면 옮길 것이 없다
        std::fs::write(base.join(POINTER), format!("{}\n", app.display())).unwrap();
        std::fs::remove_dir_all(base.join("data")).unwrap();
        make_index(&app.join("data"), 0);
        assert_eq!(resolve(&inputs(&app, &base)).kind, Kind::Home);
        std::fs::remove_file(base.join(POINTER)).unwrap();
        make_index(&base.join("data"), 1);

        // 환경 변수는 데이터 폴더만 바꾼다
        let mut inp = inputs(&app, &base);
        inp.env_data = Some(t.0.join("env"));
        let loc = resolve(&inp);
        assert_eq!((loc.kind, loc.data.clone(), loc.root.clone()), (Kind::Env, t.0.join("env"), base.clone()));

        // 사용자 폴더를 모르면 실행 파일 옆
        let loc = resolve(&Inputs { env_data: None, app_dir: app.clone(), base: None });
        assert_eq!((loc.kind, loc.data), (Kind::Portable, app.join("data")));
    }

    #[test]
    fn compares_paths_by_component() {
        let base = std::env::temp_dir().join("gs-a").join("Root");
        assert_eq!(rel_under(&base.join("x").join("y.gguf"), &base), Some(Path::new("x").join("y.gguf")));
        assert!(same_path(&base, &base.join(".")));
        assert!(rel_under(&base.with_file_name("Root2").join("x"), &base).is_none());
        assert!(inside(&base.join("x"), &base) && !inside(&base, &base));
        if cfg!(windows) {
            assert!(same_path(Path::new(r"C:\Users\A\GemmaSearch"), Path::new(r"c:\users\a\gemmasearch\")));
        }
    }

    #[test]
    fn imports_old_folder_with_wal_and_prep_files() {
        for copy in [false, true] {
            import_old_folder(copy);
        }
    }

    /// `copy`면 이름 바꾸기 대신 복사하는 길(다른 드라이브)로
    fn import_old_folder(copy: bool) {
        FORCE_COPY.with(|f| f.set(copy));
        let t = Tmp::new(if copy { "import-copy" } else { "import" });
        let (old, base) = (t.0.join("old-app"), t.0.join("local").join("GemmaSearch"));
        // 예전 폴더: 합치지 않은 쓰기 기록(WAL)이 있는 인덱스, 거기 받은 준비물, 그 경로를 적은 설정
        make_unmerged_index(&old.join("data"), 5);
        let model = old.join("models").join("embeddinggemma-2").join("m.gguf");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(&model, b"model").unwrap();
        std::fs::create_dir_all(old.join("llama.cpp")).unwrap();
        std::fs::write(old.join("llama.cpp").join("llama-server"), b"bin").unwrap();
        let custom = t.0.join("custom-llama");
        std::fs::write(
            old.join("data").join("config.toml"),
            format!(
                "llama_path = '{}'\nmodel_path = '{}'\nmmproj_path = ''\nport = 8090\n",
                custom.display(),
                model.display()
            ),
        )
        .unwrap();

        let loc = resolve(&inputs(&old, &base));
        assert_eq!(loc.kind, Kind::Legacy);
        let p = plan(&Job::Import(old.clone()), &loc).unwrap();
        // 설정이 가리키는 models만 함께 (llama.cpp는 직접 지정한 다른 곳을 쓰므로 두고)
        let names: Vec<_> = p.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["설정·인덱스 (data)", "준비물 (models)"]);
        mark_portable(&old).unwrap();

        let moved = execute(&p, &loc, &|_| {}).unwrap();
        assert_eq!(moved.home, base);
        assert_eq!(file_count(&base.join("data").join(INDEX)).unwrap(), 5);
        assert!(!old.join("data").exists() && !old.join("models").exists());
        assert!(old.join("llama.cpp").exists(), "설정이 가리키지 않는 준비물은 두고 간다");
        assert!(!is_portable(&old), "실행 파일 옆에서 가져왔으면 포터블 표시를 지운다");
        let cfg = std::fs::read_to_string(base.join("data").join("config.toml")).unwrap();
        let table: toml::Table = cfg.parse().unwrap();
        assert_eq!(table["model_path"].as_str().unwrap(), base.join("models").join("embeddinggemma-2").join("m.gguf").display().to_string());
        assert_eq!(table["llama_path"].as_str().unwrap(), custom.display().to_string(), "직접 지정한 경로는 그대로");
        assert_eq!(table["port"].as_integer(), Some(8090), "다른 항목은 그대로");
        assert_eq!(resolve(&inputs(&old, &base)).kind, Kind::Home);
        assert!(!base.with_file_name("data.partial").exists() && !base.join("data.partial").exists());
        FORCE_COPY.with(|f| f.set(false));
    }

    #[test]
    fn copy_keeps_unmerged_writes_and_rolls_back_on_failed_check() {
        FORCE_COPY.with(|f| f.set(true));
        let t = Tmp::new("copy");
        let src = t.0.join("src");
        make_unmerged_index(&src, 4);
        std::fs::create_dir_all(src.join("logs")).unwrap();
        std::fs::write(src.join("logs").join("a.log"), b"log").unwrap();

        // 합치기 전의 파일을 그대로 복사해도 복사본에서 기록이 살아난다
        let mut notes = Vec::new();
        let copied = t.0.join("copied");
        let mut bytes = 0;
        move_dir(&src, &copied, &mut |b| bytes = b, &no_check, &mut notes).unwrap();
        assert!(!src.exists() && notes.is_empty(), "확인했으면 원래 것을 지운다: {notes:?}");
        assert_eq!(bytes, tree_size(&copied));
        assert_eq!(file_count(&copied.join(INDEX)).unwrap(), 4);

        // 확인에 실패하면 복사본을 지우고 원래 것은 그대로 둔다
        let dst = t.0.join("dst");
        let fail = |_: &Path| -> Result<(), String> { Err("확인 실패".into()) };
        let err = move_dir(&copied, &dst, &mut |_| {}, &fail, &mut notes).unwrap_err();
        assert!(err.contains("확인 실패"), "{err}");
        assert!(!dst.exists() && !t.0.join("dst.partial").exists());
        assert_eq!(file_count(&copied.join(INDEX)).unwrap(), 4);

        // 비어 있지 않은 곳에는 옮기지 않는다
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("x"), b"x").unwrap();
        assert!(move_dir(&copied, &dst, &mut |_| {}, &no_check, &mut notes).is_err());
        assert!(copied.join(INDEX).is_file());
        FORCE_COPY.with(|f| f.set(false));
    }

    #[test]
    fn moves_home_to_another_folder_and_back() {
        let t = Tmp::new("moveto");
        let (app, base) = (t.0.join("app"), t.0.join("local").join("GemmaSearch"));
        std::fs::create_dir_all(&app).unwrap();
        make_index(&base.join("data"), 2);
        let model = base.join("models").join("m.gguf");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(&model, b"m").unwrap();
        std::fs::write(base.join("data").join("config.toml"), format!("model_path = '{}'\n", model.display())).unwrap();

        // 비어 있지 않은 폴더를 고르면 그 아래 GemmaSearch로
        let disk = t.0.join("disk");
        std::fs::create_dir_all(&disk).unwrap();
        std::fs::write(disk.join("other.txt"), b"x").unwrap();
        let loc = resolve(&inputs(&app, &base));
        let p = plan(&Job::MoveTo(disk.clone()), &loc).unwrap();
        let target = disk.join("GemmaSearch");
        assert_eq!(p.home, target);
        assert_eq!(p.items[0].name, "설정·인덱스 (data)");
        execute(&p, &loc, &|_| {}).unwrap();
        assert_eq!(read_pointer(&base), Some(target.clone()));
        let loc = resolve(&inputs(&app, &base));
        assert_eq!((loc.kind, loc.root.clone()), (Kind::Home, target.clone()));
        assert_eq!(file_count(&loc.data.join(INDEX)).unwrap(), 2);
        let cfg: toml::Table = std::fs::read_to_string(loc.data.join("config.toml")).unwrap().parse().unwrap();
        assert_eq!(cfg["model_path"].as_str().unwrap(), target.join("models").join("m.gguf").display().to_string());

        // 지금 위치 안이나 같은 곳으로는 옮기지 않는다
        assert!(plan(&Job::MoveTo(target.join("sub")), &loc).is_err());
        assert!(plan(&Job::MoveTo(target.clone()), &loc).is_err());

        // 기본 위치로 되돌리면 표시 파일을 지우고, 비게 된 예전 위치를 지운다
        let p = plan(&Job::MoveTo(base.clone()), &loc).unwrap();
        assert_eq!(p.home, base);
        execute(&p, &loc, &|_| {}).unwrap();
        assert!(!base.join(POINTER).exists());
        assert!(!target.exists());
        let loc = resolve(&inputs(&app, &base));
        assert_eq!((loc.kind, loc.root.clone()), (Kind::Home, base.clone()));
        assert!(base.join("models").join("m.gguf").is_file());
    }

    #[test]
    fn import_keeps_existing_index_aside() {
        let t = Tmp::new("aside");
        let (app, base, old) = (t.0.join("app"), t.0.join("local").join("GemmaSearch"), t.0.join("old"));
        std::fs::create_dir_all(&app).unwrap();
        make_index(&base.join("data"), 1);
        make_index(&old.join("data"), 7);
        let loc = resolve(&inputs(&app, &base));
        assert_eq!(loc.kind, Kind::Home);
        let p = plan(&Job::Import(old.join("data")), &loc).unwrap();
        assert_eq!(p.replaces, Some(1));
        let moved = execute(&p, &loc, &|_| {}).unwrap();
        assert_eq!(file_count(&base.join("data").join(INDEX)).unwrap(), 7);
        let aside: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("data.before-import-"))
            .collect();
        assert_eq!(aside.len(), 1, "있던 인덱스는 지우지 않고 남긴다: {:?}", moved.notes);
        // 지금 쓰는 고정 위치 자신을 가져오지는 않는다
        assert!(plan(&Job::Import(base.clone()), &resolve(&inputs(&app, &base))).is_err());
    }

    #[test]
    fn refuses_while_another_process_holds_the_data() {
        let t = Tmp::new("locked");
        let (app, base) = (t.0.join("app"), t.0.join("local").join("GemmaSearch"));
        make_index(&app.join("data"), 1);
        let loc = resolve(&inputs(&app, &base));
        let p = plan(&Job::Import(app.clone()), &loc).unwrap();
        let held = crate::config::instance_lock(&app.join("data")).unwrap();
        let err = execute(&p, &loc, &|_| {}).unwrap_err();
        assert!(err.contains(crate::config::LOCK_BUSY), "{err}");
        assert!(app.join("data").join(INDEX).is_file());
        drop(held);
    }
}
