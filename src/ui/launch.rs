//! 앱을 열기 전에 데이터를 옮기는 화면 (창이 있는 앱).
//!
//! - 처음 실행: 실행 파일 옆에 예전 데이터가 있고 고정 위치가 비어 있으면(`location::Kind::Legacy`) 옮길지 묻는다.
//! - 설정에서 옮기기·가져오기를 고르면 앱은 `--move-data`로 자신을 다시 띄우고 끝난다. 새 프로세스는 앞 프로세스가
//!   끝나기를 기다렸다가(main.rs) 이 화면에서 바로 옮긴다.
//!
//! 옮기기는 인덱스를 열기 전에 해야 한다 (열린 파일이 든 폴더는 이름을 바꿀 수 없다). 다 옮기면 같은 창에서 앱을 연다.

use crate::app::{GemmaApp, Shot};
use crate::location::{self, Job, Moved, Plan, Progress};
use crate::ui::theme::{self, space, Tone};
use crate::ui::widgets::{self, Btn};
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use parking_lot::Mutex;
use std::sync::Arc;

/// 처음에 할 일
pub enum First {
    /// 실행 파일 옆의 예전 데이터를 고정 위치로 옮길지 묻는다
    Ask,
    /// 앞 프로세스가 맡긴 일을 바로 한다
    Move(Job),
}

enum Stage {
    Ask(Result<Plan, String>),
    Moving {
        plan: Plan,
        progress: Arc<Mutex<Progress>>,
        rx: Receiver<Result<Moved, String>>,
    },
    Failed(String),
    Main(Box<GemmaApp>),
}

pub struct Launcher {
    hwnd: usize,
    /// 데이터 폴더 잠금 (main.rs가 잡아 넘긴 것, 옮긴 뒤에는 새 폴더의 것)
    lock: Option<std::fs::File>,
    stage: Stage,
    /// 화면 확인용 실행 (`--shot`): 잠그지 않는다
    shot_mode: bool,
    shot: Option<Shot>,
    /// 화면 확인용 (`--shot-move`): "옮기기"를 누른 것처럼 하고, 앱이 열린 뒤의 화면을 찍는다
    auto_move: bool,
    /// 앱이 열리면 알릴 것: (글, 경고인지)
    notices: Vec<(String, bool)>,
}

impl Launcher {
    pub fn new(cc: &eframe::CreationContext<'_>, first: First, lock: Option<std::fs::File>) -> Self {
        crate::app::install_look(&cc.egui_ctx);
        let args: Vec<String> = std::env::args().skip(1).collect();
        let flag = |f: &str| args.iter().any(|a| a == f);
        let shot_mode = flag("--shot");
        let auto_move = shot_mode && flag("--shot-move");
        let hwnd = crate::platform::native_window(cc);
        if shot_mode {
            crate::platform::make_window_passive(hwnd);
            match args.iter().position(|a| a == "--theme").and_then(|i| args.get(i + 1)).map(String::as_str) {
                Some("light") => cc.egui_ctx.set_theme(egui::ThemePreference::Light),
                Some("dark") => cc.egui_ctx.set_theme(egui::ThemePreference::Dark),
                _ => {}
            }
        }
        let mut me = Self {
            hwnd,
            lock,
            stage: Stage::Failed(String::new()),
            shot_mode,
            // `--shot-move`이면 앱이 열린 뒤 앱의 화면 확인(`app::Shot`)이 찍는다
            shot: if auto_move { None } else { Shot::from_args() },
            auto_move,
            notices: Vec::new(),
        };
        let loc = location::current();
        me.stage = match first {
            First::Ask => Stage::Ask(location::plan(&Job::Import(loc.app_dir.clone()), &loc)),
            First::Move(job) => match location::plan(&job, &loc) {
                Ok(plan) => me.start_move(plan, &cc.egui_ctx),
                Err(e) => Stage::Failed(e),
            },
        };
        me
    }

    /// 다른 스레드에서 옮기기 시작
    fn start_move(&mut self, plan: Plan, ctx: &egui::Context) -> Stage {
        // 이 프로세스가 잡은 잠금(데이터 폴더 안의 파일)을 놓아야 폴더를 옮길 수 있다
        self.lock = None;
        let progress = Arc::new(Mutex::new(Progress::default()));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (shared, job, ctx) = (progress.clone(), plan.clone(), ctx.clone());
        std::thread::spawn(move || {
            let loc = location::current();
            let result = location::execute(&job, &loc, &|p| {
                *shared.lock() = p;
                ctx.request_repaint();
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        Stage::Moving { plan, progress, rx }
    }

    /// 데이터 폴더를 (다시) 정하고 잠근 뒤 앱을 연다
    fn open_app(&mut self, ctx: &egui::Context) {
        let loc = location::refresh();
        if !self.shot_mode && self.lock.is_none() {
            // main.rs가 앱을 열기 전에 하는 일과 같다: 같은 데이터 폴더로는 하나만
            if crate::shell::already_running(&loc.data, crate::app::LEND_ON_START.load(std::sync::atomic::Ordering::SeqCst)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            match crate::config::instance_lock(&loc.data) {
                Ok(l) => self.lock = Some(l),
                Err(why) => {
                    self.stage = Stage::Failed(why);
                    return;
                }
            }
        }
        let mut app = GemmaApp::start(ctx, self.hwnd);
        // 알림 칸은 하나라 한 번에 모아 알린다. 참고할 것이 있으면 오래 보이게 경고로.
        if !self.notices.is_empty() {
            let warn = self.notices.iter().any(|(_, w)| *w);
            let text: Vec<String> = self.notices.drain(..).map(|(t, _)| t).collect();
            app.notify(text.join("\n"), warn);
        }
        self.stage = Stage::Main(Box::new(app));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if self.auto_move {
            self.auto_move = false;
            if let Stage::Ask(Ok(plan)) = &self.stage {
                let plan = plan.clone();
                self.stage = self.start_move(plan, ctx);
            }
        }
        let done = match &self.stage {
            Stage::Moving { rx, .. } => rx.try_recv().ok(),
            _ => None,
        };
        match done {
            Some(Ok(moved)) => {
                self.notices.push((format!("데이터를 {}로 옮겼습니다.", moved.home.display()), false));
                self.notices.extend(moved.notes.into_iter().map(|n| (n, true)));
                self.open_app(ctx);
            }
            Some(Err(e)) => self.stage = Stage::Failed(e),
            None => {}
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // 벽 위에 바로 (안내 카드는 유리)
        crate::ui::glass::paint_wall(ui);
        let body = egui::Frame::NONE.inner_margin(egui::Margin {
            left: 20,
            right: 12,
            top: 16,
            bottom: 0,
        });
        enum Act {
            None,
            Move,
            StayHere,
            Continue,
        }
        let mut act = Act::None;
        egui::CentralPanel::default().frame(body).show(ui, |ui| {
            widgets::page(ui, "launch_page", 680.0, |ui| match &self.stage {
                Stage::Ask(plan) => {
                    widgets::page_title(ui, "데이터를 고정 위치로 옮길까요?");
                    widgets::help(
                        ui,
                        "이번 버전부터 설정·인덱스와 앱이 받은 준비물을 실행 파일 옆이 아닌 한곳에 둡니다. \
                         옮겨 두면 새 버전을 다른 폴더에 풀어도 지금 인덱스로 바로 검색합니다.",
                    );
                    ui.add_space(space::M);
                    match plan {
                        Ok(plan) => {
                            plan_card(ui, plan);
                            ui.add_space(space::L);
                            ui.horizontal(|ui| {
                                if widgets::button(ui, Btn::Primary, "옮기기").clicked() {
                                    act = Act::Move;
                                }
                                if widgets::button(ui, Btn::Secondary, "이 폴더에서 계속 쓰기").clicked() {
                                    act = Act::StayHere;
                                }
                            });
                            ui.add_space(space::S);
                            widgets::caption(
                                ui,
                                "이 폴더에서 계속 쓰면 실행 파일 옆에 portable.txt를 만들어 다음부터 묻지 않습니다. \
                                 나중에 설정 → 일반 → 데이터 위치에서 옮길 수 있습니다.",
                            );
                        }
                        Err(e) => {
                            widgets::callout(ui, Tone::Danger, "옮길 수 없습니다", e);
                            ui.add_space(space::M);
                            if widgets::button(ui, Btn::Primary, "지금 위치로 계속").clicked() {
                                act = Act::Continue;
                            }
                        }
                    }
                }
                Stage::Moving { plan, progress, .. } => {
                    widgets::page_title(ui, "데이터를 옮기는 중");
                    widgets::help(ui, "끝나면 앱이 바로 열립니다. 창을 닫지 마세요.");
                    ui.add_space(space::M);
                    let p = progress.lock().clone();
                    widgets::card(ui, |ui| {
                        if plan.copy && p.total > 0 {
                            widgets::progress_accent(ui, p.done as f32 / p.total as f32, 6.0, true);
                            widgets::caption(
                                ui,
                                &format!("{} 복사 중 · {} / {}", p.item, location::size(p.done), location::size(p.total)),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                widgets::spinner(ui);
                                ui.label("옮기는 중…");
                            });
                        }
                    });
                    ui.add_space(space::M);
                    plan_card(ui, plan);
                }
                Stage::Failed(message) => {
                    widgets::page_title(ui, "데이터를 옮기지 못했습니다");
                    ui.add_space(space::M);
                    widgets::callout(ui, Tone::Danger, "옮기지 못했습니다", message);
                    ui.add_space(space::S);
                    widgets::caption(ui, "데이터 폴더를 옮기지 못했으면 원래 자리에 그대로 있습니다.");
                    ui.add_space(space::M);
                    if widgets::button(ui, Btn::Primary, "지금 위치로 계속").clicked() {
                        act = Act::Continue;
                    }
                }
                Stage::Main(_) => {}
            });
        });
        match act {
            Act::None => {}
            Act::Move => {
                if let Stage::Ask(Ok(plan)) = &self.stage {
                    let plan = plan.clone();
                    self.stage = self.start_move(plan, &ctx);
                }
            }
            Act::StayHere => {
                let app_dir = location::current().app_dir;
                if let Err(e) = location::mark_portable(&app_dir) {
                    self.notices.push((
                        format!("실행 파일 옆에 portable.txt를 만들지 못해 다음에 다시 묻습니다 ({e})."),
                        true,
                    ));
                }
                self.open_app(&ctx);
            }
            Act::Continue => self.open_app(&ctx),
        }
    }
}

/// 무엇을 어디로 옮기는지
fn plan_card(ui: &mut egui::Ui, plan: &Plan) {
    widgets::card(ui, |ui| {
        ui.label(RichText::new("옮길 곳").weak());
        ui.label(RichText::new(plan.home.display().to_string()).text_style(theme::ts_strong()));
        ui.add_space(space::S);
        for item in &plan.items {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{} · {}", item.name, location::size(item.bytes))).text_style(theme::ts_strong()));
                ui.label(RichText::new(item.from.display().to_string()).weak());
            });
        }
        ui.add_space(space::S);
        widgets::caption(
            ui,
            if plan.copy {
                "다른 드라이브라 복사합니다. 새 위치에서 인덱스가 열리고 파일 수가 같은지 확인한 뒤 원래 것을 지웁니다."
            } else {
                "같은 드라이브라 폴더 이름만 바꿔 바로 옮깁니다."
            },
        );
        if let Some(n) = plan.replaces {
            widgets::caption(
                ui,
                &format!("옮길 곳에 있던 인덱스(파일 {n}개)는 지우지 않고 data.before-import-<시각>으로 이름을 바꿔 둡니다."),
            );
        }
    });
}

impl eframe::App for Launcher {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Stage::Main(app) = &mut self.stage {
            return app.logic(ctx, frame);
        }
        self.poll(ctx);
        if let Some(s) = &mut self.shot {
            let busy = matches!(self.stage, Stage::Moving { .. } | Stage::Main(_));
            if s.tick(ctx, busy) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if let Stage::Main(app) = &mut self.stage {
            return app.ui(ui, frame);
        }
        self.draw(ui);
    }
}
