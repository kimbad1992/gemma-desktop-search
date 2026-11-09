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
mod update;
mod index;
mod open;
mod search;
mod server;
mod ui;
mod util;

use eframe::egui;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    // 이미 실행 중이면 그 창을 띄우고 끝낸다 (로그인 시 자동 실행 + 직접 실행이 겹쳐도 하나만)
    if shell::already_running(&config::data_dir()) {
        return Ok(());
    }
    // --background: 창 없이 트레이로 시작 (로그인 시 실행)
    let background = args.iter().any(|a| a == "--background");
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Gemma Search")
        .with_inner_size([1280.0, 860.0])
        .with_min_inner_size([640.0, 420.0])
        .with_drag_and_drop(true)
        .with_visible(!background);
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
