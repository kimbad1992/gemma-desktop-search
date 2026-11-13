//! 유리 재질: 색이 번지는 바탕(벽)과 그 위에 뜨는 유리 패널.
//!
//! 유리 한 장은 다섯 겹이다 (시안의 CSS `.g`와 같은 순서):
//! ① 뒤 흐림 + 채도, ② 반투명 채우기, ③ 위쪽 광택, ④ 테두리·위아래 안쪽 1px 선·대각선 가장자리 빛, ⑤ 그림자.
//!
//! ①은 매 프레임 흐리지 않는다. 유리 뒤에 있는 것은 거의 다 움직이지 않는 벽이라, 벽을 미리 흐리고
//! 채도를 올린 텍스처를 한 장 만들어 두고 패널 자리만큼 잘라(UV) 깐다. 결과는 진짜 흐림과 같다.
//! 움직이는 내용 위에 뜨는 것(펼침 메뉴·도움말)은 흐리지 않는 대신 진한 색(`Palette::popup_fill`)으로 칠한다.
//!
//! 벽은 원형 그라데이션 네 개를 바닥색 위에 겹친 것이다. 창 크기와 상관없이 비율로 정해지므로
//! 작은 텍스처 한 장을 창 크기로 늘려 쓴다 (테마마다 한 번 만든다).

use crate::ui::theme::{palette_of, Palette};
use eframe::egui::{self, epaint, Color32, Margin, Pos2, Rect, Shape, Vec2};

/// 창을 투명하게 만들어 OS 바탕(Windows 11의 Acrylic)을 쓸 수 있는지. main.rs가 창을 만들 때 정하고,
/// 첫 프레임에 실제 렌더러가 DX12가 아니면(투명하게 내보낼 수 없으면) 끈다 (`app.rs`).
pub static OS_BACKDROP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 유리의 종류 (채우기만 다르다)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glass {
    /// 맑은 유리: 툴바, 탭 막대처럼 글이 짧은 조작 부품
    Clear,
    /// 진한 유리: 왼쪽 메뉴, 미리보기
    Thick,
    /// 내용 유리: 결과 목록, 카드, 표처럼 글을 길게 읽는 곳
    Content,
    /// 아크릴: 다른 프로그램 위에 뜨는 창 (빠른 검색창, 알림 창)
    Acrylic,
}

impl Glass {
    pub fn fill(self, p: &Palette) -> Color32 {
        match self {
            Glass::Clear => p.g_clear,
            Glass::Thick => p.g_thick,
            Glass::Content => p.g_content,
            Glass::Acrylic => p.g_acrylic,
        }
    }

    /// 뒤를 얼마나 흐리는지 (CSS px). 아크릴은 더 세게.
    fn blur(self) -> f32 {
        if self == Glass::Acrylic {
            30.0
        } else {
            22.0
        }
    }

    fn saturate(self) -> f32 {
        if self == Glass::Acrylic {
            1.6
        } else {
            1.8
        }
    }
}

// ── 벽 ──

/// 벽 텍스처의 크기. 창 크기로 늘려 쓴다 (부드러운 그라데이션이라 작아도 된다).
const WALL_W: usize = 384;
const WALL_H: usize = 240;
/// 흐림 반경을 텍스처 픽셀로 바꿀 때 기준으로 삼는 창 크기 (설계 폭 1240 × 860 근처)
const REF_W: f32 = 1280.0;
const REF_H: f32 = 860.0;

/// 번짐 하나: 가운데(비율), 반지름(창 폭·높이의 비율), 투명해지는 지점(반지름 대비)
struct Blob {
    at: (f32, f32),
    radius: (f32, f32),
    stop: f32,
}

/// 시안의 `.wallbg`: 왼쪽 위 / 오른쪽 위 / 오른쪽 아래 / 왼쪽 아래 (`Palette::blobs` 순서)
const BLOBS: [Blob; 4] = [
    Blob { at: (0.08, 0.10), radius: (0.55, 0.65), stop: 0.70 },
    Blob { at: (0.92, 0.12), radius: (0.50, 0.60), stop: 0.70 },
    Blob { at: (0.78, 1.00), radius: (0.55, 0.60), stop: 0.72 },
    Blob { at: (0.18, 0.95), radius: (0.45, 0.55), stop: 0.72 },
];

/// 벽 그림 (sRGB 0~1, 불투명)
fn wall_pixels(p: &Palette) -> Vec<[f32; 3]> {
    let base = rgb(p.panel);
    let mut out = Vec::with_capacity(WALL_W * WALL_H);
    for y in 0..WALL_H {
        let fy = (y as f32 + 0.5) / WALL_H as f32;
        for x in 0..WALL_W {
            let fx = (x as f32 + 0.5) / WALL_W as f32;
            let mut c = base;
            // 맨 아래 번짐부터 겹친다 (CSS에서 먼저 쓴 것이 위)
            for (blob, color) in BLOBS.iter().zip(p.blobs.iter()).rev() {
                let dx = (fx - blob.at.0) / blob.radius.0;
                let dy = (fy - blob.at.1) / blob.radius.1;
                let t = ((dx * dx + dy * dy).sqrt() / blob.stop).min(1.0);
                let a = color.a() as f32 / 255.0 * (1.0 - t);
                let src = rgb(*color);
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - a) + src[i] * a;
                }
            }
            out.push(c);
        }
    }
    out
}

fn rgb(c: Color32) -> [f32; 3] {
    // 알파를 뺀 색 (unmultiplied)
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

/// 가로·세로 가우스 흐림 (가장자리는 끝 픽셀을 늘린 것으로 본다, CSS backdrop-filter와 같다)
fn blur(px: &[[f32; 3]], sx: f32, sy: f32) -> Vec<[f32; 3]> {
    let kernel = |s: f32| -> Vec<f32> {
        let r = (s * 3.0).ceil() as i32;
        let k: Vec<f32> = (-r..=r).map(|i| (-(i * i) as f32 / (2.0 * s * s)).exp()).collect();
        let sum: f32 = k.iter().sum();
        k.into_iter().map(|v| v / sum).collect()
    };
    let pass = |src: &[[f32; 3]], k: &[f32], horizontal: bool| -> Vec<[f32; 3]> {
        let r = (k.len() / 2) as i32;
        let mut dst = vec![[0.0; 3]; src.len()];
        for y in 0..WALL_H as i32 {
            for x in 0..WALL_W as i32 {
                let mut acc = [0.0; 3];
                for (j, w) in k.iter().enumerate() {
                    let o = j as i32 - r;
                    let (sx, sy) = if horizontal {
                        ((x + o).clamp(0, WALL_W as i32 - 1), y)
                    } else {
                        (x, (y + o).clamp(0, WALL_H as i32 - 1))
                    };
                    let s = src[sy as usize * WALL_W + sx as usize];
                    for i in 0..3 {
                        acc[i] += s[i] * w;
                    }
                }
                dst[y as usize * WALL_W + x as usize] = acc;
            }
        }
        dst
    };
    let h = pass(px, &kernel(sx), true);
    pass(&h, &kernel(sy), false)
}

/// CSS `saturate(s)` (sRGB 공간의 색 행렬)
fn saturate(c: [f32; 3], s: f32) -> [f32; 3] {
    let [r, g, b] = c;
    [
        (0.213 + 0.787 * s) * r + (0.715 - 0.715 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 + 0.285 * s) * g + (0.072 - 0.072 * s) * b,
        (0.213 - 0.213 * s) * r + (0.715 - 0.715 * s) * g + (0.072 + 0.928 * s) * b,
    ]
}

/// 0~1 색을 8비트로. 넓고 완만한 그라데이션이 띠지지 않게 아주 옅게 흩뿌린다.
fn to_image(px: &[[f32; 3]]) -> egui::ColorImage {
    let mut bytes = Vec::with_capacity(px.len() * 4);
    for (i, c) in px.iter().enumerate() {
        // 픽셀마다 정해진 작은 흔들림 (-0.5 ~ +0.5 단계)
        let h = (i as u32).wrapping_mul(2_654_435_761) >> 16;
        let d = (h & 0xff) as f32 / 255.0 - 0.5;
        for v in c {
            bytes.push((v * 255.0 + d).round().clamp(0.0, 255.0) as u8);
        }
        bytes.push(255);
    }
    egui::ColorImage::from_rgba_unmultiplied([WALL_W, WALL_H], &bytes)
}

/// 테마별 텍스처: 벽, 흐린 벽 (맑은·진한·내용 유리용), 더 흐린 벽 (아크릴용)
#[derive(Clone)]
struct WallTextures {
    wall: egui::TextureHandle,
    blurred: egui::TextureHandle,
    acrylic: egui::TextureHandle,
}

fn wall_textures(ctx: &egui::Context, dark: bool) -> WallTextures {
    let id = egui::Id::unique(("glass-wall", dark));
    if let Some(t) = ctx.data(|d| d.get_temp::<WallTextures>(id)) {
        return t;
    }
    let p = palette_of(dark);
    let wall = wall_pixels(p);
    let backdrop = |g: Glass| -> Vec<[f32; 3]> {
        let s = g.blur();
        blur(&wall, s * WALL_W as f32 / REF_W, s * WALL_H as f32 / REF_H)
            .into_iter()
            .map(|c| saturate(c, g.saturate()))
            .collect()
    };
    let opts = egui::TextureOptions::LINEAR;
    let t = WallTextures {
        wall: ctx.load_texture(format!("glass-wall-{dark}"), to_image(&wall), opts),
        blurred: ctx.load_texture(format!("glass-wall-blur-{dark}"), to_image(&backdrop(Glass::Thick)), opts),
        acrylic: ctx.load_texture(format!("glass-wall-acrylic-{dark}"), to_image(&backdrop(Glass::Acrylic)), opts),
    };
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// 위쪽 광택: 흰색이 위에서 46% 지점까지 투명해지는 세로 띠 (색은 칠할 때 입힌다)
fn sheen_texture(ctx: &egui::Context) -> egui::TextureId {
    let id = egui::Id::unique("glass-sheen");
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return t.id();
    }
    const N: usize = 64;
    let px: Vec<Color32> = (0..N)
        .map(|i| {
            let y = i as f32 / (N - 1) as f32;
            let a = (1.0 - y / 0.46).clamp(0.0, 1.0);
            Color32::from_white_alpha((a * 255.0).round() as u8)
        })
        .collect();
    let tex = ctx.load_texture("glass-sheen", egui::ColorImage::new([1, N], px), egui::TextureOptions::LINEAR);
    let tid = tex.id();
    ctx.data_mut(|d| d.insert_temp(id, tex));
    tid
}

/// 창 전체 (벽이 깔리는 자리)
fn wall_rect(ctx: &egui::Context) -> Rect {
    ctx.content_rect()
}

/// 벽을 칠한다. 창마다 프레임의 맨 처음에 (다른 것보다 먼저) 부른다.
pub fn paint_wall(ui: &egui::Ui) {
    let ctx = ui.ctx();
    let rect = wall_rect(ctx);
    let tex = wall_textures(ctx, ui.visuals().dark_mode).wall.id();
    let full = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
    ui.painter()
        .add(epaint::RectShape::filled(rect, 0.0, Color32::WHITE).with_texture(tex, full));
}

// ── 유리 한 장 ──

/// 145도 대각선 가장자리 빛의 색: 왼쪽 위가 밝고(0~28%), 가운데는 없고, 오른쪽 아래가 은은하다(70~100%)
fn rim_color(bounds: Rect, pos: Pos2, rim: Color32, rim2: Color32) -> Color32 {
    let a = 145f32.to_radians();
    let dir = Vec2::new(a.sin(), -a.cos());
    let len = (bounds.width() * a.sin()).abs() + (bounds.height() * a.cos()).abs();
    let t = ((pos - bounds.center()).dot(dir) / len.max(1.0) + 0.5).clamp(0.0, 1.0);
    if t < 0.28 {
        rim.gamma_multiply(1.0 - t / 0.28)
    } else if t > 0.70 {
        rim2.gamma_multiply((t - 0.70) / 0.30)
    } else {
        Color32::TRANSPARENT
    }
}

/// 둥근 사각형의 위쪽(또는 아래쪽) 반 바퀴: 왼쪽 → 꼭대기 → 오른쪽
fn half_loop(rect: Rect, r: f32, top: bool) -> Vec<Pos2> {
    use epaint::tessellator::path::add_circle_quadrant;
    let r = r.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut pts = Vec::new();
    if top {
        add_circle_quadrant(&mut pts, egui::pos2(rect.left() + r, rect.top() + r), r, 2.0);
        add_circle_quadrant(&mut pts, egui::pos2(rect.right() - r, rect.top() + r), r, 3.0);
    } else {
        add_circle_quadrant(&mut pts, egui::pos2(rect.right() - r, rect.bottom() - r), r, 0.0);
        add_circle_quadrant(&mut pts, egui::pos2(rect.left() + r, rect.bottom() - r), r, 1.0);
    }
    pts.dedup_by(|a, b| a.distance(*b) < 0.01);
    pts
}

/// 위·아래 안쪽 1px 선: 가운데가 가장 진하고 모서리를 돌아 옆으로 가면서 사라진다
/// (CSS의 `inset 0 1px 0`, `inset 0 -1px 0` 그림자)
fn edge_line(rect: Rect, r: f32, top: bool, color: Color32) -> Shape {
    let pts = half_loop(rect, r, top);
    let fade = (r * 0.6).max(1.0);
    let (y0, sign) = if top { (rect.top(), 1.0) } else { (rect.bottom(), -1.0) };
    let stroke = epaint::PathStroke::new_uv(1.0, move |_, p: Pos2| {
        let d = ((p.y - y0) * sign).max(0.0);
        color.gamma_multiply((1.0 - d / fade).clamp(0.0, 1.0))
    });
    Shape::Path(epaint::PathShape::line(pts, stroke))
}

/// 부품용: 둥근 사각형 `rect`의 위(또는 아래) 안쪽 1px 빛 (테두리 없는 버튼·칩·타일의 `inset 0 1px 0`)
pub fn inner_light(rect: Rect, radius: f32, top: bool, color: Color32) -> Shape {
    edge_line(rect.shrink(0.5), (radius - 0.5).max(0.0), top, color)
}

/// 부품용: 위쪽 광택 (위에서 46%까지 `color`에서 투명으로)
pub fn sheen(ctx: &egui::Context, rect: Rect, radius: f32, color: Color32) -> Shape {
    let full = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
    epaint::RectShape::filled(rect, radius, color)
        .with_texture(sheen_texture(ctx), full)
        .into()
}

/// 선택 표시인 물방울 알약 (`.bead`): 진한 흰 채우기 + 위 빛 + 옅은 테두리 + 그림자
pub fn bead(ui: &egui::Ui, rect: Rect, radius: f32) {
    let p = palette_of(ui.visuals().dark_mode);
    let painter = ui.painter();
    painter.add(p.bead_shadow.as_shape(rect, radius));
    painter.rect(rect, radius, p.bead, egui::Stroke::new(1.0, p.g_stroke), egui::StrokeKind::Inside);
    painter.add(inner_light(rect, radius, true, p.g_hi));
}

/// 움푹 들어간 홈 (`.well`): 입력칸, 분할 전환의 바탕. 위쪽 안에 옅은 그림자.
pub fn well_shape(dark: bool, rect: Rect, radius: f32) -> Shape {
    let p = palette_of(dark);
    let stroke = egui::Stroke::new(1.0, p.well_stroke);
    Shape::Vec(vec![
        epaint::RectShape::new(rect, radius, p.field_fill, stroke, egui::StrokeKind::Inside).into(),
        inner_light(rect.translate(egui::vec2(0.0, 0.5)), radius, true, p.well_in),
    ])
}

/// 유리 밑에 깔리는 것 (① 뒤 흐림)
enum Under {
    /// 미리 흐린 벽을 이 자리만큼 (바탕 위에 뜬 유리)
    Wall(Glass),
    /// 이번 프레임에 실제로 흐린 밑의 층 (`ui::backdrop`)
    Live(egui::TextureId, Rect),
    /// 흐리지 않는다 (채우기가 진하다)
    Nothing,
}

/// 유리 한 장의 도형들 (`rect`는 테두리 바깥선)
pub fn shapes(ctx: &egui::Context, dark: bool, rect: Rect, radius: f32, kind: Glass) -> Vec<Shape> {
    layers(ctx, dark, rect, radius, Under::Wall(kind), kind.fill(palette_of(dark)))
}

fn layers(ctx: &egui::Context, dark: bool, rect: Rect, radius: f32, under: Under, fill: Color32) -> Vec<Shape> {
    let p = palette_of(dark);
    let radius = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut out = Vec::with_capacity(9);
    // ⑤ 그림자
    for s in p.g_shadow {
        out.push(s.as_shape(rect, radius).into());
    }
    // ① 뒤 흐림
    match under {
        Under::Wall(kind) => {
            let wall = wall_rect(ctx);
            let tex = wall_textures(ctx, dark);
            let tex = if kind == Glass::Acrylic { tex.acrylic.id() } else { tex.blurred.id() };
            let uv = Rect::from_min_max(
                ((rect.min - wall.min) / wall.size()).to_pos2(),
                ((rect.max - wall.min) / wall.size()).to_pos2(),
            );
            out.push(epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex, uv).into());
        }
        Under::Live(tex, uv) => {
            out.push(epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(tex, uv).into());
        }
        Under::Nothing => {}
    }
    // ② 채우기
    out.push(epaint::RectShape::filled(rect, radius, fill).into());
    // ③ 위쪽 광택
    let full = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
    out.push(
        epaint::RectShape::filled(rect, radius, p.g_sheen)
            .with_texture(sheen_texture(ctx), full)
            .into(),
    );
    // ④ 테두리, 안쪽 선, 가장자리 빛
    let ring = rect.shrink(0.5);
    let border = egui::Stroke::new(1.0, p.g_stroke);
    out.push(epaint::RectShape::stroke(rect, radius, border, egui::StrokeKind::Inside).into());
    let inner = rect.shrink(1.5);
    let ir = (radius - 1.5).max(0.0);
    out.push(edge_line(inner, ir, true, p.g_hi));
    out.push(edge_line(inner, ir, false, p.g_lo));
    let mut pts = Vec::new();
    epaint::tessellator::path::rounded_rectangle(&mut pts, epaint::RoundedRect::new(ring, (radius - 0.5).max(0.0)));
    let (rim, rim2) = (p.g_rim, p.g_rim2);
    out.push(Shape::Path(epaint::PathShape::closed_line(
        pts,
        epaint::PathStroke::new_uv(1.0, move |b, pos| rim_color(b, pos, rim, rim2)),
    )));
    out
}

/// `rect` 자리에 유리 한 장을 칠한다 (그 위에 그릴 내용보다 먼저)
pub fn paint(ui: &egui::Ui, rect: Rect, radius: f32, kind: Glass) {
    let shapes = shapes(ui.ctx(), ui.visuals().dark_mode, rect, radius, kind);
    ui.painter().add(Shape::Vec(shapes));
}

/// 패널의 틀 안에서 부른다: 지금 영역(`ui.max_rect()`)에 안쪽 여백 `margin`을 더한 자리를 유리로.
/// 내용을 놓기 전에 불러야 내용 밑에 깔린다.
pub fn backdrop(ui: &egui::Ui, kind: Glass, radius: f32, margin: Margin) {
    let rect = ui.max_rect() + margin;
    paint(ui, rect, radius, kind);
}

/// `backdrop`과 같되 뒤의 흐린 벽을 깔지 않는다. OS가 창 뒤를 흐려 비추는 창(Acrylic) 안에서 쓴다.
pub fn backdrop_plain(ui: &egui::Ui, kind: Glass, radius: f32, margin: Margin) {
    let rect = ui.max_rect() + margin;
    let dark = ui.visuals().dark_mode;
    let shapes = layers(ui.ctx(), dark, rect, radius, Under::Nothing, kind.fill(palette_of(dark)));
    ui.painter().add(Shape::Vec(shapes));
}

/// 내용을 유리 한 장 위에 놓는다 (`egui::Frame::show`처럼). 응답의 `rect`가 유리 자리다.
pub fn show<R>(
    ui: &mut egui::Ui,
    kind: Glass,
    radius: f32,
    margin: Margin,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let slot = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::NONE.inner_margin(margin).show(ui, add);
    let shapes = shapes(ui.ctx(), ui.visuals().dark_mode, inner.response.rect, radius, kind);
    ui.painter().set(slot, Shape::Vec(shapes));
    inner
}

/// 움직이는 내용 위에 뜨는 진한 유리 (펼침 메뉴): 밑의 층을 이번 프레임에 실제로 흐려 깐다
/// (`ui::backdrop`). 흐릴 수 없으면(그래픽 오류 등) 뒤의 글자가 비치지 않게 진한 색으로.
pub fn show_floating<R>(
    ui: &mut egui::Ui,
    radius: f32,
    margin: Margin,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let slot = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::NONE.inner_margin(margin).show(ui, add);
    let rect = inner.response.rect;
    let dark = ui.visuals().dark_mode;
    let p = palette_of(dark);
    let (under, fill) = match crate::ui::backdrop::live(ui.ctx(), ui.layer_id(), rect) {
        Some((tex, uv)) => (Under::Live(tex, uv), p.g_thick),
        None => (Under::Nothing, p.popup_fill),
    };
    let shapes = layers(ui.ctx(), dark, rect, radius, under, fill);
    ui.painter().set(slot, Shape::Vec(shapes));
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_is_lit_in_the_corners_and_plain_in_the_middle_top() {
        let p = palette_of(false);
        let px = wall_pixels(p);
        let at = |fx: f32, fy: f32| px[(fy * (WALL_H - 1) as f32) as usize * WALL_W + (fx * (WALL_W - 1) as f32) as usize];
        let base = rgb(p.panel);
        // 위 가운데는 번짐이 닿지 않아 바닥색 그대로
        let mid = at(0.5, 0.0);
        assert!((0..3).all(|i| (mid[i] - base[i]).abs() < 0.01), "{mid:?} vs {base:?}");
        // 왼쪽 위는 남색 쪽으로 (파랑이 빨강보다 진하다)
        let tl = at(0.08, 0.10);
        assert!(tl[2] > tl[0] + 0.05, "{tl:?}");
    }

    #[test]
    fn saturate_keeps_gray_and_one_is_identity() {
        let g = saturate([0.5, 0.5, 0.5], 1.8);
        assert!(g.iter().all(|v| (v - 0.5).abs() < 1e-4));
        let c = [0.2, 0.4, 0.9];
        let same = saturate(c, 1.0);
        assert!((0..3).all(|i| (same[i] - c[i]).abs() < 1e-5));
    }

    #[test]
    fn rim_is_bright_top_left_and_faint_bottom_right() {
        let b = Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 100.0));
        let (rim, rim2) = (Color32::WHITE, Color32::from_white_alpha(115));
        assert_eq!(rim_color(b, b.left_top(), rim, rim2).a(), 255);
        assert_eq!(rim_color(b, b.center(), rim, rim2).a(), 0);
        let br = rim_color(b, b.right_bottom(), rim, rim2).a();
        assert!(br > 0 && br < 255, "{br}");
    }
}
