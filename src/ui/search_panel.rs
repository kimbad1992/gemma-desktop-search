//! 검색 탭: 질의 입력, 필터 한 줄(종류·폴더·경로·관련도), 결과(통합 목록 또는 섹션별), 오른쪽 미리보기

use crate::app::{FolderStats, GemmaApp};
use crate::extract::{Kind, Modality};
use crate::open::Place;
use crate::search::keyword;
use crate::search::query::{HitView, QueryKind};
use crate::search::store::Section;
use crate::ui::glass::{self, Glass};
use crate::ui::theme::{self, alpha, palette, space, Hue, Tone};
use crate::ui::widgets::{self, Btn, Icon, ScoreStyle};
use crate::util::{fmt_num, fmt_ts};
use eframe::egui::{self, RichText};
use std::sync::atomic::Ordering;

/// 결과 줄 왼쪽 그림(썸네일 또는 종류 타일)의 한 변
const THUMB: f32 = 48.0;
pub const SEARCH_BOX: &str = "search_box";
const CARD_W: f32 = 168.0;

/// 미리보기(진한 유리)의 안쪽 여백
pub const PREVIEW_PAD: egui::Margin = egui::Margin::same(16);

/// 처음 화면에 보여주는 예시 검색어
const EXAMPLES: [&str; 3] = ["바다가 보이는 사진", "예산 회의록", "고양이 영상"];

// 결과를 추리는 규칙은 화면과 상관없어 `search::query`에 있다
pub use crate::search::query::{locator_label, passing, unified};

impl GemmaApp {
    pub fn search_tab(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let running = self.search_rx.is_some();

        // ── 툴바: 맑은 유리 한 장에 질의 입력과 필터 한 줄 ──
        let (submit, pick_image) = glass::show(ui, Glass::Clear, 28.0, egui::Margin::same(10), |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 10.0;
            let r = self.query_field(ui, running);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.vertical(|ui| self.filter_bar(ui));
            });
            r
        })
        .inner;
        ui.add_space(space::M);
        if pick_image {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("이미지", &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff", "heic", "avif"])
                .pick_file()
            {
                self.start_search(QueryKind::Image(p));
            }
        }
        if submit && !running {
            let q = self.query.clone();
            self.start_search(QueryKind::Text(q));
        }

        // ── 지금 검색할 수 없는 이유와 해결 방법 ──
        let server_state = self.server.state();
        let loading = self.store_loading.load(Ordering::SeqCst);
        let files = self.store.read().file_count();
        let (mut go_settings, mut start_server) = (false, false);
        if loading {
            // 요약 줄에 진행을 보인다
        } else if files == 0 {
            widgets::callout_ui(ui, Tone::Warn, "검색할 자료가 아직 없습니다", |ui| {
                ui.label("설정에서 폴더를 추가하고 검색 준비(인덱싱)를 하면 여기서 찾을 수 있습니다.");
                go_settings = widgets::button(ui, Btn::Secondary, "설정 열기").clicked();
            });
            ui.add_space(space::M);
        } else if !server_state.is_running() && !self.cfg.server_on_demand {
            // 자동으로 켜는 설정이면 검색할 때 알아서 켜지므로 알릴 필요가 없다
            widgets::callout_ui(ui, Tone::Warn, "서버가 꺼져 있습니다", |ui| {
                ui.label("검색하려면 서버를 켜야 합니다.");
                start_server = widgets::button(ui, Btn::Secondary, "서버 시작").clicked();
            });
            ui.add_space(space::M);
        }
        if go_settings {
            self.tab = crate::app::Tab::Settings;
        }
        if start_server {
            self.start_server_manual();
        }
        // 끌어다 놓기 안내
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            widgets::callout(ui, Tone::Info, "이미지를 놓으면 비슷한 이미지·영상을 찾습니다", "");
            ui.add_space(space::M);
        }

        // ── 결과 ──
        let Some(outcome) = self.outcome.take() else {
            // 아직 검색하지 않았을 때: 결과 목록 자리에 안내와 예시 검색어
            let mut example = None;
            self.result_sheet(ui, running, None, |_, ui| {
                widgets::empty_state(
                    ui,
                    "문서·사진·영상·소리를 한 번에 찾습니다",
                    "한국어와 영어 모두 됩니다. 사진을 창에 끌어다 놓으면 비슷한 사진·영상을 찾습니다.",
                );
                ui.add_space(space::M);
                ui.vertical_centered(|ui| {
                    widgets::caption(ui, "이렇게 찾아 보세요");
                });
                ui.add_space(space::XS);
                let gap = 8.0;
                let total: f32 = EXAMPLES.iter().map(|ex| widgets::chip_width(ui, ex)).sum::<f32>()
                    + gap * (EXAMPLES.len() - 1) as f32;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                    for ex in EXAMPLES {
                        if widgets::icon_chip(ui, false, Icon::Search, ex).clicked() {
                            example = Some(ex);
                        }
                    }
                });
            });
            if let Some(ex) = example {
                self.query = ex.to_string();
                if !running {
                    self.start_search(QueryKind::Text(ex.to_string()));
                }
            }
            return;
        };
        // 종류 탭: 전체(종류와 상관없이 일치도 순) / 섹션별 전체 보기 (최소 일치도 미만은 제외)
        let min = self.cfg.min_relevance;
        let mut visible: Vec<(Section, Vec<&HitView>)> = outcome
            .sections
            .iter()
            .map(|(s, h)| (*s, passing(h, min)))
            .filter(|(s, h)| !h.is_empty() && self.filter_sections[*s as usize])
            .collect();
        // "고양이 영상"처럼 종류를 말한 질의는 그 섹션을 먼저
        visible.sort_by_key(|(s, _)| {
            outcome.kind_hints.iter().position(|k| k == s).unwrap_or(usize::MAX)
        });
        if let Some(v) = self.result_view {
            if !visible.iter().any(|(s, _)| *s == v) {
                self.result_view = None;
            }
        }
        let all = unified(&visible, &outcome.kind_hints);

        // 화면에 보이는 순서 (키보드로 이동)
        let shown: Vec<(Section, &HitView)> = match self.result_view {
            None => all.clone(),
            Some(sec) => visible
                .iter()
                .filter(|(s, _)| *s == sec)
                .flat_map(|(s, h)| h.iter().map(move |hv| (*s, *hv)))
                .collect(),
        };
        self.reconcile_selection(&shown);
        // 화면 확인용 (`--shot … --select`): 첫 결과를 고른 상태로
        if self.shot.as_mut().is_some_and(|s| !shown.is_empty() && std::mem::take(&mut s.select)) {
            let (sec, hv) = shown[0];
            self.user_select(sec, hv.hit.file_id);
        }
        self.handle_result_keys(&ctx, &shown);
        self.note_selected_row(&shown);
        // 선택한 결과는 오른쪽 미리보기(진한 유리)에 크게. 탭 막대와 같은 높이에서 시작한다.
        let selected = self.selected.and_then(|(s, f)| {
            shown
                .iter()
                .find(|(ss, hv)| *ss == s && hv.hit.file_id == f)
                .map(|(_, hv)| *hv)
        });
        // 창이 아주 좁으면 목록만 보여 준다 (Enter·더블클릭으로 여는 것은 그대로). 넓어도 본문의 절반 남짓까지만.
        let avail = ui.available_width();
        if let (Some(hv), true) = (selected, avail >= 600.0) {
            let max_w = (avail * 0.55).clamp(280.0, 800.0);
            egui::Panel::right("preview_panel")
                .resizable(true)
                .show_separator_line(false)
                .default_size(430.0_f32.min(max_w))
                .size_range(280.0..=max_w)
                .frame(preview_frame())
                .show(ui, |ui| {
                    glass::backdrop(ui, Glass::Thick, 24.0, PREVIEW_PAD);
                    self.preview_panel(ui, &ctx, hv, outcome.expr.as_deref())
                });
        }

        if !visible.is_empty() {
            widgets::tab_bar(ui, |ui| {
                let all_text = format!("전체 {}", all.len());
                if widgets::tab(ui, self.result_view.is_none(), Icon::LayoutGrid, &all_text)
                    .on_hover_text("파일 종류와 상관없이 관련도가 높은 순서 (같은 파일은 한 번만)")
                    .clicked()
                {
                    self.result_view = None;
                }
                for (sec, hits) in &visible {
                    let sel = self.result_view == Some(*sec);
                    let text = format!("{} {}", sec.label(), hits.len());
                    if widgets::tab(ui, sel, section_icon(*sec), &text).clicked() {
                        self.result_view = Some(*sec);
                    }
                }
            });
            ui.add_space(space::S + 2.0);
        }

        self.result_sheet(ui, running, Some(&outcome), |this, ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // 줄 사이는 1px 구분선으로만 (`result_row`)
                ui.spacing_mut().item_spacing.y = 0.0;
                match this.result_view {
                    None => {
                        let mut prev_sel = true;
                        for (sec, hv) in &all {
                            prev_sel = this.result_row(ui, &ctx, *sec, hv, true, !prev_sel);
                        }
                    }
                    Some(sec) => {
                        if let Some((_, hits)) = visible.iter().find(|(s, _)| *s == sec) {
                            this.render_hits(ui, &ctx, sec, hits);
                        }
                    }
                }
                if visible.is_empty() && outcome.error.is_none() {
                    if min > 0.0 && outcome.total_files() > 0 {
                        widgets::empty_state(
                            ui,
                            &format!("관련도 {min:.1} 이상인 결과가 없습니다"),
                            "맞는 파일이 없거나 약하게만 맞습니다. 필터 줄의 관련도를 낮춰 보세요.",
                        );
                    } else {
                        widgets::empty_state(ui, "결과가 없습니다", "필터 줄의 종류·폴더·경로를 확인하세요.");
                    }
                }
                ui.add_space(space::L);
            });
        });
        self.outcome = Some(outcome);
    }

    /// 결과 목록 자리: 남은 높이를 채우는 내용 유리 한 장. 맨 위에 요약 줄과 구분선, 그 아래 `add`.
    fn result_sheet(
        &mut self,
        ui: &mut egui::Ui,
        running: bool,
        outcome: Option<&crate::search::query::Outcome>,
        add: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let pad = 6.0;
        // 창 아래 가장자리와 띄운다 (본문 틀에는 아래 여백이 없다)
        let h = (ui.available_height() - theme::EDGE as f32).max(160.0);
        glass::show(ui, Glass::Content, 22.0, egui::Margin::same(pad as i8), |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(h - pad * 2.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 14,
                    right: 14,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| self.summary_line(ui, running, outcome));
            let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
            let x = egui::Rangef::new(line.left() + 4.0, line.right() - 4.0);
            let c = palette(ui.visuals()).card_stroke;
            ui.painter().hline(x, line.center().y, egui::Stroke::new(1.0, c));
            ui.add_space(4.0);
            add(self, ui);
        });
    }

    /// 결과 목록 맨 위의 요약 줄: 검색어 · 개수 · (오른쪽) 걸린 시간. 검색 중이거나 준비 중이면 그것을.
    fn summary_line(&mut self, ui: &mut egui::Ui, running: bool, outcome: Option<&crate::search::query::Outcome>) {
        let server_state = self.server.state();
        let loading = self.store_loading.load(Ordering::SeqCst);
        let (files, chunks) = {
            let st = self.store.read();
            (st.file_count(), st.live_chunks())
        };
        // 줄 높이를 글자에 맞춘다 (기본은 버튼 높이). 줄을 만들기 전에 정한다.
        ui.spacing_mut().interact_size.y = 20.0;
        ui.horizontal(|ui| {
            if loading {
                widgets::spinner(ui);
                let s = self.store_status.lock().clone();
                ui.label(if s.is_empty() { "인덱스를 불러오는 중…".to_string() } else { s });
            } else if running {
                widgets::spinner(ui);
                let secs = self.search_started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                ui.label(format!("검색 중… {secs:.1}초"));
            } else if self.pending_search_waiting() || (server_state.is_running() && !server_state.is_ready()) {
                widgets::spinner(ui);
                ui.label("서버를 준비하는 중…");
            } else if let Some(o) = outcome {
                if let Some(e) = &o.error {
                    widgets::tone_label(ui, Tone::Danger, format!("검색 실패: {e}"));
                } else {
                    let min = self.cfg.min_relevance;
                    let shown: usize = o.sections.iter().map(|(_, v)| passing(v, min).len()).sum();
                    ui.label(RichText::new(&o.label).text_style(theme::ts_strong()).strong());
                    widgets::caption(
                        ui,
                        &if min > 0.0 {
                            format!("{shown}개 파일 · 관련도 {min:.1} 이상")
                        } else {
                            format!("{shown}개 파일")
                        },
                    );
                    // 걸린 시간은 오른쪽 끝에. 자리가 모자라면(좁은 창) 뺀다.
                    let took = format!("{:.2}초 · 벡터 {}개 비교", o.elapsed.as_secs_f32(), fmt_num(o.searched_chunks as u64));
                    let font = egui::TextStyle::Small.resolve(ui.style());
                    let w = ui.painter().layout_no_wrap(took.clone(), font, egui::Color32::PLACEHOLDER).size().x;
                    if ui.available_width() >= w + 16.0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            widgets::caption(ui, &took);
                        });
                    }
                }
            } else if files > 0 {
                widgets::caption(
                    ui,
                    &format!("파일 {}개 · 벡터 {}개에서 찾습니다", fmt_num(files as u64), fmt_num(chunks as u64)),
                );
            }
        });
    }

    /// 질의 입력: 홈(알약) 안에 돋보기 · 입력칸 · 이미지로 검색 · 검색 버튼. (제출, 이미지 고르기)를 돌려준다.
    fn query_field(&mut self, ui: &mut egui::Ui, running: bool) -> (bool, bool) {
        let ctx = ui.ctx().clone();
        let p = palette(ui.visuals());
        let box_id = egui::Id::unique(SEARCH_BOX);
        let focused = ctx.memory(|m| m.has_focus(box_id));
        let ring = ctx.animate_bool_with_time(box_id.with("ring"), focused, 0.12);
        let (mut submit, mut pick_image) = (false, false);
        let slot = ui.painter().add(egui::Shape::Noop);
        let inner = egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 16,
                right: 8,
                top: 8,
                bottom: 8,
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    if running {
                        ui.add(egui::Spinner::new().size(18.0));
                    } else {
                        widgets::icon(ui, Icon::Search, 18.0, p.accent_text);
                    }
                    // 오른쪽 버튼 두 개 자리를 남기고 입력칸이 나머지를 채운다
                    let w = (ui.available_width() - 116.0).max(120.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .id(box_id)
                            .hint_text("찾고 싶은 내용을 적어 보세요 (예: 바다가 보이는 사진, 예산 회의록, 고양이 영상)")
                            .font(theme::query_font())
                            .frame(egui::Frame::NONE)
                            .align(egui::Align2::LEFT_CENTER)
                            .desired_width(w),
                    );
                    if self.focus_search {
                        resp.request_focus();
                        self.focus_search = false;
                    }
                    if self.select_query {
                        // 단축키로 불렀을 때: 이전 질의를 전부 선택해 바로 새로 입력
                        self.select_query = false;
                        if let Some(mut st) = egui::text_edit::TextEditState::load(ui.ctx(), box_id) {
                            let n = self.query.chars().count();
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(n),
                            )));
                            st.store(ui.ctx(), box_id);
                        }
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                        resp.request_focus();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if widgets::button_enabled(ui, Btn::Primary, !running, "검색").clicked() {
                            submit = true;
                        }
                        if widgets::icon_button_enabled(ui, Btn::Ghost, !running, Icon::ImagePlus, "")
                            .on_hover_text("이미지로 검색: 이미지 파일을 고르거나 창에 끌어다 놓으면 비슷한 이미지·영상을 찾습니다")
                            .clicked()
                        {
                            pick_image = true;
                        }
                    });
                });
            });
        // 홈 + 초점 표시 (강조색 안쪽 테두리와 바깥 3px 고리)
        let rect = inner.response.rect;
        let r = rect.height() / 2.0;
        let mut shapes = vec![glass::well_shape(ui.visuals().dark_mode, rect, r)];
        if ring > 0.0 {
            shapes.push(
                egui::epaint::RectShape::stroke(
                    rect,
                    r,
                    egui::Stroke::new(1.0, alpha(p.accent, 0.55 * ring)),
                    egui::StrokeKind::Inside,
                )
                .into(),
            );
            shapes.push(
                egui::epaint::RectShape::stroke(
                    rect,
                    r,
                    egui::Stroke::new(3.0, alpha(p.accent, 0.18 * ring)),
                    egui::StrokeKind::Outside,
                )
                .into(),
            );
        }
        ui.painter().set(slot, egui::Shape::Vec(shapes));
        (submit, pick_image)
    }

    /// 필터 한 줄: 종류 · 폴더(둘 다 체크 드롭다운) · 경로 · 관련도
    fn filter_bar(&mut self, ui: &mut egui::Ui) {
        use crate::search::store::OTHER_FOLDER_KEY;
        let p = palette(ui.visuals());
        let folders = self.folders.lock().clone();
        let has_folders = !(folders.list.is_empty() && folders.other == 0);
        let (mut folders_changed, mut reset) = (false, false);

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            // 종류
            let on = self.filter_sections.iter().filter(|b| **b).count();
            let total = self.filter_sections.len();
            let summary = if on == total { "종류 전체".to_string() } else { format!("종류 {on}/{total}") };
            widgets::multi_dropdown(ui, "filter_kinds", Icon::Shapes, &summary, on != total, |ui| {
                for s in Section::ALL {
                    ui.checkbox(&mut self.filter_sections[s as usize], s.label());
                }
                ui.separator();
                if ui.button("모두 선택").clicked() {
                    self.filter_sections.fill(true);
                }
            });

            // 폴더: 인덱스에 들어 있는 폴더 가운데 골라 그 안에서만 검색
            let mut folders_filtered = false;
            if has_folders {
                let excluded = &self.cfg.search_excluded_folders;
                let other_on = folders.other > 0 && !excluded.iter().any(|k| k == OTHER_FOLDER_KEY);
                let n_total = folders.list.len() + usize::from(folders.other > 0);
                let n_on = folders.list.iter().filter(|f| !excluded.contains(&f.key)).count() + usize::from(other_on);
                folders_filtered = n_on != n_total;
                let summary = if folders_filtered { format!("폴더 {n_on}/{n_total}") } else { "폴더 전체".to_string() };
                widgets::multi_dropdown(ui, "filter_folders", Icon::Folder, &summary, folders_filtered, |ui| {
                    folders_changed |= self.folder_menu(ui, &folders);
                });
            }

            // 경로: 칩 모양의 작은 입력칸 (홈)
            let path_on = !self.path_filter.trim().is_empty();
            let slot = ui.painter().add(egui::Shape::Noop);
            let path_resp = egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 12,
                    top: 1,
                    bottom: 1,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_height(26.0);
                        // 입력칸의 최소 높이(버튼 높이)를 칩 높이에 맞춘다
                        ui.spacing_mut().interact_size.y = 24.0;
                        ui.spacing_mut().item_spacing.x = 6.0;
                        widgets::icon(ui, Icon::ListFilter, 14.0, if path_on { p.accent_text } else { p.muted });
                        ui.add(
                            egui::TextEdit::singleline(&mut self.path_filter)
                                .hint_text("경로에 포함된 글자")
                                .font(egui::FontId::proportional(13.0))
                                .frame(egui::Frame::NONE)
                                .align(egui::Align2::LEFT_CENTER)
                                .desired_width(150.0),
                        );
                    });
                })
                .response;
            {
                let rect = path_resp.rect;
                let r = rect.height() / 2.0;
                let mut shapes = vec![glass::well_shape(ui.visuals().dark_mode, rect, r)];
                if path_on {
                    shapes.push(
                        egui::epaint::RectShape::stroke(
                            rect,
                            r,
                            egui::Stroke::new(1.0, alpha(p.accent, 0.55)),
                            egui::StrokeKind::Inside,
                        )
                        .into(),
                    );
                }
                ui.painter().set(slot, egui::Shape::Vec(shapes));
            }
            path_resp.on_hover_text("경로에 이 글자가 들어 있는 결과만 보여줍니다 (예: 사진 2024)");

            // 관련도: 줄이 바뀔 때 막대와 "끄기"가 떨어지지 않게 한 덩어리로 놓는다
            let group = egui::vec2(262.0, 28.0);
            ui.allocate_ui_with_layout(group, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_space(4.0);
                ui.label(RichText::new("관련도").weak());
                let mut v = self.cfg.min_relevance;
                let r = widgets::slider(ui, &mut v, 0.0..=12.0, 0.1, 120.0).on_hover_text(
                    "이 관련도보다 낮은 결과를 숨깁니다 (다시 검색하지 않고 바로 적용).\n관련도 = 이 검색에서 대부분의 파일(중앙값)보다 표준편차 몇 배만큼 튀는지.\n대략 7 이상 뚜렷함, 5.5 이상 관련 있음. 맞는 파일이 없는 검색은 보통 5.5 아래라 결과가 비게 됩니다.",
                );
                if r.changed() {
                    self.cfg.min_relevance = v;
                }
                let mut persist = r.drag_stopped() || (r.changed() && !r.dragged());
                // 숫자 폭이 바뀌어도(5.5 → 10.0) 옆의 버튼이 움직이지 않게 자리를 고정한다
                ui.allocate_ui_with_layout(
                    egui::vec2(30.0, 20.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let text = if self.cfg.min_relevance > 0.0 {
                            RichText::new(format!("{:.1}", self.cfg.min_relevance)).text_style(theme::ts_strong())
                        } else {
                            RichText::new("끔").weak()
                        };
                        ui.label(text);
                    },
                );
                if self.cfg.min_relevance > 0.0
                    && widgets::small_button(ui, Btn::Ghost, "끄기")
                        .on_hover_text("관련도와 상관없이 모두 보여줍니다")
                        .clicked()
                {
                    self.cfg.min_relevance = 0.0;
                    persist = true;
                }
                if persist {
                    self.persist_min_score();
                }
            });

            // 걸러내는 중일 때만 초기화 링크
            if (on != total || folders_filtered || path_on) && ui.link("필터 초기화").clicked() {
                reset = true;
            }
        });

        if reset {
            self.filter_sections.fill(true);
            self.path_filter.clear();
            if !self.cfg.search_excluded_folders.is_empty() {
                self.cfg.search_excluded_folders.clear();
                folders_changed = true;
            }
        }
        if folders_changed {
            self.on_folder_selection_changed();
        }
    }

    /// 폴더 드롭다운의 내용 (지금 대상 폴더 + 예전에 인덱싱한 폴더). 선택이 바뀌었으면 true.
    fn folder_menu(&mut self, ui: &mut egui::Ui, folders: &FolderStats) -> bool {
        use crate::search::store::OTHER_FOLDER_KEY;
        let mut changed = false;
        let excluded = &mut self.cfg.search_excluded_folders;
        let all = folders.list.iter().all(|f| !excluded.contains(&f.key))
            && (folders.other == 0 || !excluded.iter().any(|k| k == OTHER_FOLDER_KEY));
        if ui.button(if all { "모두 해제" } else { "모두 선택" }).clicked() {
            excluded.clear();
            if all {
                excluded.extend(folders.list.iter().map(|f| f.key.clone()));
                if folders.other > 0 {
                    excluded.push(OTHER_FOLDER_KEY.to_string());
                }
            }
            changed = true;
        }
        ui.separator();
        for f in &folders.list {
            let mut on = !excluded.contains(&f.key);
            let mut text = RichText::new(format!("{} ({})", short_path(&f.path), fmt_num(f.files)));
            if !f.current {
                text = text.italics();
            }
            let hover = if f.current {
                format!("{}\n지금 인덱싱 대상 폴더", f.path)
            } else {
                format!("{}\n예전에 인덱싱한 폴더 (지금은 대상에서 빠짐, 결과는 남아 있음)", f.path)
            };
            if ui.checkbox(&mut on, text).on_hover_text(hover).changed() {
                if on {
                    excluded.retain(|k| k != &f.key);
                } else {
                    excluded.push(f.key.clone());
                }
                changed = true;
            }
        }
        if folders.other > 0 {
            let mut on = !excluded.iter().any(|k| k == OTHER_FOLDER_KEY);
            if ui
                .checkbox(&mut on, RichText::new(format!("기타 위치 ({})", fmt_num(folders.other))).italics())
                .on_hover_text("위 폴더 어디에도 속하지 않는 인덱스 파일 (폴더 기록 기능 이전에 인덱싱한 파일 등)")
                .changed()
            {
                if on {
                    excluded.retain(|k| k != OTHER_FOLDER_KEY);
                } else {
                    excluded.push(OTHER_FOLDER_KEY.to_string());
                }
                changed = true;
            }
        }
        changed
    }

    /// 사용자가 직접 고른 선택 (↑↓·클릭). 그때의 검색어를 적어 두어, 같은 검색의 결과가 다시 와도 지킨다.
    pub(crate) fn user_select(&mut self, sec: Section, file_id: i64) {
        self.selected = Some((sec, file_id));
        self.nav_query = Some(self.query.trim().to_string());
    }

    /// 새 결과가 왔으면 고르던 줄을 다시 맞춘다 (`app.rs`의 poll이 `reselect`를 세운다)
    pub(crate) fn reconcile_selection(&mut self, shown: &[(Section, &HitView)]) {
        let Some(row) = self.reselect.take() else { return };
        let keys: Vec<(Section, i64)> = shown.iter().map(|(s, hv)| (*s, hv.hit.file_id)).collect();
        self.selected = kept_selection(self.selected, row, &keys);
        self.scroll_to_selected = self.selected.is_some();
    }

    /// 선택이 화면 목록의 몇 번째 줄인지 적어 둔다 (다음 결과가 왔을 때 쓴다)
    pub(crate) fn note_selected_row(&mut self, shown: &[(Section, &HitView)]) {
        self.selected_row = self
            .selected
            .and_then(|(s, f)| shown.iter().position(|(ss, hv)| *ss == s && hv.hit.file_id == f));
    }

    /// ↑↓ 이동, Enter 열기(시점·페이지), Ctrl+Enter 폴더에서 보기, Ctrl+C 파일 복사, Esc 검색창으로
    pub(crate) fn handle_result_keys(&mut self, ctx: &egui::Context, shown: &[(Section, &HitView)]) {
        let box_id = egui::Id::unique(SEARCH_BOX);
        let box_focused = ctx.memory(|m| m.has_focus(box_id));
        // 다른 입력칸(경로 필터 등)에 입력 중일 때만 양보한다. 결과 카드를 클릭해 생긴 포커스는 무시.
        let other_focused = !box_focused
            && ctx
                .memory(|m| m.focused())
                .is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some());
        if shown.is_empty() || other_focused {
            return;
        }
        let pos = self
            .selected
            .and_then(|(s, f)| shown.iter().position(|(ss, hv)| *ss == s && hv.hit.file_id == f));
        if box_focused && self.quick.is_some() {
            // 빠른 검색창 (PowerToys Run처럼): 입력은 계속하면서 ↑↓로 선택만 움직인다. Enter는 quick_ui에서 처리.
            let (down, up, reveal) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
                )
            });
            let last = shown.len() - 1;
            let p = if down {
                Some(pos.map(|p| (p + 1).min(last)).unwrap_or(0))
            } else if up {
                Some(pos.map(|p| p.saturating_sub(1)).unwrap_or(0))
            } else {
                pos
            };
            if down || up {
                if let Some(p) = p {
                    self.user_select(shown[p].0, shown[p].1.hit.file_id);
                    self.scroll_to_selected = true;
                }
            }
            if reveal {
                if let Some(p) = p {
                    crate::platform::reveal_in_file_manager(&shown[p].1.hit.path);
                    self.leave_quick_after_open = true;
                }
            }
            return;
        }
        if box_focused {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                let (s, hv) = shown[pos.unwrap_or(0)];
                self.user_select(s, hv.hit.file_id);
                self.scroll_to_selected = true;
                ctx.memory_mut(|m| m.surrender_focus(box_id));
            }
            return;
        }
        let (down, up, enter, reveal, esc, copy) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
            )
        });
        let last = shown.len() - 1;
        let new_pos = if down {
            Some(pos.map(|p| (p + 1).min(last)).unwrap_or(0))
        } else if up {
            match pos {
                Some(0) | None => {
                    self.focus_search = true;
                    None
                }
                Some(p) => Some(p - 1),
            }
        } else {
            pos
        };
        if down || up {
            if let Some(p) = new_pos {
                self.user_select(shown[p].0, shown[p].1.hit.file_id);
                self.scroll_to_selected = true;
            }
        }
        if esc {
            self.focus_search = true;
        }
        let Some(p) = new_pos else { return };
        let hv = shown[p].1;
        if enter {
            self.open_hit(hv);
        } else if reveal {
            crate::platform::reveal_in_file_manager(&hv.hit.path);
        } else if copy {
            self.copy_file(&hv.hit.path);
        }
    }

    /// 일치한 자리(영상·오디오 시점, PDF 페이지)에서 연다
    pub fn open_hit(&mut self, hv: &HitView) {
        if self.quick.is_some() {
            self.leave_quick_after_open = true;
        }
        let place = Place::of(&hv.hit.path, hv.hit.kind, hv.hit.modality, hv.hit.locator);
        if let Some(desc) = crate::open::open_at(&hv.hit.path, place) {
            self.notify(format!("열기: {desc}"), false);
        }
    }

    pub fn copy_file(&mut self, path: &str) {
        match crate::platform::copy_file_to_clipboard(path) {
            Ok(()) => self.notify("파일을 클립보드에 복사했습니다 (탐색기·메신저에 붙여넣기)", false),
            Err(e) => self.notify(format!("파일 복사 실패: {e}"), true),
        }
    }

    fn render_hits(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sec: Section, hits: &[&HitView]) {
        if is_media(sec) {
            // 카드 폭을 알고 있으니 한 줄에 들어갈 개수를 계산해 줄 단위로 배치
            ui.spacing_mut().item_spacing.y = 6.0;
            let gap = ui.spacing().item_spacing.x;
            let cols = ((ui.available_width() + gap) / (CARD_W + 16.0 + gap)).floor().max(1.0) as usize;
            for row in hits.chunks(cols) {
                ui.horizontal(|ui| {
                    for hv in row {
                        self.media_card(ui, ctx, sec, hv);
                    }
                });
            }
        } else {
            let mut prev_sel = true;
            for hv in hits {
                prev_sel = self.result_row(ui, ctx, sec, hv, false, !prev_sel);
            }
        }
    }

    /// 결과 줄·카드의 틀: 평소에는 바탕이 없고, 마우스를 올리면 카드 색, 고르면 강조색 테두리와 옅은 강조 배경.
    /// 내용을 먼저 놓은 뒤 그 자리에 마우스가 있는지 보고 바탕을 칠한다.
    /// 결과 줄·카드의 틀: 평소에는 바탕이 없고, 마우스를 올리면 옅은 강조색, 고르면 강조색 테두리와 옅은 강조 배경
    /// (+ 위쪽 안 빛). 내용을 먼저 놓은 뒤 그 자리에 마우스가 있는지 보고 바탕을 칠한다.
    pub(crate) fn hit_box(
        ui: &mut egui::Ui,
        selected: bool,
        margin: egui::Margin,
        add: impl FnOnce(&mut egui::Ui),
    ) -> egui::Response {
        let p = palette(ui.visuals());
        let slot = ui.painter().add(egui::Shape::Noop);
        let inner = egui::Frame::NONE.inner_margin(margin).show(ui, add);
        let rect = inner.response.rect;
        let r = 15.0;
        if selected {
            let stroke = egui::Stroke::new(1.0, alpha(p.accent, 0.46));
            ui.painter().set(
                slot,
                egui::Shape::Vec(vec![
                    egui::epaint::RectShape::new(rect, r, alpha(p.accent, 0.13), stroke, egui::StrokeKind::Inside).into(),
                    glass::inner_light(rect, r, true, p.g_hi),
                ]),
            );
        } else if ui.rect_contains_pointer(rect) {
            ui.painter().set(slot, egui::epaint::RectShape::filled(rect, r, alpha(p.accent, 0.06)));
        }
        inner.response.interact(egui::Sense::click())
    }

    /// 이미지·영상용 썸네일 카드
    fn media_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sec: Section, hv: &HitView) {
        const W: f32 = CARD_W;
        const H: f32 = 118.0;
        let tex = hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c));
        let is_sel = self.selected == Some((sec, hv.hit.file_id));
        let resp = Self::hit_box(ui, is_sel, egui::Margin::same(7), |ui| {
            ui.set_width(W);
            ui.vertical(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(W, H), egui::Sense::hover());
                match &tex {
                    Some(t) => {
                        egui::Image::new(t)
                            .uv(cover_uv(t.size_vec2(), W / H))
                            .corner_radius(8)
                            .paint_at(ui, rect);
                    }
                    None => {
                        let (hue, glyph) = kind_look(hv);
                        let v = ui.visuals();
                        ui.painter().rect_filled(rect, 8.0, hue.bg(v));
                        widgets::paint_icon(ui.painter(), rect.center(), glyph, 30.0, hue.fg(v));
                    }
                }
                ui.add(egui::Label::new(RichText::new(&hv.file_name).text_style(theme::ts_strong()).strong()).truncate());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    widgets::score_bar(ui, hv.hit.score, ScoreStyle::Compact);
                    if let Some(loc) = locator_label(hv) {
                        ui.label(RichText::new(loc).small().color(Tone::Info.fg(ui.visuals())));
                    }
                    if hv.keyword_match() {
                        ui.label(RichText::new("검색어").small().color(Tone::Ok.fg(ui.visuals())))
                            .on_hover_text("파일 이름이나 폴더 이름에 검색어가 들어 있습니다");
                    }
                });
            });
        });
        self.after_hit_response(ctx, sec, hv, &resp);
        resp.on_hover_text(format!("{}\n클릭: 미리보기 · 더블클릭: 열기 · 우클릭: 메뉴", hv.hit.path));
    }

    /// 결과 카드/줄 공통: 클릭 선택, 더블클릭 열기, 우클릭 메뉴, 키보드 이동 시 화면에 보이게
    fn after_hit_response(&mut self, ctx: &egui::Context, sec: Section, hv: &HitView, resp: &egui::Response) {
        if resp.clicked() {
            self.user_select(sec, hv.hit.file_id);
        }
        if resp.double_clicked() {
            self.open_hit(hv);
        }
        if self.scroll_to_selected && self.selected == Some((sec, hv.hit.file_id)) {
            resp.scroll_to_me(Some(egui::Align::Center));
            self.scroll_to_selected = false;
        }
        resp.context_menu(|ui| {
            if ui.button(open_label(hv)).clicked() {
                self.open_hit(hv);
                ui.close();
            }
            if ui.button("폴더에서 보기").clicked() {
                crate::platform::reveal_in_file_manager(&hv.hit.path);
                ui.close();
            }
            if ui.button("파일 복사").clicked() {
                self.copy_file(&hv.hit.path);
                ui.close();
            }
            if ui.button("경로 복사").clicked() {
                ctx.copy_text(hv.hit.path.clone());
                ui.close();
            }
        });
    }

    /// show_kind: 통합 목록에서 어떤 종류의 결과인지 표시
    /// show_kind: 통합 목록에서 어떤 종류의 결과인지 표시. `divider`면 위에 1px 구분선.
    /// 이 줄이 선택돼 있으면 true (다음 줄이 구분선을 빼도록).
    fn result_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        sec: Section,
        hv: &HitView,
        show_kind: bool,
        divider: bool,
    ) -> bool {
        let is_sel = self.selected == Some((sec, hv.hit.file_id));
        // 줄 사이 구분선 (고른 줄의 위아래는 빼서 강조 테두리와 겹치지 않게)
        let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        if divider && !is_sel {
            let c = palette(ui.visuals()).card_stroke;
            let x = egui::Rangef::new(line.left() + 12.0, line.right() - 12.0);
            ui.painter().hline(x, line.center().y, egui::Stroke::new(1.0, c));
        }
        let tex = hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c));
        let resp = Self::hit_box(ui, is_sel, egui::Margin::symmetric(12, 10), |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                // 썸네일 또는 종류 타일
                hit_visual(ui, tex.as_ref(), hv, THUMB);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    let loc = locator_label(hv);
                    let keyword = hv.keyword_match();
                    // 어디가 맞았는지 알리는 꼬리표들
                    let tags = |ui: &mut egui::Ui| {
                        if show_kind {
                            widgets::tone_tag(ui, Tone::Muted, sec.label());
                        }
                        if let Some(loc) = &loc {
                            widgets::tone_tag(ui, Tone::Info, loc);
                        }
                        if keyword {
                            widgets::tone_tag(ui, Tone::Ok, "검색어 포함")
                                .on_hover_text("파일 이름이나 내용에 검색어가 그대로 들어 있습니다");
                        }
                    };
                    let mut tags_w = 0.0;
                    for (on, text) in [
                        (show_kind, sec.label()),
                        (loc.is_some(), loc.as_deref().unwrap_or_default()),
                        (keyword, "검색어 포함"),
                    ] {
                        if on {
                            tags_w += widgets::tag_width(ui, text) + 6.0;
                        }
                    }
                    // 이름 · 꼬리표 · 점수가 한 줄에 다 들어가지 않으면(창이 좁거나 이름이 길면) 꼬리표를 다음 줄로 내린다
                    const SCORE_W: f32 = 112.0;
                    let name_font = theme::ts_item().resolve(ui.style());
                    let natural = ui
                        .painter()
                        .layout_no_wrap(hv.file_name.clone(), name_font, egui::Color32::PLACEHOLDER)
                        .size()
                        .x;
                    let inline = natural + 8.0 + tags_w + SCORE_W <= ui.available_width();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let name_w = (ui.available_width() - SCORE_W - if inline { tags_w + 8.0 } else { 0.0 }).max(40.0);
                        ui.scope(|ui| {
                            ui.set_max_width(name_w);
                            ui.add(
                                egui::Label::new(RichText::new(&hv.file_name).text_style(theme::ts_item()).strong())
                                    .truncate(),
                            );
                        });
                        if inline {
                            ui.add_space(2.0);
                            tags(ui);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            widgets::score_bar(ui, hv.hit.score, ScoreStyle::Row);
                        });
                    });
                    if !inline && tags_w > 0.0 {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().interact_size.y = 20.0;
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);
                            tags(ui);
                        });
                    }
                    ui.add(egui::Label::new(RichText::new(&hv.hit.path).small().weak()).truncate());
                    if let Some(h) = &hv.highlight {
                        ui.add(egui::Label::new(highlight_job(ui, h, 13.0)).wrap());
                    } else if !hv.snippet.is_empty() && hv.hit.modality == Modality::Text {
                        ui.add(egui::Label::new(RichText::new(&hv.snippet).size(13.0)).wrap());
                    }
                });
            });
        });
        self.after_hit_response(ctx, sec, hv, &resp);
        is_sel
    }
}

/// 미리보기 패널의 틀: 본문 안에 놓인 카드 (왼쪽·아래에 바깥 여백)
/// 미리보기 패널의 틀: 왼쪽은 결과 목록과 띄우고 아래는 창 가장자리와 띄운다.
/// 유리는 패널 안에서 `glass::backdrop`으로 칠한다.
fn preview_frame() -> egui::Frame {
    egui::Frame::NONE.inner_margin(PREVIEW_PAD).outer_margin(egui::Margin {
        left: theme::GAP,
        right: 0,
        top: 0,
        bottom: theme::EDGE,
    })
}

/// 파일 종류를 알리는 색과 아이콘 (썸네일이 없는 결과의 타일, 미리보기 머리)
pub fn kind_look(hv: &HitView) -> (Hue, Icon) {
    match hv.hit.kind {
        Kind::Text if is_code(&hv.file_name) => (Hue::Teal, Icon::FileCode),
        Kind::Text => (Hue::Indigo, Icon::FileText),
        Kind::Doc => (Hue::Coral, Icon::BookOpen),
        Kind::Image => (Hue::Amber, Icon::Image),
        Kind::Video => (Hue::Violet, Icon::Film),
        Kind::Audio => (Hue::Green, Icon::AudioLines),
        Kind::Other => (Hue::Gray, Icon::File),
    }
}

fn is_code(name: &str) -> bool {
    const CODE: [&str; 36] = [
        "rs", "py", "js", "ts", "tsx", "jsx", "c", "h", "cpp", "hpp", "cc", "cs", "java", "kt", "go", "rb", "php",
        "swift", "sh", "ps1", "bat", "cmd", "sql", "html", "css", "scss", "json", "yaml", "yml", "toml", "xml", "lua",
        "vue", "dart", "ini", "gradle",
    ];
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| CODE.iter().any(|c| ext.eq_ignore_ascii_case(c)))
}

/// 섹션 칩의 아이콘
fn section_icon(s: Section) -> Icon {
    match s {
        Section::Docs => Icon::FileText,
        Section::Images => Icon::Image,
        Section::Videos => Icon::Film,
        Section::Audio => Icon::AudioLines,
        Section::Names => Icon::Type,
    }
}

/// 그림을 가로/세로 비율이 `aspect`인 칸에 꽉 차게 넣을 때 쓸 가운데 부분 (넘치는 쪽을 잘라 낸다)
fn cover_uv(tex: egui::Vec2, aspect: f32) -> egui::Rect {
    let ta = tex.x / tex.y.max(1.0);
    let (w, h) = if ta > aspect { (aspect / ta, 1.0) } else { (1.0, ta / aspect) };
    egui::Rect::from_center_size(egui::pos2(0.5, 0.5), egui::vec2(w, h))
}

/// 결과 왼쪽의 그림: 썸네일이 있으면 정사각형으로 잘라 보여주고, 없으면 종류 타일
pub fn hit_visual(ui: &mut egui::Ui, tex: Option<&egui::TextureHandle>, hv: &HitView, size: f32) {
    match tex {
        Some(t) => {
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
            egui::Image::new(t)
                .uv(cover_uv(t.size_vec2(), 1.0))
                .corner_radius(size * 0.22)
                .paint_at(ui, rect);
        }
        None => {
            let (hue, glyph) = kind_look(hv);
            widgets::kind_tile(ui, hue, glyph, size);
        }
    }
}

/// 긴 경로는 앞부분을 줄여서 표시 (`…\사진\2024`)
fn short_path(p: &str) -> String {
    use crate::paths::{is_sep, SEP};
    let p = p.trim_end_matches(is_sep);
    if p.chars().count() <= 32 {
        return p.to_string();
    }
    let parts: Vec<&str> = crate::paths::parts(p).collect();
    match parts.len() {
        0 => p.to_string(),
        1 => crate::util::truncate_chars(p, 32),
        n => format!("…{SEP}{}{SEP}{}", parts[n - 2], parts[n - 1]),
    }
}

/// 강조 표식이 든 발췌를 글자색·배경으로 강조한 레이아웃으로
pub fn highlight_job(ui: &egui::Ui, text: &str, size: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let normal = ui.visuals().text_color();
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() && c != keyword::MARK_START && c != keyword::MARK_END { ' ' } else { c })
        .collect();
    for (part, hot) in keyword::split_marks(&flat) {
        let mut fmt = egui::TextFormat {
            font_id: egui::FontId::proportional(size),
            color: normal,
            ..Default::default()
        };
        if hot {
            let p = palette(ui.visuals());
            fmt.color = p.highlight_fg;
            fmt.background = p.highlight_bg;
        }
        job.append(&part, 0.0, fmt);
    }
    job
}

/// "01:30부터 열기", "4페이지 열기" 또는 "열기"
pub fn open_label(hv: &HitView) -> String {
    match Place::of(&hv.hit.path, hv.hit.kind, hv.hit.modality, hv.hit.locator) {
        Place::Seconds(t) => format!("{}부터 열기", fmt_ts((t * 1000.0) as i64)),
        Place::Page(n) => format!("{n}페이지 열기"),
        Place::Start => "열기".into(),
    }
}

fn is_media(s: Section) -> bool {
    matches!(s, Section::Images | Section::Videos)
}

/// 새 결과 목록에서 선택을 정한다: 고르던 것이 그대로 있으면 그것, 없으면 같은 줄 위치
/// (목록이 짧아졌으면 마지막 줄, 비었으면 선택 없음).
fn kept_selection<K: PartialEq + Copy>(prev: Option<K>, row: usize, shown: &[K]) -> Option<K> {
    match prev {
        Some(k) if shown.contains(&k) => Some(k),
        _ => shown.get(row.min(shown.len().saturating_sub(1))).copied(),
    }
}

#[cfg(test)]
mod tests {
    use super::kept_selection;

    #[test]
    fn keeps_the_chosen_item_when_results_are_replaced() {
        // 같은 검색이 다시 돌아 같은 결과가 왔다: 고르던 것 그대로
        assert_eq!(kept_selection(Some(30), 2, &[10, 20, 30, 40]), Some(30));
        // 순서가 바뀌어도 고르던 파일을 따라간다
        assert_eq!(kept_selection(Some(30), 2, &[30, 10, 20]), Some(30));
        // 고르던 파일이 빠졌으면 같은 줄 위치
        assert_eq!(kept_selection(Some(30), 2, &[10, 20, 50, 60]), Some(50));
        // 목록이 짧아졌으면 마지막 줄, 비었으면 선택 없음
        assert_eq!(kept_selection(Some(30), 2, &[10]), Some(10));
        assert_eq!(kept_selection(Some(30), 2, &[] as &[i32]), None);
    }
}
