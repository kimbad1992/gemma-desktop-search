//! `gemma-search install`: 이 기기(Linux 서버)를 명령 하나로 다른 PC가 쓰는 서버로 만든다.
//!
//! 1. 준비물(llama.cpp·모델)을 받는다. `download`와 같아서 받을 것과 크기를 먼저 보여 주고 묻는다(`--yes`면 묻지 않는다).
//!    이미 받아 둔 것은 건너뛴다.
//! 2. 다른 기기가 닿게 연다: 설정에 접속 주소(기본 `0.0.0.0:21870`), 접속 토큰, 기기 ID를 적는다.
//! 3. systemd 사용자 서비스(`~/.config/systemd/user/gemma-search.service`)를 쓰고 켠다. 관리자 권한은 쓰지 않는다.
//!    로그아웃한 뒤에도 돌려면 linger가 켜져 있어야 한다. 켜지 못하면 할 일을 알려만 준다.
//! 4. 서비스가 답하는지 확인하고, 이 기기의 공유 폴더와 참여 코드, 다른 PC에서 할 일을 보여 준다.
//!
//! `--join <참여 코드>`를 주면 그 기기를 메시 기기로 넣는다(2단계). 메시 기기끼리는 서로 상태를 확인하고, 그 기기가
//! 인덱싱하면 이 기기가 쉬는 동안 계산을 나눈다(따로 켜지 않아도 된다, `service.rs`의 메시 부분).
//!
//! 여러 번 실행해도 된다(서비스 파일을 고쳐 쓰고 다시 띄운다). `install --remove`는 서비스만 내리고 지운다.

use std::path::Path;

use crate::cli::{parse_args, usage_error, wants_help};

pub const USAGE: &str = "\
서버로 쓰기 (Linux):
  gemma-search install            이 기기를 다른 PC가 쓰는 서버로 만든다. 준비물을 받고, 다른 기기가 닿게 열고,
                                  systemd 사용자 서비스로 늘 띄운 뒤 참여 코드를 보여 준다 (관리자 권한 필요 없음).
                                  여러 번 실행해도 된다
      --join <참여 코드>          그 기기의 메시에 참여한다 (그 기기에서 gemma-search join-code).
                                  메시 기기끼리는 서로 상태를 확인하고, 인덱싱하는 기기를 쉬는 동안 돕는다
      --bind <주소:포트>          접속을 받을 주소 (기본 0.0.0.0:21870)
      --llama <판>                llama.cpp 판을 직접 고른다: cpu, vulkan, cuda13, cuda12
      -y, --yes                   받을 것을 묻지 않고 받는다
      --remove                    서비스를 내리고 지운다 (설정·인덱스·준비물은 남긴다)";

/// 서비스 파일 맨 위에 넣는 표식. 이 줄이 있으면 우리가 만든 파일이라 고쳐 쓰거나 지워도 된다.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MARK: &str = "# gemma-search install이 만든 파일입니다. 다시 실행하면 고쳐 씁니다 (내리기: gemma-search install --remove)";

pub fn install(args: &[String]) -> i32 {
    if wants_help(args) {
        println!("{USAGE}");
        return 0;
    }
    let a = match parse_args(args, &["--bind", "--llama", "--join"], &["--yes", "--remove"], false) {
        Ok(a) => a,
        Err(e) => return usage_error(&e),
    };
    #[cfg(target_os = "linux")]
    {
        if a.flag("--remove") {
            return linux::remove();
        }
        linux::install(a.value("--bind"), a.value("--llama"), a.value("--join"), a.flag("--yes"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = a;
        eprintln!(
            "install은 Linux 서버용 명령입니다. Windows에서는 앱을 켜 두면 되고, 다른 기기는 설정 > 일반 > 내 다른 기기에서 넣습니다."
        );
        2
    }
}

/// 접속을 받을 주소: 직접 준 것 → 설정에 있던 다른 기기가 닿는 주소 → 기본 `0.0.0.0:21870`.
/// 이 기기에서만 닿는 주소(127.0.0.1)는 받지 않는다 (다른 PC가 쓰게 하는 명령이라).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn choose_bind(given: Option<&str>, configured: &str) -> Result<String, String> {
    let bind = match given.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None => match configured.trim() {
            c if !c.is_empty() && !crate::service::bind_is_loopback(c) => c.to_string(),
            _ => format!("0.0.0.0:{}", crate::config::DEFAULT_SERVICE_PORT),
        },
    };
    if bind.parse::<std::net::SocketAddr>().is_err() {
        return Err(format!("--bind에는 주소:포트를 주세요 (예: 0.0.0.0:21870). 지금: {bind}"));
    }
    if crate::service::bind_is_loopback(&bind) {
        return Err(format!(
            "{bind}는 이 기기에서만 닿는 주소입니다. 다른 PC가 쓰게 하려면 0.0.0.0:<포트>나 이 기기의 주소를 주세요."
        ));
    }
    Ok(bind)
}

/// systemd 서비스 파일. `data`는 `GEMMA_SEARCH_DATA`로 데이터 위치를 바꿔 설치했을 때 그 위치.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn unit_text(exe: &Path, data: Option<&str>) -> String {
    let mut s = format!(
        "{MARK}\n[Unit]\nDescription=Gemma Search\n\n[Service]\nExecStart={} serve\nRestart=on-failure\nRestartSec=5\n",
        systemd_quote(&exe.display().to_string())
    );
    if let Some(d) = data {
        s += &format!("Environment={}\n", systemd_quote(&format!("GEMMA_SEARCH_DATA={d}")));
    }
    s += "\n[Install]\nWantedBy=default.target\n";
    s
}

/// systemd 서비스 파일에 넣을 값: `%`는 `%%`로, 빈칸·따옴표·역슬래시가 있으면 큰따옴표로 감싼다
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn systemd_quote(v: &str) -> String {
    let v = v.replace('%', "%%");
    if v.chars().any(|c| c.is_whitespace() || c == '"' || c == '\\' || c == '\'') {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        v
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::config::{self, Config};
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{Duration, Instant};

    const UNIT: &str = "gemma-search";
    /// 서비스가 답하기를 기다리는 시간 (처음 켤 때 인덱스를 여는 시간을 넉넉히)
    const WAIT: Duration = Duration::from_secs(40);

    pub fn install(bind: Option<&str>, llama: Option<&str>, join: Option<&str>, yes: bool) -> i32 {
        // 참여 코드는 받기 전에 먼저 읽어 본다 (잘못 붙여 넣었으면 받기 전에 알린다)
        let join_code = match join.map(crate::mesh::JoinCode::decode) {
            None => None,
            Some(Ok(c)) => Some(c),
            Some(Err(e)) => {
                eprintln!("참여 코드를 읽지 못했습니다: {e}");
                return 2;
            }
        };
        let exe = match std::env::current_exe().and_then(|p| p.canonicalize()) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("실행 파일 위치를 알 수 없습니다: {e}");
                return 1;
            }
        };
        if exe.starts_with("/tmp") || exe.starts_with("/var/tmp") || exe.starts_with("/dev/shm") {
            eprintln!(
                "실행 파일이 임시 폴더에 있습니다 ({}). 서비스가 이 파일을 계속 쓰므로 ~/gemma-search 같은 곳에 두고 다시 실행하세요.",
                exe.display()
            );
            return 1;
        }
        let data = config::data_dir();
        println!("Gemma Search v{} 서버 설치", env!("CARGO_PKG_VERSION"));
        println!("  실행 파일  {}", exe.display());
        println!("  데이터     {}", data.display());

        // ── 1. 준비물 ──
        println!("\n[1/4] 준비물 (llama.cpp와 모델)");
        let mut dargs: Vec<String> = Vec::new();
        if let Some(l) = llama {
            dargs.extend(["--llama".to_string(), l.to_string()]);
        }
        if yes {
            dargs.push("--yes".to_string());
        }
        let code = crate::prepare::download_for_install(&dargs);
        if code != 0 {
            eprintln!("\n준비물이 없어 설치를 멈췄습니다. 다시 실행하면 받은 데까지 이어서 받습니다.");
            return code;
        }

        // ── 2. 다른 기기가 닿게 열기 ──
        println!("\n[2/4] 다른 기기가 닿게 열기");
        let mut cfg = Config::load(&data);
        let bind = match choose_bind(bind, &cfg.service_bind) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{e}");
                return 2;
            }
        };
        let port = bind.rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).unwrap_or(config::DEFAULT_SERVICE_PORT);
        // 그 포트에 무엇이 있는지: 우리 서비스(다시 띄우면 된다), 다른 프로그램(다른 포트를 써야 한다)
        let ours_running = hello(&bind).is_some();
        let unit_active = systemctl(&["is-active", "--quiet", UNIT]).is_ok();
        if ours_running && !unit_active {
            eprintln!(
                "이 기기에서 Gemma Search 서비스가 이미 떠 있습니다 (포트 {port}, systemd 밖에서 띄운 것).\n\
                 그 서비스를 끄고(Ctrl+C 또는 kill) 다시 실행하세요."
            );
            return 1;
        }
        if !ours_running {
            if let Err(e) = std::net::TcpListener::bind(&bind) {
                eprintln!(
                    "{bind}에서 접속을 받을 수 없습니다: {e}\n\
                     다른 프로그램이 포트 {port}를 쓰고 있으면 다른 포트로 다시 실행하세요: {} install --bind 0.0.0.0:<포트>",
                    crate::prepare::me()
                );
                return 1;
            }
        }
        let had_token = !cfg.service_token.trim().is_empty();
        cfg.service_bind = bind.clone();
        if !had_token {
            cfg.service_token = crate::service::new_token();
        }
        crate::mesh::ensure_device_id(&mut cfg);
        cfg.setup_done = true;
        if let Err(e) = cfg.save(&data) {
            eprintln!("설정을 저장하지 못했습니다 ({}): {e}", data.join("config.toml").display());
            return 1;
        }
        println!(
            "  {bind}로 엽니다. {}",
            if had_token { "있던 접속 토큰을 그대로 씁니다." } else { "접속 토큰을 만들었습니다." }
        );
        // 메시 참여: 그 기기에 닿는지, 참여 코드가 맞는지 확인하고 넣는다
        if let Some(jc) = &join_code {
            if jc.id == cfg.device_id {
                println!("  참여 코드가 이 기기 것입니다. 메시에는 다른 기기의 코드를 넣습니다.");
            } else {
                match crate::mesh::client::join(jc) {
                    Ok(d) => {
                        println!("  메시에 참여했습니다: {} ({})", d.name, d.addrs.join(", "));
                        cfg.mesh_devices.retain(|x| x.id != d.id);
                        cfg.mesh_devices.push(d);
                        if let Err(e) = cfg.save(&data) {
                            eprintln!("설정을 저장하지 못했습니다: {e}");
                            return 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("  메시에 참여하지 못했습니다: {e}");
                        eprintln!("  그 기기의 서비스가 떠 있는지 확인하고 다시 실행하세요.");
                        return 1;
                    }
                }
            }
        }

        // ── 3. 늘 띄우기 ──
        println!("\n[3/4] 늘 띄우기 (systemd 사용자 서비스)");
        let mut notes: Vec<String> = Vec::new();
        let service = if let Err(why) = systemctl(&["show-environment"]) {
            notes.push(format!("systemd 사용자 서비스를 쓸 수 없어 늘 띄우지 못했습니다 ({why})."));
            notes.push(format!(
                "직접 띄우세요: nohup {} serve > ~/gemma-search.log 2>&1 &   (기기를 다시 켜면 다시 띄워야 합니다)",
                crate::prepare::me()
            ));
            false
        } else {
            match register(&exe, &mut notes) {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("  서비스를 띄우지 못했습니다: {e}");
                    eprintln!("  로그: journalctl --user -u {UNIT} -n 50");
                    return 1;
                }
            }
        };

        // ── 4. 확인 ──
        println!("\n[4/4] 확인");
        if service {
            let t0 = Instant::now();
            let up = loop {
                if let Some(h) = hello(&bind) {
                    break Some(h);
                }
                // 켜지다 실패했으면 더 기다리지 않는다
                if t0.elapsed() > Duration::from_secs(3) && systemctl(&["is-failed", "--quiet", UNIT]).is_ok() {
                    break None;
                }
                if t0.elapsed() > WAIT {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(500));
            };
            match up {
                Some(h) => println!("  서비스가 답합니다: {} v{} (포트 {port})", h.name, h.version),
                None => {
                    eprintln!("  서비스가 답하지 않습니다. 로그를 보세요: journalctl --user -u {UNIT} -n 50");
                    return 1;
                }
            }
        }
        let cfg = Config::load(&data);
        summary(&cfg, &bind, service, &notes);
        0
    }

    /// 서비스 파일을 쓰고 켠다. 로그아웃한 뒤에도 돌게 linger를 켜 본다(못 켜면 `notes`에 할 일).
    fn register(exe: &Path, notes: &mut Vec<String>) -> Result<(), String> {
        let path = unit_path();
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| format!("{}를 만들지 못했습니다: {e}", dir.display()))?;
        let data_env = std::env::var("GEMMA_SEARCH_DATA")
            .ok()
            .filter(|d| !d.trim().is_empty())
            .and_then(|d| std::path::absolute(d.trim()).ok())
            .map(|d| d.display().to_string());
        let text = unit_text(exe, data_env.as_deref());
        match std::fs::read_to_string(&path) {
            Ok(old) if old == text => println!("  서비스 파일은 그대로입니다: {}", path.display()),
            Ok(old) => {
                if !old.contains(MARK) {
                    // 사용자가 직접 만든 파일: 지우지 않고 옆에 옮겨 둔다
                    let bak = path.with_extension("service.bak");
                    std::fs::rename(&path, &bak).map_err(|e| format!("원래 서비스 파일을 옮기지 못했습니다: {e}"))?;
                    notes.push(format!("원래 있던 서비스 파일은 {}로 옮겨 두었습니다.", bak.display()));
                }
                std::fs::write(&path, &text).map_err(|e| format!("{}에 쓰지 못했습니다: {e}", path.display()))?;
                println!("  서비스 파일을 고쳐 썼습니다: {}", path.display());
            }
            Err(_) => {
                std::fs::write(&path, &text).map_err(|e| format!("{}에 쓰지 못했습니다: {e}", path.display()))?;
                println!("  서비스 파일을 만들었습니다: {}", path.display());
            }
        }
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", UNIT])?;
        // 이미 떠 있어도 새 설정·실행 파일로 다시 띄운다
        systemctl(&["restart", UNIT])?;
        println!("  켰습니다. 이 기기를 다시 켜도 자동으로 뜹니다.");
        match linger() {
            Some(true) => {}
            _ => {
                // 관리자 권한 없이 될 때만 (암호를 묻지 않는다)
                let _ = Command::new("loginctl").args(["--no-ask-password", "enable-linger"]).output();
                if linger() != Some(true) {
                    notes.push(format!(
                        "지금은 이 계정이 로그인해 있는 동안만 서비스가 떠 있습니다. 로그아웃하거나 기기를 다시 켜도 떠 있게 하려면 관리자 권한으로 한 번 실행하세요: sudo loginctl enable-linger {}",
                        user()
                    ));
                }
            }
        }
        Ok(())
    }

    /// `install --remove`: 서비스를 내리고 우리가 만든 서비스 파일을 지운다 (설정·인덱스·준비물은 남긴다)
    pub fn remove() -> i32 {
        let path = unit_path();
        if systemctl(&["show-environment"]).is_ok() {
            let _ = systemctl(&["disable", "--now", UNIT]);
        }
        match std::fs::read_to_string(&path) {
            Ok(old) if old.contains(MARK) => {
                if let Err(e) = std::fs::remove_file(&path) {
                    eprintln!("서비스 파일을 지우지 못했습니다 ({}): {e}", path.display());
                    return 1;
                }
                println!("서비스를 내리고 지웠습니다: {}", path.display());
            }
            Ok(_) => println!("서비스를 내렸습니다. 직접 만든 서비스 파일이라 지우지 않았습니다: {}", path.display()),
            Err(_) => println!("등록된 서비스 파일이 없습니다 ({}).", path.display()),
        }
        let _ = systemctl(&["daemon-reload"]);
        let me = crate::prepare::me();
        println!("설정·인덱스·준비물은 남겼습니다 (위치는 `{me} data`로 봅니다). 다시 띄우려면 {me} install");
        0
    }

    /// 다 끝난 뒤 보여 줄 것: 닿는 주소, 공유 폴더, 참여 코드, 다른 PC에서 할 일
    fn summary(cfg: &Config, bind: &str, service: bool, notes: &[String]) {
        let me = crate::prepare::me();
        let (port, addrs) = crate::service::advertised(bind);
        let shares = crate::mesh::share::list();
        let name = crate::lend::machine_name();
        println!();
        if service {
            println!("설치를 마쳤습니다. 이 기기가 켜져 있는 동안 서비스가 늘 떠 있습니다.");
        } else {
            println!("설정을 마쳤습니다. 서비스는 직접 띄워야 합니다 (아래 참고).");
        }
        println!("  접속 주소  {} (포트 {port})", addrs.join(", "));
        let others: Vec<&str> = cfg.mesh_devices.iter().filter(|d| d.id != cfg.device_id).map(|d| d.name.as_str()).collect();
        if !others.is_empty() {
            println!(
                "  메시       {} · 계산 나누기 {}",
                others.join(", "),
                if cfg.lend_mesh_idle { "켜짐 (그 기기가 인덱싱하면 이 기기가 쉬는 동안 돕습니다)" } else { "꺼짐" }
            );
        }
        if shares.is_empty() {
            println!("  공유 폴더  없음");
            println!("             다른 PC는 이 기기가 네트워크에 공유한 폴더(Samba) 아래만 추가할 수 있습니다.");
            println!("             공유를 먼저 만드세요 (관리자 권한이 필요합니다).");
        } else {
            for (i, (n, p)) in shares.iter().enumerate() {
                let head = if i == 0 { "  공유 폴더  " } else { "             " };
                println!("{head}{n} → {}", p.display());
            }
        }
        let example = match shares.first() {
            Some((n, _)) => format!("\\\\{name}\\{n}\\…"),
            None => format!("\\\\{name}\\<공유 이름>\\…"),
        };
        println!();
        println!("다른 PC에서 쓰기");
        println!("  1. 그 PC의 Gemma Search(v0.10.1 이상)에서 이 기기의 공유 폴더를 검색할 폴더로 추가합니다.");
        println!("     예: {example}");
        println!("  2. 참여 코드를 물으면 아래 한 줄을 붙여 넣습니다. 접속 토큰이 들어 있으니 남에게 보이지 마세요.");
        println!();
        println!("{}", crate::service::join_code(cfg));
        println!();
        println!("  그러면 이 기기가 그 폴더를 인덱싱하고(그 PC가 그래픽으로 돕습니다), 그 PC에서 바로 검색됩니다.");
        if !notes.is_empty() {
            println!();
            for n in notes {
                println!("참고: {n}");
            }
        }
        println!();
        println!("  참여 코드 다시 보기  {me} join-code");
        println!("  상태                 {me} status");
        if service {
            println!("  로그                 journalctl --user -u {UNIT} -f");
            println!("  내리기               {me} install --remove");
        }
    }

    /// 그 주소의 서비스가 Gemma Search로 답하면 그 답 (모든 주소로 열었으면 127.0.0.1로 묻는다)
    fn hello(bind: &str) -> Option<crate::mesh::client::Hello> {
        let (host, port) = bind.rsplit_once(':')?;
        let port: u16 = port.parse().ok()?;
        let host = match host.trim_matches(['[', ']']) {
            "0.0.0.0" | "::" | "" => "127.0.0.1",
            h => h,
        };
        let base = crate::mesh::base_of(host, port);
        let owner = crate::mesh::client::Owner::with_bases(vec![base.clone()], "", Duration::from_secs(2));
        let (_, v) = owner.json("GET", "/api/hello", None).ok()?;
        crate::mesh::client::hello_from(v, &base)
    }

    /// `systemctl --user …`. 실패하면 그 까닭(오류 출력).
    fn systemctl(args: &[&str]) -> Result<(), String> {
        let out = Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .map_err(|e| format!("systemctl을 실행하지 못했습니다: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if err.is_empty() { format!("systemctl --user {}: 실패", args.join(" ")) } else { err })
        }
    }

    /// 이 계정의 linger가 켜졌는지 (`loginctl`이 없거나 모르면 None)
    fn linger() -> Option<bool> {
        let out = Command::new("loginctl")
            .args(["show-user", &user(), "--property=Linger", "--value"])
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim() == "yes")
    }

    fn user() -> String {
        std::env::var("USER")
            .ok()
            .filter(|u| !u.is_empty())
            .or_else(|| {
                let out = Command::new("id").arg("-un").output().ok()?;
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            })
            .unwrap_or_default()
    }

    /// `~/.config/systemd/user/gemma-search.service` (`XDG_CONFIG_HOME`를 따른다)
    fn unit_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from(".config"));
        base.join("systemd").join("user").join(format!("{UNIT}.service"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_defaults_and_keeps_open_address() {
        assert_eq!(choose_bind(None, "").unwrap(), "0.0.0.0:21870");
        // 예전 기본값(이 기기에서만)은 다른 기기가 닿는 기본 주소로
        assert_eq!(choose_bind(None, "127.0.0.1:21870").unwrap(), "0.0.0.0:21870");
        // 사용자가 열어 둔 주소는 그대로
        assert_eq!(choose_bind(None, "0.0.0.0:8787").unwrap(), "0.0.0.0:8787");
        assert_eq!(choose_bind(Some("100.90.136.114:21870"), "0.0.0.0:8787").unwrap(), "100.90.136.114:21870");
        assert!(choose_bind(Some("127.0.0.1:21870"), "").is_err());
        assert!(choose_bind(Some("leeubuntu"), "").is_err());
    }

    #[test]
    fn unit_file_quotes_paths() {
        let t = unit_text(Path::new("/home/kim/gemma-search/gemma-search"), None);
        assert!(t.starts_with(MARK));
        assert!(t.contains("\nExecStart=/home/kim/gemma-search/gemma-search serve\n"));
        assert!(t.contains("\nWantedBy=default.target\n"));
        assert!(!t.contains("Environment="));
        let t = unit_text(Path::new("/home/kim/내 프로그램/gemma-search"), Some("/data/100%"));
        assert!(t.contains("\nExecStart=\"/home/kim/내 프로그램/gemma-search\" serve\n"));
        assert!(t.contains("\nEnvironment=GEMMA_SEARCH_DATA=/data/100%%\n"));
        assert_eq!(systemd_quote("a\"b c"), "\"a\\\"b c\"");
    }
}
