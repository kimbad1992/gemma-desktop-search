// 릴리스 빌드는 콘솔 창 없이 GUI로만 실행
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod autostart;
mod config;
mod download;
mod embed;
mod extract;
mod headless;
mod shell;
mod sysinfo;
mod update;
mod watch;
mod index;
mod open;
mod precheck;
mod search;
mod server;
mod ui;
mod util;

use eframe::egui;

fn main() -> eframe::Result {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // exe를 바꾸기 전의 위치를 기억해 둔다 (업데이트 뒤 같은 자리의 새 exe로 다시 시작)
    update::exe_path();
    // 업데이트 뒤 다시 시작한 경우: 이전 프로세스가 끝나기를 기다린다
    if let Some(i) = args.iter().position(|a| a == "--after-update") {
        if let Some(pid) = args.get(i + 1).and_then(|p| p.parse::<u32>().ok()) {
            update::wait_for_previous(pid);
        }
        args.drain(i..(i + 2).min(args.len()));
    }
    if args.first().map(|a| a == "--update").unwrap_or(false) {
        std::process::exit(update::run_cli());
    }
    if args.first().map(|a| a == "--headless-test").unwrap_or(false) {
        std::process::exit(headless::run(&args[1..]));
    }
    if args.first().map(|a| a == "--stress").unwrap_or(false) {
        std::process::exit(headless::stress(&args[1..]));
    }
    if args.first().map(|a| a == "--check-update").unwrap_or(false) {
        println!("지금 버전 v{}", env!("CARGO_PKG_VERSION"));
        match update::check() {
            Ok(Some((tag, url))) => println!("새 버전 {tag}: {url}"),
            Ok(None) => println!("새 버전 없음"),
            Err(e) => println!("확인 실패: {e}"),
        }
        return Ok(());
    }
    if args.first().map(|a| a == "--sysinfo").unwrap_or(false) {
        std::process::exit(headless::sysinfo());
    }
    if args.first().map(|a| a == "--query-once").unwrap_or(false) {
        std::process::exit(headless::query_once(&args[1..]));
    }
    if args.first().map(|a| a == "--bench-compare").unwrap_or(false) {
        std::process::exit(headless::bench_compare(&args[1..]));
    }
    if args.first().map(|a| a == "--index-once").unwrap_or(false) {
        std::process::exit(headless::index_once());
    }
    if args.first().map(|a| a == "--compat").unwrap_or(false) {
        std::process::exit(headless::compat(&args[1..]));
    }
    if args.first().map(|a| a == "--compare-queries").unwrap_or(false) {
        std::process::exit(headless::compare_queries(&args[1..]));
    }
    if args.first().map(|a| a == "--bench").unwrap_or(false) {
        std::process::exit(headless::bench(args.get(1).map(String::as_str).unwrap_or("README.md")));
    }
    // 화면 확인용 --shot <png>: 창을 화면에 띄우지 않고 그려서 PNG로 저장하고 끝낸다 (app::Shot)
    let shot = args.iter().any(|a| a == "--shot");
    // 이미 실행 중이면 그 창을 띄우고 끝낸다 (로그인 시 자동 실행 + 직접 실행이 겹쳐도 하나만)
    if !shot && shell::already_running(&config::data_dir()) {
        return Ok(());
    }
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
