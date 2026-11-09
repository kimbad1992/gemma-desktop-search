//! 서버 탭: 시작/중지, 상태, 자가진단, 로그

use crate::app::GemmaApp;
use crate::server::{SelfTest, ServerState};
use crate::ui::widgets;
use crate::util::fmt_dur;
use eframe::egui::{self, RichText};

impl GemmaApp {
    pub fn server_tab(&mut self, ui: &mut egui::Ui) {
        let (state, pid, started, ready, port, cmdline, selftest) = {
            let s = self.server.shared.lock();
            (
                s.state.clone(),
                s.pid,
                s.started_at,
                s.ready_at,
                s.port,
                s.command_line.clone(),
                s.selftest.clone(),
            )
        };

        ui.horizontal(|ui| {
            let pulse = matches!(state, ServerState::Starting | ServerState::Loading);
            widgets::badge(ui, widgets::server_color(&state), state.label(), true, pulse);
            if pulse {
                ui.spinner();
            }
            ui.add_space(16.0);
            let running = state.is_running();
            if ui
                .add_enabled(!running && state != ServerState::External, egui::Button::new(RichText::new("▶ 서버 시작").size(15.0)))
                .clicked()
            {
                self.start_server_manual();
            }
            if ui
                .add_enabled(running || state == ServerState::External, egui::Button::new(RichText::new("■ 중지").size(15.0)))
                .on_hover_text("llama-server 프로세스를 종료합니다 (외부 서버는 연결만 해제)")
                .clicked()
            {
                self.stop_server();
            }
            if ui
                .add_enabled(running, egui::Button::new(RichText::new("⟳ 재시작").size(15.0)))
                .clicked()
            {
                self.stop_server();
                self.start_server_manual();
            }
            if ui
                .add_enabled(!running && state != ServerState::External, egui::Button::new("외부 서버 연결"))
                .on_hover_text("이미 같은 포트에서 실행 중인 llama-server를 그대로 사용")
                .clicked()
            {
                match self.server.connect_external(&self.cfg) {
                    Ok(()) => self.notify("외부 서버에 연결했습니다", false),
                    Err(e) => self.notify(e.to_string(), true),
                }
            }
        });

        if let ServerState::Failed(msg) = &state {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(widgets::RED.gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0, widgets::RED))
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.colored_label(widgets::RED, RichText::new(msg).strong());
                });
        }

        ui.add_space(8.0);
        widgets::section_frame(ui, |ui| {
            egui::Grid::new("server_info").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("실행 파일").weak());
                match crate::server::resolve_binary(&self.cfg.llama_path) {
                    Ok((p, serve)) => ui.label(format!(
                        "{}{}",
                        p.display(),
                        if serve { "  (llama serve 모드)" } else { "" }
                    )),
                    Err(e) => ui.colored_label(widgets::RED, e.to_string()),
                };
                ui.end_row();
                ui.label(RichText::new("모델").weak());
                ui.label(&self.cfg.model_path);
                ui.end_row();
                ui.label(RichText::new("mmproj").weak());
                ui.label(if self.cfg.has_mmproj() {
                    self.cfg.mmproj_path.as_str()
                } else {
                    "(없음 — 텍스트 전용)"
                });
                ui.end_row();
                ui.label(RichText::new("주소").weak());
                ui.label(format!(
                    "http://127.0.0.1:{}",
                    if port != 0 { port } else { self.cfg.port }
                ));
                ui.end_row();
                ui.label(RichText::new("PID").weak());
                ui.label(pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()));
                ui.end_row();
                ui.label(RichText::new("가동 시간").weak());
                ui.label(match (started, state.is_ready() || state.is_running()) {
                    (Some(t), true) => fmt_dur(t.elapsed()),
                    _ => "-".into(),
                });
                ui.end_row();
                ui.label(RichText::new("준비까지").weak());
                ui.label(match (started, ready) {
                    (Some(s), Some(r)) => format!("{:.1}초", (r - s).as_secs_f32()),
                    _ => "-".into(),
                });
                ui.end_row();
            });
            if !cmdline.is_empty() {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("명령줄").weak());
                    if ui.small_button("복사").clicked() {
                        ui.ctx().copy_text(cmdline.clone());
                    }
                });
                ui.add(egui::Label::new(RichText::new(&cmdline).monospace().size(11.0)).wrap());
            }
        });

        ui.add_space(8.0);
        widgets::section_frame(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("모달리티 자가진단").strong().size(15.0));
                if ui
                    .add_enabled(state.is_ready(), egui::Button::new("다시 진단"))
                    .clicked()
                {
                    self.server.rerun_selftest();
                }
            });
            match &selftest {
                SelfTest::NotRun => {
                    ui.label(RichText::new("서버가 준비되면 자동으로 실행됩니다").weak());
                }
                SelfTest::Running => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("텍스트 · 이미지 · 오디오 임베딩을 확인하는 중…");
                    });
                }
                SelfTest::Done(c) => {
                    egui::Grid::new("caps").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
                        for (name, chk) in [("텍스트", &c.text), ("이미지", &c.image), ("오디오", &c.audio)] {
                            let (color, label) = widgets::check_style(chk);
                            ui.label(RichText::new(name).strong());
                            ui.colored_label(color, label);
                            ui.label(RichText::new(chk.detail()).small());
                            ui.end_row();
                        }
                    });
                    ui.label(
                        RichText::new(format!("임베딩 차원 {} · 진단 {}ms", c.dim, c.elapsed_ms))
                            .small()
                            .weak(),
                    );
                }
            }
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("서버 로그").strong());
            if ui.small_button("복사").clicked() {
                let all: Vec<String> = self.server.logs.lock().iter().cloned().collect();
                ui.ctx().copy_text(all.join("\n"));
            }
            if ui.small_button("지우기").clicked() {
                self.server.logs.lock().clear();
            }
        });
        let lines: Vec<String> = self.server.logs.lock().iter().cloned().collect();
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(6)
            .inner_margin(6)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show_rows(ui, row_h, lines.len(), |ui, range| {
                        for line in &lines[range] {
                            let color = if line.starts_with("[앱]") {
                                widgets::BLUE
                            } else if line.contains(" E ") || line.to_lowercase().contains("error") {
                                widgets::RED
                            } else if line.contains(" W ") {
                                widgets::ORANGE
                            } else {
                                ui.visuals().text_color()
                            };
                            ui.add(
                                egui::Label::new(RichText::new(line).monospace().size(11.5).color(color))
                                    .extend(),
                            );
                        }
                    });
            });
    }
}
