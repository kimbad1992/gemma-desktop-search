//! 준비물 받기 명령: `gemma-search download`
//! 이 기기에 맞는 llama.cpp(b12201)와 EmbeddingGemma 2 모델을 받아 SHA-256을 확인하고 넣는다.
//!
//!   gemma-search download                   알맞은 llama.cpp와 모델을 받는다 (무엇을 받을지 먼저 보여 주고 묻는다)
//!   gemma-search download --list            고를 수 있는 llama.cpp 판과 크기, 이 기기의 추천
//!   gemma-search download --llama vulkan    판을 직접 고른다 (cpu, vulkan, cuda13, cuda12)
//!   gemma-search download --only model      모델만 (또는 --only llama)
//!   gemma-search download --dir <폴더>      받을 곳 (기본: 데이터 위치. `gemma-search data`로 확인)
//!   gemma-search download --yes             묻지 않고 받는다 (-y)
//!
//! 권한: 관리자 권한(sudo)을 쓰지 않고 시스템 폴더·패키지·그룹을 건드리지 않는다. 받을 곳은 이 계정이 쓸 수 있는
//! 폴더여야 한다. 그래픽을 쓰는 데 필요한 것(라이브러리, 장치 권한)이 빠졌으면 무엇이 필요한지만 알려 주고 CPU판을 고른다.
//!
//! 다 받으면 위치를 설정 파일에 적어 두므로 `gemma-search serve`가 바로 찾는다.

use crate::cli::{human_secs, parse_args, usage_error, wants_help};
use crate::config::{self, Config};
use crate::download::{self, DlState, Pack, LLAMA_BUILD};
use crate::sysinfo::Vendor;
use parking_lot::Mutex;
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 크기는 앱의 받기 화면과 같이 10진 MB로 적는다 (모델 865MB)
const MB: f64 = 1_000_000.0;
/// 터미널이 아닐 때(로그로 남길 때) 진행을 한 줄씩 남기는 간격
const LOG_EVERY: Duration = Duration::from_secs(10);

/// 고른 llama.cpp 판과 그 까닭
struct Choice {
    pack: Pack,
    why: String,
    /// 더 빠른 판을 쓰려면 필요한 것 (그래픽 라이브러리·장치 권한 등). 알려 주기만 한다.
    notes: Vec<String>,
}

pub fn download(args: &[String]) -> i32 {
    if wants_help(args) {
        println!("{}", crate::service::USAGE);
        return 0;
    }
    let a = match parse_args(args, &["--llama", "--only", "--dir"], &["--list", "--yes"], false) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };
    let (want_llama, want_model) = match a.value("--only").map(|o| o.trim().to_lowercase()) {
        None => (true, true),
        Some(o) if o == "model" => (false, true),
        Some(o) if o == "llama" || o == "llama.cpp" => (true, false),
        Some(o) => return usage_error(&format!("--only에는 model이나 llama를 주세요 (지금: {o})")),
    };
    let root = match a.value("--dir") {
        Some(d) => match std::path::absolute(d.trim()) {
            Ok(p) => p,
            Err(e) => return usage_error(&format!("--dir 경로가 잘못됐습니다: {e}")),
        },
        None => config::prep_dir(),
    };
    let picked = match a.value("--llama") {
        None => None,
        Some(k) => match Pack::LLAMAS.into_iter().find(|p| p.key() == k.trim().to_lowercase()) {
            Some(pack) => Some(Choice { pack, why: "직접 고른 판".to_string(), notes: Vec::new() }),
            None => return usage_error(&format!("모르는 판입니다: {k} (cpu, vulkan, cuda13, cuda12 가운데서 고르세요)")),
        },
    };
    if a.flag("--list") {
        return list(&root);
    }

    // ── 받을 것 정하기 ──
    let llama = match (want_llama, picked) {
        (false, _) => None,
        (true, Some(c)) => Some(c),
        (true, None) => {
            eprintln!("이 기기에 맞는 llama.cpp 판을 고르는 중…");
            Some(auto_choice())
        }
    };
    if let Some(c) = &llama {
        if c.pack.assets().is_empty() {
            eprintln!(
                "이 OS({})용 llama.cpp는 아직 받을 수 없습니다. llama.cpp를 직접 받아 serve에 --llama <폴더>로 주세요.",
                os_label()
            );
            return 1;
        }
    }
    println!("Gemma Search 준비물 받기");
    println!("  받을 곳    {}", root.display());
    let mut todo: Vec<Pack> = Vec::new();
    if let Some(c) = &llama {
        let have = download::installed(c.pack, &root);
        let now = match download::installed_llama(&root) {
            Some((k, b)) if !have => format!(" · 지금 있는 {k} 판({b})을 바꿉니다"),
            _ => String::new(),
        };
        println!(
            "  llama.cpp  {} {} · {}{now}",
            pack_name(c.pack),
            if have { "이미 있음".to_string() } else { size(c.pack.total_size()) },
            c.why
        );
        if !have {
            todo.push(c.pack);
        }
    }
    if want_model {
        let have = download::installed(Pack::Model, &root);
        println!(
            "  모델       EmbeddingGemma 2 Q8 {}",
            if have { "이미 있음".to_string() } else { size(Pack::Model.total_size()) }
        );
        if !have {
            todo.push(Pack::Model);
        }
    }
    for n in llama.iter().flat_map(|c| &c.notes) {
        println!("  참고: {n}");
    }
    if todo.is_empty() {
        println!("받을 것이 없습니다. 모두 받아 두었습니다.");
        return finish(&root, want_llama, want_model, &[]);
    }
    let total: u64 = todo.iter().map(|p| p.total_size()).sum();
    println!("  받을 크기  {}", size(total));

    // ── 받을 수 있는지: 쓰기 권한, 디스크 ──
    if let Err(e) = check_writable(&root) {
        eprintln!(
            "\n{}에 쓸 수 없습니다: {e}\n이 계정이 쓸 수 있는 폴더에 실행 파일을 두거나(예: ~/gemma-search), --dir <폴더>로 받을 곳을 정하세요.",
            root.display()
        );
        return 1;
    }
    // 묶음은 받은 뒤 푸는 동안 잠깐 두 배 남짓 자리를 쓴다
    let need_mb = todo
        .iter()
        .map(|p| p.total_size() as f64 / MB * if *p == Pack::Model { 1.0 } else { 3.0 })
        .sum::<f64>() as u64
        + 100;
    let free_mb = crate::platform::free_disk_mb(&root);
    if free_mb > 0 && free_mb < need_mb {
        eprintln!("\n디스크 공간이 모자랍니다: 약 {need_mb}MB가 필요한데 {free_mb}MB 남았습니다.");
        return 1;
    }

    if !a.flag("--yes") && std::io::stdin().is_terminal() {
        print!("받을까요? [Y/n] ");
        let _ = std::io::stdout().flush();
        let mut ans = String::new();
        let _ = std::io::stdin().read_line(&mut ans);
        let ans = ans.trim().to_lowercase();
        if !(ans.is_empty() || ["y", "yes", "예", "네", "ㅇ"].contains(&ans.as_str())) {
            println!("받지 않았습니다.");
            return 1;
        }
    }

    // ── 받기 ──
    let lock = match lock_root(&root) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("끊기거나 Ctrl+C로 멈춰도, 다시 실행하면 받은 데까지 이어서 받습니다.");
    let mut after = Vec::new();
    for (i, pack) in todo.iter().enumerate() {
        println!("[{}/{}] {}", i + 1, todo.len(), title(*pack));
        match run_with_progress(*pack, &root) {
            Ok(inst) => {
                println!("  넣음: {}", inst.dest.display());
                if let Some(old) = inst.kept_old {
                    after.push(format!(
                        "원래 있던 llama.cpp 폴더는 지우지 않고 {}로 옮겨 두었습니다. 필요 없으면 지워도 됩니다.",
                        old.display()
                    ));
                }
            }
            Err(e) => {
                eprintln!("  받지 못했습니다: {e}");
                eprintln!("다시 실행하면 받은 데까지 이어서 받습니다.");
                drop(lock);
                let _ = std::fs::remove_file(root.join(LOCK_FILE));
                return 1;
            }
        }
    }
    drop(lock);
    let _ = std::fs::remove_file(root.join(LOCK_FILE));
    finish(&root, want_llama, want_model, &after)
}

/// 받은 위치를 설정에 적고, 다음에 할 일을 알려 준다
fn finish(root: &Path, llama: bool, model: bool, notes: &[String]) -> i32 {
    let data = config::data_dir();
    let mut cfg = Config::load(&data);
    let before = (cfg.llama_path.clone(), cfg.model_path.clone(), cfg.mmproj_path.clone());
    if llama {
        cfg.llama_path = Pack::LlamaCpu.dest(root).display().to_string();
    }
    if model {
        let (m, p) = download::model_files(root);
        cfg.model_path = m.display().to_string();
        cfg.mmproj_path = p.display().to_string();
    }
    let after = (cfg.llama_path.clone(), cfg.model_path.clone(), cfg.mmproj_path.clone());
    if after != before {
        match cfg.save(&data) {
            Ok(()) => println!("설정에 위치를 적었습니다: {}", data.join("config.toml").display()),
            Err(e) => {
                eprintln!("설정을 저장하지 못했습니다 ({e}). serve를 띄울 때 위치를 직접 주세요:");
                eprintln!("  {} serve --llama {} --model {} --mmproj {}", me(), q(&after.0), q(&after.1), q(&after.2));
            }
        }
    }
    for n in notes {
        println!("참고: {n}");
    }
    // 서비스나 앱이 이 데이터 폴더로 떠 있으면 다시 띄워야 새 것을 쓴다 (데이터 폴더가 아직 없으면 떠 있을 리 없다)
    if data.is_dir() {
        if let Err(e) = config::instance_lock(&data) {
            if e.starts_with(config::LOCK_BUSY) {
                println!("참고: 서비스(또는 앱)가 이 데이터 폴더로 떠 있습니다. 끄고 다시 띄워야 새로 받은 것을 씁니다.");
            }
        }
    }
    println!();
    println!("준비됐습니다. 다음:");
    if cfg.roots.is_empty() {
        println!("  {} serve --root <검색할 폴더>   서비스를 띄운다 (폴더는 설정에 남으니 다음부터는 serve만)", me());
    } else {
        println!("  {} serve   서비스를 띄운다", me());
    }
    println!("  {} status  다른 터미널에서 상태를 본다", me());
    0
}

/// 고를 수 있는 판과 이 기기의 추천
fn list(root: &Path) -> i32 {
    eprintln!("이 기기를 살펴보는 중…");
    let rec = auto_choice();
    let have = download::installed_llama(root);
    println!("llama.cpp {LLAMA_BUILD} ({}) 가운데 고를 수 있는 판:", os_label());
    let mut any = false;
    for p in Pack::LLAMAS {
        if p.assets().is_empty() {
            continue;
        }
        any = true;
        let mut tail = String::new();
        if p == rec.pack {
            tail += "  ← 이 기기 추천";
        }
        if have.as_ref().is_some_and(|(k, b)| k == p.key() && b == LLAMA_BUILD) {
            tail += "  (받아 둠)";
        }
        println!("  {:<7} {:>6}  {}{tail}", p.key(), size(p.total_size()), describe(p));
    }
    if !any {
        println!("  (이 OS용은 아직 없습니다. llama.cpp를 직접 받아 serve에 --llama <폴더>로 주세요)");
    }
    println!("추천 까닭: {}", rec.why);
    for n in &rec.notes {
        println!("참고: {n}");
    }
    println!(
        "모델: EmbeddingGemma 2 Q8 {}{}",
        size(Pack::Model.total_size()),
        if download::installed(Pack::Model, root) { "  (받아 둠)" } else { "" }
    );
    println!("받을 곳: {}", root.display());
    println!("받기:");
    println!("  {} download                  추천 판과 모델을 받는다", me());
    println!("  {} download --llama <판>     판을 골라 받는다", me());
    0
}

// ── 판 고르기 ──

fn auto_choice() -> Choice {
    let info = crate::sysinfo::probe(&config::prep_dir(), false);
    let plan = crate::sysinfo::recommend(&info);
    if let Some(g) = info.gpus.iter().find(|g| g.vendor == Vendor::Nvidia) {
        let driver = g.driver.clone().unwrap_or_else(|| "?".into());
        let major = g.driver.as_deref().and_then(crate::sysinfo::driver_major).unwrap_or(0);
        // Linux의 CUDA 12판은 12.8이라 드라이버 570 이상이 필요하다 (Windows는 12.4)
        let pack = if cfg!(target_os = "linux") && plan.llama == Pack::LlamaCuda12 && major < 570 {
            Pack::LlamaVulkan
        } else {
            plan.llama
        };
        return Choice { pack, why: format!("{} (드라이버 {driver})", g.name), notes: Vec::new() };
    }
    other_gpu_choice(&plan)
}

/// NVIDIA가 아닐 때: `--sysinfo`(앱의 처음 설정)와 같은 추천을 따른다. Linux에서는 Vulkan으로 쓸 수 있는 그래픽만
/// 그래픽카드 목록에 들어가므로(`platform::vulkan_gpu`), CPU판이 됐으면 그 까닭을 함께 알려 준다.
fn other_gpu_choice(plan: &crate::sysinfo::Plan) -> Choice {
    if plan.llama != Pack::LlamaCpu {
        return Choice {
            pack: plan.llama,
            why: format!("{}, Vulkan으로 씀 (CPU판과 견주려면 --llama cpu)", plan.device),
            notes: Vec::new(),
        };
    }
    #[allow(unused_mut)]
    let mut notes: Vec<String> = Vec::new();
    #[cfg(target_os = "linux")]
    {
        match crate::platform::vulkan_gpu() {
            Err(why) => notes = why,
            Ok((dev, _)) => notes.push(format!("{dev}: 쓸 수 있는 메모리가 2GB보다 적어 CPU판을 고릅니다.")),
        }
        notes.push(format!("그래픽을 쓰려면 `{} download --llama vulkan`으로 바꿀 수 있습니다.", me()));
    }
    Choice { pack: Pack::LlamaCpu, why: "쓸 수 있는 그래픽이 없어 CPU로 돌립니다".to_string(), notes }
}

// ── 받는 동안 ──

const LOCK_FILE: &str = ".gemma-search-download.lock";

/// 같은 곳에 둘이 함께 받지 않게 (같은 .part 파일을 함께 쓰면 깨진다)
fn lock_root(root: &Path) -> Result<std::fs::File, String> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK_FILE))
        .map_err(|e| format!("{}에 쓸 수 없습니다: {e}", root.display()))?;
    match f.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("다른 download가 같은 곳에 받는 중입니다. 그것이 끝난 뒤 다시 실행하세요.".to_string())
        }
        _ => Ok(f),
    }
}

fn check_writable(root: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    let probe = root.join(format!(".gemma-search-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(&probe)
}

/// 받기를 다른 스레드에서 돌리며 진행을 그린다
fn run_with_progress(pack: Pack, root: &Path) -> anyhow::Result<download::Installed> {
    let st = Arc::new(Mutex::new(DlState::default()));
    let (st2, root2) = (st.clone(), root.to_path_buf());
    let job = std::thread::spawn(move || download::install(pack, &root2, &st2));
    let mut bar = Bar::new(std::io::stderr().is_terminal());
    while !job.is_finished() {
        let s = st.lock().clone();
        bar.draw(&s);
        std::thread::sleep(Duration::from_millis(150));
    }
    bar.end();
    job.join().unwrap_or_else(|_| Err(anyhow::anyhow!("받는 중에 문제가 생겨 멈췄습니다")))
}

/// 진행 막대: 터미널이면 한 줄을 고쳐 쓰고, 아니면 가끔 한 줄씩 남긴다
struct Bar {
    tty: bool,
    file: String,
    since: Instant,
    start_done: u64,
    drawn: bool,
    last_log: Option<Instant>,
}

impl Bar {
    fn new(tty: bool) -> Self {
        Self { tty, file: String::new(), since: Instant::now(), start_done: 0, drawn: false, last_log: None }
    }

    fn draw(&mut self, s: &DlState) {
        if s.file.is_empty() {
            return;
        }
        if s.file != self.file {
            self.end();
            self.file = s.file.clone();
            self.since = Instant::now();
            self.start_done = s.done;
            self.last_log = None;
        }
        let line = if s.total > 0 && !s.file.ends_with('…') {
            let frac = (s.done as f64 / s.total as f64).clamp(0.0, 1.0);
            let secs = self.since.elapsed().as_secs_f64();
            // 속도와 남은 시간은 1초쯤 재 본 뒤부터 (처음에는 0으로 보이므로)
            let speed = if secs >= 1.0 {
                let rate = s.done.saturating_sub(self.start_done) as f64 / secs;
                let eta = if rate > 1024.0 && s.done < s.total {
                    format!("  {} 남음", human_secs(((s.total - s.done) as f64 / rate) as u64 + 1))
                } else {
                    String::new()
                };
                format!("  {:.1}MB/s{eta}", rate / MB)
            } else {
                String::new()
            };
            format!(
                "  {}  {} {:>3.0}%  {:.1}/{:.1}MB{speed}",
                crate::util::truncate_chars(&s.file, 48),
                bar(frac, 20),
                frac * 100.0,
                s.done as f64 / MB,
                s.total as f64 / MB,
            )
        } else {
            format!("  {}", s.file)
        };
        if self.tty {
            eprint!("\r\x1b[2K{line}");
            let _ = std::io::stderr().flush();
            self.drawn = true;
        } else if self.last_log.is_none_or(|t| t.elapsed() >= LOG_EVERY) {
            eprintln!("{line}");
            self.last_log = Some(Instant::now());
        }
    }

    /// 그리던 줄을 마친다
    fn end(&mut self) {
        if self.tty && self.drawn {
            eprintln!();
            self.drawn = false;
        }
    }
}

fn bar(frac: f64, width: usize) -> String {
    let full = (frac * width as f64).round() as usize;
    format!("{}{}", "█".repeat(full.min(width)), "░".repeat(width - full.min(width)))
}

// ── 글자 ──

fn size(bytes: u64) -> String {
    let mb = bytes as f64 / MB;
    if mb >= 1000.0 {
        format!("{:.1}GB", mb / 1000.0)
    } else {
        format!("{mb:.0}MB")
    }
}

fn os_label() -> String {
    let arch = if cfg!(target_arch = "x86_64") { "x64" } else { std::env::consts::ARCH };
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        o => o,
    };
    format!("{os} {arch}")
}

fn pack_name(p: Pack) -> &'static str {
    match p {
        Pack::LlamaCpu => "CPU판",
        Pack::LlamaVulkan => "Vulkan판",
        Pack::LlamaCuda13 => "CUDA 13판",
        Pack::LlamaCuda12 => "CUDA 12판",
        Pack::Model => "모델",
    }
}

fn describe(p: Pack) -> &'static str {
    match p {
        Pack::LlamaCpu => "CPU만 씀 (그래픽 없이, 가장 느림)",
        Pack::LlamaVulkan => "그래픽 (Intel·AMD·NVIDIA, Vulkan 드라이버 필요)",
        Pack::LlamaCuda13 => "NVIDIA (드라이버 580 이상)",
        Pack::LlamaCuda12 if cfg!(windows) => "NVIDIA (드라이버 551 이상)",
        Pack::LlamaCuda12 => "NVIDIA (드라이버 570 이상)",
        Pack::Model => "EmbeddingGemma 2 Q8",
    }
}

fn title(p: Pack) -> String {
    match p {
        Pack::Model => format!("모델 EmbeddingGemma 2 Q8 ({})", size(p.total_size())),
        _ => format!("llama.cpp {LLAMA_BUILD} {} ({})", pack_name(p), size(p.total_size())),
    }
}

/// 안내에 쓸 이 프로그램의 이름: 경로로 실행했으면 그 경로(복사해 그대로 칠 수 있게), 아니면 gemma-search
pub(crate) fn me() -> String {
    std::env::args()
        .next()
        .filter(|a| a.contains('/') || a.contains('\\'))
        .map(|a| q(&a))
        .unwrap_or_else(|| "gemma-search".to_string())
}

/// 빈칸이 든 경로는 따옴표로
fn q(s: &str) -> String {
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_bars() {
        assert_eq!(size(23_067_593), "23MB");
        assert_eq!(size(864_676_512), "865MB");
        assert_eq!(size(1_600_000_000), "1.6GB");
        assert_eq!(bar(0.5, 4), "██░░");
        assert_eq!(bar(1.2, 3), "███");
        assert_eq!(q("/a b/x"), "\"/a b/x\"");
    }

    #[test]
    fn every_pack_has_a_name_and_this_os_has_llama_builds() {
        for p in Pack::LLAMAS {
            assert!(!describe(p).is_empty() && !pack_name(p).is_empty());
            if cfg!(any(windows, all(target_os = "linux", target_arch = "x86_64"))) {
                assert!(p.total_size() > 0, "{p:?}");
                assert!(p.assets().iter().all(|a| a.unpack && a.url.contains(a.name)));
            }
        }
        assert_eq!(Pack::Model.total_size(), 309_855_456 + 554_821_056);
    }
}
