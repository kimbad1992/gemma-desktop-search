//! 공용 위젯: 화면 틀(page·card·section), 제목·도움말, 안내 상자, 버튼, 토글, 탭 묶음, 배지, 통계 카드, 토스트.
//!
//! 색과 크기는 `theme`에서 가져온다. 화면 코드는 여기 있는 부품만 조합해서 모양을 통일한다.

use crate::index::progress::RunState;
use crate::server::ServerState;
use crate::ui::theme::{self, palette, space, Tone};
use eframe::egui::{self, Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, TextStyle, Vec2};

// 점·막대 채움용 색. 글자·테두리에는 `Tone`을 쓴다 (밝은 테마에서 대비가 맞도록).
pub const GREEN: Color32 = Color32::from_rgb(46, 160, 67);
pub const YELLOW: Color32 = Color32::from_rgb(214, 158, 26);
pub const GRAY: Color32 = Color32::from_rgb(128, 128, 128);
pub const BLUE: Color32 = Color32::from_rgb(56, 132, 244);
pub const RED: Color32 = Color32::from_rgb(214, 54, 51);

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

pub fn server_tone(s: &ServerState) -> Tone {
    match s {
        ServerState::Stopped => Tone::Muted,
        ServerState::Starting | ServerState::Loading => Tone::Warn,
        ServerState::Ready | ServerState::External => Tone::Ok,
        ServerState::Failed(_) => Tone::Danger,
    }
}

pub fn run_tone(s: RunState) -> Tone {
    match s {
        RunState::Idle | RunState::Stopped => Tone::Muted,
        RunState::Running => Tone::Ok,
        RunState::Paused | RunState::Stopping => Tone::Warn,
        RunState::Completed => Tone::Info,
        RunState::Failed => Tone::Danger,
    }
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

// ── 화면 틀 ──

/// 가운데 정렬된 최대 폭 `max_w`의 영역 (스크롤 없음: 화면 위에 고정되는 머리 줄 등)
pub fn centered<R>(ui: &mut egui::Ui, max_w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let avail = ui.available_width();
    let w = avail.min(max_w);
    let pad = ((avail - w) / 2.0).floor().max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(pad);
        ui.vertical(|ui| {
            ui.set_width(w);
            add(ui)
        })
        .inner
    })
    .inner
}

/// 화면 확인용 (`--scroll <px>`): 0이 아니면 모든 `page`를 이만큼 아래로 내린 채 고정한다.
/// 긴 화면의 아랫부분을 입력 없이 캡처하기 위한 것.
pub static TEST_SCROLL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// 화면 전체를 감싸는 틀: 세로 스크롤 + 가운데 정렬된 최대 폭 `max_w`의 본문
pub fn page<R>(ui: &mut egui::Ui, id: &str, max_w: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut area = egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]);
    let pinned = TEST_SCROLL.load(std::sync::atomic::Ordering::Relaxed);
    if pinned > 0 {
        area = area.vertical_scroll_offset(pinned as f32);
    }
    area
        .show(ui, |ui| {
            centered(ui, max_w, |ui| {
                ui.add_space(space::S);
                let r = add(ui);
                ui.add_space(space::L);
                r
            })
        })
        .inner
}

/// 테두리 있는 카드 (폭은 가능한 만큼 채운다)
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = palette(ui.visuals());
    egui::Frame::new()
        .fill(p.card_fill)
        .stroke(Stroke::new(1.0, p.card_stroke))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::same(theme::CARD_PAD))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// 제목 + (도움말) + 카드
pub fn section<R>(ui: &mut egui::Ui, title: &str, help_text: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(space::S);
    // 제목 줄 높이를 버튼이 있는 `section_with`와 맞춘다 (두 열로 나란히 놓였을 때 어긋나지 않게)
    ui.horizontal(|ui| {
        ui.set_min_height(ui.spacing().interact_size.y);
        section_title(ui, title);
    });
    if let Some(h) = help_text {
        help(ui, h);
    }
    card(ui, add)
}

/// 제목 줄 오른쪽에 버튼 등을 둘 수 있는 섹션. `header_right`의 반환값과 본문의 반환값을 함께 돌려준다.
pub fn section_with<H, R>(
    ui: &mut egui::Ui,
    title: &str,
    header_right: impl FnOnce(&mut egui::Ui) -> H,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (H, R) {
    ui.add_space(space::S);
    let h = ui
        .horizontal(|ui| {
            ui.set_min_height(ui.spacing().interact_size.y);
            section_title(ui, title);
            ui.with_layout(Layout::right_to_left(Align::Center), header_right).inner
        })
        .inner;
    let r = card(ui, add);
    (h, r)
}

/// 접었다 펴는 섹션. 닫혀 있으면 None.
pub fn collapsing_section<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    title: impl Into<egui::WidgetText>,
    default_open: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    egui::CollapsingHeader::new(title)
        .id_salt(id_salt)
        .default_open(default_open)
        .show(ui, add)
        .body_returned
}

// ── 글자 ──

/// 화면 제목 (20 굵게)
pub fn page_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).text_style(TextStyle::Heading).strong());
}

/// 섹션 제목 (16 굵게)
pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).text_style(theme::ts_section()).strong());
}

/// 카드 안의 작은 제목 (15 굵게)
pub fn subheading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).text_style(theme::ts_item()).strong());
}

/// 도움말·설명 (본문 크기, 옅은 색)
pub fn help(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).weak());
}

/// 덧붙이는 작은 글 (11.5, 옅은 색)
pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).small().weak());
}

/// 의미 색으로 쓴 글
pub fn tone_label(ui: &mut egui::Ui, tone: Tone, text: impl Into<String>) -> egui::Response {
    ui.colored_label(tone.fg(ui.visuals()), text.into())
}

/// 아이콘 + 글 한 줄 (✔ 준비됨 / ✖ 없음)
pub fn status_line(ui: &mut egui::Ui, tone: Tone, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(tone.fg(ui.visuals()), tone.icon());
        ui.label(text);
    });
}

// ── 안내 상자 ──

fn callout_frame(ui: &egui::Ui, tone: Tone) -> egui::Frame {
    let v = ui.visuals();
    egui::Frame::new()
        .fill(tone.bg(v))
        .stroke(Stroke::new(1.0, tone.fg(v).gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(Margin::symmetric(12, 10))
}

/// 안내·경고·오류 상자. 본문이 비어 있으면 제목만.
pub fn callout(ui: &mut egui::Ui, tone: Tone, title: &str, body: &str) {
    callout_ui(ui, tone, title, |ui| {
        if !body.is_empty() {
            ui.label(body);
        }
    });
}

/// 안에 버튼 등을 넣을 수 있는 안내 상자
pub fn callout_ui<R>(ui: &mut egui::Ui, tone: Tone, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let fg = tone.fg(ui.visuals());
    callout_frame(ui, tone)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !title.is_empty() {
                ui.label(
                    RichText::new(format!("{} {title}", tone.icon()))
                        .text_style(theme::ts_strong())
                        .color(fg),
                );
            }
            add(ui)
        })
        .inner
}

// ── 버튼 ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    /// 그 화면의 주된 동작 (강조색 채움)
    Primary,
    /// 보통 버튼
    Secondary,
    /// 되돌리기 어려운 동작 (빨강)
    Danger,
    /// 테두리 없는 보조 버튼
    Ghost,
}

pub fn button(ui: &mut egui::Ui, kind: Btn, text: &str) -> egui::Response {
    button_enabled(ui, kind, true, text)
}

pub fn button_enabled(ui: &mut egui::Ui, kind: Btn, enabled: bool, text: &str) -> egui::Response {
    ui.scope(|ui| {
        let p = palette(ui.visuals());
        let dark = ui.visuals().dark_mode;
        let danger_bg = Tone::Danger.bg(ui.visuals());
        let w = &mut ui.visuals_mut().widgets;
        match kind {
            Btn::Primary => {
                // Button::fill은 hover 표시가 사라지므로 상태별 색을 직접 준다
                let hover = mix(p.accent, if dark { Color32::WHITE } else { Color32::BLACK }, 0.12);
                let press = mix(p.accent, Color32::BLACK, 0.18);
                for (wv, fill) in [(&mut w.inactive, p.accent), (&mut w.hovered, hover), (&mut w.active, press)] {
                    wv.weak_bg_fill = fill;
                    wv.bg_fill = fill;
                    wv.bg_stroke = Stroke::NONE;
                    wv.fg_stroke = Stroke::new(1.0, p.on_accent);
                }
            }
            Btn::Danger => {
                w.inactive.weak_bg_fill = danger_bg;
                w.inactive.bg_stroke = Stroke::new(1.0, p.danger.gamma_multiply(0.5));
                w.inactive.fg_stroke = Stroke::new(1.0, p.danger);
                for wv in [&mut w.hovered, &mut w.active] {
                    wv.weak_bg_fill = p.danger;
                    wv.bg_fill = p.danger;
                    wv.bg_stroke = Stroke::NONE;
                    wv.fg_stroke = Stroke::new(1.0, p.on_accent);
                }
            }
            Btn::Secondary | Btn::Ghost => {}
        }
        let b = egui::Button::new(text);
        let b = if kind == Btn::Ghost { b.frame_when_inactive(false) } else { b };
        ui.add_enabled(enabled, b)
    })
    .inner
}

// ── 입력 줄 ──

/// 설정 화면의 라벨 칸 폭
pub const FIELD_LABEL_W: f32 = 190.0;

/// 라벨 | 컨트롤, 그 아래 작은 도움말
pub fn field<R>(ui: &mut egui::Ui, label: &str, help_text: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let r = ui
        .horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(FIELD_LABEL_W, ui.spacing().interact_size.y),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_width(FIELD_LABEL_W);
                    ui.label(label);
                },
            );
            add(ui)
        })
        .inner;
    if let Some(h) = help_text {
        ui.horizontal(|ui| {
            ui.add_space(FIELD_LABEL_W + ui.spacing().item_spacing.x);
            caption(ui, h);
        });
    }
    r
}

/// 켜고 끄는 스위치
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(38.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let p = palette(ui.visuals());
        let off = if ui.visuals().dark_mode { Color32::from_gray(78) } else { Color32::from_gray(190) };
        let mut bg = mix(off, p.accent, t);
        if resp.hovered() {
            bg = mix(bg, if ui.visuals().dark_mode { Color32::WHITE } else { Color32::BLACK }, 0.08);
        }
        if !ui.is_enabled() {
            bg = bg.gamma_multiply(0.5);
        }
        let r = rect.height() / 2.0;
        ui.painter().rect_filled(rect, r, bg);
        let cx = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
        ui.painter().circle_filled(egui::pos2(cx, rect.center().y), r - 3.0, Color32::WHITE);
    }
    resp
}

/// 스위치 + 제목 + 한 줄 설명
pub fn toggle_row(ui: &mut egui::Ui, on: &mut bool, title: &str, desc: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let r = toggle(ui, on);
        ui.add_space(space::XS);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).text_style(theme::ts_strong()).strong());
            if !desc.is_empty() {
                ui.label(RichText::new(desc).weak());
            }
        });
        r
    })
    .inner
}

/// 붙어 있는 선택 버튼 묶음 (탭, 일반|고급). 값이 바뀌면 true.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let p = palette(ui.visuals());
    let mut changed = false;
    egui::Frame::new()
        .fill(p.inset_fill)
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (v, label) in options {
                    let selected = *value == *v;
                    let b = egui::Button::selectable(selected, *label).min_size(egui::vec2(68.0, 26.0));
                    if ui.add(b).clicked() && !selected {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// 둥근 선택 칩 (선택되지 않았을 때도 테두리가 보여 누를 수 있음을 알 수 있다)
pub fn chip(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    ui.add(
        egui::Button::selectable(selected, text)
            .frame_when_inactive(true)
            .corner_radius(CornerRadius::same(13)),
    )
}

// ── 상태 표시 ──

/// 둥근 테두리 + 색 점 + 텍스트. pulse=true면 점이 깜빡여 "진행 중"을 표시.
pub fn badge(ui: &mut egui::Ui, color: Color32, text: &str, large: bool, pulse: bool) -> egui::Response {
    let size = if large { 16.0 } else { 13.0 };
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
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(if large { 14 } else { 10 }))
        .inner_margin(if large {
            Margin::symmetric(12, 5)
        } else {
            Margin::symmetric(8, 2)
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = size * 0.34;
                let dot = |ui: &mut egui::Ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(r * 2.0 + 2.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), r, dot_color);
                };
                let label = |ui: &mut egui::Ui| {
                    ui.label(
                        RichText::new(text)
                            .size(size)
                            .family(egui::FontFamily::Name(theme::SEMIBOLD.into()))
                            .color(color),
                    );
                };
                // 오른쪽 정렬 줄 안에서는 넣는 순서가 뒤집히므로, 점이 항상 글자 왼쪽에 오게 순서를 바꾼다
                if ui.layout().prefer_right_to_left() {
                    label(ui);
                    dot(ui);
                } else {
                    dot(ui);
                    label(ui);
                }
            });
        })
        .response
}

/// 의미 색으로 그리는 배지 (밝은 테마에서도 읽히는 색)
pub fn tone_badge(ui: &mut egui::Ui, tone: Tone, text: &str, large: bool, pulse: bool) -> egui::Response {
    let color = tone.fg(ui.visuals());
    badge(ui, color, text, large, pulse)
}

/// 큰 숫자 + 라벨 카드
pub fn stat_card(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    let p = palette(ui.visuals());
    egui::Frame::new()
        .fill(p.card_fill)
        .stroke(Stroke::new(1.0, p.card_stroke))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_min_width(112.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(value).text_style(theme::ts_stat()).color(color));
                ui.label(RichText::new(label).small().weak());
            });
        });
}

pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
}

/// 얇은 진행 막대 (글자는 막대 밖에 따로 쓴다)
pub fn progress(ui: &mut egui::Ui, frac: f32, color: Color32, height: f32, animate: bool) -> egui::Response {
    ui.add(
        egui::ProgressBar::new(frac.clamp(0.0, 1.0))
            .desired_height(height)
            .fill(color)
            .animate(animate)
            .corner_radius(CornerRadius::same((height / 2.0) as u8)),
    )
}

// ── 드롭다운 · 관련도 · 빈 화면 ──

/// 체크 항목 여러 개를 담는 드롭다운. 안을 눌러도 닫히지 않고, 바깥을 누르거나 Esc로 닫힌다.
/// `active`면 걸러내는 중이라는 뜻으로 요약 글자를 강조색으로 쓴다.
pub fn multi_dropdown(ui: &mut egui::Ui, id_salt: &str, summary: &str, active: bool, add: impl FnOnce(&mut egui::Ui)) {
    let text = if active {
        RichText::new(summary).color(palette(ui.visuals()).accent)
    } else {
        RichText::new(summary)
    };
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(text)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, add);
}

/// 관련도의 뜻: 7 이상 뚜렷함, 5.5 이상 관련 있음, 그 아래 약함
pub fn score_tone(score: f32) -> Tone {
    if score >= 7.0 {
        Tone::Ok
    } else if score >= 5.5 {
        Tone::Info
    } else {
        Tone::Muted
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScoreStyle {
    /// 숫자 + 짧은 막대 (썸네일 카드, 빠른 검색창)
    Compact,
    /// 숫자 + 막대 (결과 줄)
    Row,
    /// "관련도 7.2" + 막대 (미리보기)
    Labeled,
}

/// 관련도 표시: 뜻에 맞는 색의 숫자와 얇은 막대 (0~10)
pub fn score_bar(ui: &mut egui::Ui, score: f32, style: ScoreStyle) -> egui::Response {
    let fg = score_tone(score).fg(ui.visuals());
    let track = palette(ui.visuals()).inset_fill;
    let (width, text) = match style {
        ScoreStyle::Compact => (34.0, format!("{score:.1}")),
        ScoreStyle::Row => (56.0, format!("{score:.1}")),
        ScoreStyle::Labeled => (72.0, format!("관련도 {score:.1}")),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let number = |ui: &mut egui::Ui| {
            ui.label(RichText::new(&text).text_style(theme::ts_strong()).color(fg));
        };
        let bar = |ui: &mut egui::Ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 6.0), Sense::hover());
            ui.painter().rect_filled(rect, 3.0, track);
            let mut fill = rect;
            fill.set_width(width * (score / 10.0).clamp(0.0, 1.0));
            ui.painter().rect_filled(fill, 3.0, fg);
        };
        // 오른쪽 정렬 줄에서도 숫자가 막대 왼쪽에 오게
        if ui.layout().prefer_right_to_left() {
            bar(ui);
            number(ui);
        } else {
            number(ui);
            bar(ui);
        }
    })
    .response
}

/// 아직 내용이 없을 때 화면 가운데에 놓는 안내
pub fn empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.add_space(space::XL * 2.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).text_style(theme::ts_section()).strong());
        ui.label(RichText::new(body).weak());
    });
}

// ── 미리보기 ──

/// 화면 요소 미리보기 (`--tab gallery`). 두 테마에서 부품 모양을 한눈에 확인하는 용도.
pub fn gallery(ui: &mut egui::Ui) {
    page(ui, "gallery", theme::PAGE_W, |ui| {
        page_title(ui, "화면 요소 미리보기");
        help(ui, "새 화면에 쓰는 글자 크기, 색, 카드, 버튼 모양입니다.");

        section(ui, "검색할 폴더", Some("여기에 넣은 폴더 안의 파일을 찾을 수 있습니다."), |ui| {
            status_line(ui, Tone::Ok, r"C:\Users\me\Documents");
            status_line(ui, Tone::Danger, r"D:\사진 (찾을 수 없음)");
            ui.add_space(space::XS);
            ui.horizontal_wrapped(|ui| {
                button(ui, Btn::Primary, "폴더 추가…");
                button(ui, Btn::Secondary, "되돌리기");
                button(ui, Btn::Ghost, "처음 설정 다시 열기");
                button(ui, Btn::Danger, "전체 재구축…");
                button_enabled(ui, Btn::Primary, false, "저장");
            });
        });

        section(ui, "내용까지 살펴볼 파일 종류", None, |ui| {
            let (mut on, mut off) = (true, false);
            toggle_row(ui, &mut on, "사진·그림", "사진에 무엇이 찍혔는지로 찾습니다");
            ui.add_space(space::XS);
            toggle_row(ui, &mut off, "소리·음악 (실험 기능)", "들리는 소리로 찾습니다. 아직 정확하지 않을 수 있습니다");
        });

        section(ui, "안내 상자", None, |ui| {
            callout(ui, Tone::Ok, "바로 검색할 수 있습니다", "");
            callout(ui, Tone::Info, "서버 설정을 바꿨습니다", "다시 시작해야 적용됩니다.");
            callout(ui, Tone::Warn, "검색하려면 아래 준비가 필요합니다", "검색할 폴더를 추가하세요.");
            callout(ui, Tone::Danger, "서버를 시작하지 못했습니다", "모델 파일을 찾을 수 없습니다.");
        });

        section(ui, "상태 표시", None, |ui| {
            ui.horizontal_wrapped(|ui| {
                tone_badge(ui, Tone::Ok, "서버 준비됨", true, false);
                tone_badge(ui, Tone::Warn, "인덱싱 42%", false, false);
                tone_badge(ui, Tone::Muted, "대기", false, false);
                tone_badge(ui, Tone::Danger, "실패", false, false);
            });
            ui.add_space(space::XS);
            ui.horizontal_wrapped(|ui| {
                let v = ui.visuals().clone();
                stat_card(ui, "발견한 파일", "56,237", Tone::Neutral.fg(&v));
                stat_card(ui, "처리 완료", "17,463", Tone::Ok.fg(&v));
                stat_card(ui, "오류", "12", Tone::Danger.fg(&v));
            });
            ui.add_space(space::XS);
            let accent = palette(ui.visuals()).accent;
            progress(ui, 0.42, accent, 10.0, false);
            caption(ui, "42.3% · 12,345 / 29,000 · 남은 시간 약 12분");
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                chip(ui, true, "전체 12");
                chip(ui, false, "문서 내용 5");
                chip(ui, false, "이미지 4");
            });
            let mut view = 0;
            segmented(ui, &mut view, &[(0, "일반"), (1, "고급")]);
            field(ui, "포트", Some("바꾸면 서버를 다시 시작해야 적용됩니다"), |ui| ui.label("8090"));
            subheading(ui, "카드 안의 작은 제목");
            caption(ui, "덧붙이는 작은 글");
        });
    });
}

// ── 알림 ──

/// 오른쪽 아래에 뜨는 알림. 닫기를 누르면 true.
pub fn toast(ctx: &egui::Context, text: &str, error: bool) -> bool {
    let mut close = false;
    egui::Area::new(egui::Id::unique("toast"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let tone = if error { Tone::Danger } else { Tone::Info };
            let p = palette(ui.visuals());
            let fg = tone.fg(ui.visuals());
            egui::Frame::new()
                .fill(p.card_fill)
                .stroke(Stroke::new(1.0, fg.gamma_multiply(0.7)))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_max_width(440.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(fg, tone.icon());
                        ui.add(egui::Label::new(text).wrap());
                        if ui.small_button("✕").on_hover_text("닫기").clicked() {
                            close = true;
                        }
                    });
                });
        });
    close
}
