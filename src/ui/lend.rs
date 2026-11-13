//! 창 있는 앱에서 계산 빌려주기 (메시 1단계 ④): 다른 기기의 서비스(소유 기기)가 인덱싱할 때 이 PC의 그래픽으로
//! 임베딩을 나눠 맡는다. 일 자체는 `crate::lend::LendSession`이 하고, 여기서는 앱과 잇는다.
//!
//! - 앱의 llama-server를 같이 쓴다: 세션이 서버가 필요하다고 하면 앱이 켠다(`start_server_auto`). 빌려주는 동안에는
//!   앱의 자동 끄기가 서버를 끄지 않고, 한동안 일감이 없으면 앱의 자동 끄기가 끈다(일감이 생기면 세션이 다시 켜 달라고 한다).
//! - 빌려주는 동안은 화면 오른쪽 아래 오버레이를 늘 띄운다(앱 창을 보고 있어도, "인덱싱 알림 창" 설정과 상관없이).
//!   전체 화면 앱(게임·영상·발표) 중에는 오버레이를 숨기고 빌려주기도 쉰다(새 일감을 받지 않음).
//! - 시작: 트레이 메뉴, 설정 > 고급 > 계산 빌려주기, 명령 `gemma-search.exe lend --server …`(앱이 떠 있으면 그 앱에 알린다).

use crate::app::GemmaApp;
use crate::lend::{LendHooks, LendParams, LendSession, LendStatus, Phase};
use crate::ui::theme::{space, Tone};
use crate::ui::widgets::{self, Btn};
use crate::util::fmt_num;
use eframe::egui::{self, RichText};
use parking_lot::Mutex;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 단계마다의 색
pub fn phase_tone(p: Phase) -> Tone {
    match p {
        Phase::Working => Tone::Ok,
        Phase::Connecting | Phase::StartingServer => Tone::Info,
        Phase::Paused | Phase::Resting | Phase::Stopping => Tone::Warn,
        Phase::Failed => Tone::Danger,
        Phase::Waiting | Phase::OwnerIdle | Phase::OwnerPaused | Phase::ServerOff | Phase::Stopped => Tone::Muted,
    }
}

impl GemmaApp {
    /// 오버레이·설정에 보여 줄 빌려주기 상태: 돌고 있는 세션, 아니면 실패로 끝난 마지막 세션(닫기 전까지)
    pub fn lend_status_arc(&self) -> Option<Arc<Mutex<LendStatus>>> {
        if let Some(s) = &self.lend {
            return Some(s.status.clone());
        }
        self.lend_last.clone()
    }

    /// 빌려주기를 시작한다 (설정의 주소·토큰으로)
    pub fn start_lending(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.lend.is_some() {
            return Err("이미 빌려주고 있습니다".to_string());
        }
        let given = self.cfg.lend_server.trim().to_string();
        if given.is_empty() {
            return Err("설정 > 고급 > 계산 빌려주기에서 소유 기기의 주소를 넣으세요".to_string());
        }
        let base = crate::cli::base_url(&given);
        let token = Some(self.cfg.lend_token.trim().to_string())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                (crate::cli::is_local(&base) && !self.cfg.service_token.trim().is_empty())
                    .then(|| self.cfg.service_token.trim().to_string())
            });
        self.start_lending_to(ctx, base, token)
    }

    /// 이 주소·토큰의 소유 기기에 빌려주기를 시작한다 (설정의 빌려주기, 원격 라이브러리 인덱싱 동안의 자동 빌려주기)
    pub fn start_lending_to(&mut self, ctx: &egui::Context, base: String, token: Option<String>) -> Result<(), String> {
        if self.lend.is_some() {
            return Err("이미 빌려주고 있습니다".to_string());
        }
        let (flag, c1, c2) = (self.lend_want_server.clone(), ctx.clone(), ctx.clone());
        let log_server = self.server.clone();
        let hooks = LendHooks {
            // 서버는 앱이 켠다 (다음 프레임의 lend_logic)
            want_server: Arc::new(move || {
                flag.store(true, Ordering::SeqCst);
                c1.request_repaint();
                Ok(())
            }),
            rest: {
                let srv = self.server.clone();
                let need = crate::sysinfo::lend_vram_need_mb(self.cfg.ubatch_size);
                Arc::new(move || {
                    if crate::shell::user_busy() {
                        return Some("전체 화면 앱을 쓰는 중".to_string());
                    }
                    // 우리 llama-server를 켜기 전에만 본다(켠 뒤에는 우리 몫이 섞인다): 다른 프로그램(다른 LLM 서버, 게임 등)이
                    // 그래픽 메모리를 잡고 있어 모자라면 켜지 않는다 (NVIDIA만. 모르면 막지 않는다)
                    let st = srv.state();
                    if !st.is_running() && !st.is_ready() {
                        if let Some(free) = crate::sysinfo::nvidia_free_vram_mb().filter(|f| *f < need) {
                            return Some(format!(
                                "남은 그래픽 메모리 {:.1}GB (필요 약 {:.1}GB)",
                                free as f64 / 1024.0,
                                need as f64 / 1024.0
                            ));
                        }
                    }
                    None
                })
            },
            stop_server_when_idle: false,
            stop_server_at_end: false,
            log: Arc::new(move |m: &str| log_server.log(format!("[빌려주기] {m}"))),
            changed: Arc::new(move || c2.request_repaint()),
        };
        let params = LendParams {
            base: base.clone(),
            token,
            name: crate::lend::machine_name(),
            cfg: self.cfg.clone(),
        };
        self.lend = Some(LendSession::start(params, self.server.clone(), hooks));
        self.lend_last = None;
        self.lend_overlay_dismissed = false;
        self.notify(format!("계산 빌려주기를 시작합니다 → {base}"), false);
        Ok(())
    }

    /// 끝내라고 알린다 (받은 일감을 마저 처리하고 끝난다)
    pub fn stop_lending(&mut self) {
        if let Some(s) = &self.lend {
            s.stop();
        }
    }

    pub fn toggle_lending(&mut self, ctx: &egui::Context) {
        if self.lend.is_some() {
            self.stop_lending();
            return;
        }
        if let Err(e) = self.start_lending(ctx) {
            self.notify(e, true);
            self.open_lend_settings(ctx);
        }
    }

    /// 두 번째로 실행된 `lend` 명령: 그 명령이 저장한 주소·토큰을 읽어 시작한다
    pub fn start_lending_from_command(&mut self, ctx: &egui::Context) {
        let disk = crate::config::Config::load(&self.data_dir);
        for c in [&mut self.cfg, &mut self.saved_cfg] {
            c.lend_server = disk.lend_server.clone();
            c.lend_token = disk.lend_token.clone();
        }
        if self.lend.is_some() {
            self.notify("이미 계산을 빌려주고 있습니다", false);
            return;
        }
        if let Err(e) = self.start_lending(ctx) {
            self.notify(e, true);
            self.open_lend_settings(ctx);
        }
    }

    /// 설정 > 고급(계산 빌려주기 칸이 있는 곳)을 연다
    pub fn open_lend_settings(&mut self, ctx: &egui::Context) {
        self.tab = crate::app::Tab::Settings;
        self.settings_view = crate::app::SettingsView::Advanced;
        self.show_window(ctx);
    }

    /// 매 프레임 (app.poll): 서버를 켜 달라는 요청, 빌려주는 동안 서버 자동 끄기 막기, 끝난 세션 정리
    pub fn lend_logic(&mut self) {
        if self.lend.is_none() {
            return;
        }
        if self.lend_want_server.swap(false, Ordering::SeqCst) && !self.server.state().is_running() && !self.server.state().is_ready() {
            self.server.log("[빌려주기] 일감을 처리하려고 서버를 켭니다");
            self.start_server_auto();
        }
        let Some(st) = self.lend.as_ref().map(|s| s.status()) else { return };
        // 일감을 처리하는 동안(과 마지막 일감 뒤 잠깐)은 앱의 자동 끄기가 서버를 끄지 않게
        if st.busy > 0 || st.phase == Phase::StartingServer || st.last_job.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
            self.last_activity = Instant::now();
        }
        let finished = self.lend.as_ref().is_some_and(|s| s.is_finished());
        if finished {
            if let Some(s) = self.lend.take() {
                let st = s.status();
                if st.phase == Phase::Failed {
                    self.notify(format!("계산 빌려주기를 멈췄습니다: {}", st.note), true);
                    // 까닭을 오버레이와 설정에 남긴다 (닫기 전까지)
                    self.lend_last = Some(s.status.clone());
                } else {
                    self.notify(format!("계산 빌려주기를 끝냈습니다 (일감 {}개 처리)", fmt_num(st.jobs)), false);
                }
            }
        }
    }

    /// 트레이 툴팁에 붙일 말 (빌려주지 않으면 None)
    pub fn lend_tray_tip(&self) -> Option<String> {
        let st = self.lend.as_ref()?.status();
        Some(match st.phase {
            Phase::Working if st.rate > 0.0 => format!("Gemma Search · 계산 빌려주는 중 (초당 {:.0}개)", st.rate),
            p => format!("Gemma Search · {}", p.label()),
        })
    }

    /// 설정 > 고급 > 계산 빌려주기
    pub(crate) fn advanced_lend(&mut self, ui: &mut egui::Ui) {
        let mut act: Option<bool> = None;
        let status = self.lend_status_arc().map(|a| a.lock().clone());
        let running = self.lend.is_some();
        widgets::section(
            ui,
            "계산 빌려주기",
            Some("다른 기기의 Gemma Search 서비스가 인덱싱할 때, 이 PC의 그래픽으로 임베딩을 나눠 맡습니다. 이 PC는 나가는 접속만 합니다."),
            |ui| {
                let c = &mut self.cfg;
                egui::Grid::new("adv_lend").num_columns(2).spacing([14.0, 8.0]).show(ui, |ui| {
                    ui.label("소유 기기 주소");
                    ui.add_enabled(
                        !running,
                        egui::TextEdit::singleline(&mut c.lend_server)
                            .hint_text("http://192.168.0.100:8787")
                            .desired_width(320.0),
                    );
                    ui.end_row();
                    ui.label("접속 토큰");
                    ui.add_enabled(
                        !running,
                        egui::TextEdit::singleline(&mut c.lend_token)
                            .password(true)
                            .hint_text("그 기기 설정의 service_token")
                            .desired_width(320.0),
                    );
                    ui.end_row();
                });
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    if running {
                        if widgets::button(ui, Btn::Secondary, "빌려주기 끝내기").clicked() {
                            act = Some(false);
                        }
                        if self.lend_overlay_dismissed
                            && widgets::button(ui, Btn::Ghost, "상태 창 다시 보이기")
                                .on_hover_text("화면 오른쪽 아래의 빌려주기 상태 창을 다시 띄웁니다")
                                .clicked()
                        {
                            self.lend_overlay_dismissed = false;
                        }
                    } else if widgets::button_enabled(ui, Btn::Primary, !c.lend_server.trim().is_empty(), "빌려주기 시작").clicked() {
                        act = Some(true);
                    }
                    if let Some(st) = &status {
                        let tone = phase_tone(st.phase).fg(ui.visuals());
                        widgets::dot(ui, tone);
                        let mut line = st.phase.label().to_string();
                        if st.jobs > 0 {
                            line += &format!(" · 일감 {}개 · 입력 {}개", fmt_num(st.jobs), fmt_num(st.inputs));
                        }
                        ui.label(RichText::new(line).color(tone));
                    }
                });
                if let Some(st) = &status {
                    if !st.note.is_empty() {
                        widgets::help(ui, &st.note);
                    }
                    if !st.signature.is_empty() {
                        widgets::caption(
                            ui,
                            &format!("이 PC({})의 서버: {} · 슬롯 {}개", st.name, st.signature, st.slots),
                        );
                    }
                }
                widgets::help(
                    ui,
                    "두 기기의 llama.cpp 빌드와 모델 파일이 같아야 합니다. 빌려주는 동안 화면 오른쪽 아래에 진행이 보이고, \
                     전체 화면 앱(게임·영상)을 쓰는 동안은 새 일감을 받지 않습니다. 파일 내용(글 조각·줄인 그림·영상 구간)이 \
                     그 기기와 오가므로 집 LAN이나 Tailscale 같은 VPN 안에서만 쓰세요.",
                );
            },
        );
        match act {
            Some(true) => {
                // 주소·토큰은 바로 저장한다 (다른 설정의 저장 단추와 상관없이)
                let mut s = self.saved_cfg.clone();
                s.lend_server = self.cfg.lend_server.clone();
                s.lend_token = self.cfg.lend_token.clone();
                if s.save(&self.data_dir).is_ok() {
                    self.saved_cfg = s;
                }
                let ctx = ui.ctx().clone();
                if let Err(e) = self.start_lending(&ctx) {
                    self.notify(e, true);
                }
            }
            Some(false) => self.stop_lending(),
            None => {}
        }
    }
}

/// 화면 확인용(`--shot … --lend-demo`): 빌려주는 중인 것처럼 보이는 상태
pub fn demo_status() -> LendStatus {
    LendStatus {
        phase: Phase::Working,
        owner: "http://192.168.0.100:8787".into(),
        name: "DESKTOP-4090".into(),
        signature: "llama:b12201-a2b301831|embeddinggemma-2-q8_0.gguf".into(),
        slots: 8,
        busy: 6,
        jobs: 1585,
        inputs: 1585,
        failed: 0,
        rate: 26.4,
        last_job: Some(Instant::now()),
        note: String::new(),
    }
}
