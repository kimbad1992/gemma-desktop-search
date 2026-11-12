//! 테마: 글자 크기·간격·색을 한 곳에서 정한다.
//!
//! 시스템의 어두운/밝은 테마를 그대로 따른다 (egui가 매 프레임 고른다). 여기서는 두 테마의 스타일을
//! 각각 손봐 두기만 한다. 화면 코드는 색 상수 대신 `Tone`·`Hue`와 `palette()`를 쓴다.
//!
//! 모양의 기준(리퀴드 글래스): 앱 아이콘의 남색·청록이 은은하게 번지는 바탕(벽) 위에 유리 패널이 뜬다.
//! 글을 길게 읽는 곳은 진한 내용 유리, 툴바·탭·메뉴는 맑은 유리(`ui::glass`). 버튼·칩·입력칸은 끝까지
//! 둥근 알약 모양, 강조색은 남색→청록 그라데이션(`gradient`), 아이콘은 Lucide 글꼴(`ICONS`).

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

/// 카드(내용 유리) 모서리 / egui 기본 부품(입력칸·체크 상자)의 모서리. 우리 버튼·칩은 끝까지 둥글다.
pub const RADIUS: u8 = 22;
pub const RADIUS_SM: u8 = 10;
/// 카드 안쪽 여백
pub const CARD_PAD: i8 = 16;
/// 버튼·입력칸 높이
pub const CONTROL_H: f32 = 32.0;
/// 글 위주 화면(설정·마법사)의 최대 폭 / 표가 있는 화면(인덱스·서버)의 최대 폭
pub const PAGE_W: f32 = 920.0;
pub const DASH_W: f32 = 1100.0;
/// 왼쪽 메뉴 폭 (넓을 때 / 창이 좁아 아이콘만 보일 때)
pub const NAV_W: f32 = 204.0;
pub const NAV_W_COMPACT: f32 = 64.0;
/// 떠 있는 패널과 창 가장자리 사이 / 패널 사이
pub const EDGE: i8 = 12;
pub const GAP: i8 = 14;

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

/// 테마별 색. 이름이 `g_`로 시작하는 것은 유리 재질(`ui::glass`), 나머지는 글자·부품 색이다.
/// 반투명 색은 벽이나 유리 위에 겹쳐 칠하는 것을 전제로 한다.
pub struct Palette {
    /// 벽의 바닥색 (불투명). 창을 처음 지우는 색, 제목 표시줄 색으로도 쓴다.
    pub panel: Color32,
    /// 벽에 번지는 빛 네 개 (왼쪽 위, 오른쪽 위, 오른쪽 아래, 왼쪽 아래)
    pub blobs: [Color32; 4],
    /// 유리 채우기: 맑음(툴바·탭) / 진함(메뉴·미리보기) / 내용(결과 목록·카드) / 아크릴(다른 프로그램 위에 뜨는 창)
    pub g_clear: Color32,
    pub g_thick: Color32,
    pub g_content: Color32,
    pub g_acrylic: Color32,
    /// 흐림 없이 칠하는 떠 있는 것(펼침 메뉴·도움말·알림)의 바탕: 뒤의 글자가 비치지 않을 만큼 진하게
    pub popup_fill: Color32,
    /// 유리 테두리 1px / 위쪽 안쪽 밝은 선 / 아래쪽 안쪽 옅은 선
    pub g_stroke: Color32,
    pub g_hi: Color32,
    pub g_lo: Color32,
    /// 가장자리 빛 (왼쪽 위 / 오른쪽 아래)
    pub g_rim: Color32,
    pub g_rim2: Color32,
    /// 위쪽 광택 그라데이션의 시작색
    pub g_sheen: Color32,
    /// 유리 그림자 두 겹 (넓고 옅은 것, 좁고 진한 것)
    pub g_shadow: [Shadow; 2],
    /// 선택된 탭·메뉴의 물방울 알약
    pub bead: Color32,
    pub bead_shadow: Shadow,
    /// 카드 안 줄무늬 (표의 홀수 줄)
    pub stripe: Color32,
    /// 움푹 들어간 홈: 로그, 미디어 바탕
    pub inset_fill: Color32,
    /// 입력칸·분할 전환의 홈 / 홈 테두리 / 홈 위쪽 안쪽 그림자
    pub field_fill: Color32,
    pub well_stroke: Color32,
    pub well_in: Color32,
    /// 보통 버튼·칩: 바탕 / 올렸을 때 / 눌렀을 때 / 테두리 / 올렸을 때 테두리
    pub control_fill: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,
    pub control_stroke: Color32,
    pub control_stroke_hover: Color32,
    /// 막대·스위치의 빈 부분
    pub track: Color32,
    /// 카드 안 구분선
    pub card_stroke: Color32,
    /// 글이 놓이는 내용 유리 채우기 (`g_content`와 같다)
    pub card_fill: Color32,
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
    /// 선택된 줄의 옅은 강조 배경 (반투명)
    pub accent_soft: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub selection_bg: Color32,
    pub selection_fg: Color32,
    /// 검색어 강조: 배경 / 글자
    pub highlight_bg: Color32,
    pub highlight_fg: Color32,
    /// 상태 색을 바탕에 섞는 비율 (배지·꼬리표·안내 상자)
    pub tint: f32,
}

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

const fn shadow(y: i8, blur: u8, color: Color32) -> Shadow {
    Shadow {
        offset: [0, y],
        blur,
        spread: 0,
        color,
    }
}

static DARK: Palette = Palette {
    panel: Color32::from_rgb(10, 12, 19),
    blobs: [rgba(98, 114, 255, 107), rgba(25, 194, 181, 66), rgba(214, 92, 150, 51), rgba(124, 92, 255, 71)],
    g_clear: rgba(40, 45, 68, 107),
    g_thick: rgba(24, 28, 44, 153),
    g_content: rgba(15, 18, 28, 194),
    g_acrylic: rgba(20, 23, 36, 189),
    popup_fill: rgba(22, 25, 38, 245),
    g_stroke: rgba(255, 255, 255, 26),
    g_hi: rgba(255, 255, 255, 51),
    g_lo: rgba(255, 255, 255, 10),
    g_rim: rgba(255, 255, 255, 107),
    g_rim2: rgba(255, 255, 255, 31),
    g_sheen: rgba(255, 255, 255, 15),
    g_shadow: [shadow(14, 36, rgba(0, 0, 0, 115)), shadow(1, 2, rgba(0, 0, 0, 102))],
    bead: rgba(255, 255, 255, 33),
    bead_shadow: shadow(2, 10, rgba(0, 0, 0, 89)),
    stripe: rgba(255, 255, 255, 10),
    inset_fill: rgba(0, 0, 0, 77),
    field_fill: rgba(0, 0, 0, 61),
    well_stroke: rgba(255, 255, 255, 15),
    well_in: rgba(0, 0, 0, 82),
    control_fill: rgba(255, 255, 255, 18),
    control_hover: rgba(255, 255, 255, 31),
    control_active: rgba(255, 255, 255, 10),
    control_stroke: rgba(255, 255, 255, 26),
    control_stroke_hover: rgba(255, 255, 255, 56),
    track: rgba(255, 255, 255, 33),
    card_stroke: rgba(255, 255, 255, 20),
    card_fill: rgba(15, 18, 28, 194),
    text: Color32::from_rgb(221, 225, 239),
    text_strong: Color32::from_rgb(245, 246, 251),
    muted: Color32::from_rgb(149, 156, 182),
    accent: Color32::from_rgb(98, 114, 255),
    accent2: Color32::from_rgb(25, 194, 181),
    accent_text: Color32::from_rgb(166, 176, 255),
    on_accent: Color32::WHITE,
    accent_soft: rgba(98, 114, 255, 46),
    ok: Color32::from_rgb(82, 217, 155),
    warn: Color32::from_rgb(244, 177, 76),
    danger: Color32::from_rgb(255, 127, 119),
    selection_bg: Color32::from_rgb(44, 52, 112),
    selection_fg: Color32::from_rgb(222, 226, 255),
    highlight_bg: Color32::from_rgb(19, 70, 73),
    highlight_fg: Color32::from_rgb(140, 240, 229),
    tint: 0.18,
};

static LIGHT: Palette = Palette {
    panel: Color32::from_rgb(231, 235, 246),
    blobs: [rgba(98, 114, 255, 107), rgba(25, 194, 181, 92), rgba(255, 150, 120, 77), rgba(160, 120, 255, 61)],
    g_clear: rgba(255, 255, 255, 112),
    g_thick: rgba(255, 255, 255, 153),
    g_content: rgba(255, 255, 255, 204),
    g_acrylic: rgba(246, 248, 253, 184),
    popup_fill: rgba(250, 251, 254, 247),
    g_stroke: rgba(255, 255, 255, 179),
    g_hi: rgba(255, 255, 255, 242),
    g_lo: rgba(255, 255, 255, 77),
    g_rim: rgba(255, 255, 255, 242),
    g_rim2: rgba(255, 255, 255, 115),
    g_sheen: rgba(255, 255, 255, 87),
    g_shadow: [shadow(12, 32, rgba(38, 48, 110, 33)), shadow(1, 3, rgba(38, 48, 110, 20))],
    bead: rgba(255, 255, 255, 224),
    bead_shadow: shadow(2, 8, rgba(38, 48, 110, 33)),
    stripe: rgba(30, 40, 90, 10),
    inset_fill: rgba(20, 24, 40, 13),
    field_fill: rgba(255, 255, 255, 140),
    well_stroke: rgba(30, 40, 90, 20),
    well_in: rgba(30, 40, 90, 18),
    control_fill: rgba(255, 255, 255, 158),
    control_hover: rgba(255, 255, 255, 214),
    control_active: rgba(255, 255, 255, 115),
    control_stroke: rgba(30, 40, 90, 28),
    control_stroke_hover: rgba(30, 40, 90, 56),
    track: rgba(30, 40, 90, 31),
    card_stroke: rgba(30, 40, 90, 23),
    card_fill: rgba(255, 255, 255, 204),
    text: Color32::from_rgb(38, 42, 59),
    text_strong: Color32::from_rgb(16, 19, 31),
    muted: Color32::from_rgb(88, 96, 120),
    accent: Color32::from_rgb(79, 91, 230),
    accent2: Color32::from_rgb(14, 160, 150),
    accent_text: Color32::from_rgb(59, 70, 207),
    on_accent: Color32::WHITE,
    accent_soft: rgba(79, 91, 230, 33),
    ok: Color32::from_rgb(19, 130, 74),
    warn: Color32::from_rgb(149, 94, 0),
    danger: Color32::from_rgb(201, 42, 42),
    selection_bg: Color32::from_rgb(218, 223, 255),
    selection_fg: Color32::from_rgb(48, 58, 180),
    highlight_bg: Color32::from_rgb(199, 239, 233),
    highlight_fg: Color32::from_rgb(6, 92, 86),
    tint: 0.14,
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

/// 색에 알파를 곱한 반투명 색 (CSS의 `color-mix(in srgb, 색 n%, transparent)`)
pub fn alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, ca] = c.to_srgba_unmultiplied();
    let a = (ca as f32 / 255.0 * a).clamp(0.0, 1.0);
    Color32::from_rgba_unmultiplied(r, g, b, (a * 255.0).round() as u8)
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

    /// 옅은 배경 색 (안내 상자, 배지, 꼬리표): 글자색을 `tint`만큼
    pub fn bg(self, v: &Visuals) -> Color32 {
        let p = palette(v);
        match self {
            Tone::Neutral => p.stripe,
            _ => alpha(self.fg(v), p.tint),
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

    /// 옅은 배경 색 (반투명)
    pub fn bg(self, v: &Visuals) -> Color32 {
        alpha(self.fg(v), palette(v).tint)
    }
}

fn apply_palette(v: &mut Visuals, p: &Palette) {
    // 창을 처음 지우는 색 (그 위에 벽을 칠한다)
    v.panel_fill = p.panel;
    // 펼침 메뉴·도움말·대화상자: 흐림 없이 칠하므로 진하게
    v.window_fill = p.popup_fill;
    v.window_stroke = Stroke::new(1.0, p.g_stroke);
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
    v.window_shadow = p.g_shadow[0];
    v.popup_shadow = Shadow {
        offset: [0, 10],
        blur: 28,
        ..p.g_shadow[0]
    };

    let w = &mut v.widgets;
    // 글자·구분선
    w.noninteractive.bg_fill = p.card_fill;
    w.noninteractive.weak_bg_fill = p.card_fill;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    // egui 기본 버튼·체크박스·드롭다운도 우리 보통 버튼과 같은 색으로
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
        s.visuals.window_corner_radius = CornerRadius::same(20);
        s.visuals.menu_corner_radius = CornerRadius::same(16);
    });
    ctx.style_mut_of(Theme::Dark, |s| apply_palette(&mut s.visuals, &DARK));
    ctx.style_mut_of(Theme::Light, |s| apply_palette(&mut s.visuals, &LIGHT));
}
