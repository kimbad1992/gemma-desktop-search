//! 테마: 글자 크기·간격·색을 한 곳에서 정한다.
//!
//! 시스템의 어두운/밝은 테마를 그대로 따른다 (egui가 매 프레임 고른다). 여기서는 두 테마의 스타일을
//! 각각 손봐 두기만 한다. 화면 코드는 색 상수 대신 `Tone`과 `palette()`를 쓴다.

use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, Visuals};

/// 굵기별 글꼴 패밀리 이름 (`app::setup_fonts`가 항상 바인딩한다): 버튼 500, 강조 라벨 600, 제목·숫자 700
pub const MEDIUM: &str = "medium";
pub const SEMIBOLD: &str = "semibold";
pub const BOLD: &str = "bold";

/// 간격 단위
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

/// 카드 모서리 / 위젯 모서리
pub const RADIUS: u8 = 8;
pub const RADIUS_SM: u8 = 6;
/// 카드 안쪽 여백
pub const CARD_PAD: i8 = 14;
/// 글 위주 화면(설정·마법사)의 최대 폭 / 표가 있는 화면(인덱스·서버)의 최대 폭
pub const PAGE_W: f32 = 920.0;
pub const DASH_W: f32 = 1100.0;

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

/// 통계 숫자 (22 굵게)
pub fn ts_stat() -> TextStyle {
    TextStyle::Name("stat".into())
}

/// 검색 입력칸 글꼴
pub fn query_font() -> FontId {
    FontId::new(18.0, FontFamily::Proportional)
}

/// 테마별 색
pub struct Palette {
    pub panel: Color32,
    pub card_fill: Color32,
    pub card_stroke: Color32,
    /// 카드 안 줄무늬·옅은 강조
    pub stripe: Color32,
    /// 움푹 들어간 영역: 로그, 탭 묶음의 바탕
    pub inset_fill: Color32,
    pub accent: Color32,
    /// 강조색 위의 글자
    pub on_accent: Color32,
    /// 선택된 행의 옅은 강조 배경
    pub accent_soft: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    /// 덜 중요한 글자
    pub muted: Color32,
    pub selection_bg: Color32,
    pub selection_fg: Color32,
    /// 검색어 강조 배경
    pub highlight_bg: Color32,
}

static DARK: Palette = Palette {
    panel: Color32::from_gray(27),
    card_fill: Color32::from_gray(34),
    card_stroke: Color32::from_gray(54),
    stripe: Color32::from_gray(40),
    inset_fill: Color32::from_gray(18),
    accent: Color32::from_rgb(88, 150, 255),
    on_accent: Color32::WHITE,
    accent_soft: Color32::from_rgb(34, 52, 84),
    ok: Color32::from_rgb(63, 185, 80),
    warn: Color32::from_rgb(227, 160, 40),
    danger: Color32::from_rgb(248, 81, 73),
    muted: Color32::from_gray(150),
    selection_bg: Color32::from_rgb(36, 74, 130),
    selection_fg: Color32::from_rgb(214, 230, 255),
    highlight_bg: Color32::from_rgb(96, 72, 20),
};

static LIGHT: Palette = Palette {
    panel: Color32::from_gray(246),
    card_fill: Color32::WHITE,
    card_stroke: Color32::from_gray(222),
    stripe: Color32::from_gray(245),
    inset_fill: Color32::from_gray(234),
    accent: Color32::from_rgb(9, 105, 218),
    on_accent: Color32::WHITE,
    accent_soft: Color32::from_rgb(225, 237, 255),
    ok: Color32::from_rgb(26, 127, 55),
    warn: Color32::from_rgb(154, 103, 0),
    danger: Color32::from_rgb(207, 34, 46),
    muted: Color32::from_gray(105),
    selection_bg: Color32::from_rgb(214, 230, 252),
    selection_fg: Color32::from_rgb(20, 76, 160),
    highlight_bg: Color32::from_rgb(255, 236, 170),
};

pub fn palette(v: &Visuals) -> &'static Palette {
    if v.dark_mode {
        &DARK
    } else {
        &LIGHT
    }
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
            Tone::Neutral => v.text_color(),
            Tone::Muted => p.muted,
            Tone::Info => p.accent,
            Tone::Ok => p.ok,
            Tone::Warn => p.warn,
            Tone::Danger => p.danger,
        }
    }

    /// 옅은 배경 색 (안내 상자, 배지)
    pub fn bg(self, v: &Visuals) -> Color32 {
        match self {
            Tone::Neutral | Tone::Muted => palette(v).stripe,
            _ => self.fg(v).gamma_multiply(if v.dark_mode { 0.14 } else { 0.10 }),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tone::Neutral => "•",
            Tone::Muted => "–",
            Tone::Info => "ℹ",
            Tone::Ok => "✔",
            Tone::Warn => "⚠",
            Tone::Danger => "✖",
        }
    }
}

fn apply_palette(v: &mut Visuals, p: &Palette) {
    v.panel_fill = p.panel;
    v.window_fill = p.card_fill;
    v.window_stroke = Stroke::new(1.0, p.card_stroke);
    v.faint_bg_color = p.stripe;
    v.weak_text_color = Some(p.muted);
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    v.selection.bg_fill = p.selection_bg;
    v.selection.stroke = Stroke::new(1.0, p.selection_fg);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
}

/// 앱을 시작할 때 한 번: 두 테마의 스타일을 모두 맞춘다
pub fn install(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        let fam = |name: &str| FontFamily::Name(name.into());
        // 이름 붙인 글자 스타일은 두 테마 모두에 있어야 한다 (없으면 그릴 때 패닉)
        s.text_styles = [
            (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, fam(MEDIUM))),
            (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
            (TextStyle::Heading, FontId::new(20.0, fam(BOLD))),
            (ts_strong(), FontId::new(14.0, fam(SEMIBOLD))),
            (ts_item(), FontId::new(15.0, fam(SEMIBOLD))),
            (ts_section(), FontId::new(16.0, fam(BOLD))),
            (ts_stat(), FontId::new(22.0, fam(BOLD))),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.combo_height = 320.0;
        let r = CornerRadius::same(RADIUS_SM);
        let w = &mut s.visuals.widgets;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = r;
        }
        s.visuals.window_corner_radius = CornerRadius::same(10);
        s.visuals.menu_corner_radius = CornerRadius::same(RADIUS);
    });
    ctx.style_mut_of(Theme::Dark, |s| apply_palette(&mut s.visuals, &DARK));
    ctx.style_mut_of(Theme::Light, |s| apply_palette(&mut s.visuals, &LIGHT));
}
