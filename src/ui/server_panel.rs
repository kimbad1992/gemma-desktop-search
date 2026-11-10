//! 서버 탭: 상태와 시작/중지, 기능 점검(자가진단), 상세 정보, 로그

use crate::app::GemmaApp;
use crate::embed::selftest::Check;
use crate::server::{SelfTest, ServerState};
use crate::ui::theme::{self, palette, space, Tone};
use crate::ui::widgets::{self, Btn};
use crate::util::fmt_dur;
use eframe::egui::{self, RichText};

/// 머리 카드에서 누른 버튼
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    None,
    Start,
    Stop,
    Restart,
    Connect,
}

/// 상세 정보의 한 줄: 이름 | 값 (길면 잘리고, 마우스를 올리면 전체가 보인다)
fn detail_row(ui: &mut egui::Ui, key: &str, value: &str) {
    const KEY_W: f32 = 76.0;
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(KEY_W, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(KEY_W);
                ui.label(RichText::new(key).weak());
            },
        );
        ui.add(egui::Label::new(value).truncate()).on_hover_text(value);
    });
}

fn check_tone(c: &Check) -> (Tone, &'static str) {
    match c {
        Check::Ok(_) => (Tone::Ok, "사용 가능"),
        Check::Warn(_) => (Tone::Warn, "주의"),
        Check::Fail(_) => (Tone::Danger, "사용 불가"),
        Check::Skipped(_) => (Tone::Muted, "건너뜀"),
    }
}

impl GemmaApp {
    pub fn server_tab(&mut self, ui: &mut egui::Ui) {
        // 로그 영역 높이: 화면이 스크롤되므로 창 높이에 맞춰 제한한다
        let log_h = (ui.available_height() * 0.45).clamp(220.0, 520.0);
        widgets::page(ui, "server_page", theme::DASH_W, |ui| {
            self.server_header(ui);
            if ui.available_width() >= 900.0 {
                ui.columns(2, |cols| {
                    self.server_checks(&mut cols[0]);
                    self.server_details(&mut cols[1]);
                });
            } else {
                self.server_checks(ui);
                self.server_details(ui);
            }
            self.server_log(ui, log_h);
        });
    }

    /// 상태 배지, 한 줄 설명, 상황에 맞는 버튼
    fn server_header(&mut self, ui: &mut egui::Ui) {
        let (state, started, ready, port) = {
            let s = self.server.shared.lock();
            (s.state.clone(), s.started_at, s.ready_at, s.port)
        };
        let running = state.is_running();
        let external = state == ServerState::External;
        let busy = matches!(state, ServerState::Starting | ServerState::Loading);
        let mut act = Act::None;

        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                widgets::tone_badge(ui, widgets::server_tone(&state), state.label(), true, busy);
                if busy {
                    widgets::spinner(ui);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 오른쪽부터 놓인다: 가장 중요한 버튼을 먼저
                    if external {
                        if widgets::button(ui, Btn::Secondary, "연결 해제")
                            .on_hover_text("외부 서버는 그대로 두고 연결만 끊습니다")
                            .clicked()
                        {
                            act = Act::Stop;
                        }
                    } else if running {
                        if widgets::icon_button(ui, Btn::Secondary, widgets::Icon::Square, "중지")
                            .on_hover_text("llama-server 프로세스를 종료합니다")
                            .clicked()
                        {
                            act = Act::Stop;
                        }
                        if widgets::icon_button(ui, Btn::Secondary, widgets::Icon::RotateCw, "재시작").clicked() {
                            act = Act::Restart;
                        }
                    } else {
                        if widgets::icon_button(ui, Btn::Primary, widgets::Icon::Play, "서버 시작").clicked() {
                            act = Act::Start;
                        }
                        if widgets::button(ui, Btn::Ghost, "외부 서버 연결")
                            .on_hover_text("이미 같은 포트에서 실행 중인 llama-server를 그대로 사용합니다")
                            .clicked()
                        {
                            act = Act::Connect;
                        }
                    }
                });
            });
            widgets::help(ui, "AI 모델을 돌리는 llama.cpp 서버입니다. 검색·인덱싱할 때 자동으로 켜집니다.");
            let mut line = format!("http://127.0.0.1:{}", if port != 0 { port } else { self.cfg.port });
            if let (Some(t), true) = (started, running) {
                line += &format!(" · 가동 {}", fmt_dur(t.elapsed()));
            }
            if let (Some(s), Some(r)) = (started, ready) {
                line += &format!(" · 준비까지 {:.1}초", (r - s).as_secs_f32());
            }
            widgets::caption(ui, &line);

            if let ServerState::Failed(msg) = &state {
                ui.add_space(space::XS);
                widgets::callout(ui, Tone::Danger, "서버를 시작하지 못했습니다", msg);
            }
        });

        match act {
            Act::None => {}
            Act::Start => self.start_server_manual(),
            Act::Stop => self.stop_server(),
            Act::Restart => {
                self.stop_server();
                self.start_server_manual();
            }
            Act::Connect => match self.server.connect_external(&self.cfg) {
                Ok(()) => self.notify("외부 서버에 연결했습니다", false),
                Err(e) => self.notify(e.to_string(), true),
            },
        }
    }

    /// 기능 점검: 서버가 글·이미지·오디오를 실제로 처리하는지 (자가진단)
    fn server_checks(&mut self, ui: &mut egui::Ui) {
        let (state, selftest) = {
            let s = self.server.shared.lock();
            (s.state.clone(), s.selftest.clone())
        };
        let (rerun, ()) = widgets::section_with(
            ui,
            "기능 점검",
            |ui| {
                ui.add_enabled(state.is_ready(), egui::Button::new("다시 점검"))
                    .on_hover_text("텍스트·이미지·오디오 임베딩을 다시 시험합니다")
                    .clicked()
            },
            |ui| match &selftest {
                SelfTest::NotRun => widgets::help(ui, "서버가 준비되면 자동으로 확인합니다."),
                SelfTest::Running => {
                    ui.horizontal(|ui| {
                        widgets::spinner(ui);
                        ui.label("텍스트 · 이미지 · 오디오 임베딩을 확인하는 중…");
                    });
                }
                SelfTest::Done(c) => {
                    for (name, chk) in [("텍스트", &c.text), ("이미지", &c.image), ("오디오", &c.audio)] {
                        let (tone, label) = check_tone(chk);
                        ui.horizontal(|ui| {
                            ui.colored_label(tone.fg(ui.visuals()), tone.icon());
                            ui.label(RichText::new(name).text_style(theme::ts_strong()).strong());
                            widgets::tone_label(ui, tone, label);
                        });
                        widgets::caption(ui, &chk.detail());
                    }
                    ui.add_space(space::XS);
                    widgets::caption(ui, &format!("임베딩 차원 {} · 점검 {}ms", c.dim, c.elapsed_ms));
                }
            },
        );
        if rerun {
            self.server.rerun_selftest();
        }
    }

    /// 상세 정보: 경로, 주소, 프로세스, 명령줄
    fn server_details(&mut self, ui: &mut egui::Ui) {
        let (state, pid, started, cmdline) = {
            let s = self.server.shared.lock();
            (s.state.clone(), s.pid, s.started_at, s.command_line.clone())
        };
        widgets::section(ui, "상세 정보", None, |ui| {
            match crate::server::resolve_binary(&self.cfg.llama_path) {
                Ok((p, serve)) => detail_row(
                    ui,
                    "실행 파일",
                    &format!("{}{}", p.display(), if serve { "  (llama serve 모드)" } else { "" }),
                ),
                Err(e) => detail_row(ui, "실행 파일", &e.to_string()),
            }
            detail_row(ui, "모델", &self.cfg.model_path);
            detail_row(
                ui,
                "mmproj",
                if self.cfg.has_mmproj() {
                    self.cfg.mmproj_path.as_str()
                } else {
                    "(없음: 텍스트 전용)"
                },
            );
            detail_row(ui, "PID", &pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()));
            detail_row(
                ui,
                "가동 시간",
                &match (started, state.is_ready() || state.is_running()) {
                    (Some(t), true) => fmt_dur(t.elapsed()),
                    _ => "-".into(),
                },
            );
            if !cmdline.is_empty() {
                widgets::collapsing_section(ui, "server_cmdline", "명령줄", false, |ui| {
                    if widgets::small_button(ui, Btn::Secondary, "복사").clicked() {
                        ui.ctx().copy_text(cmdline.clone());
                    }
                    ui.add(egui::Label::new(RichText::new(&cmdline).monospace()).wrap());
                });
            }
        });
    }

    /// 서버 로그 (높이 제한, 아래에 붙어 따라간다)
    fn server_log(&mut self, ui: &mut egui::Ui, height: f32) {
        let lines: Vec<String> = self.server.logs.lock().iter().cloned().collect();
        let ((copy, clear), ()) = widgets::section_with(
            ui,
            "서버 로그",
            |ui| {
                // 오른쪽부터 놓인다
                let clear = widgets::small_button(ui, Btn::Secondary, "지우기").clicked();
                let copy = widgets::small_button(ui, Btn::Secondary, "복사").clicked();
                (copy, clear)
            },
            |ui| {
                let p = palette(ui.visuals());
                let (accent, danger, warn, text) = (
                    p.accent,
                    Tone::Danger.fg(ui.visuals()),
                    Tone::Warn.fg(ui.visuals()),
                    ui.visuals().text_color(),
                );
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
                egui::Frame::new()
                    .fill(p.inset_fill)
                    .corner_radius(egui::CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        // 바깥 화면이 스크롤되므로 높이를 고정해 준다 (안 그러면 내용 높이만큼만 잡힌다)
                        ui.set_height(height);
                        egui::ScrollArea::both()
                            .id_salt("server_log")
                            .auto_shrink([false, false])
                            .stick_to_bottom(true)
                            .show_rows(ui, row_h, lines.len(), |ui, range| {
                                for line in &lines[range] {
                                    let color = if line.starts_with("[앱]") {
                                        accent
                                    } else if line.contains(" E ") || line.to_lowercase().contains("error") {
                                        danger
                                    } else if line.contains(" W ") {
                                        warn
                                    } else {
                                        text
                                    };
                                    ui.add(egui::Label::new(RichText::new(line).monospace().color(color)).extend());
                                }
                            });
                    });
            },
        );
        if copy {
            ui.ctx().copy_text(lines.join("\n"));
        }
        if clear {
            self.server.logs.lock().clear();
        }
    }
}
