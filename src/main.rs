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
    if let Some(code) = command(&args) {
        std::process::exit(code);
    }
    #[cfg(feature = "gui")]
    if let Err(e) = gui(&args) {
        eprintln!("창을 띄우지 못했습니다: {e}");
        std::process::exit(1);
    }
    #[cfg(not(feature = "gui"))]
    {
        eprintln!("Gemma Search v{} (창 없는 서비스판)\n\n{}\n\n{}", env!("CARGO_PKG_VERSION"), service::USAGE, USAGE);
        std::process::exit(2);
    }
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
        "help" | "--help" | "-h" => {
            println!("Gemma Search v{}\n\n{}", env!("CARGO_PKG_VERSION"), service::USAGE);
            #[cfg(not(feature = "gui"))]
            println!("\n{USAGE}");
            0
        }
        "--update" => update::run_cli(),
        "--check-update" => {
            println!("지금 버전 v{}", env!("CARGO_PKG_VERSION"));
            match update::check() {
                Ok(Some((tag, url))) => println!("새 버전 {tag}: {url}"),
                Ok(None) => println!("새 버전 없음"),
                Err(e) => println!("확인 실패: {e}"),
            }
            0
        }
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
                                  (서비스가 떠 있을 때는 `gemma-search index`)
  gemma-search --check-update     새 버전 확인";

/// 창이 있는 앱
#[cfg(feature = "gui")]
fn gui(args: &[String]) -> eframe::Result {
    use eframe::egui;
    // 화면 확인용 --shot <png>: 창을 화면에 띄우지 않고 그려서 PNG로 저장하고 끝낸다 (app::Shot)
    let shot = args.iter().any(|a| a == "--shot");
    // 이미 실행 중이면 그 창을 띄우고 끝낸다 (로그인 시 자동 실행 + 직접 실행이 겹쳐도 하나만)
    if !shot && shell::already_running(&config::data_dir()) {
        return Ok(());
    }
    // 서비스(gemma-search serve)가 이 데이터 폴더를 쓰는 중이면 띄우지 않는다: 인덱스를 둘이 함께 고치면 안 된다
    let _lock = if shot {
        None
    } else {
        match config::instance_lock(&config::data_dir()) {
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
        .with_visible(!background && !shot);
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
    eframe::run_native(
        "Gemma Search",
        options,
        Box::new(|cc| Ok(Box::new(app::GemmaApp::new(cc)))),
    )
}
