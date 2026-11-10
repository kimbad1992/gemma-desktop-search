//! 테마: 글자 크기·간격·색을 한 곳에서 정한다.
//!
//! 시스템의 어두운/밝은 테마를 그대로 따른다 (egui가 매 프레임 고른다). 여기서는 두 테마의 스타일을
//! 각각 손봐 두기만 한다. 화면 코드는 색 상수 대신 `Tone`·`Hue`와 `palette()`를 쓴다.
//!
//! 모양의 기준: 푸른 기가 도는 바탕, 앱 아이콘의 남색→청록 그라데이션을 강조색으로(`gradient`),
//! 둥근 모서리(카드 12, 버튼·입력칸 9), 아이콘은 Lucide 글꼴(`ICONS`).

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Shadow, Stroke, TextStyle, Theme, Visuals};

/// 굵기별 글꼴 패밀리 이름 (`app::setup_fonts`가 항상 바인딩한다): 버튼 500, 강조 라벨 600, 제목·숫자 700
pub const MEDIUM: &str = "medium";
pub const SEMIBOLD: &str = "semibold";
pub const BOLD: &str = "bold";
/// 아이콘 글꼴(Lucide)만 든 패밀리. 아이콘을 따로 그릴 때 쓴다 (`widgets::icon`).
pub const ICONS: &str = "icons";

/// 간격 단위
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

/// 카드 모서리 / 버튼·입력칸 모서리
pub const RADIUS: u8 = 12;
pub const RADIUS_SM: u8 = 9;
/// 카드 안쪽 여백
pub const CARD_PAD: i8 = 16;
/// 버튼·입력칸 높이
pub const CONTROL_H: f32 = 32.0;
/// 글 위주 화면(설정·마법사)의 최대 폭 / 표가 있는 화면(인덱스·서버)의 최대 폭
pub const PAGE_W: f32 = 920.0;
pub const DASH_W: f32 = 1100.0;
/// 왼쪽 메뉴 폭 (넓을 때 / 창이 좁아 아이콘만 보일 때)
pub const NAV_W: f32 = 184.0;
pub const NAV_W_COMPACT: f32 = 60.0;

/// 섹션 제목 (16 굵게)
pub fn ts_section() -> TextStyle {
    TextStyle::Name("section".into())
}

/// 항목 제목: 파일 이름, 카드 안의 작은 제목 (15, 굵기 600)
pub fn ts_item() -> TextStyle {
    TextStyle::Name("item".into())
}

/// 강조 라벨: 토글 제목, 표 머리 (본문 크기, 굵기 600)
pub fn ts_strong() -> TextStyle {
    TextStyle::Name("strong".into())
}

/// 통계 숫자 (24 굵게)
pub fn ts_stat() -> TextStyle {
    TextStyle::Name("stat".into())
}

/// 검색 입력칸 글꼴
pub fn query_font() -> FontId {
    FontId::new(17.0, FontFamily::Proportional)
}

/// 테마별 색
pub struct Palette {
    /// 본문 바탕
    pub panel: Color32,
    /// 왼쪽 메뉴·미리보기처럼 한 단 가라앉은 바탕
    pub sidebar: Color32,
    pub card_fill: Color32,
    /// 카드·구분선의 옅은 테두리
    pub card_stroke: Color32,
    /// 카드 안 줄무늬·옅은 강조
    pub stripe: Color32,
    /// 움푹 들어간 영역: 로그, 탭 묶음의 바탕
    pub inset_fill: Color32,
    /// 입력칸 바탕
    pub field_fill: Color32,
    /// 보통 버튼: 바탕 / 올렸을 때 / 눌렀을 때 / 테두리 / 올렸을 때 테두리
    pub control_fill: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub control_stroke: Color32,
    pub control_stroke_hover: Color32,
    /// 막대·스위치의 빈 부분
    pub track: Color32,
    /// 떠 있는 것(메뉴·알림)의 바탕
    pub popup_fill: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    /// 덜 중요한 글자
    pub muted: Color32,
    /// 강조색 (그라데이션의 시작, 남색)
    pub accent: Color32,
    /// 그라데이션의 끝 (청록)
    pub accent2: Color32,
    /// 바탕 위에 강조색으로 쓰는 글자·링크
    pub accent_text: Color32,
    /// 강조색 위의 글자
    pub on_accent: Color32,
    /// 선택된 행의 옅은 강조 배경
    pub accent_soft: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub selection_bg: Color32,
    pub selection_fg: Color32,
    /// 검색어 강조: 배경 / 글자
    pub highlight_bg: Color32,
    pub highlight_fg: Color32,
}

static DARK: Palette = Palette {
    panel: Color32::from_rgb(14, 16, 22),
    sidebar: Color32::from_rgb(10, 12, 17),
    card_fill: Color32::from_rgb(20, 23, 32),
    card_stroke: Color32::from_rgb(35, 40, 55),
    stripe: Color32::from_rgb(25, 29, 40),
    inset_fill: Color32::from_rgb(9, 10, 15),
    field_fill: Color32::from_rgb(22, 25, 35),
    control_fill: Color32::from_rgb(27, 31, 43),
    control_hover: Color32::from_rgb(35, 40, 56),
    control_active: Color32::from_rgb(24, 27, 38),
    control_stroke: Color32::from_rgb(45, 51, 70),
    control_stroke_hover: Color32::from_rgb(66, 74, 102),
    track: Color32::from_rgb(40, 45, 62),
    popup_fill: Color32::from_rgb(25, 29, 41),
    text: Color32::from_rgb(214, 218, 232),
    text_strong: Color32::from_rgb(244, 245, 250),
    muted: Color32::from_rgb(134, 141, 164),
    accent: Color32::from_rgb(98, 114, 255),
    accent2: Color32::from_rgb(25, 194, 181),
    accent_text: Color32::from_rgb(150, 163, 255),
    on_accent: Color32::WHITE,
    accent_soft: Color32::from_rgb(28, 33, 60),
    ok: Color32::from_rgb(72, 214, 146),
    warn: Color32::from_rgb(244, 176, 72),
    danger: Color32::from_rgb(255, 120, 112),
    selection_bg: Color32::from_rgb(44, 52, 112),
    selection_fg: Color32::from_rgb(222, 226, 255),
    highlight_bg: Color32::from_rgb(20, 62, 66),
    highlight_fg: Color32::from_rgb(134, 236, 226),
};

static LIGHT: Palette = Palette {
    panel: Color32::from_rgb(246, 247, 251),
    sidebar: Color32::from_rgb(237, 239, 247),
    card_fill: Color32::WHITE,
    card_stroke: Color32::from_rgb(224, 228, 239),
    stripe: Color32::from_rgb(243, 245, 250),
    inset_fill: Color32::from_rgb(234, 237, 246),
    field_fill: Color32::WHITE,
    control_fill: Color32::WHITE,
    control_hover: Color32::from_rgb(243, 245, 252),
    control_active: Color32::from_rgb(232, 235, 247),
    control_stroke: Color32::from_rgb(212, 217, 232),
    control_stroke_hover: Color32::from_rgb(176, 184, 210),
    track: Color32::from_rgb(218, 222, 236),
    popup_fill: Color32::WHITE,
    text: Color32::from_rgb(40, 44, 60),
    text_strong: Color32::from_rgb(18, 20, 31),
    muted: Color32::from_rgb(103, 111, 135),
    accent: Color32::from_rgb(79, 91, 230),
    accent2: Color32::from_rgb(14, 160, 150),
    accent_text: Color32::from_rgb(66, 77, 214),
    on_accent: Color32::WHITE,
    accent_soft: Color32::from_rgb(232, 235, 255),
    ok: Color32::from_rgb(20, 134, 72),
    warn: Color32::from_rgb(166, 104, 0),
    danger: Color32::from_rgb(208, 44, 44),
    selection_bg: Color32::from_rgb(218, 223, 255),
    selection_fg: Color32::from_rgb(48, 58, 180),
    highlight_bg: Color32::from_rgb(204, 242, 237),
    highlight_fg: Color32::from_rgb(8, 104, 98),
};

pub fn palette(v: &Visuals) -> &'static Palette {
    palette_of(v.dark_mode)
}

pub fn palette_of(dark: bool) -> &'static Palette {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// 강조 그라데이션의 두 끝 색 (왼쪽 남색 → 오른쪽 청록)
pub fn gradient(dark: bool) -> (Color32, Color32) {
    let p = if dark { &DARK } else { &LIGHT };
    (p.accent, p.accent2)
}

/// 의미가 있는 색: 화면 코드는 초록·빨강 같은 색 이름 대신 이것을 쓴다
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// 보통 글자색
    Neutral,
    /// 덜 중요함 (꺼짐, 대기)
    Muted,
    Info,
    Ok,
    Warn,
    Danger,
}

impl Tone {
    /// 글자·테두리 색
    pub fn fg(self, v: &Visuals) -> Color32 {
        let p = palette(v);
        match self {
            Tone::Neutral => p.text,
            Tone::Muted => p.muted,
            Tone::Info => p.accent_text,
            Tone::Ok => p.ok,
            Tone::Warn => p.warn,
            Tone::Danger => p.danger,
        }
    }

    /// 옅은 배경 색 (안내 상자, 배지)
    pub fn bg(self, v: &Visuals) -> Color32 {
        match self {
            Tone::Neutral | Tone::Muted => palette(v).stripe,
            _ => self.fg(v).gamma_multiply(if v.dark_mode { 0.13 } else { 0.10 }),
        }
    }

    /// 뜻에 맞는 아이콘
    pub fn glyph(self) -> lucide_icons::Icon {
        use lucide_icons::Icon;
        match self {
            Tone::Neutral => Icon::Dot,
            Tone::Muted => Icon::Minus,
            Tone::Info => Icon::Info,
            Tone::Ok => Icon::Check,
            Tone::Warn => Icon::TriangleAlert,
            Tone::Danger => Icon::CircleAlert,
        }
    }

    /// 라벨로 놓는 아이콘 (아이콘 글꼴로 쓴 글자 하나). 색은 `ui.colored_label`로 입힌다.
    pub fn icon(self) -> egui::RichText {
        egui::RichText::new(self.glyph().unicode().to_string()).font(FontId::new(15.0, FontFamily::Name(ICONS.into())))
    }
}

/// 파일 종류를 구별하는 색 (결과 줄의 종류 타일). 뜻(좋음·나쁨)은 없다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hue {
    Indigo,
    Teal,
    Coral,
    Amber,
    Violet,
    Green,
    Gray,
}

impl Hue {
    /// 아이콘·글자 색
    pub fn fg(self, v: &Visuals) -> Color32 {
        let (dark, light) = match self {
            Hue::Indigo => ((157, 168, 255), (79, 91, 230)),
            Hue::Teal => ((95, 220, 209), (12, 140, 130)),
            Hue::Coral => ((255, 150, 128), (204, 78, 54)),
            Hue::Amber => ((244, 180, 84), (172, 110, 0)),
            Hue::Violet => ((198, 154, 255), (128, 80, 216)),
            Hue::Green => ((104, 222, 164), (22, 138, 74)),
            Hue::Gray => ((154, 160, 180), (104, 112, 134)),
        };
        let (r, g, b) = if v.dark_mode { dark } else { light };
        Color32::from_rgb(r, g, b)
    }

    /// 옅은 배경 색
    pub fn bg(self, v: &Visuals) -> Color32 {
        self.fg(v).gamma_multiply(if v.dark_mode { 0.16 } else { 0.12 })
    }
}

fn apply_palette(v: &mut Visuals, p: &Palette) {
    let dark = v.dark_mode;
    v.panel_fill = p.panel;
    v.window_fill = p.popup_fill;
    v.window_stroke = Stroke::new(1.0, p.card_stroke);
    v.faint_bg_color = p.stripe;
    v.extreme_bg_color = p.inset_fill;
    v.text_edit_bg_color = Some(p.field_fill);
    v.code_bg_color = p.stripe;
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent_text;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    v.selection.bg_fill = p.selection_bg;
    v.selection.stroke = Stroke::new(1.0, p.selection_fg);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent_text);
    v.slider_trailing_fill = true;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    let shade = |a: f32| Color32::from_black_alpha((a * if dark { 255.0 } else { 90.0 }) as u8);
    v.window_shadow = Shadow {
        offset: [0, 14],
        blur: 36,
        spread: 0,
        color: shade(0.5),
    };
    v.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: shade(0.42),
    };

    let w = &mut v.widgets;
    // 글자·구분선
    w.noninteractive.bg_fill = p.card_fill;
    w.noninteractive.weak_bg_fill = p.card_fill;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    // egui 기본 버튼·체크박스·드롭다운도 우리 보통 버튼과 같은 모양으로
    w.inactive.bg_fill = p.control_fill;
    w.inactive.weak_bg_fill = p.control_fill;
    w.inactive.bg_stroke = Stroke::new(1.0, p.control_stroke);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_fill = p.control_hover;
    w.hovered.weak_bg_fill = p.control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.control_stroke_hover);
    w.hovered.fg_stroke = Stroke::new(1.5, p.text_strong);
    w.active.bg_fill = p.control_active;
    w.active.weak_bg_fill = p.control_active;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.5, p.text_strong);
    w.open.bg_fill = p.control_hover;
    w.open.weak_bg_fill = p.control_hover;
    w.open.bg_stroke = Stroke::new(1.0, p.accent);
    w.open.fg_stroke = Stroke::new(1.0, p.text_strong);
    for wv in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.expansion = 0.0;
    }
}

/// 앱을 시작할 때 한 번: 두 테마의 스타일을 모두 맞춘다
pub fn install(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        let fam = |name: &str| FontFamily::Name(name.into());
        // 이름 붙인 글자 스타일은 두 테마 모두에 있어야 한다 (없으면 그릴 때 패닉)
        s.text_styles = [
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, fam(MEDIUM))),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
            (TextStyle::Heading, FontId::new(22.0, fam(BOLD))),
            (ts_strong(), FontId::new(14.0, fam(SEMIBOLD))),
            (ts_item(), FontId::new(15.0, fam(SEMIBOLD))),
            (ts_section(), FontId::new(16.0, fam(BOLD))),
            (ts_stat(), FontId::new(24.0, fam(BOLD))),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.button_padding = egui::vec2(14.0, 6.0);
        s.spacing.interact_size.y = CONTROL_H;
        s.spacing.combo_height = 320.0;
        s.spacing.menu_margin = egui::Margin::same(8);
        s.spacing.icon_width = 18.0;
        // 얇은 스크롤 막대
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.spacing.icon_width_inner = 10.0;
        let r = CornerRadius::same(RADIUS_SM);
        let w = &mut s.visuals.widgets;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = r;
        }
        s.visuals.window_corner_radius = CornerRadius::same(14);
        s.visuals.menu_corner_radius = CornerRadius::same(RADIUS);
    });
    ctx.style_mut_of(Theme::Dark, |s| apply_palette(&mut s.visuals, &DARK));
    ctx.style_mut_of(Theme::Light, |s| apply_palette(&mut s.visuals, &LIGHT));
}
