//! 검색 결과 미리보기 (오른쪽): 일치한 부분을 크게 — 사진, 영상의 그 장면, 문서의 그 구간(검색어 강조)

use crate::app::GemmaApp;
use crate::extract::{Kind, Modality};
use crate::search::query::HitView;
use crate::ui::search_panel::{highlight_job, kind_look, locator_label, open_label};
use crate::ui::widgets;
use crate::util::fmt_ts;
use eframe::egui::{self, RichText};

/// 왜 이 결과가 나왔는지 한 줄로
fn why(hv: &HitView) -> String {
    let base = match (hv.hit.modality, hv.hit.kind) {
        (Modality::Video, _) => format!("{}부터 30초 구간의 장면과 일치", fmt_ts(hv.hit.locator)),
        (Modality::Image, Kind::Video) => format!("{} 장면과 일치", fmt_ts(hv.hit.locator)),
        (Modality::Image, _) => "사진 내용과 일치".to_string(),
        (Modality::Audio, _) => format!("{}부터 들리는 소리와 일치", fmt_ts(hv.hit.locator)),
        (Modality::Text, _) => match locator_label(hv) {
            Some(l) => format!("{l} 내용과 일치"),
            None => "문서 내용과 일치".to_string(),
        },
        (Modality::Name, _) => "파일 이름·폴더 이름과 일치".to_string(),
    };
    if hv.keyword_match() {
        format!("{base} · 검색어 포함")
    } else {
        base
    }
}

/// 큰 미리보기 그림: 사진은 원본, 영상은 일치한 장면 (구간이면 가운데)
pub fn make_preview(
    path: &str,
    kind: Kind,
    modality: Modality,
    locator: i64,
    ffmpeg: Option<&std::path::Path>,
    segment_secs: f64,
) -> Option<egui::ColorImage> {
    let p = std::path::Path::new(path);
    let img = match kind {
        Kind::Image => crate::extract::image::load_image(p, ffmpeg).ok()?,
        Kind::Video => {
            let mut t = locator.max(0) as f64 / 1000.0;
            if modality == Modality::Video {
                t += segment_secs / 2.0;
            }
            let jpeg = crate::extract::media::frame_at(ffmpeg?, p, t).ok()?;
            image::load_from_memory(&jpeg).ok()?
        }
        _ => return None,
    };
    let img = img.thumbnail(1024, 1024).to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

impl GemmaApp {
    pub fn preview_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hv: &HitView, expr: Option<&str>) {
        use crate::ui::theme::{self, Tone};
        use crate::ui::widgets::{Btn, Icon, ScoreStyle};
        // 빠른 검색창은 고른 줄의 미리보기를 늘 보여 주므로 닫기 버튼을 두지 않는다
        // (선택을 풀어도 바로 첫 줄이 다시 선택돼 아무 일도 일어나지 않는다). 큰 창에서만 보인다.
        let show_close = self.quick.is_none();
        // 머리: 종류 타일 · 이름과 경로 · 닫기
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (hue, glyph) = kind_look(hv);
            widgets::kind_tile(ui, hue, glyph, 38.0);
            let close_w = if show_close { theme::CONTROL_H + 10.0 } else { 0.0 };
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - close_w).max(60.0));
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add(egui::Label::new(RichText::new(&hv.file_name).text_style(theme::ts_item()).strong()).truncate());
                ui.add(egui::Label::new(RichText::new(&hv.hit.path).small().weak()).truncate())
                    .on_hover_text(&hv.hit.path);
            });
            if show_close
                && widgets::icon_button(ui, Btn::Ghost, Icon::X, "")
                    .on_hover_text("미리보기 닫기")
                    .clicked()
            {
                self.selected = None;
            }
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            widgets::score_bar(ui, hv.hit.score, ScoreStyle::Labeled).on_hover_text(format!(
                "관련도 {:.1} = 의미 {:.1} + 검색어 {:.1}\n\n관련도는 이 검색에서 대부분의 파일(중앙값)보다 표준편차 몇 배만큼 튀는지입니다.\n어떤 검색어에나 잘 걸리는 파일(밋밋한 스크린샷 등)은 그만큼 빼고 계산합니다.\n대략 7 이상 뚜렷함, 5.5 이상 관련 있음, 그 아래는 약함.\n(코사인 유사도 원래 값 {:.3})",
                hv.hit.score,
                hv.hit.meaning,
                hv.hit.score - hv.hit.meaning,
                hv.hit.cosine
            ));
            widgets::tone_label(ui, Tone::Info, why(hv));
        });
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::icon_button(ui, Btn::Primary, Icon::ExternalLink, &open_label(hv))
                .on_hover_text("Enter")
                .clicked()
            {
                self.open_hit(hv);
            }
            if widgets::icon_button(ui, Btn::Secondary, Icon::FolderOpen, "폴더에서 보기")
                .on_hover_text("Ctrl+Enter")
                .clicked()
            {
                crate::util::reveal_path(&hv.hit.path);
            }
            if widgets::icon_button(ui, Btn::Secondary, Icon::Copy, "파일 복사")
                .on_hover_text("Ctrl+C")
                .clicked()
            {
                self.copy_file(&hv.hit.path);
            }
        });
        ui.add_space(6.0);
        widgets::hairline(ui);
        ui.add_space(4.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // 그림: 사진은 원본에서, 영상은 그 장면을 ffmpeg로 (불러오는 동안은 썸네일)
            let wants_image = matches!(hv.hit.kind, Kind::Image | Kind::Video);
            if wants_image {
                let tex = self
                    .preview_texture(ctx, hv)
                    .or_else(|| hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c)));
                let w = ui.available_width();
                match tex {
                    Some(t) => {
                        let size = t.size_vec2();
                        let scale = (w / size.x).min(520.0 / size.y).min(2.0);
                        ui.add(egui::Image::new(&t).fit_to_exact_size(size * scale).corner_radius(10));
                    }
                    None => {
                        widgets::spinner(ui);
                    }
                }
                if self.preview_loading(hv) {
                    ui.label(RichText::new("큰 그림을 불러오는 중…").small().weak());
                }
            }
            // 글: 문서 구간 전체를 검색어 강조로
            if hv.hit.modality == Modality::Text {
                let text = {
                    let db = self.db.lock();
                    expr.and_then(|e| db.fts_highlight(hv.hit.chunk_id, e))
                        .or_else(|| db.chunk_text(hv.hit.chunk_id))
                };
                ui.add_space(4.0);
                match text {
                    Some(t) => {
                        ui.add(egui::Label::new(highlight_job(ui, &t, 14.0)).wrap());
                    }
                    None => {
                        ui.add(egui::Label::new(RichText::new(&hv.snippet).size(14.0)).wrap());
                        ui.label(
                            RichText::new("(이 문서는 키워드 색인 이전에 인덱싱돼 일부만 보입니다. 다음 인덱싱 때 전체가 들어갑니다)")
                                .small()
                                .weak(),
                        );
                    }
                }
            }
            // 빠른 검색창은 아래에 단축키 안내가 늘 보인다
            if self.quick.is_none() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("↑↓ 이동 · Enter 열기 · Ctrl+Enter 폴더에서 보기 · Ctrl+C 파일 복사 · Esc 검색창")
                        .small()
                        .weak(),
                );
            }
        });
    }
}
