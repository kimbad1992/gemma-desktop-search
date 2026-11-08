//! 검색 탭: 질의 입력, 섹션 필터, 결과(섹션별 Top-K, 썸네일, 타임스탬프)

use crate::app::GemmaApp;
use crate::extract::{Kind, Modality};
use crate::search::query::{HitView, QueryKind};
use crate::search::store::Section;
use crate::ui::widgets;
use crate::util::{fmt_num, fmt_ts};
use eframe::egui::{self, RichText};
use std::sync::atomic::Ordering;

const THUMB: f32 = 72.0;
const CARD_W: f32 = 168.0;

/// 점수 내림차순으로 정렬된 결과에서 최소 일치도 이상인 앞부분
fn passing(hits: &[HitView], min: f32) -> &[HitView] {
    let n = hits.iter().take_while(|h| h.hit.score >= min).count();
    &hits[..n]
}

impl GemmaApp {
    pub fn search_tab(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let running = self.search_rx.is_some();

        // ── 질의 입력 ──
        ui.add_space(4.0);
        let mut submit = false;
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 250.0).max(200.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("찾고 싶은 내용을 자연어로 (예: 바다가 보이는 사진, 예산 회의록, 고양이 영상)")
                    .font(egui::FontId::proportional(18.0))
                    .desired_width(w)
                    .margin(egui::vec2(8.0, 6.0)),
            );
            if self.focus_search {
                resp.request_focus();
                self.focus_search = false;
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
                resp.request_focus();
            }
            if ui
                .add_enabled(!running, egui::Button::new(RichText::new("검색").size(16.0)))
                .clicked()
            {
                submit = true;
            }
            if ui
                .add_enabled(!running, egui::Button::new("이미지로 검색…"))
                .on_hover_text("이미지 파일을 고르거나 창에 끌어다 놓으면 비슷한 이미지·영상을 찾습니다")
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("이미지", &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff", "heic", "avif"])
                    .pick_file()
                {
                    self.start_search(QueryKind::Image(p));
                }
            }
        });
        if submit && !running {
            let q = self.query.clone();
            self.start_search(QueryKind::Text(q));
        }

        // ── 필터 ──
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("보기:").weak());
            for s in Section::ALL {
                ui.checkbox(&mut self.filter_sections[s as usize], s.label());
            }
            ui.separator();
            ui.label(RichText::new("경로 포함:").weak());
            ui.add(
                egui::TextEdit::singleline(&mut self.path_filter)
                    .hint_text("예: 사진 2024")
                    .desired_width(180.0),
            );
            ui.separator();
            ui.label(RichText::new("최소 일치도:").weak());
            let mut v = self.cfg.min_score;
            let r = ui
                .add(
                    egui::Slider::new(&mut v, 0.0..=0.95)
                        .step_by(0.01)
                        .fixed_decimals(2),
                )
                .on_hover_text(
                    "이 점수보다 낮은 결과를 숨깁니다 (다시 검색하지 않고 바로 적용).\n대체로 0.7 이상이면 잘 맞는 결과입니다.",
                );
            if r.changed() {
                self.cfg.min_score = v;
            }
            let mut persist = r.drag_stopped() || (r.changed() && !r.dragged());
            if self.cfg.min_score > 0.0 && ui.small_button("끄기").clicked() {
                self.cfg.min_score = 0.0;
                persist = true;
            }
            if persist {
                self.persist_min_score();
            }
        });

        // ── 상태 줄 ──
        ui.add_space(4.0);
        let server_ready = self.server.state().is_ready();
        let (files, chunks) = {
            let st = self.store.read();
            (st.file_count(), st.live_chunks())
        };
        ui.horizontal_wrapped(|ui| {
            if self.store_loading.load(Ordering::SeqCst) {
                ui.spinner();
                ui.label("인덱스를 불러오는 중…");
            } else if !server_ready {
                widgets::badge(ui, widgets::ORANGE, "서버가 준비되지 않았습니다", false, false);
                if !self.server.state().is_running() && ui.button("서버 시작").clicked() {
                    self.start_server();
                }
            } else if running {
                ui.spinner();
                let secs = self.search_started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                ui.label(RichText::new(format!("검색 중… {secs:.1}초")).size(15.0));
            } else if let Some(o) = &self.outcome {
                if let Some(e) = &o.error {
                    ui.colored_label(widgets::RED, format!("검색 실패: {e}"));
                } else {
                    let min = self.cfg.min_score;
                    let counts: Vec<(Section, usize)> = o
                        .sections
                        .iter()
                        .map(|(s, v)| (*s, passing(v, min).len()))
                        .collect();
                    let shown: usize = counts.iter().map(|(_, n)| n).sum();
                    let text = if min > 0.0 {
                        format!("검색 완료 · {shown}개 파일 (일치도 {min:.2} 이상)")
                    } else {
                        format!("검색 완료 · {shown}개 파일")
                    };
                    widgets::badge(ui, widgets::BLUE, &text, false, false);
                    ui.label(RichText::new(&o.label).strong());
                    ui.label(
                        counts
                            .iter()
                            .map(|(s, n)| format!("{} {n}", s.label()))
                            .collect::<Vec<_>>()
                            .join(" · "),
                    );
                    ui.label(
                        RichText::new(format!(
                            "({:.2}초, 벡터 {}개 비교)",
                            o.elapsed.as_secs_f32(),
                            fmt_num(o.searched_chunks as u64)
                        ))
                        .weak(),
                    );
                }
            }
            if files == 0 && !self.store_loading.load(Ordering::SeqCst) {
                ui.colored_label(
                    widgets::ORANGE,
                    "인덱스가 비어 있습니다 — 설정에서 폴더를 추가하고 인덱스 탭에서 인덱싱하세요",
                );
            } else {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("파일 {} · 벡터 {}", fmt_num(files as u64), fmt_num(chunks as u64)))
                            .weak()
                            .small(),
                    );
                });
            }
        });

        // 끌어다 놓기 안내
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            ui.add_space(6.0);
            widgets::badge(ui, widgets::BLUE, "이미지를 놓으면 비슷한 이미지·영상을 찾습니다", true, false);
        }

        ui.separator();

        // ── 결과 ──
        let Some(outcome) = self.outcome.take() else {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("문서 · 이미지 · 영상 · 오디오를 한 번에 의미로 검색합니다").size(16.0).weak());
                ui.label(RichText::new("한국어/영어 모두 가능 · 이미지를 창에 끌어다 놓으면 이미지로 검색").weak());
            });
            return;
        };
        // 섹션 칩: 전체 요약 / 섹션별 전체 보기 (최소 일치도 미만은 제외)
        let min = self.cfg.min_score;
        let visible: Vec<(Section, &[HitView])> = outcome
            .sections
            .iter()
            .map(|(s, h)| (*s, passing(h, min)))
            .filter(|(s, h)| !h.is_empty() && self.filter_sections[*s as usize])
            .collect();
        if let Some(v) = self.result_view {
            if !visible.iter().any(|(s, _)| *s == v) {
                self.result_view = None;
            }
        }
        if !visible.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let total: usize = visible.iter().map(|(_, h)| h.len()).sum();
                if ui
                    .selectable_label(self.result_view.is_none(), RichText::new(format!("전체 요약 {total}")).size(14.0))
                    .clicked()
                {
                    self.result_view = None;
                }
                for (sec, hits) in &visible {
                    let sel = self.result_view == Some(*sec);
                    if ui
                        .selectable_label(sel, RichText::new(format!("{} {}", sec.label(), hits.len())).size(14.0))
                        .clicked()
                    {
                        self.result_view = Some(*sec);
                    }
                }
            });
            ui.add_space(4.0);
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match self.result_view {
                None => {
                    for (sec, hits) in &visible {
                        let limit = if is_media(*sec) { 8 } else { 3 };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(sec.label()).size(16.0).strong());
                            ui.label(RichText::new(format!("{}개", hits.len())).weak());
                            if hits.len() > limit && ui.link(format!("모두 보기 ›")).clicked() {
                                self.result_view = Some(*sec);
                            }
                        });
                        self.render_hits(ui, &ctx, *sec, &hits[..hits.len().min(limit)]);
                        ui.add_space(10.0);
                    }
                }
                Some(sec) => {
                    if let Some((_, hits)) = visible.iter().find(|(s, _)| *s == sec) {
                        self.render_hits(ui, &ctx, sec, hits);
                    }
                }
            }
            if visible.is_empty() && outcome.error.is_none() {
                if min > 0.0 && outcome.total_files() > 0 {
                    ui.label(format!(
                        "일치도 {min:.2} 이상인 결과가 없습니다. 최소 일치도를 낮춰 보세요."
                    ));
                } else {
                    ui.label("결과가 없습니다. 필터를 확인하세요.");
                }
            }
        });
        self.outcome = Some(outcome);
    }

    fn render_hits(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sec: Section, hits: &[HitView]) {
        if is_media(sec) {
            // 카드 폭을 알고 있으니 한 줄에 들어갈 개수를 계산해 줄 단위로 배치
            let gap = ui.spacing().item_spacing.x;
            let cols = ((ui.available_width() + gap) / (CARD_W + 12.0 + gap)).floor().max(1.0) as usize;
            for row in hits.chunks(cols) {
                ui.horizontal(|ui| {
                    for hv in row {
                        self.media_card(ui, ctx, hv);
                    }
                });
            }
        } else {
            for hv in hits {
                self.result_row(ui, ctx, hv);
            }
        }
    }

    /// 이미지·영상용 썸네일 카드
    fn media_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hv: &HitView) {
        const W: f32 = CARD_W;
        const H: f32 = 118.0;
        let tex = hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c));
        let resp = egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(6)
            .inner_margin(6)
            .show(ui, |ui| {
                ui.set_width(W);
                ui.vertical(|ui| {
                    match &tex {
                        Some(t) => {
                            ui.add_sized([W, H], egui::Image::new(t).max_size(egui::vec2(W, H)).corner_radius(4));
                        }
                        None => {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(W, H), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                kind_tag(hv.hit.kind),
                                egui::FontId::proportional(13.0),
                                ui.visuals().weak_text_color(),
                            );
                        }
                    }
                    ui.add(egui::Label::new(RichText::new(&hv.file_name).strong()).truncate());
                    ui.horizontal(|ui| {
                        let score = hv.hit.score;
                        ui.add(
                            egui::ProgressBar::new(score.clamp(0.0, 1.0))
                                .desired_width(56.0)
                                .desired_height(14.0)
                                .fill(score_color(score))
                                .text(RichText::new(format!("{score:.3}")).size(10.5)),
                        );
                        if let Some(loc) = locator_label(hv) {
                            ui.label(RichText::new(loc).size(11.5).color(widgets::BLUE));
                        }
                    });
                });
            })
            .response
            .interact(egui::Sense::click());
        if resp.double_clicked() {
            crate::util::open_path(&hv.hit.path);
        }
        resp.context_menu(|ui| {
            if ui.button("열기").clicked() {
                crate::util::open_path(&hv.hit.path);
                ui.close();
            }
            if ui.button("폴더에서 보기").clicked() {
                crate::util::reveal_path(&hv.hit.path);
                ui.close();
            }
            if ui.button("경로 복사").clicked() {
                ctx.copy_text(hv.hit.path.clone());
                ui.close();
            }
        });
        resp.on_hover_text(format!("{}\n더블클릭: 열기 · 우클릭: 메뉴", hv.hit.path));
    }

    fn result_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, hv: &HitView) {
        let frame = egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(8, 6));
        let resp = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // 썸네일 또는 종류 아이콘
                    let tex = hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c));
                    match tex {
                        Some(t) => {
                            ui.add(egui::Image::new(&t).max_size(egui::vec2(THUMB, THUMB)).corner_radius(4));
                        }
                        None => {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(THUMB, 40.0), egui::Sense::hover());
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                kind_tag(hv.hit.kind),
                                egui::FontId::proportional(13.0),
                                ui.visuals().weak_text_color(),
                            );
                        }
                    }
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let score = hv.hit.score;
                            ui.add(
                                egui::ProgressBar::new(score.clamp(0.0, 1.0))
                                    .desired_width(64.0)
                                    .desired_height(16.0)
                                    .fill(score_color(score))
                                    .text(RichText::new(format!("{score:.3}")).size(11.0)),
                            );
                            ui.label(RichText::new(&hv.file_name).strong().size(15.0));
                            if let Some(loc) = locator_label(hv) {
                                widgets::badge(ui, widgets::BLUE, &loc, false, false);
                            }
                        });
                        ui.add(egui::Label::new(RichText::new(&hv.hit.path).small().weak()).truncate());
                        if !hv.snippet.is_empty() && hv.hit.modality == Modality::Text {
                            ui.add(egui::Label::new(RichText::new(&hv.snippet).size(13.0)).wrap());
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("열기").clicked() {
                                crate::util::open_path(&hv.hit.path);
                            }
                            if ui.small_button("폴더에서 보기").clicked() {
                                crate::util::reveal_path(&hv.hit.path);
                            }
                            if ui.small_button("경로 복사").clicked() {
                                ctx.copy_text(hv.hit.path.clone());
                            }
                        });
                    });
                });
            })
            .response;
        if resp.interact(egui::Sense::click()).double_clicked() {
            crate::util::open_path(&hv.hit.path);
        }
        ui.add_space(2.0);
    }
}

fn is_media(s: Section) -> bool {
    matches!(s, Section::Images | Section::Videos)
}

fn kind_tag(k: Kind) -> &'static str {
    match k {
        Kind::Text => "TEXT",
        Kind::Doc => "DOC",
        Kind::Image => "IMG",
        Kind::Video => "VIDEO",
        Kind::Audio => "AUDIO",
        Kind::Other => "FILE",
    }
}

fn score_color(s: f32) -> egui::Color32 {
    if s >= 0.7 {
        widgets::GREEN
    } else if s >= 0.5 {
        widgets::BLUE
    } else {
        widgets::GRAY
    }
}

fn locator_label(hv: &HitView) -> Option<String> {
    match (hv.hit.modality, hv.hit.kind) {
        (Modality::Image, Kind::Video) => Some(format!("{} 장면", fmt_ts(hv.hit.locator))),
        (Modality::Audio, _) => Some(format!("{}부터", fmt_ts(hv.hit.locator))),
        (Modality::Text, _) if hv.hit.locator > 0 => Some(format!("{}번째 구간", hv.hit.locator + 1)),
        _ => None,
    }
}
