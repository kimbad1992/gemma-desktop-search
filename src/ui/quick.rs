//! 빠른 검색창 (Ctrl+Shift+Space): PowerToys Run처럼 화면 위쪽 가운데에 뜨는 작은 창.
//! 입력하는 대로 검색하고, ↑↓ Enter로 바로 연다. Esc나 다른 곳을 누르면 사라진다.

use crate::app::GemmaApp;
use crate::search::query::HitView;
use crate::search::store::Section;
use crate::ui::search_panel::{hit_visual, locator_label, passing, unified, SEARCH_BOX};
use crate::ui::widgets::{self, Btn, Icon};
use eframe::egui::{self, RichText};

/// 빠른 검색창에 보여주는 결과 수
const QUICK_LIMIT: usize = 12;
/// 결과 줄 왼쪽 그림의 한 변
const THUMB: f32 = 40.0;

impl GemmaApp {
    pub fn quick_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let p = crate::ui::theme::palette(ui.visuals());
        let acrylic = self.quick_acrylic && crate::ui::glass::OS_BACKDROP.load(std::sync::atomic::Ordering::SeqCst);
        let mut frame = egui::Frame::new();
        if acrylic {
            // OS의 Acrylic이 창 뒤를 흐려 비춘다: 그 위에 옅은 색만 (둥근 모서리와 테두리는 OS가 그린다)
            let [r, g, b, _] = p.g_acrylic.to_srgba_unmultiplied();
            ui.painter()
                .rect_filled(ctx.content_rect(), 0.0, egui::Color32::from_rgba_unmultiplied(r, g, b, 110));
        } else {
            // 아크릴을 쓸 수 없으면(Windows 10, 투명 효과 꺼짐): 벽 위에 아크릴 색을 칠한 불투명 창
            crate::ui::glass::paint_wall(ui);
            ui.painter().rect_filled(ctx.content_rect(), 0.0, p.g_acrylic);
            // 테두리 없는 창이라, 뒤의 화면과 구별되게 강조색 테두리를 두른다
            frame = frame.stroke(egui::Stroke::new(1.0, crate::ui::theme::alpha(p.accent, 0.75)));
        }
        let frame = frame.inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 12,
            bottom: 8,
        });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            // ── 입력 ──
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.add_space(4.0);
                widgets::icon(ui, Icon::Search, 20.0, p.accent_text);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::unique(SEARCH_BOX))
                        .hint_text("찾고 싶은 내용을 설명하세요 (사진, 영상, 문서, 소리)")
                        .font(crate::ui::theme::query_font())
                        .desired_width(ui.available_width() - 130.0)
                        .frame(egui::Frame::NONE)
                        .align(egui::Align2::LEFT_CENTER),
                );
                if self.focus_search {
                    resp.request_focus();
                    self.focus_search = false;
                }
                if self.select_query {
                    self.select_query = false;
                    let id = egui::Id::unique(SEARCH_BOX);
                    if let Some(mut st) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
                        let n = self.query.chars().count();
                        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(n),
                        )));
                        st.store(ui.ctx(), id);
                    }
                }
                if resp.changed() {
                    // 입력을 멈추면 바로 검색. 한글 조합이 확정될 때처럼 글자는 그대로인데 "바뀜"만 온 경우,
                    // 지금 결과가 이미 그 검색어의 것이면 다시 찾지 않는다.
                    let current = self.query_edited.is_none()
                        && self.search_rx.is_none()
                        && self.outcome.is_some()
                        && matches!(&self.last_query, Some(crate::search::query::QueryKind::Text(q))
                            if q.trim() == self.query.trim());
                    if !current {
                        self.query_edited = Some(std::time::Instant::now());
                    }
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    // Enter: 선택한 결과를 연다. 입력이 결과보다 새로우면 바로 검색하고, 결과가 오면 첫 결과를 연다.
                    resp.request_focus();
                    if !self.query.trim().is_empty() {
                        self.quick_enter = true;
                        if self.query_edited.is_some() || self.outcome.is_none() {
                            self.query_edited = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, Btn::Ghost, Icon::Maximize2, "큰 창")
                        .on_hover_text("같은 검색을 큰 창에서 (Ctrl+O)")
                        .clicked()
                    {
                        self.open_full_from_quick(&ctx);
                    }
                    let st = self.server.state();
                    if self.pending_search_waiting() || !st.is_ready() && st.is_running() {
                        widgets::spinner(ui);
                        ui.label(RichText::new("서버 준비 중").small().weak());
                    } else if self.search_rx.is_some() {
                        widgets::spinner(ui);
                    }
                });
            });
            ui.add_space(4.0);
            widgets::hairline(ui);
            // 아래에 늘 보이는 단축키 안내
            egui::Panel::bottom("quick_hint")
                .show_separator_line(false)
                .frame(egui::Frame::new().inner_margin(egui::Margin {
                    left: 4,
                    right: 4,
                    top: 6,
                    bottom: 0,
                }))
                .show(ui, |ui| {
                    widgets::caption(ui, "↑↓ 이동 · Enter 열기 · Ctrl+Enter 폴더에서 보기 · Ctrl+O 큰 창 · Esc 닫기");
                });
            ui.add_space(4.0);

            // ── 결과 ──
            let Some(outcome) = self.outcome.take() else {
                ui.add_space(12.0);
                ui.label(RichText::new("입력하는 대로 찾습니다.").weak());
                return;
            };
            let min = self.cfg.min_relevance;
            let visible: Vec<(Section, Vec<&HitView>)> = outcome
                .sections
                .iter()
                .map(|(s, h)| (*s, passing(h, min)))
                .filter(|(s, h)| !h.is_empty() && self.filter_sections[*s as usize])
                .collect();
            let mut shown = unified(&visible, &outcome.kind_hints);
            shown.truncate(QUICK_LIMIT);
            self.reconcile_selection(&shown);
            self.handle_result_keys(&ctx, &shown);
            if shown.is_empty() {
                ui.add_space(12.0);
                ui.label(if outcome.error.is_some() {
                    format!("검색 실패: {}", outcome.error.as_deref().unwrap_or_default())
                } else {
                    format!("관련도 {min:.1} 이상인 결과가 없습니다 (맞는 파일이 없거나 약합니다)")
                });
            } else if self.selected.is_none() {
                self.selected = Some((shown[0].0, shown[0].1.hit.file_id));
            }
            self.note_selected_row(&shown);
            let selected = self.selected.and_then(|(s, f)| {
                shown.iter().find(|(ss, hv)| *ss == s && hv.hit.file_id == f).map(|(_, hv)| *hv)
            });
            // Enter를 눌렀고 결과가 지금 입력에 대한 것이면 연다
            if self.quick_enter && self.query_edited.is_none() && self.search_rx.is_none() {
                self.quick_enter = false;
                if let Some(hv) = selected.or(shown.first().map(|(_, hv)| *hv)) {
                    self.open_hit(hv);
                }
            }
            if let Some(hv) = selected {
                egui::Panel::right("quick_preview")
                    .resizable(false)
                    .show_separator_line(false)
                    .exact_size(ui.available_width() * 0.46)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(14)).outer_margin(egui::Margin {
                        left: 10,
                        right: 0,
                        top: 0,
                        bottom: 2,
                    }))
                    .show(ui, |ui| {
                        use crate::ui::glass::{self, Glass};
                        if acrylic {
                            glass::backdrop_plain(ui, Glass::Content, 20.0, egui::Margin::same(14));
                        } else {
                            glass::backdrop(ui, Glass::Content, 20.0, egui::Margin::same(14));
                        }
                        self.preview_panel(ui, &ctx, hv, outcome.expr.as_deref())
                    });
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (sec, hv) in &shown {
                    self.quick_row(ui, &ctx, *sec, hv);
                }
            });
            self.outcome = Some(outcome);
        });
    }

    fn quick_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sec: Section, hv: &HitView) {
        use crate::ui::theme::{self, Tone};
        let is_sel = self.selected == Some((sec, hv.hit.file_id));
        let tex = hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c));
        let resp = Self::hit_box(ui, is_sel, egui::Margin::symmetric(8, 6), |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                hit_visual(ui, tex.as_ref(), hv, THUMB);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(
                        egui::Label::new(RichText::new(&hv.file_name).text_style(theme::ts_strong()).strong()).truncate(),
                    );
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 7.0;
                        widgets::score_bar(ui, hv.hit.score, widgets::ScoreStyle::Compact);
                        ui.label(RichText::new(sec.label()).small().weak());
                        if let Some(l) = locator_label(hv) {
                            ui.label(RichText::new(l).small().color(Tone::Info.fg(ui.visuals())));
                        }
                        if hv.keyword_match() {
                            ui.label(RichText::new("검색어 포함").small().color(Tone::Ok.fg(ui.visuals())));
                        }
                    });
                });
            });
        });
        if resp.clicked() {
            self.user_select(sec, hv.hit.file_id);
        }
        if resp.double_clicked() {
            self.open_hit(hv);
        }
        if self.scroll_to_selected && is_sel {
            resp.scroll_to_me(Some(egui::Align::Center));
            self.scroll_to_selected = false;
        }
    }
}
