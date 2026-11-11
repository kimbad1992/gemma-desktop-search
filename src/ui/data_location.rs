//! 설정 > 일반 > 데이터 위치: 설정·인덱스와 받은 준비물이 어디 있는지 보여 주고, 옮기거나 예전 데이터를 가져온다.
//! 확인 창에서 고르면 앱이 다시 시작하면서 옮긴다 (`GemmaApp::restart_to_move`, `ui::launch`).

use crate::app::GemmaApp;
use crate::location::{self, Job, Kind};
use crate::ui::theme::{self, space, Tone};
use crate::ui::widgets::{self, Btn};
use eframe::egui::{self, RichText};

impl GemmaApp {
    /// 설정 > 일반의 "데이터 위치" 칸
    pub fn basic_location(&mut self, ui: &mut egui::Ui) {
        let loc = location::current();
        let busy = self.indexer.is_active() || self.dl.lock().running;
        let busy_why = "인덱싱이나 준비물 받기가 끝난 뒤에 옮길 수 있습니다";
        let mut job = None;
        let mut import = false;
        widgets::section(
            ui,
            "데이터 위치",
            Some("설정·인덱스와 앱이 받은 준비물(llama.cpp, 모델)을 두는 곳입니다. 새 버전을 다른 폴더에 풀어도 이곳을 그대로 씁니다."),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    let tone = if loc.kind == Kind::Home { Tone::Ok } else { Tone::Info };
                    widgets::tone_tag(ui, tone, loc.kind_label());
                    ui.label(RichText::new(loc.root.display().to_string()).text_style(theme::ts_strong()));
                    if widgets::small_button(ui, Btn::Ghost, "폴더 보기").clicked() {
                        crate::platform::reveal_in_file_manager(&loc.data.display().to_string());
                    }
                });
                match (loc.kind, &loc.home) {
                    (Kind::Env, _) => widgets::caption(
                        ui,
                        &format!(
                            "환경 변수 GEMMA_SEARCH_DATA로 데이터 폴더를 {}로 정해 두었습니다. 여기서는 옮기지 않습니다.",
                            loc.data.display()
                        ),
                    ),
                    (Kind::Home, _) => {
                        ui.add_space(space::XS);
                        ui.horizontal_wrapped(|ui| {
                            if widgets::button_enabled(ui, Btn::Secondary, !busy, "다른 곳으로 옮기기…")
                                .on_hover_text("고른 폴더로 통째로 옮깁니다 (예: 여유가 있는 다른 드라이브)")
                                .on_disabled_hover_text(busy_why)
                                .clicked()
                            {
                                if let Some(dir) = rfd::FileDialog::new().set_title("옮길 폴더 고르기").pick_folder() {
                                    job = Some(Job::MoveTo(dir));
                                }
                            }
                            if widgets::button_enabled(ui, Btn::Secondary, !busy, "예전 데이터 가져오기…")
                                .on_hover_text("예전 버전을 풀어 둔 폴더(또는 그 data 폴더)의 설정·인덱스를 가져옵니다")
                                .on_disabled_hover_text(busy_why)
                                .clicked()
                            {
                                import = true;
                            }
                        });
                    }
                    (Kind::Portable | Kind::Legacy, Some(home)) => {
                        widgets::caption(
                            ui,
                            &format!(
                                "실행 파일 옆을 쓰고 있습니다. 고정 위치({})로 옮기면 새 버전을 다른 폴더에 풀어도 같은 인덱스를 씁니다.",
                                home.display()
                            ),
                        );
                        ui.add_space(space::XS);
                        if widgets::button_enabled(ui, Btn::Secondary, !busy, "고정 위치로 옮기기")
                            .on_disabled_hover_text(busy_why)
                            .clicked()
                        {
                            job = Some(Job::Import(loc.app_dir.clone()));
                        }
                    }
                    (_, None) => widgets::caption(ui, "사용자 폴더를 알 수 없어 실행 파일 옆을 씁니다."),
                }
                if loc.kind != Kind::Env {
                    widgets::caption(ui, "옮기면 앱이 다시 시작됩니다. 지금 위치를 확인하려면 명령줄에서 `gemma-search data`.");
                }
            },
        );
        if import {
            self.pick_import();
        }
        if let Some(job) = job {
            self.propose_move(job);
        }
    }

    /// 예전 데이터 가져오기: 폴더를 고르게 하고 확인 창을 띄운다 (설정과 처음 설정에서)
    pub fn pick_import(&mut self) {
        if let Some(dir) = rfd::FileDialog::new()
            .set_title("예전 버전 폴더(또는 그 data 폴더) 고르기")
            .pick_folder()
        {
            self.propose_move(Job::Import(dir));
        }
    }

    /// 할 일을 확인하고 확인 창에 띄운다. 할 수 없으면 까닭을 알린다.
    pub fn propose_move(&mut self, job: Job) {
        match location::plan(&job, &location::current()) {
            Ok(plan) => self.data_move = Some(plan),
            Err(e) => self.notify(e, true),
        }
    }

    /// 옮기기 확인 창
    pub fn data_move_dialog(&mut self, ctx: &egui::Context) {
        let Some(plan) = &self.data_move else { return };
        let title = match plan.job {
            Job::Import(_) => "데이터 가져오기",
            Job::MoveTo(_) => "데이터 옮기기",
        };
        let (mut go, mut cancel) = (false, false);
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(560.0);
                ui.label(RichText::new("옮길 곳").weak());
                ui.label(RichText::new(plan.home.display().to_string()).text_style(theme::ts_strong()));
                ui.add_space(space::S);
                for item in &plan.items {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(format!("{} · {}", item.name, location::size(item.bytes)))
                                .text_style(theme::ts_strong()),
                        );
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
                    widgets::callout(
                        ui,
                        Tone::Warn,
                        "지금 인덱스를 바꿉니다",
                        &format!(
                            "지금 인덱스(파일 {n}개)는 지우지 않고 data.before-import-<시각>으로 이름을 바꿔 둡니다. \
                             필요 없으면 나중에 직접 지우세요."
                        ),
                    );
                }
                if matches!(plan.job, Job::Import(_)) {
                    widgets::caption(ui, "가져온 폴더에서는 데이터가 빠집니다. 그 폴더의 예전 버전은 빈 데이터로 시작합니다.");
                }
                ui.add_space(space::M);
                ui.horizontal(|ui| {
                    go = widgets::button(ui, Btn::Primary, "옮기고 다시 시작").clicked();
                    cancel = widgets::button(ui, Btn::Secondary, "취소").clicked();
                });
            });
        if cancel {
            self.data_move = None;
        } else if go {
            if let Some(plan) = self.data_move.take() {
                self.restart_to_move(ctx, &plan.job);
            }
        }
    }
}
