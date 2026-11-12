//! 오른쪽 아래 상태 창: 인덱싱 중이고 앱 창을 보고 있지 않을 때, 그리고 계산을 빌려주는 동안(늘) 화면 오른쪽 아래에
//! 작은 창을 띄운다. 다른 작업을 하다가 PC가 왜 바쁜지(GPU·CPU 사용) 바로 알 수 있게, 일시정지·중지도 여기서.
//! 둘이 겹치면 한 창에 두 칸으로 보인다.
//!
//! 본 창이 트레이로 숨겨져 있어도 떠야 하므로, 스스로 다시 그리는 별도 창(deferred viewport)으로 만든다.
//! (즉시형 창은 본 창이 숨겨져 있으면 eframe 안에서 패닉이 난다)
//! 버튼은 채널로 앱에 알리고, 앱의 logic에서 처리한다.

use crate::app::GemmaApp;
use crate::index::progress::{Progress, RunState};
use crate::lend::{LendStatus, Phase};
use crate::ui::widgets::{self, Btn, Icon};
use crate::util::{fmt_dur, fmt_num};
use crossbeam_channel::Sender;
use eframe::egui::{self, RichText};
use parking_lot::Mutex;
use std::sync::Arc;

/// 창 폭
const WIDTH: f32 = 392.0;
/// 인덱싱 칸 높이: 본문 14·작은 글 11.5 기준으로 다섯 줄
const INDEX_HEIGHT: f32 = 136.0;
/// 빌려주기 칸 높이: 네 줄
const LEND_HEIGHT: f32 = 118.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    Pause,
    Resume,
    Stop,
    Open,
    Dismiss,
    /// 계산 빌려주기
    LendPause,
    LendResume,
    LendStop,
    LendSettings,
    LendDismiss,
}

impl GemmaApp {
    /// 인덱싱 칸을 보일지
    fn index_overlay_wanted(&self, ctx: &egui::Context) -> bool {
        if !self.cfg.index_overlay || self.overlay_dismissed || self.setup_open || self.shot.is_some() {
            return false;
        }
        let (state, brief) = {
            let p = self.indexer.progress.lock();
            // 폴더 감시가 돌린 실행은 금방 끝나는 일이 많다: 몇 초 넘게 걸릴 때만 띄운다
            let brief = p.targeted && p.started.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(4));
            (p.state, brief)
        };
        if brief || !matches!(state, RunState::Running | RunState::Paused | RunState::Stopping) {
            return false;
        }
        // 앱 창을 보고 있으면 필요 없다 (인덱스 탭에 다 나옴)
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        !(self.window_visible && focused && !minimized && self.quick.is_none())
    }

    /// 빌려주기 칸을 보일지: 빌려주는 동안은 늘 (앱 창을 보고 있어도, "인덱싱 알림 창" 설정과 상관없이).
    /// 실패로 끝났으면 까닭을 닫을 때까지 보인다.
    fn lend_overlay_wanted(&self) -> bool {
        !self.lend_overlay_dismissed && !self.setup_open && self.shot.is_none() && self.lend_status_arc().is_some()
    }

    /// 오버레이를 띄울지 (app.logic에서도 씀)
    pub fn overlay_should_show(&self, ctx: &egui::Context) -> bool {
        // 전체 화면 게임·영상·발표 중에는 숨긴다 (게임 위에 창이 뜨거나 포커스가 넘어가는 문제 방지)
        if self.overlay_user_busy() {
            return false;
        }
        self.lend_overlay_wanted() || self.index_overlay_wanted(ctx)
    }

    /// 전체 화면 상태 확인 (1초에 한 번만 묻는다)
    pub(crate) fn overlay_user_busy(&self) -> bool {
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
                OverlayAction::LendPause | OverlayAction::LendResume => {
                    if let Some(s) = &self.lend {
                        s.set_paused(a == OverlayAction::LendPause);
                    }
                }
                OverlayAction::LendStop => self.stop_lending(),
                OverlayAction::LendSettings => {
                    self.tab = crate::app::Tab::Settings;
                    self.settings_view = crate::app::SettingsView::Advanced;
                    self.show_window_from_overlay(ctx);
                }
                OverlayAction::LendDismiss => {
                    // 실패로 끝난 세션이면 까닭을 지우고, 돌고 있으면 이번 세션 동안 숨긴다
                    if self.lend.is_none() {
                        self.lend_last = None;
                    } else {
                        self.lend_overlay_dismissed = true;
                    }
                }
            }
        }
    }

    /// 매 프레임 호출 (app.ui): 보여야 하면 오버레이 창을 유지한다 (부르지 않으면 egui가 닫는다)
    pub fn index_overlay(&mut self, ctx: &egui::Context) {
        if self.overlay_user_busy() {
            return;
        }
        let lend = self.lend_overlay_wanted().then(|| self.lend_status_arc()).flatten();
        let progress = self.index_overlay_wanted(ctx).then(|| self.indexer.progress.clone());
        if lend.is_none() && progress.is_none() {
            return;
        }
        let height = progress.as_ref().map(|_| INDEX_HEIGHT).unwrap_or(0.0)
            + lend.as_ref().map(|_| LEND_HEIGHT).unwrap_or(0.0)
            + if progress.is_some() && lend.is_some() { 6.0 } else { 0.0 };
        let size = [WIDTH, height];
        // 주 모니터 작업 영역의 오른쪽 아래 (물리 픽셀 → 논리)
        let npp = ctx.native_pixels_per_point().unwrap_or(1.0).max(0.5);
        let pos = crate::shell::primary_work_area().map(|(x, y, w, h)| {
            egui::pos2((x + w) as f32 / npp - size[0] - 16.0, (y + h) as f32 / npp - size[1] - 16.0)
        });
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Gemma Search")
            .with_inner_size(size)
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false);
        if let Some(p) = pos {
            builder = builder.with_position(p);
        }
        let tx = self.overlay_tx.clone();
        let id = egui::ViewportId::from_hash_of("index_overlay");
        ctx.show_viewport_deferred(id, builder, move |ui, _class| overlay_ui(ui, progress.as_ref(), lend.as_ref(), &tx));
    }
}

/// 오버레이 내용: 빌려주기 칸(있으면 위), 인덱싱 칸(있으면 아래)
pub(crate) fn overlay_ui(
    ui: &mut egui::Ui,
    progress: Option<&Arc<Mutex<Progress>>>,
    lend: Option<&Arc<Mutex<LendStatus>>>,
    tx: &Sender<OverlayAction>,
) {
    let ctx = ui.ctx().clone();
    let pal = crate::ui::theme::palette(ui.visuals());
    // 아크릴 유리: 벽 위에 아크릴 채우기 (창은 불투명하다)
    crate::ui::glass::paint_wall(ui);
    ui.painter().rect_filled(ctx.content_rect(), 0.0, pal.g_acrylic);
    // 테두리 없는 작은 창이라, 뒤의 화면과 구별되게 테두리를 두른다
    let frame = egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, crate::ui::theme::alpha(pal.accent, 0.75)))
        .inner_margin(egui::Margin::symmetric(12, 10));
    egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        if let Some(l) = lend {
            lend_section(ui, &l.lock().clone(), tx);
        }
        if let Some(p) = progress {
            if lend.is_some() {
                ui.add_space(2.0);
                ui.separator();
            }
            index_section(ui, &p.lock().clone(), tx);
        }
    });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

fn sender(ui: &egui::Ui, tx: &Sender<OverlayAction>) -> impl Fn(OverlayAction) {
    let (ctx, tx) = (ui.ctx().clone(), tx.clone());
    move |a| {
        let _ = tx.send(a);
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }
}

fn lend_section(ui: &mut egui::Ui, st: &LendStatus, tx: &Sender<OverlayAction>) {
    let send = sender(ui, tx);
    let tone = crate::ui::lend::phase_tone(st.phase).fg(ui.visuals());
    let warn = crate::ui::theme::Tone::Warn.fg(ui.visuals());
    ui.horizontal(|ui| {
        widgets::dot(ui, tone);
        ui.label(RichText::new(format!("Gemma Search · {}", st.phase.label())).strong());
        if st.phase.is_busy() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("GPU 사용 중").small().color(warn)).on_hover_text(
                    "다른 기기의 인덱싱을 위해 이 PC의 llama-server가 그래픽카드로 임베딩하고 있습니다. 일시정지하면 새 일감을 받지 않습니다.",
                );
            });
        }
    });
    let mut line = format!("→ {}", st.owner_short());
    if st.jobs > 0 {
        line += &format!(" · 일감 {}개", fmt_num(st.jobs));
    }
    if st.rate >= 0.1 {
        line += &format!(" · 초당 {:.1}개", st.rate);
    }
    if st.busy > 0 {
        line += &format!(" · 처리 중 {}", st.busy);
    }
    ui.label(RichText::new(line).small());
    let detail = if !st.note.is_empty() {
        st.note.clone()
    } else if !st.signature.is_empty() {
        format!("{} · 슬롯 {}개", st.signature, st.slots)
    } else {
        String::new()
    };
    ui.add(egui::Label::new(RichText::new(&detail).small().weak()).truncate())
        .on_hover_text(&detail);
    ui.horizontal(|ui| {
        if st.phase.is_live() {
            if st.phase == Phase::Paused {
                if widgets::small_icon_button(ui, Btn::Primary, Icon::Play, "다시").clicked() {
                    send(OverlayAction::LendResume);
                }
            } else if st.phase != Phase::Stopping
                && widgets::small_icon_button(ui, Btn::Secondary, Icon::Pause, "일시정지")
                    .on_hover_text("새 일감을 받지 않습니다 (받은 일감은 마저 처리합니다)")
                    .clicked()
            {
                send(OverlayAction::LendPause);
            }
            if st.phase != Phase::Stopping
                && widgets::small_icon_button(ui, Btn::Secondary, Icon::Square, "끝내기").clicked()
            {
                send(OverlayAction::LendStop);
            }
        }
        if widgets::small_button(ui, Btn::Ghost, "설정").clicked() {
            send(OverlayAction::LendSettings);
        }
        let (label, hint) = if st.phase.is_live() {
            ("숨기기", "이번 빌려주기 동안 숨깁니다. 트레이 아이콘의 메뉴(\"빌려주기 상태 창 보이기\")나 설정에서 다시 볼 수 있습니다")
        } else {
            ("닫기", "이 알림을 닫습니다")
        };
        if widgets::small_button(ui, Btn::Ghost, label).on_hover_text(hint).clicked() {
            send(OverlayAction::LendDismiss);
        }
    });
}

fn index_section(ui: &mut egui::Ui, p: &Progress, tx: &Sender<OverlayAction>) {
    let send = sender(ui, tx);
    let tone = widgets::run_tone(p.state).fg(ui.visuals());
    let warn = crate::ui::theme::Tone::Warn.fg(ui.visuals());
    ui.horizontal(|ui| {
        widgets::dot(ui, tone);
        let head = match p.state {
            RunState::Paused => "인덱싱 일시정지".to_string(),
            RunState::Stopping => "인덱싱 중지하는 중…".to_string(),
            _ if p.scanning => format!("폴더 확인 중 · {}개 발견", fmt_num(p.discovered)),
            _ => format!("인덱싱 {:.0}%", p.fraction().unwrap_or(0.0) * 100.0),
        };
        ui.label(RichText::new(format!("Gemma Search · {head}")).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("GPU·CPU 사용 중").small().color(warn))
                .on_hover_text("llama-server가 그래픽카드로 파일을 임베딩하고 있습니다. 일시정지하면 바로 멈춥니다.");
        });
    });
    let frac = if p.scanning { 0.0 } else { p.fraction().unwrap_or(0.0) };
    if p.state == RunState::Running {
        widgets::progress_accent(ui, frac, 6.0, false);
    } else {
        widgets::progress(ui, frac, tone, 6.0, false);
    }
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
                if widgets::small_icon_button(ui, Btn::Primary, Icon::Play, "재개").clicked() {
                    send(OverlayAction::Resume);
                }
            }
            RunState::Running => {
                if widgets::small_icon_button(ui, Btn::Secondary, Icon::Pause, "일시정지").clicked() {
                    send(OverlayAction::Pause);
                }
            }
            _ => {}
        }
        if p.state != RunState::Stopping && widgets::small_icon_button(ui, Btn::Secondary, Icon::Square, "중지").clicked() {
            send(OverlayAction::Stop);
        }
        if widgets::small_button(ui, Btn::Ghost, "창 열기").clicked() {
            send(OverlayAction::Open);
        }
        if widgets::small_button(ui, Btn::Ghost, "숨기기")
            .on_hover_text("이번 인덱싱 동안 숨깁니다 (설정에서 아예 끌 수 있음)")
            .clicked()
        {
            send(OverlayAction::Dismiss);
        }
    });
}
