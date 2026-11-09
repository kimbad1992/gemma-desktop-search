//! 인덱싱 오버레이: 인덱싱 중이고 앱 창을 보고 있지 않을 때, 화면 오른쪽 아래에 작은 상태 창을 띄운다.
//! 다른 작업을 하다가 PC가 왜 바쁜지(GPU·CPU 사용) 바로 알 수 있게, 일시정지·중지도 여기서.
//!
//! 본 창이 트레이로 숨겨져 있어도 떠야 하므로, 스스로 다시 그리는 별도 창(deferred viewport)으로 만든다.
//! (즉시형 창은 본 창이 숨겨져 있으면 eframe 안에서 패닉이 난다)
//! 버튼은 채널로 앱에 알리고, 앱의 logic에서 처리한다.

use crate::app::GemmaApp;
use crate::index::progress::{Progress, RunState};
use crate::ui::widgets;
use crate::util::{fmt_dur, fmt_num};
use crossbeam_channel::Sender;
use eframe::egui::{self, RichText};
use parking_lot::Mutex;
use std::sync::Arc;

const SIZE: [f32; 2] = [360.0, 112.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    Pause,
    Resume,
    Stop,
    Open,
    Dismiss,
}

impl GemmaApp {
    /// 오버레이를 띄울지 (app.logic에서도 씀)
    pub fn overlay_should_show(&self, ctx: &egui::Context) -> bool {
        if !self.cfg.index_overlay || self.overlay_dismissed || self.setup_open {
            return false;
        }
        let state = self.indexer.progress.lock().state;
        if !matches!(state, RunState::Running | RunState::Paused | RunState::Stopping) {
            return false;
        }
        // 전체 화면 게임·영상·발표 중에는 숨긴다 (게임 위에 창이 뜨거나 포커스가 넘어가는 문제 방지)
        if self.overlay_user_busy() {
            return false;
        }
        // 앱 창을 보고 있으면 필요 없다 (인덱스 탭에 다 나옴)
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        !(self.window_visible && focused && !minimized && self.quick.is_none())
    }

    /// 전체 화면 상태 확인 (1초에 한 번만 묻는다)
    fn overlay_user_busy(&self) -> bool {
        let mut c = self.overlay_busy_cache.lock();
        if c.0.is_none_or(|t: std::time::Instant| t.elapsed() > std::time::Duration::from_secs(1)) {
            *c = (Some(std::time::Instant::now()), crate::shell::user_busy());
        }
        c.1
    }

    /// 새 인덱싱이 시작되면 "숨기기"를 풀고, 오버레이 버튼을 처리한다 (app.logic에서 매번)
    pub fn overlay_logic(&mut self, ctx: &egui::Context) {
        let state = self.indexer.progress.lock().state;
        if state == RunState::Running && self.overlay_last_state != RunState::Running && self.overlay_last_state != RunState::Paused {
            self.overlay_dismissed = false;
        }
        self.overlay_last_state = state;
        let actions: Vec<OverlayAction> = self.overlay_rx.try_iter().collect();
        for a in actions {
            match a {
                OverlayAction::Pause => self.indexer.pause(),
                OverlayAction::Resume => self.indexer.resume(),
                OverlayAction::Stop => self.indexer.stop(),
                OverlayAction::Dismiss => self.overlay_dismissed = true,
                OverlayAction::Open => {
                    self.tab = crate::app::Tab::Index;
                    self.show_window_from_overlay(ctx);
                }
            }
        }
    }

    /// 매 프레임 호출 (app.ui): 보여야 하면 오버레이 창을 유지한다 (부르지 않으면 egui가 닫는다)
    pub fn index_overlay(&mut self, ctx: &egui::Context) {
        if !self.overlay_should_show(ctx) {
            return;
        }
        // 주 모니터 작업 영역의 오른쪽 아래 (물리 픽셀 → 논리)
        let npp = ctx.native_pixels_per_point().unwrap_or(1.0).max(0.5);
        let pos = crate::shell::primary_work_area().map(|(x, y, w, h)| {
            egui::pos2(
                (x + w) as f32 / npp - SIZE[0] - 16.0,
                (y + h) as f32 / npp - SIZE[1] - 16.0,
            )
        });
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Gemma Search 인덱싱")
            .with_inner_size(SIZE)
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false);
        if let Some(p) = pos {
            builder = builder.with_position(p);
        }
        let progress = self.indexer.progress.clone();
        let tx = self.overlay_tx.clone();
        let id = egui::ViewportId::from_hash_of("index_overlay");
        ctx.show_viewport_deferred(id, builder, move |ui, _class| overlay_ui(ui, &progress, &tx));
    }
}

fn overlay_ui(ui: &mut egui::Ui, progress: &Arc<Mutex<Progress>>, tx: &Sender<OverlayAction>) {
    let p = progress.lock().clone();
    let ctx = ui.ctx().clone();
    let send = |a: OverlayAction| {
        let _ = tx.send(a);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    };
    let frame = egui::Frame::new()
        .fill(ui.visuals().panel_fill)
        .stroke(egui::Stroke::new(1.0, widgets::run_color(p.state)))
        .inner_margin(10);
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::dot(ui, widgets::run_color(p.state));
            let head = match p.state {
                RunState::Paused => "인덱싱 일시정지".to_string(),
                RunState::Stopping => "인덱싱 중지하는 중…".to_string(),
                _ if p.scanning => format!("폴더 확인 중 · {}개 발견", fmt_num(p.discovered)),
                _ => format!("인덱싱 {:.0}%", p.fraction().unwrap_or(0.0) * 100.0),
            };
            ui.label(RichText::new(format!("Gemma Search · {head}")).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("GPU·CPU 사용 중").small().color(widgets::ORANGE))
                    .on_hover_text("llama-server가 그래픽카드로 파일을 임베딩하고 있습니다. 일시정지하면 바로 멈춥니다.");
            });
        });
        let frac = if p.scanning { 0.0 } else { p.fraction().unwrap_or(0.0) };
        ui.add(egui::ProgressBar::new(frac).desired_height(8.0).fill(widgets::run_color(p.state)));
        let (fps, _) = p.rates();
        let mut line = format!("{} / {}", fmt_num(p.processed), fmt_num(p.discovered));
        if fps > 0.0 {
            line += &format!(" · {fps:.1}개/초");
        }
        if let Some(eta) = p.eta() {
            line += &format!(" · 남은 {}", fmt_dur(eta));
        }
        ui.label(RichText::new(line).small());
        let name = std::path::Path::new(&p.current)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        ui.add(egui::Label::new(RichText::new(name).small().weak()).truncate())
            .on_hover_text(&p.current);
        ui.horizontal(|ui| {
            match p.state {
                RunState::Paused => {
                    if ui.small_button("▶ 재개").clicked() {
                        send(OverlayAction::Resume);
                    }
                }
                RunState::Running => {
                    if ui.small_button("⏸ 일시정지").clicked() {
                        send(OverlayAction::Pause);
                    }
                }
                _ => {}
            }
            if p.state != RunState::Stopping && ui.small_button("■ 중지").clicked() {
                send(OverlayAction::Stop);
            }
            if ui.small_button("창 열기").clicked() {
                send(OverlayAction::Open);
            }
            if ui
                .small_button("숨기기")
                .on_hover_text("이번 인덱싱 동안 숨깁니다 (설정에서 아예 끌 수 있음)")
                .clicked()
            {
                send(OverlayAction::Dismiss);
            }
        });
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}
