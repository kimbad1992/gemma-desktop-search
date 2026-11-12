//! 공용 위젯: 화면 틀(page·card·section), 제목·도움말, 안내 상자, 버튼, 토글, 탭 묶음, 칩, 배지, 통계 카드, 토스트.
//!
//! 색과 크기는 `theme`에서 가져온다. 화면 코드는 여기 있는 부품만 조합해서 모양을 통일한다.
//! 버튼·칩·토글·막대는 egui 기본 위젯을 쓰지 않고 직접 그린다 (그라데이션, 올렸을 때의 움직임, 아이콘).

use crate::index::progress::RunState;
use crate::server::ServerState;
use crate::ui::glass::{self, Glass};
use crate::ui::theme::{self, alpha, palette, space, Hue, Tone};
use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Shadow, Stroke,
    StrokeKind, TextStyle, Vec2,
};
pub use lucide_icons::Icon;

// 점·막대 채움용 색. 글자·테두리에는 `Tone`을 쓴다 (밝은 테마에서 대비가 맞도록).
pub const GREEN: Color32 = Color32::from_rgb(52, 199, 128);
pub const YELLOW: Color32 = Color32::from_rgb(240, 170, 60);
pub const GRAY: Color32 = Color32::from_rgb(128, 134, 152);
pub const BLUE: Color32 = Color32::from_rgb(98, 114, 255);
pub const RED: Color32 = Color32::from_rgb(240, 84, 78);

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

/// 두 색 사이 (알파까지 함께: 반투명 색끼리 섞어도 된다)
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()), f(a.a(), b.a()))
}

// ── 아이콘 · 그라데이션 ──

pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(theme::ICONS.into()))
}

/// `center`에 아이콘을 그린다 (자리를 차지하지 않는다)
pub fn paint_icon(painter: &egui::Painter, center: Pos2, icon: Icon, size: f32, color: Color32) {
    let g = painter.layout_no_wrap(icon.unicode().to_string(), icon_font(size), color);
    painter.galley(center - g.size() / 2.0, g, color);
}

/// 아이콘 하나 (정사각형 자리를 차지한다)
pub fn icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_icon(ui.painter(), rect.center(), icon, size, color);
    }
    resp
}

/// 글 한 줄 높이에 맞춰 놓는 아이콘 (위로 정렬된 줄에서 글자와 높이가 맞게)
fn icon_in_row(ui: &mut egui::Ui, glyph: Icon, size: f32, color: Color32, row_h: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, row_h.max(size)), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_icon(ui.painter(), rect.center(), glyph, size, color);
    }
}

const GRADIENT_N: usize = 64;

fn gradient_texture(ctx: &egui::Context, dark: bool) -> egui::TextureId {
    let id = egui::Id::unique(("accent-gradient", dark));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t.id();
    }
    let (a, b) = theme::gradient(dark);
    let px: Vec<Color32> = (0..GRADIENT_N).map(|i| mix(a, b, i as f32 / (GRADIENT_N - 1) as f32)).collect();
    let tex = ctx.load_texture(
        format!("accent-gradient-{dark}"),
        egui::ColorImage::new([GRADIENT_N, 1], px),
        egui::TextureOptions::LINEAR,
    );
    let tid = tex.id();
    ctx.data_mut(|d| d.insert_temp(id, tex));
    tid
}

/// 강조 그라데이션(남색→청록)으로 채운 둥근 사각형. `tint`로 투명도를 조절한다 (흰색이면 그대로).
pub fn paint_gradient(ui: &egui::Ui, rect: Rect, radius: impl Into<CornerRadius>, tint: Color32) {
    let tex = gradient_texture(ui.ctx(), ui.visuals().dark_mode);
    // 양 끝 픽셀이 번지지 않게 반 픽셀 안쪽만 쓴다
    let half = 0.5 / GRADIENT_N as f32;
    let uv = Rect::from_min_max(egui::pos2(half, 0.0), egui::pos2(1.0 - half, 1.0));
    ui.painter()
        .add(egui::epaint::RectShape::filled(rect, radius, tint).with_texture(tex, uv));
}

/// 올렸을 때 서서히 바뀌는 정도 (0~1)
fn hover_t(ui: &egui::Ui, resp: &egui::Response, on: bool) -> f32 {
    ui.ctx().animate_bool_with_time(resp.id.with("hover"), on && resp.hovered(), 0.12)
}

/// 키보드로 골랐을 때의 테두리
fn focus_ring(ui: &egui::Ui, resp: &egui::Response, rect: Rect, radius: f32) {
    if resp.has_focus() {
        let c = palette(ui.visuals()).accent_text;
        ui.painter()
            .rect_stroke(rect.expand(2.0), radius + 2.0, Stroke::new(1.5, c), StrokeKind::Outside);
    }
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

/// 화면 확인용 (`--open filter_kinds`): 이 이름의 드롭다운을 펼친 채로 그린다.
pub static TEST_OPEN: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

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
                ui.add_space(space::XL);
                r
            })
        })
        .inner
}

/// 카드 안쪽 여백
pub const CARD_MARGIN: Margin = Margin {
    left: theme::CARD_PAD,
    right: theme::CARD_PAD,
    top: 14,
    bottom: 14,
};


/// 내용 유리 카드 (폭은 가능한 만큼 채운다)
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    glass::show(ui, Glass::Content, theme::RADIUS as f32, CARD_MARGIN, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    })
    .inner
}

/// 제목 + (도움말) + 카드
pub fn section<R>(ui: &mut egui::Ui, title: &str, help_text: Option<&str>, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.add_space(space::M);
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
    ui.add_space(space::M);
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

/// 한 줄짜리 옅은 구분선
pub fn hairline(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, palette(ui.visuals()).card_stroke);
}

// ── 글자 ──

/// 화면 제목 (22 굵게)
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

/// 덧붙이는 작은 글 (12, 옅은 색)
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
        ui.spacing_mut().interact_size.y = 22.0;
        let row = ui.text_style_height(&TextStyle::Body);
        let fg = tone.fg(ui.visuals());
        icon_in_row(ui, tone.glyph(), 15.0, fg, row);
        ui.label(text);
    });
}

/// 글자 옆에 놓는 돌아가는 표시
pub fn spinner(ui: &mut egui::Ui) -> egui::Response {
    ui.add(egui::Spinner::new().size(16.0))
}

/// 아이콘과 글을 한 줄로 이은 레이아웃 (글이 길면 줄이 바뀐다)
pub fn icon_text_job(ui: &egui::Ui, glyph: Icon, text: &str, color: Color32) -> egui::text::LayoutJob {
    let font = TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    let icon_fmt = egui::TextFormat {
        font_id: icon_font(font.size + 1.0),
        color,
        valign: Align::Center,
        ..Default::default()
    };
    job.append(&glyph.unicode().to_string(), 0.0, icon_fmt);
    let text_fmt = egui::TextFormat {
        font_id: font,
        color,
        valign: Align::Center,
        ..Default::default()
    };
    job.append(text, 6.0, text_fmt);
    job
}

/// 뜻에 맞는 아이콘 + 같은 색의 글 (✔ 파일 있음 / ⚠ 비어 있음)
pub fn tone_note(ui: &mut egui::Ui, tone: Tone, text: &str) -> egui::Response {
    let job = icon_text_job(ui, tone.glyph(), text, tone.fg(ui.visuals()));
    ui.add(egui::Label::new(job).wrap())
}

/// 순서가 있는 단계의 섹션 (처음 설정): 번호 동그라미 + 제목 + (도움말) + 카드. 끝난 단계는 번호 대신 체크.
pub fn step_section<R>(
    ui: &mut egui::Ui,
    n: u32,
    title: &str,
    done: bool,
    help_text: Option<&str>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.add_space(space::M);
    ui.horizontal(|ui| {
        ui.set_min_height(ui.spacing().interact_size.y);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
        let v = ui.visuals();
        let p = palette(v);
        if done {
            ui.painter().circle_filled(rect.center(), 12.0, Tone::Ok.bg(v));
            paint_icon(ui.painter(), rect.center(), Icon::Check, 14.0, Tone::Ok.fg(v));
        } else {
            ui.painter().circle_filled(rect.center(), 12.0, p.accent_soft);
            let font = FontId::new(13.0, FontFamily::Name(theme::SEMIBOLD.into()));
            let g = ui.painter().layout_no_wrap(n.to_string(), font, p.accent_text);
            ui.painter().galley(rect.center() - g.size() / 2.0, g, p.accent_text);
        }
        section_title(ui, title);
    });
    if let Some(h) = help_text {
        help(ui, h);
    }
    card(ui, add)
}

// ── 안내 상자 ──

fn callout_frame(ui: &egui::Ui, tone: Tone) -> egui::Frame {
    let v = ui.visuals();
    egui::Frame::new()
        .fill(tone.bg(v))
        .stroke(Stroke::new(1.0, alpha(tone.fg(v), 0.30)))
        .corner_radius(CornerRadius::same(14))
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
            ui.horizontal_top(|ui| {
                let row = ui.text_style_height(&theme::ts_strong());
                icon_in_row(ui, tone.glyph(), 16.0, fg, row);
                ui.vertical(|ui| {
                    if !title.is_empty() {
                        ui.label(RichText::new(title).text_style(theme::ts_strong()).color(fg));
                    }
                    add(ui)
                })
                .inner
            })
            .inner
        })
        .inner
}

// ── 버튼 ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    /// 그 화면의 주된 동작 (강조 그라데이션)
    Primary,
    /// 보통 버튼
    Secondary,
    /// 되돌리기 어려운 동작 (빨강)
    Danger,
    /// 테두리 없는 보조 버튼
    Ghost,
}

pub fn button(ui: &mut egui::Ui, kind: Btn, text: &str) -> egui::Response {
    button_impl(ui, kind, true, None, text, theme::CONTROL_H)
}

pub fn button_enabled(ui: &mut egui::Ui, kind: Btn, enabled: bool, text: &str) -> egui::Response {
    button_impl(ui, kind, enabled, None, text, theme::CONTROL_H)
}

/// 아이콘이 붙은 버튼. `text`가 비어 있으면 아이콘만 있는 정사각형 버튼.
pub fn icon_button(ui: &mut egui::Ui, kind: Btn, icon: Icon, text: &str) -> egui::Response {
    button_impl(ui, kind, true, Some(icon), text, theme::CONTROL_H)
}

pub fn icon_button_enabled(ui: &mut egui::Ui, kind: Btn, enabled: bool, icon: Icon, text: &str) -> egui::Response {
    button_impl(ui, kind, enabled, Some(icon), text, theme::CONTROL_H)
}

/// 좁은 자리에 놓는 낮은 버튼 (표 안, 줄 끝)
pub fn small_button(ui: &mut egui::Ui, kind: Btn, text: &str) -> egui::Response {
    button_impl(ui, kind, true, None, text, 26.0)
}

/// 아이콘이 붙은 낮은 버튼
pub fn small_icon_button(ui: &mut egui::Ui, kind: Btn, icon: Icon, text: &str) -> egui::Response {
    button_impl(ui, kind, true, Some(icon), text, 26.0)
}

fn button_impl(ui: &mut egui::Ui, kind: Btn, enabled: bool, icon: Option<Icon>, text: &str, h: f32) -> egui::Response {
    let enabled = enabled && ui.is_enabled();
    let p = palette(ui.visuals());
    let dark = ui.visuals().dark_mode;
    let small = h < theme::CONTROL_H;
    let mut font = TextStyle::Button.resolve(ui.style());
    if small {
        font.size = 13.0;
    }
    let text_g = (!text.is_empty()).then(|| ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER));
    let icon_size = if small { 14.0 } else { 16.0 };
    let gap = match (icon.is_some() && text_g.is_some(), small) {
        (false, _) => 0.0,
        (true, true) => 6.0,
        (true, false) => 7.0,
    };
    let text_w = text_g.as_ref().map_or(0.0, |g| g.size().x);
    let content_w = text_w + gap + if icon.is_some() { icon_size } else { 0.0 };
    // 끝까지 둥근 알약: 글자가 있으면 좌우 15(작은 버튼 11), 아이콘만 있으면 원
    let size = match &text_g {
        None => egui::vec2(h, h),
        Some(_) => egui::vec2((content_w + if small { 22.0 } else { 30.0 }).ceil(), h),
    };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, text));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hov = hover_t(ui, &resp, enabled);
    let down = enabled && resp.is_pointer_button_down_on();
    let radius = h / 2.0;
    let dim = if enabled { 1.0 } else { 0.45 };
    let painter = ui.painter();
    let fg = match kind {
        Btn::Primary => {
            // 강조색 그림자 (올리면 조금 더 짙게)
            if !down {
                let glow = Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: alpha(p.accent, (0.38 + 0.14 * hov) * dim),
                };
                painter.add(glow.as_shape(rect, radius));
            }
            paint_gradient(ui, rect, radius, Color32::WHITE.gamma_multiply(dim));
            // 위쪽 흰 광택, 안쪽 위 흰 선, 안쪽 아래 어두운 선
            painter.add(glass::sheen(ui.ctx(), rect, radius, Color32::from_white_alpha(71).gamma_multiply(dim)));
            painter.add(glass::inner_light(rect, radius, true, Color32::from_white_alpha(128).gamma_multiply(dim)));
            painter.add(glass::inner_light(rect, radius, false, Color32::from_black_alpha(36).gamma_multiply(dim)));
            let over = if down {
                Color32::from_black_alpha(40)
            } else {
                Color32::from_white_alpha((30.0 * hov) as u8)
            };
            painter.rect_filled(rect, radius, over);
            p.on_accent.gamma_multiply(dim)
        }
        Btn::Secondary => {
            let fill = if down { p.control_active } else { mix(p.control_fill, p.control_hover, hov) };
            let stroke = mix(p.control_stroke, p.control_stroke_hover, hov);
            painter.rect(
                rect,
                radius,
                fill.gamma_multiply(dim),
                Stroke::new(1.0, stroke.gamma_multiply(dim)),
                StrokeKind::Inside,
            );
            painter.add(glass::inner_light(rect, radius, true, p.g_hi.gamma_multiply(dim)));
            mix(p.text, p.text_strong, hov).gamma_multiply(dim)
        }
        Btn::Ghost => {
            let wash = if dark { Color32::WHITE } else { Color32::BLACK };
            let a = if down { 0.10 } else { 0.07 * hov };
            painter.rect_filled(rect, radius, wash.gamma_multiply(a));
            mix(p.muted, p.text_strong, hov).gamma_multiply(dim)
        }
        Btn::Danger => {
            let a = if down { 0.26 } else { 0.12 + 0.08 * hov };
            painter.rect(
                rect,
                radius,
                alpha(p.danger, a * dim),
                Stroke::new(1.0, alpha(p.danger, (0.36 + 0.2 * hov) * dim)),
                StrokeKind::Inside,
            );
            p.danger.gamma_multiply(dim)
        }
    };
    let mut x = rect.center().x - content_w / 2.0;
    let cy = rect.center().y;
    if let Some(ic) = icon {
        paint_icon(painter, egui::pos2(x + icon_size / 2.0, cy), ic, icon_size, fg);
        x += icon_size + gap;
    }
    if let Some(g) = text_g {
        painter.galley(egui::pos2(x, cy - g.size().y / 2.0), g, fg);
    }
    focus_ring(ui, &resp, rect, radius);
    resp
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
    let size = egui::vec2(46.0, 26.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Switch, enabled, *on, ""));
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(resp.id, *on);
        let p = palette(ui.visuals());
        let dim = if enabled { 1.0 } else { 0.5 };
        let r = rect.height() / 2.0;
        let mut off = p.track;
        if resp.hovered() {
            off = mix(off, p.control_stroke_hover, 0.6);
        }
        ui.painter().rect_filled(rect, r, off.gamma_multiply(dim));
        ui.painter()
            .add(glass::inner_light(rect.translate(egui::vec2(0.0, 0.5)), r, true, p.well_in.gamma_multiply(dim)));
        if t > 0.0 {
            paint_gradient(ui, rect, r, Color32::WHITE.gamma_multiply(t * dim));
            ui.painter()
                .add(glass::sheen(ui.ctx(), rect, r, Color32::from_white_alpha(56).gamma_multiply(t * dim)));
        }
        // 가로로 긴 흰 손잡이 (28×20)
        let (kw, kh) = (28.0, 20.0);
        let x = egui::lerp((rect.left() + 3.0)..=(rect.right() - 3.0 - kw), t);
        let knob = Rect::from_min_size(egui::pos2(x, rect.center().y - kh / 2.0), egui::vec2(kw, kh));
        let shade = Shadow {
            offset: [0, 1],
            blur: 4,
            spread: 0,
            color: Color32::from_black_alpha(71),
        };
        ui.painter().add(shade.as_shape(knob, kh / 2.0));
        ui.painter()
            .rect_filled(knob, kh / 2.0, Color32::WHITE.gamma_multiply(if enabled { 1.0 } else { 0.8 }));
        focus_ring(ui, &resp, rect, r);
    }
    resp
}

/// 스위치 + 제목 + 한 줄 설명
pub fn toggle_row(ui: &mut egui::Ui, on: &mut bool, title: &str, desc: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let r = toggle(ui, on);
        ui.add_space(space::S);
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

/// 붙어 있는 선택 버튼 묶음 (일반|고급). 값이 바뀌면 true.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let p = palette(ui.visuals());
    let mut changed = false;
    // 홈(알약) 안에 선택지를 늘어놓고, 고른 것은 물방울 알약으로
    let slot = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::NONE.inner_margin(Margin::same(3)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (v, label) in options {
                let selected = *value == *v;
                let font = if selected {
                    FontId::new(14.0, FontFamily::Name(theme::SEMIBOLD.into()))
                } else {
                    TextStyle::Button.resolve(ui.style())
                };
                let g = ui.painter().layout_no_wrap((*label).to_owned(), font, Color32::PLACEHOLDER);
                let size = egui::vec2((g.size().x + 36.0).max(72.0), 28.0);
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, *label));
                let hov = hover_t(ui, &resp, !selected);
                if selected {
                    glass::bead(ui, rect, 14.0);
                }
                let fg = if selected { p.text_strong } else { mix(p.muted, p.text_strong, hov) };
                ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
                focus_ring(ui, &resp, rect, 14.0);
                if resp.clicked() && !selected {
                    *value = *v;
                    changed = true;
                }
            }
        });
    });
    let rect = inner.response.rect;
    ui.painter()
        .set(slot, glass::well_shape(ui.visuals().dark_mode, rect, rect.height() / 2.0));
    changed
}

/// 둥근 선택 칩 (선택되지 않았을 때도 테두리가 보여 누를 수 있음을 알 수 있다)
pub fn chip(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    chip_impl(ui, selected, false, None, text, false)
}

/// 아이콘이 붙은 칩의 폭 (여러 개를 가운데에 놓을 때 미리 잰다)
pub fn chip_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = FontId::new(13.0, FontFamily::Name(theme::MEDIUM.into()));
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    (g.size().x + 20.0 + 24.0).ceil()
}

/// 아이콘이 붙은 칩
pub fn icon_chip(ui: &mut egui::Ui, selected: bool, icon: Icon, text: &str) -> egui::Response {
    chip_impl(ui, selected, false, Some(icon), text, false)
}

/// `selected`면 물방울 알약(고른 것), `accent`면 강조색으로 물든 알약(걸러내는 중), 아니면 보통 칩
fn chip_impl(
    ui: &mut egui::Ui,
    selected: bool,
    accent: bool,
    icon: Option<Icon>,
    text: &str,
    chevron: bool,
) -> egui::Response {
    let p = palette(ui.visuals());
    let h = 28.0;
    let family = if selected { theme::SEMIBOLD } else { theme::MEDIUM };
    let font = FontId::new(13.0, FontFamily::Name(family.into()));
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let icon_w = if icon.is_some() { 14.0 + 6.0 } else { 0.0 };
    let chev_w = if chevron { 6.0 + 12.0 } else { 0.0 };
    let size = egui::vec2((g.size().x + icon_w + chev_w + 24.0).ceil(), h);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, text));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hov = hover_t(ui, &resp, true);
    let r = h / 2.0;
    let fg = if selected {
        glass::bead(ui, rect, r);
        p.text_strong
    } else if accent {
        ui.painter().rect(
            rect,
            r,
            alpha(p.accent, 0.14 + 0.06 * hov),
            Stroke::new(1.0, alpha(p.accent, 0.45)),
            StrokeKind::Inside,
        );
        ui.painter().add(glass::inner_light(rect, r, true, p.g_hi));
        p.accent_text
    } else {
        ui.painter().rect(
            rect,
            r,
            mix(p.control_fill, p.control_hover, hov),
            Stroke::new(1.0, mix(p.control_stroke, p.control_stroke_hover, hov)),
            StrokeKind::Inside,
        );
        ui.painter().add(glass::inner_light(rect, r, true, p.g_hi));
        mix(p.text, p.text_strong, hov)
    };
    let mut x = rect.left() + 12.0;
    let cy = rect.center().y;
    if let Some(ic) = icon {
        let c = if selected || accent { p.accent_text } else { mix(p.muted, p.text, hov) };
        paint_icon(ui.painter(), egui::pos2(x + 7.0, cy), ic, 14.0, c);
        x += 20.0;
    }
    let text_w = g.size().x;
    ui.painter().galley(egui::pos2(x, cy - g.size().y / 2.0), g, fg);
    if chevron {
        let c = if accent { p.accent_text } else { p.muted };
        paint_icon(ui.painter(), egui::pos2(x + text_w + 6.0 + 6.0, cy), Icon::ChevronDown, 13.0, c);
    }
    focus_ring(ui, &resp, rect, r);
    resp
}

/// 탭 막대(`tab_bar`) 안의 탭 하나: 고른 탭은 물방울 알약, 나머지는 바탕 없이 흐린 글자
pub fn tab(ui: &mut egui::Ui, selected: bool, icon: Icon, text: &str) -> egui::Response {
    let p = palette(ui.visuals());
    let dark = ui.visuals().dark_mode;
    let h = 30.0;
    let family = if selected { theme::SEMIBOLD } else { theme::MEDIUM };
    let font = FontId::new(13.0, FontFamily::Name(family.into()));
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let size = egui::vec2((g.size().x + 15.0 + 6.0 + 26.0).ceil(), h);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, text));
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hov = hover_t(ui, &resp, !selected);
    let r = h / 2.0;
    if selected {
        glass::bead(ui, rect, r);
    } else if hov > 0.0 {
        let wash = if dark { Color32::WHITE } else { Color32::BLACK };
        ui.painter().rect_filled(rect, r, wash.gamma_multiply(0.05 * hov));
    }
    let fg = if selected { p.text_strong } else { mix(p.muted, p.text_strong, hov) };
    let cy = rect.center().y;
    paint_icon(
        ui.painter(),
        egui::pos2(rect.left() + 13.0 + 7.5, cy),
        icon,
        15.0,
        if selected { p.accent_text } else { fg },
    );
    ui.painter()
        .galley(egui::pos2(rect.left() + 13.0 + 15.0 + 6.0, cy - g.size().y / 2.0), g, fg);
    focus_ring(ui, &resp, rect, r);
    resp
}

/// 탭들을 담는 맑은 유리 알약 막대 (안쪽 4)
pub fn tab_bar<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    glass::show(ui, Glass::Clear, 19.0, Margin::same(4), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            add(ui)
        })
        .inner
    })
    .inner
}

/// 정보를 알리는 작은 꼬리표 (누를 수 없다): 결과 줄의 "검색어 포함", "4페이지"
pub fn tag(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) -> egui::Response {
    let font = FontId::new(11.5, FontFamily::Name(theme::MEDIUM.into()));
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, fg);
    let size = egui::vec2((g.size().x + 16.0).ceil(), 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 10.0, bg);
        ui.painter().galley(rect.center() - g.size() / 2.0, g, fg);
    }
    resp
}

/// 꼬리표의 폭 (한 줄에 들어갈 자리를 미리 잴 때)
pub fn tag_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = FontId::new(11.5, FontFamily::Name(theme::MEDIUM.into()));
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    (g.size().x + 16.0).ceil()
}

/// 의미 색 꼬리표
pub fn tone_tag(ui: &mut egui::Ui, tone: Tone, text: &str) -> egui::Response {
    let v = ui.visuals();
    let (fg, bg) = (tone.fg(v), tone.bg(v));
    tag(ui, text, fg, bg)
}

/// 파일 종류를 알리는 색 타일 (썸네일이 없는 결과의 왼쪽)
pub fn kind_tile(ui: &mut egui::Ui, hue: Hue, icon: Icon, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_kind_tile(ui, rect, hue, icon);
    }
    resp
}

/// 종류 타일을 `rect`에 그린다: 종류 색이 옅게 든 바탕 + 위 빛 + 종류 색 테두리 + 아이콘
pub fn paint_kind_tile(ui: &egui::Ui, rect: Rect, hue: Hue, icon: Icon) {
    let v = ui.visuals();
    let size = rect.width().min(rect.height());
    let r = (size * 0.27).round();
    let fg = hue.fg(v);
    ui.painter()
        .rect(rect, r, hue.bg(v), Stroke::new(1.0, alpha(fg, 0.24)), StrokeKind::Inside);
    ui.painter().add(glass::inner_light(rect, r, true, palette(v).g_hi));
    paint_icon(ui.painter(), rect.center(), icon, (size * 0.46).round(), fg);
}

/// 왼쪽 메뉴의 한 줄. `compact`면 아이콘만. `mark`는 "저장하지 않은 변경 있음" 같은 점.
pub fn nav_item(ui: &mut egui::Ui, selected: bool, icon: Icon, label: &str, compact: bool, mark: bool) -> egui::Response {
    let p = palette(ui.visuals());
    let dark = ui.visuals().dark_mode;
    let size = egui::vec2(ui.available_width(), 38.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, selected, label));
    if ui.is_rect_visible(rect) {
        let hov = hover_t(ui, &resp, !selected);
        let r = rect.height() / 2.0;
        if selected {
            glass::bead(ui, rect, r);
        } else if hov > 0.0 {
            let wash = if dark { Color32::WHITE } else { Color32::BLACK };
            ui.painter().rect_filled(rect, r, wash.gamma_multiply(0.06 * hov));
        }
        let fg = if selected { p.text_strong } else { mix(p.muted, p.text_strong, hov) };
        let icon_fg = if selected { p.accent_text } else { fg };
        let cy = rect.center().y;
        if compact {
            paint_icon(ui.painter(), rect.center(), icon, 18.0, icon_fg);
        } else {
            paint_icon(ui.painter(), egui::pos2(rect.left() + 12.0 + 8.5, cy), icon, 17.0, icon_fg);
            let font = if selected {
                FontId::new(14.0, FontFamily::Name(theme::SEMIBOLD.into()))
            } else {
                FontId::new(14.0, FontFamily::Name(theme::MEDIUM.into()))
            };
            let g = ui.painter().layout_no_wrap(label.to_owned(), font, fg);
            ui.painter().galley(egui::pos2(rect.left() + 39.0, cy - g.size().y / 2.0), g, fg);
        }
        if mark {
            let c = if compact {
                egui::pos2(rect.right() - 12.0, rect.top() + 11.0)
            } else {
                egui::pos2(rect.right() - 15.0, cy)
            };
            ui.painter().circle_filled(c, 3.5, p.warn);
        }
        focus_ring(ui, &resp, rect, r);
    }
    if compact {
        resp.on_hover_text(label)
    } else {
        resp
    }
}

// ── 상태 표시 ──

/// 둥근 테두리 + 색 점 + 텍스트. pulse=true면 점이 깜빡여 "진행 중"을 표시.
pub fn badge(ui: &mut egui::Ui, color: Color32, text: &str, large: bool, pulse: bool) -> egui::Response {
    let size = if large { 15.0 } else { 12.5 };
    let dot_color = if pulse {
        let t = ui.input(|i| i.time) as f32;
        let a = 0.45 + 0.55 * (t * 3.2).sin().abs();
        ui.ctx().request_repaint();
        color.gamma_multiply(a)
    } else {
        color
    };
    let tint = palette(ui.visuals()).tint;
    egui::Frame::new()
        .fill(alpha(color, tint))
        .stroke(Stroke::new(1.0, alpha(color, 0.34)))
        .corner_radius(CornerRadius::same(if large { 15 } else { 11 }))
        .inner_margin(if large {
            Margin::symmetric(13, 6)
        } else {
            Margin::symmetric(9, 3)
        })
        .show(ui, |ui| {
            // 줄 높이를 글자에 맞춘다: 큰 배지 30, 작은 배지 22 (기본은 버튼 높이). 줄을 만들기 전에 정한다.
            ui.spacing_mut().interact_size.y = if large { 18.0 } else { 16.0 };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let r = size * 0.30;
                let dot = |ui: &mut egui::Ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(r * 2.0 + 2.0), Sense::hover());
                    ui.painter().circle_filled(rect.center(), r, dot_color);
                };
                let label = |ui: &mut egui::Ui| {
                    ui.label(
                        RichText::new(text)
                            .size(size)
                            .family(FontFamily::Name(theme::SEMIBOLD.into()))
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
    glass::show(ui, Glass::Clear, 18.0, Margin::symmetric(14, 11), |ui| {
        ui.set_min_width(116.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(value).text_style(theme::ts_stat()).color(color));
            ui.label(RichText::new(label).small().weak());
        });
    });
}

/// 상태 점 (둘레에 옅은 빛)
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(15.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 7.5, alpha(color, 0.24));
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn progress_impl(ui: &mut egui::Ui, frac: f32, fill: Option<Color32>, height: f32, animate: bool) -> egui::Response {
    let frac = frac.clamp(0.0, 1.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    if ui.is_rect_visible(rect) {
        let r = height / 2.0;
        let p = palette(ui.visuals());
        ui.painter().rect_filled(rect, r, p.track);
        ui.painter()
            .add(glass::inner_light(rect.translate(egui::vec2(0.0, 0.5)), r, true, p.well_in));
        if frac > 0.0 {
            let mut bar = rect;
            bar.set_width((rect.width() * frac).max(height));
            match fill {
                Some(c) => {
                    ui.painter().rect_filled(bar, r, c);
                }
                None => {
                    paint_gradient(ui, bar, r, Color32::WHITE);
                    ui.painter()
                        .add(glass::sheen(ui.ctx(), bar, r, Color32::from_white_alpha(97)));
                }
            }
            if animate {
                // 찬 부분 위를 지나가는 옅은 빛: 움직이고 있음을 알린다
                let t = (ui.input(|i| i.time) as f32 / 1.8).fract();
                let w = 90.0;
                let x = bar.left() - w + (bar.width() + w * 2.0) * t;
                let shine = Rect::from_min_max(egui::pos2(x, bar.top()), egui::pos2(x + w, bar.bottom()));
                ui.painter()
                    .with_clip_rect(bar.intersect(ui.clip_rect()))
                    .rect_filled(shine, r, Color32::from_white_alpha(56));
                ui.ctx().request_repaint();
            }
        }
    }
    resp
}

/// 얇은 진행 막대 (글자는 막대 밖에 따로 쓴다)
pub fn progress(ui: &mut egui::Ui, frac: f32, color: Color32, height: f32, animate: bool) -> egui::Response {
    progress_impl(ui, frac, Some(color), height, animate)
}

/// 강조 그라데이션으로 차는 진행 막대
pub fn progress_accent(ui: &mut egui::Ui, frac: f32, height: f32, animate: bool) -> egui::Response {
    progress_impl(ui, frac, None, height, animate)
}

/// 값 하나를 고르는 막대: 끌거나 눌러서 바꾼다. 왼쪽이 강조 그라데이션으로 찬다.
pub fn slider(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    step: f32,
    width: f32,
) -> egui::Response {
    let (lo, hi) = (*range.start(), *range.end());
    // 손잡이(24×16 흰 알약)의 반 폭
    let knob = 12.0;
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(width, 22.0), Sense::click_and_drag());
    let (x0, x1) = (rect.left() + knob, rect.right() - knob);
    let snap = |v: f32| {
        let v = if step > 0.0 { (v / step).round() * step } else { v };
        v.clamp(lo, hi)
    };
    let mut new = *value;
    if let Some(pos) = resp.interact_pointer_pos() {
        new = snap(lo + ((pos.x - x0) / (x1 - x0)).clamp(0.0, 1.0) * (hi - lo));
    }
    if resp.has_focus() {
        let (left, right) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
        if left {
            new = snap(new - step);
        }
        if right {
            new = snap(new + step);
        }
    }
    if (new - *value).abs() > f32::EPSILON {
        *value = new;
        resp.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let p = palette(ui.visuals());
        let t = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0);
        let cy = rect.center().y;
        let track = Rect::from_min_max(egui::pos2(x0, cy - 2.0), egui::pos2(x1, cy + 2.0));
        ui.painter().rect_filled(track, 2.0, p.track);
        let kx = egui::lerp(x0..=x1, t);
        if t > 0.0 {
            let filled = Rect::from_min_max(track.min, egui::pos2(kx, track.max.y));
            paint_gradient(ui, filled, 2.0, Color32::WHITE);
        }
        let active = resp.hovered() || resp.dragged();
        let grow = ui.ctx().animate_bool_with_time(resp.id.with("grow"), active, 0.1);
        let k = Rect::from_center_size(egui::pos2(kx, cy), egui::vec2(24.0, 16.0));
        if grow > 0.0 {
            ui.painter().rect_filled(k.expand(4.0 * grow), 8.0 + 4.0 * grow, alpha(p.accent, 0.25 * grow));
        }
        let shade = Shadow {
            offset: [0, 1],
            blur: 4,
            spread: 0,
            color: Color32::from_black_alpha(71),
        };
        ui.painter().add(shade.as_shape(k, 8.0));
        ui.painter()
            .rect(k, 8.0, Color32::WHITE, Stroke::new(0.5, Color32::from_black_alpha(26)), StrokeKind::Inside);
        focus_ring(ui, &resp, rect.shrink2(egui::vec2(0.0, 2.0)), 9.0);
    }
    resp
}

// ── 드롭다운 · 관련도 · 빈 화면 ──

/// 체크 항목 여러 개를 담는 드롭다운. 안을 눌러도 닫히지 않고, 바깥을 누르거나 Esc로 닫힌다.
/// `active`면 걸러내는 중이라는 뜻으로 강조색으로 그린다.
pub fn multi_dropdown(
    ui: &mut egui::Ui,
    id_salt: &str,
    icon: Icon,
    summary: &str,
    active: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    let resp = chip_impl(ui, false, active, Some(icon), summary, true);
    let popup_id = egui::Id::unique(("multi_dropdown", id_salt));
    // 화면 확인용 (`--shot … --open <이름>`): 그 드롭다운을 펼친 채로
    if TEST_OPEN.lock().as_deref() == Some(id_salt) {
        egui::Popup::open_id(ui.ctx(), popup_id);
    }
    // 결과 목록 위에 펼쳐지므로 밑을 실제로 흐린 진한 유리로 (`glass::show_floating`)
    egui::Popup::menu(&resp)
        .id(popup_id)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .gap(6.0)
        .frame(egui::Frame::NONE)
        .show(|ui| {
            glass::show_floating(ui, 16.0, Margin::same(8), |ui| {
                ui.set_min_width(200.0);
                egui::ScrollArea::vertical().max_height(340.0).show(ui, add);
            });
        });
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

/// 관련도 표시: 뜻에 맞는 색의 숫자와 얇은 막대 (0~10). 뚜렷하게 맞으면 막대가 강조 그라데이션.
pub fn score_bar(ui: &mut egui::Ui, score: f32, style: ScoreStyle) -> egui::Response {
    let tone = score_tone(score);
    let fg = tone.fg(ui.visuals());
    let track = palette(ui.visuals()).track;
    let (width, text) = match style {
        ScoreStyle::Compact => (34.0, format!("{score:.1}")),
        ScoreStyle::Row => (56.0, format!("{score:.1}")),
        ScoreStyle::Labeled => (72.0, format!("관련도 {score:.1}")),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let number = |ui: &mut egui::Ui| {
            ui.label(RichText::new(&text).text_style(theme::ts_strong()).color(fg));
        };
        let bar = |ui: &mut egui::Ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 5.0), Sense::hover());
            ui.painter().rect_filled(rect, 2.5, track);
            let mut fill = rect;
            fill.set_width((width * (score / 10.0).clamp(0.0, 1.0)).max(5.0));
            if tone == Tone::Ok {
                paint_gradient(ui, fill, 2.5, Color32::WHITE);
            } else {
                ui.painter().rect_filled(fill, 2.5, fg.gamma_multiply(if tone == Tone::Muted { 0.7 } else { 1.0 }));
            }
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
    empty_state_icon(ui, Icon::Search, title, body);
}

pub fn empty_state_icon(ui: &mut egui::Ui, glyph: Icon, title: &str, body: &str) {
    ui.add_space(space::XL * 2.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
        let p = palette(ui.visuals());
        paint_gradient(ui, rect, 16.0, Color32::WHITE.gamma_multiply(0.20));
        paint_icon(ui.painter(), rect.center(), glyph, 26.0, p.accent_text);
        ui.add_space(space::S);
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

        section(ui, "버튼", Some("주 버튼은 화면에 하나만 둡니다."), |ui| {
            ui.horizontal_wrapped(|ui| {
                icon_button(ui, Btn::Primary, Icon::Play, "인덱싱 시작");
                icon_button(ui, Btn::Secondary, Icon::FolderPlus, "폴더 추가…");
                button(ui, Btn::Secondary, "되돌리기");
                button(ui, Btn::Ghost, "처음 설정 다시 열기");
                icon_button(ui, Btn::Danger, Icon::Trash2, "전체 재구축…");
                button_enabled(ui, Btn::Primary, false, "저장");
                icon_button(ui, Btn::Ghost, Icon::X, "");
                small_button(ui, Btn::Secondary, "빼기");
            });
            ui.add_space(space::XS);
            ui.horizontal_wrapped(|ui| {
                let _ = ui.button("egui 기본 버튼");
                let mut c = true;
                ui.checkbox(&mut c, "체크 항목");
                let mut text = String::from("입력칸");
                ui.add(egui::TextEdit::singleline(&mut text).desired_width(140.0));
                let _ = ui.link("링크");
            });
        });

        section(ui, "검색할 폴더", Some("여기에 넣은 폴더 안의 파일을 찾을 수 있습니다."), |ui| {
            status_line(ui, Tone::Ok, r"C:\Users\me\Documents");
            status_line(ui, Tone::Danger, r"D:\사진 (찾을 수 없음)");
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
            progress_accent(ui, 0.42, 8.0, false);
            caption(ui, "42.3% · 12,345 / 29,000 · 남은 시간 약 12분");
            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                chip(ui, true, "전체 12");
                chip(ui, false, "문서 내용 5");
                icon_chip(ui, false, Icon::Image, "이미지 4");
                let v = ui.visuals().clone();
                tone_tag(ui, Tone::Ok, "검색어 포함");
                tag(ui, "4페이지", Hue::Gray.fg(&v), Hue::Gray.bg(&v));
            });
            ui.horizontal(|ui| {
                for (hue, glyph) in [
                    (Hue::Indigo, Icon::FileText),
                    (Hue::Teal, Icon::FileCode),
                    (Hue::Coral, Icon::BookOpen),
                    (Hue::Amber, Icon::Image),
                    (Hue::Violet, Icon::Film),
                    (Hue::Green, Icon::AudioLines),
                    (Hue::Gray, Icon::File),
                ] {
                    kind_tile(ui, hue, glyph, 40.0);
                }
                let mut v = 5.5;
                slider(ui, &mut v, 0.0..=12.0, 0.1, 140.0);
                score_bar(ui, 8.4, ScoreStyle::Row);
                score_bar(ui, 6.1, ScoreStyle::Row);
                score_bar(ui, 3.9, ScoreStyle::Row);
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
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-20.0, -20.0))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            let tone = if error { Tone::Danger } else { Tone::Info };
            let p = palette(ui.visuals());
            let fg = tone.fg(ui.visuals());
            egui::Frame::new()
                .fill(p.popup_fill)
                .stroke(Stroke::new(1.0, alpha(fg, 0.45)))
                .corner_radius(CornerRadius::same(16))
                .shadow(ui.visuals().popup_shadow)
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_max_width(440.0);
                    ui.horizontal(|ui| {
                        icon(ui, tone.glyph(), 16.0, fg);
                        ui.add(egui::Label::new(text).wrap());
                        if button_impl(ui, Btn::Ghost, true, Some(Icon::X), "", 26.0)
                            .on_hover_text("닫기")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
        });
    close
}
