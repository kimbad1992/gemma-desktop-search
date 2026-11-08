// 릴리스 빌드는 콘솔 창 없이 GUI로만 실행
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod embed;
mod extract;
mod headless;
mod index;
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
    if args.first().map(|a| a == "--bench").unwrap_or(false) {
        std::process::exit(headless::bench(args.get(1).map(String::as_str).unwrap_or("README.md")));
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gemma Search")
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([860.0, 560.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Gemma Search",
        options,
        Box::new(|cc| Ok(Box::new(app::GemmaApp::new(cc)))),
    )
}
