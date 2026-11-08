//! 공용 위젯: 상태 배지, 통계 카드, 색상

use crate::embed::selftest::Check;
use crate::index::progress::RunState;
use crate::server::ServerState;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Sense, Stroke, Vec2};

pub const GREEN: Color32 = Color32::from_rgb(46, 160, 67);
pub const YELLOW: Color32 = Color32::from_rgb(214, 158, 26);
pub const GRAY: Color32 = Color32::from_rgb(128, 128, 128);
pub const BLUE: Color32 = Color32::from_rgb(56, 132, 244);
pub const RED: Color32 = Color32::from_rgb(214, 54, 51);
pub const ORANGE: Color32 = Color32::from_rgb(230, 120, 30);

pub fn server_color(s: &ServerState) -> Color32 {
    match s {
        ServerState::Stopped => GRAY,
        ServerState::Starting | ServerState::Loading => YELLOW,
        ServerState::Ready | ServerState::External => GREEN,
        ServerState::Failed(_) => RED,
    }
}

pub fn run_color(s: RunState) -> Color32 {
    match s {
        RunState::Idle | RunState::Stopped => GRAY,
        RunState::Running => GREEN,
        RunState::Paused | RunState::Stopping => YELLOW,
        RunState::Completed => BLUE,
        RunState::Failed => RED,
    }
}

pub fn check_style(c: &Check) -> (Color32, &'static str) {
    match c {
        Check::Ok(_) => (GREEN, "✔ 사용 가능"),
        Check::Warn(_) => (ORANGE, "⚠ 주의"),
        Check::Fail(_) => (RED, "✖ 사용 불가"),
        Check::Skipped(_) => (GRAY, "– 건너뜀"),
    }
}

/// 둥근 테두리 + 색 점 + 텍스트. pulse=true면 점이 깜빡여 "진행 중"을 표시.
pub fn badge(ui: &mut egui::Ui, color: Color32, text: &str, large: bool, pulse: bool) -> egui::Response {
    let size = if large { 20.0 } else { 13.0 };
    let dot_color = if pulse {
        let t = ui.input(|i| i.time) as f32;
        let a = 0.45 + 0.55 * (t * 3.2).sin().abs();
        ui.ctx().request_repaint();
        color.gamma_multiply(a)
    } else {
        color
    };
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.8)))
        .corner_radius(CornerRadius::same(if large { 14 } else { 10 }))
        .inner_margin(if large {
            Margin::symmetric(14, 6)
        } else {
            Margin::symmetric(8, 2)
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = size * 0.36;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(r * 2.0 + 2.0), Sense::hover());
                ui.painter().circle_filled(rect.center(), r, dot_color);
                ui.label(RichText::new(text).size(size).color(color).strong());
            });
        })
        .response
}

/// 큰 숫자 + 라벨 카드
pub fn stat_card(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_min_width(110.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(value).size(22.0).strong().color(color));
                ui.label(RichText::new(label).size(12.0).weak());
            });
        });
}

pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
}

pub fn section_frame<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}
