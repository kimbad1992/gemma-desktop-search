// 창이 있는 앱의 릴리스 빌드는 콘솔 창 없이 실행한다. 서비스판(gui 기능 없이 빌드)은 콘솔 프로그램이다.
#![cfg_attr(all(not(debug_assertions), feature = "gui"), windows_subsystem = "windows")]
// 서비스판에는 화면에서만 쓰는 설정·함수가 쓰이지 않은 채 남는다
#![cfg_attr(not(feature = "gui"), allow(dead_code))]

#[cfg(feature = "gui")]
mod app;
#[cfg(feature = "gui")]
mod autostart;
mod cli;
mod config;
mod download;
mod embed;
mod extract;
mod headless;
mod index;
mod lend;
mod location;
#[cfg(feature = "gui")]
mod open;
mod paths;
mod platform;
mod precheck;
mod prepare;
mod search;
mod server;
mod service;
#[cfg(feature = "gui")]
mod shell;
mod sysinfo;
#[cfg(feature = "gui")]
mod ui;
mod update;
mod util;
mod watch;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // exe를 바꾸기 전의 위치를 기억해 둔다 (업데이트 뒤 같은 자리의 새 exe로 다시 시작)
    update::exe_path();
    // 업데이트 뒤 다시 시작한 경우: 이전 프로세스가 끝나기를 기다린다
    if let Some(i) = args.iter().position(|a| a == "--after-update") {
        if let Some(pid) = args.get(i + 1).and_then(|p| p.parse::<u32>().ok()) {
            platform::wait_for_process_exit(pid, std::time::Duration::from_secs(20));
        }
        args.drain(i..(i + 2).min(args.len()));
    }
    let move_job = take_move_job(&mut args);
    if let Some(code) = command(&args) {
        std::process::exit(code);
    }
    #[cfg(feature = "gui")]
    if let Err(e) = gui(&args, move_job) {
        eprintln!("창을 띄우지 못했습니다: {e}");
        std::process::exit(1);
    }
    #[cfg(not(feature = "gui"))]
    {
        let _ = move_job;
        eprintln!(
            "Gemma Search v{} (창 없는 서비스판)\n\n{}\n\n{}\n\n{}",
            env!("CARGO_PKG_VERSION"),
            service::USAGE,
            location::USAGE,
            USAGE
        );
        std::process::exit(2);
    }
}

/// 데이터를 옮기려고 다시 시작한 경우 (`--move-data <import|to> <경로> <이전 pid>`, app.rs의 `restart_to_move`):
/// 이전 프로세스가 끝나기를 기다리고 할 일을 돌려준다. 옮기기는 창에서 한다 (`ui::launch`).
fn take_move_job(args: &mut Vec<String>) -> Option<location::Job> {
    let i = args.iter().position(|a| a == "--move-data")?;
    let taken: Vec<String> = args.drain(i..(i + 4).min(args.len())).collect();
    if let Some(pid) = taken.get(3).and_then(|p| p.parse::<u32>().ok()) {
        platform::wait_for_process_exit(pid, std::time::Duration::from_secs(20));
    }
    location::Job::from_args(taken.get(1)?, taken.get(2)?)
}

/// 창 없이 도는 명령이면 실행하고 종료 코드를 돌려준다. 아니면 None (창을 띄운다).
fn command(args: &[String]) -> Option<i32> {
    let rest = args.get(1..).unwrap_or_default();
    Some(match args.first()?.as_str() {
        "serve" | "--serve" => service::serve(rest),
        // 준비물(llama.cpp, 모델) 받기 (prepare.rs)
        "download" => prepare::download(rest),
        // 떠 있는 서비스에 묻는 명령 (cli.rs)
        "search" | "--search" => cli::search(rest),
        "status" => cli::status(rest),
        "index" => cli::index(rest),
        "rescan" => cli::rescan(rest),
        // 계산 빌려주기: 다른 기기의 서비스가 인덱싱할 때 이 기기로 임베딩을 나눠 맡는다 (lend.rs).
        // 창 있는 빌드에서는 콘솔이 없으므로 앱(트레이)에서 빌려주고 진행을 오버레이에 보인다 (ui/lend.rs).
        // `--console`을 주면 창 있는 빌드에서도 콘솔 방식으로 돈다 (출력을 파일로 받을 때)
        #[cfg(feature = "gui")]
        "lend" if !rest.iter().any(|a| a == "--console") => return gui_lend(rest),
        "lend" => lend::lend(rest),
        // 데이터 위치 확인·옮기기 (location.rs)
        "data" => location::cli(rest),
        "help" | "--help" | "-h" => {
            println!("Gemma Search v{}\n\n{}\n\n{}", env!("CARGO_PKG_VERSION"), service::USAGE, location::USAGE);
            #[cfg(not(feature = "gui"))]
            println!("\n{USAGE}");
            0
        }
        // 업데이트 (update.rs). 예전 이름 --update, --check-update도 받는다
        "update" | "--update" => update::cli(rest),
        "--check-update" => update::cli(&["--check".to_string()]),
        "--sysinfo" => headless::sysinfo(),
        "--index-once" => headless::index_once(),
        "--query-once" => headless::query_once(rest),
        "--compat" => headless::compat(rest),
        "--headless-test" => headless::run(rest),
        "--stress" => headless::stress(rest),
        "--bench" => headless::bench(rest.first().map(String::as_str).unwrap_or("README.md")),
        "--bench-compare" => headless::bench_compare(rest),
        "--compare-queries" => headless::compare_queries(rest),
        _ => return None,
    })
}

/// 서비스판(창 없는 빌드)을 명령 없이 실행했을 때의 안내
#[cfg(not(feature = "gui"))]
const USAGE: &str = "\
그 밖의 명령:
  gemma-search --sysinfo          사양 확인과 추천 설정
  gemma-search --index-once       바뀐 파일을 한 번 인덱싱하고 끝낸다
                                  (서비스가 떠 있을 때는 `gemma-search index`)";

/// 창 있는 빌드의 `lend`: 주소·토큰을 설정에 저장하고 앱을 빌려주기로 시작한다 (None이면 이어서 앱을 띄운다).
/// 앱이 이미 떠 있으면 그 앱에 알린다 (`shell::already_running`).
#[cfg(feature = "gui")]
fn gui_lend(args: &[String]) -> Option<i32> {
    let data_dir = config::data_dir();
    let mut cfg = config::Config::load(&data_dir);
    if let Some((base, token, _name)) = lend::params_from_args(args, &cfg) {
        cfg.lend_server = base;
        if let Some(t) = token {
            cfg.lend_token = t;
        }
        if let Err(e) = cfg.save(&data_dir) {
            rfd::MessageDialog::new()
                .set_title("Gemma Search")
                .set_description(format!("설정을 저장하지 못했습니다: {e}"))
                .set_level(rfd::MessageLevel::Error)
                .show();
            return Some(1);
        }
    } else if cfg.lend_server.trim().is_empty() {
        rfd::MessageDialog::new()
            .set_title("Gemma Search")
            .set_description(format!("소유 기기 서비스의 주소가 필요합니다.\n\n{}", lend::USAGE))
            .set_level(rfd::MessageLevel::Warning)
            .show();
        return Some(2);
    }
    app::LEND_ON_START.store(true, std::sync::atomic::Ordering::SeqCst);
    None
}

/// 창이 있는 앱
#[cfg(feature = "gui")]
fn gui(args: &[String], move_job: Option<location::Job>) -> eframe::Result {
    use eframe::egui;
    use ui::launch::{First, Launcher};
    // 화면 확인용 --shot <png>: 창을 화면에 띄우지 않고 그려서 PNG로 저장하고 끝낸다 (app::Shot)
    let shot = args.iter().any(|a| a == "--shot");
    let loc = location::current();
    // 앱을 열기 전에 할 일: 이전 프로세스가 맡긴 옮기기, 또는 실행 파일 옆의 예전 데이터를 옮길지 묻기
    let first = match move_job {
        Some(job) => Some(First::Move(job)),
        None if loc.kind == location::Kind::Legacy => Some(First::Ask),
        None => None,
    };
    // 옮기는 경우에는 옮긴 뒤의 데이터 폴더로 아래 두 가지를 본다 (ui::launch)
    let moving = matches!(first, Some(First::Move(_)));
    // 이미 실행 중이면 그 창을 띄우고 끝낸다 (로그인 시 자동 실행 + 직접 실행이 겹쳐도 하나만)
    let lend_on_start = app::LEND_ON_START.load(std::sync::atomic::Ordering::SeqCst);
    if !shot && !moving && shell::already_running(&loc.data, lend_on_start) {
        return Ok(());
    }
    // 서비스(gemma-search serve)가 이 데이터 폴더를 쓰는 중이면 띄우지 않는다: 인덱스를 둘이 함께 고치면 안 된다
    let mut lock = if shot || moving {
        None
    } else {
        match config::instance_lock(&loc.data) {
            Ok(lock) => Some(lock),
            Err(why) => {
                rfd::MessageDialog::new()
                    .set_title("Gemma Search")
                    .set_description(why)
                    .set_level(rfd::MessageLevel::Warning)
                    .show();
                return Ok(());
            }
        }
    };
    // --background: 창 없이 트레이로 시작 (로그인 시 실행)
    let background = args.iter().any(|a| a == "--background");
    // --size 1280x860: 창 크기 (화면 확인용)
    let size = args
        .iter()
        .position(|a| a == "--size")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.split_once('x'))
        .and_then(|(w, h)| Some([w.parse::<f32>().ok()?, h.parse::<f32>().ok()?]))
        .unwrap_or([1280.0, 860.0]);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Gemma Search")
        .with_inner_size(size)
        .with_min_inner_size([640.0, 420.0])
        .with_drag_and_drop(true)
        // 옮길지 묻거나 옮기는 화면은 트레이로 시작(--background)했어도 보여 준다
        .with_visible(!shot && (!background || first.is_some()));
    if shot {
        // 숨긴 채로 그리지만, eframe이 첫 그림 뒤에 창을 한 번 보이게 하므로 화면 밖에 두고 포커스도 받지 않게 한다
        viewport = viewport
            .with_position([-30000.0, -30000.0])
            .with_active(false)
            .with_taskbar(false);
    }
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    // 옮기는 화면을 거치면 잠금을 그 화면이 들고 있다가 옮길 때 놓고, 옮긴 뒤 새 폴더를 잠근다
    let launcher_lock = if first.is_some() { lock.take() } else { None };
    let _lock = lock;
    eframe::run_native(
        "Gemma Search",
        options,
        Box::new(move |cc| {
            Ok(match first {
                Some(first) => Box::new(Launcher::new(cc, first, launcher_lock)) as Box<dyn eframe::App>,
                None => Box::new(app::GemmaApp::new(cc)),
            })
        }),
    )
}
