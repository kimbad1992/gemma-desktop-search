//! 검색 탭: 질의 입력, 필터 한 줄(종류·폴더·경로·관련도), 결과(통합 목록 또는 섹션별), 오른쪽 미리보기

use crate::app::{FolderStats, GemmaApp};
use crate::extract::{Kind, Modality};
use crate::open::Place;
use crate::search::keyword;
use crate::search::query::{HitView, QueryKind};
use crate::search::store::Section;
use crate::ui::theme::{self, palette, space, Tone};
use crate::ui::widgets::{self, Btn, ScoreStyle};
use crate::util::{fmt_num, fmt_ts};
use eframe::egui::{self, RichText};
use std::sync::atomic::Ordering;

const THUMB: f32 = 72.0;
pub const SEARCH_BOX: &str = "search_box";
const CARD_W: f32 = 168.0;

/// 처음 화면에 보여주는 예시 검색어
const EXAMPLES: [&str; 3] = ["바다가 보이는 사진", "예산 회의록", "고양이 영상"];

/// 최소 일치도 이상이거나 질의 단어가 들어 있는 결과 (순위는 키워드 가산점까지 반영된 순서 그대로)
pub fn passing(hits: &[HitView], min: f32) -> Vec<&HitView> {
    hits.iter().filter(|h| h.visible(min)).collect()
}

impl GemmaApp {
    pub fn search_tab(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let running = self.search_rx.is_some();

        // ── 질의 입력 ──
        ui.add_space(space::XS);
        let (mut submit, mut pick_image) = (false, false);
        ui.horizontal(|ui| {
            // 버튼 두 개 자리를 남기고 입력칸이 나머지를 채운다
            let w = (ui.available_width() - 236.0).max(200.0);
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(egui::Id::unique(SEARCH_BOX))
                    .hint_text("찾고 싶은 내용을 적어 보세요 (예: 바다가 보이는 사진, 예산 회의록, 고양이 영상)")
                    .font(theme::query_font())
                    .desired_width(w)
                    .margin(egui::vec2(10.0, 8.0)),
            );
            if self.focus_search {
                resp.request_focus();
                self.focus_search = false;
            }
            if self.select_query {
                // 단축키로 불렀을 때: 이전 질의를 전부 선택해 바로 새로 입력
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
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
                resp.request_focus();
            }
            if widgets::button_enabled(ui, Btn::Primary, !running, "검색").clicked() {
                submit = true;
            }
            if widgets::button_enabled(ui, Btn::Secondary, !running, "이미지로 검색…")
                .on_hover_text("이미지 파일을 고르거나 창에 끌어다 놓으면 비슷한 이미지·영상을 찾습니다")
                .clicked()
            {
                pick_image = true;
            }
        });
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

        // ── 필터 한 줄 ──
        self.filter_bar(ui);

        // ── 지금 검색할 수 없는 이유와 해결 방법 ──
        let server_state = self.server.state();
        let loading = self.store_loading.load(Ordering::SeqCst);
        let (files, chunks) = {
            let st = self.store.read();
            (st.file_count(), st.live_chunks())
        };
        let (mut go_settings, mut start_server) = (false, false);
        if loading {
            ui.horizontal(|ui| {
                ui.spinner();
                let s = self.store_status.lock().clone();
                ui.label(if s.is_empty() { "인덱스를 불러오는 중…".to_string() } else { s });
            });
        } else if files == 0 {
            widgets::callout_ui(ui, Tone::Warn, "검색할 자료가 아직 없습니다", |ui| {
                ui.label("설정에서 폴더를 추가하고 검색 준비(인덱싱)를 하면 여기서 찾을 수 있습니다.");
                go_settings = widgets::button(ui, Btn::Secondary, "설정 열기").clicked();
            });
        } else if !server_state.is_running() && !self.cfg.server_on_demand {
            // 자동으로 켜는 설정이면 검색할 때 알아서 켜지므로 알릴 필요가 없다
            widgets::callout_ui(ui, Tone::Warn, "서버가 꺼져 있습니다", |ui| {
                ui.label("검색하려면 서버를 켜야 합니다.");
                start_server = widgets::button(ui, Btn::Secondary, "서버 시작").clicked();
            });
        }
        if go_settings {
            self.tab = crate::app::Tab::Settings;
        }
        if start_server {
            self.start_server_manual();
        }

        // ── 요약 줄 ──
        ui.horizontal(|ui| {
            if running {
                ui.spinner();
                let secs = self.search_started.map(|t| t.elapsed().as_secs_f32()).unwrap_or(0.0);
                ui.label(format!("검색 중… {secs:.1}초"));
            } else if self.pending_search_waiting() || (server_state.is_running() && !server_state.is_ready()) {
                ui.spinner();
                ui.label("서버를 준비하는 중…");
            } else if let Some(o) = &self.outcome {
                if let Some(e) = &o.error {
                    widgets::tone_label(ui, Tone::Danger, format!("검색 실패: {e}"));
                } else {
                    let min = self.cfg.min_relevance;
                    let shown: usize = o.sections.iter().map(|(_, v)| passing(v, min).len()).sum();
                    ui.label(RichText::new(&o.label).text_style(theme::ts_strong()).strong());
                    ui.label(if min > 0.0 {
                        format!("{shown}개 파일 · 관련도 {min:.1} 이상")
                    } else {
                        format!("{shown}개 파일")
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        widgets::caption(
                            ui,
                            &format!("{:.2}초 · 벡터 {}개 비교", o.elapsed.as_secs_f32(), fmt_num(o.searched_chunks as u64)),
                        );
                    });
                }
            } else if files > 0 {
                widgets::caption(
                    ui,
                    &format!("파일 {}개 · 벡터 {}개에서 찾습니다", fmt_num(files as u64), fmt_num(chunks as u64)),
                );
            }
        });

        // 끌어다 놓기 안내
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            widgets::callout(ui, Tone::Info, "이미지를 놓으면 비슷한 이미지·영상을 찾습니다", "");
        }
        ui.add_space(space::XS);

        // ── 결과 ──
        let Some(outcome) = self.outcome.take() else {
            widgets::empty_state(
                ui,
                "문서·사진·영상·소리를 한 번에 찾습니다",
                "한국어와 영어 모두 됩니다. 사진을 창에 끌어다 놓으면 비슷한 사진·영상을 찾습니다.",
            );
            let mut example = None;
            ui.add_space(space::M);
            ui.vertical_centered(|ui| {
                widgets::caption(ui, "이렇게 찾아 보세요");
                for ex in EXAMPLES {
                    if widgets::chip(ui, false, ex).clicked() {
                        example = Some(ex);
                    }
                }
            });
            if let Some(ex) = example {
                self.query = ex.to_string();
                if !running {
                    self.start_search(QueryKind::Text(ex.to_string()));
                }
            }
            return;
        };
        // 섹션 칩: 전체(종류와 상관없이 일치도 순) / 섹션별 전체 보기 (최소 일치도 미만은 제외)
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
        if !visible.is_empty() {
            ui.horizontal_wrapped(|ui| {
                if widgets::chip(ui, self.result_view.is_none(), &format!("전체 {}", all.len()))
                    .on_hover_text("파일 종류와 상관없이 관련도가 높은 순서 (같은 파일은 한 번만)")
                    .clicked()
                {
                    self.result_view = None;
                }
                for (sec, hits) in &visible {
                    let sel = self.result_view == Some(*sec);
                    if widgets::chip(ui, sel, &format!("{} {}", sec.label(), hits.len())).clicked() {
                        self.result_view = Some(*sec);
                    }
                }
            });
            ui.add_space(space::XS);
        }

        // 화면에 보이는 순서 (키보드로 이동)
        let shown: Vec<(Section, &HitView)> = match self.result_view {
            None => all.clone(),
            Some(sec) => visible
                .iter()
                .filter(|(s, _)| *s == sec)
                .flat_map(|(s, h)| h.iter().map(move |hv| (*s, *hv)))
                .collect(),
        };
        self.handle_result_keys(&ctx, &shown);
        // 선택한 결과는 오른쪽에 크게
        let selected = self.selected.and_then(|(s, f)| {
            shown
                .iter()
                .find(|(ss, hv)| *ss == s && hv.hit.file_id == f)
                .map(|(_, hv)| *hv)
        });
        if let Some(hv) = selected {
            egui::Panel::right("preview_panel")
                .resizable(true)
                .default_size(420.0)
                .size_range(300.0..=800.0)
                .show(ui, |ui| self.preview_panel(ui, &ctx, hv, outcome.expr.as_deref()));
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match self.result_view {
                None => {
                    for (sec, hv) in &all {
                        self.result_row(ui, &ctx, *sec, hv, true);
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
                    widgets::empty_state(
                        ui,
                        &format!("관련도 {min:.1} 이상인 결과가 없습니다"),
                        "맞는 파일이 없거나 약하게만 맞습니다. 필터 줄의 관련도를 낮춰 보세요.",
                    );
                } else {
                    widgets::empty_state(ui, "결과가 없습니다", "필터 줄의 종류·폴더·경로를 확인하세요.");
                }
            }
        });
        self.outcome = Some(outcome);
    }

    /// 필터 한 줄: 종류 · 폴더(둘 다 체크 드롭다운) · 경로 · 관련도
    fn filter_bar(&mut self, ui: &mut egui::Ui) {
        use crate::search::store::OTHER_FOLDER_KEY;
        let folders = self.folders.lock().clone();
        let has_folders = !(folders.list.is_empty() && folders.other == 0);
        let (mut folders_changed, mut reset) = (false, false);

        ui.horizontal_wrapped(|ui| {
            // 종류
            let on = self.filter_sections.iter().filter(|b| **b).count();
            let total = self.filter_sections.len();
            let summary = if on == total { "종류: 전체".to_string() } else { format!("종류: {on}/{total}") };
            widgets::multi_dropdown(ui, "filter_kinds", &summary, on != total, |ui| {
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
                let summary = if folders_filtered { format!("폴더: {n_on}/{n_total}") } else { "폴더: 전체".to_string() };
                widgets::multi_dropdown(ui, "filter_folders", &summary, folders_filtered, |ui| {
                    folders_changed |= self.folder_menu(ui, &folders);
                });
            }

            // 경로
            ui.add(
                egui::TextEdit::singleline(&mut self.path_filter)
                    .hint_text("경로에 포함된 글자")
                    .desired_width(170.0),
            )
            .on_hover_text("경로에 이 글자가 들어 있는 결과만 보여줍니다 (예: 사진 2024)");

            // 관련도: 줄이 바뀔 때 슬라이더와 "끄기"가 떨어지지 않게 한 덩어리로 놓는다
            let group = egui::vec2(310.0, ui.spacing().interact_size.y);
            ui.allocate_ui_with_layout(group, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(RichText::new("관련도").weak());
                let mut v = self.cfg.min_relevance;
                let r = ui
                    .scope(|ui| {
                        ui.spacing_mut().slider_width = 120.0;
                        ui.add(egui::Slider::new(&mut v, 0.0..=12.0).step_by(0.1).fixed_decimals(1))
                    })
                    .inner
                    .on_hover_text(
                        "이 관련도보다 낮은 결과를 숨깁니다 (다시 검색하지 않고 바로 적용).\n관련도 = 이 검색에서 대부분의 파일(중앙값)보다 표준편차 몇 배만큼 튀는지.\n대략 7 이상 뚜렷함, 5.5 이상 관련 있음. 맞는 파일이 없는 검색은 보통 5.5 아래라 결과가 비게 됩니다.",
                    );
                if r.changed() {
                    self.cfg.min_relevance = v;
                }
                let mut persist = r.drag_stopped() || (r.changed() && !r.dragged());
                if self.cfg.min_relevance > 0.0
                    && ui.small_button("끄기").on_hover_text("관련도와 상관없이 모두 보여줍니다").clicked()
                {
                    self.cfg.min_relevance = 0.0;
                    persist = true;
                }
                if persist {
                    self.persist_min_score();
                }
            });

            // 걸러내는 중일 때만 초기화 링크
            if (on != total || folders_filtered || !self.path_filter.trim().is_empty())
                && ui.link("필터 초기화").clicked()
            {
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
                    self.selected = Some((shown[p].0, shown[p].1.hit.file_id));
                    self.scroll_to_selected = true;
                }
            }
            if reveal {
                if let Some(p) = p {
                    crate::util::reveal_path(&shown[p].1.hit.path);
                    self.leave_quick_after_open = true;
                }
            }
            return;
        }
        if box_focused {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                let (s, hv) = shown[pos.unwrap_or(0)];
                self.selected = Some((s, hv.hit.file_id));
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
                self.selected = Some((shown[p].0, shown[p].1.hit.file_id));
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
            crate::util::reveal_path(&hv.hit.path);
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
        match crate::util::copy_file_to_clipboard(path) {
            Ok(()) => self.notify("파일을 클립보드에 복사했습니다 (탐색기·메신저에 붙여넣기)", false),
            Err(e) => self.notify(format!("파일 복사 실패: {e}"), true),
        }
    }

    fn render_hits(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sec: Section, hits: &[&HitView]) {
        if is_media(sec) {
            // 카드 폭을 알고 있으니 한 줄에 들어갈 개수를 계산해 줄 단위로 배치
            let gap = ui.spacing().item_spacing.x;
            let cols = ((ui.available_width() + gap) / (CARD_W + 12.0 + gap)).floor().max(1.0) as usize;
            for row in hits.chunks(cols) {
                ui.horizontal(|ui| {
                    for hv in row {
                        self.media_card(ui, ctx, sec, hv);
                    }
                });
            }
        } else {
            for hv in hits {
                self.result_row(ui, ctx, sec, hv, false);
            }
        }
    }

    /// 결과 카드·줄의 틀: 선택된 것은 강조색 테두리와 옅은 강조 배경
    fn hit_frame(ui: &egui::Ui, selected: bool) -> egui::Frame {
        let p = palette(ui.visuals());
        egui::Frame::new()
            .fill(if selected { p.accent_soft } else { p.card_fill })
            .stroke(egui::Stroke::new(1.0, if selected { p.accent } else { p.card_stroke }))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS))
    }

    /// 이미지·영상용 썸네일 카드
    fn media_card(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sec: Section, hv: &HitView) {
        const W: f32 = CARD_W;
        const H: f32 = 118.0;
        let tex = hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c));
        let is_sel = self.selected == Some((sec, hv.hit.file_id));
        let resp = Self::hit_frame(ui, is_sel)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_width(W);
                ui.vertical(|ui| {
                    match &tex {
                        Some(t) => {
                            ui.add_sized([W, H], egui::Image::new(t).max_size(egui::vec2(W, H)).corner_radius(4));
                        }
                        None => {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(W, H), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 4.0, palette(ui.visuals()).inset_fill);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                kind_tag(hv.hit.kind),
                                egui::FontId::proportional(13.0),
                                ui.visuals().weak_text_color(),
                            );
                        }
                    }
                    ui.add(egui::Label::new(RichText::new(&hv.file_name).text_style(theme::ts_strong()).strong()).truncate());
                    ui.horizontal(|ui| {
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
            })
            .response
            .interact(egui::Sense::click());
        self.after_hit_response(ctx, sec, hv, &resp);
        resp.on_hover_text(format!("{}\n클릭: 미리보기 · 더블클릭: 열기 · 우클릭: 메뉴", hv.hit.path));
    }

    /// 결과 카드/줄 공통: 클릭 선택, 더블클릭 열기, 우클릭 메뉴, 키보드 이동 시 화면에 보이게
    fn after_hit_response(&mut self, ctx: &egui::Context, sec: Section, hv: &HitView, resp: &egui::Response) {
        if resp.clicked() {
            self.selected = Some((sec, hv.hit.file_id));
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
                crate::util::reveal_path(&hv.hit.path);
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
    fn result_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, sec: Section, hv: &HitView, show_kind: bool) {
        let is_sel = self.selected == Some((sec, hv.hit.file_id));
        let resp = Self::hit_frame(ui, is_sel)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // 썸네일 또는 종류 표시
                    let tex = hv.thumb_chunk.and_then(|c| self.thumb_texture(ctx, c));
                    match tex {
                        Some(t) => {
                            ui.add(egui::Image::new(&t).max_size(egui::vec2(THUMB, THUMB)).corner_radius(4));
                        }
                        None => {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(THUMB, 44.0), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 4.0, palette(ui.visuals()).inset_fill);
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                kind_tag(hv.hit.kind),
                                egui::FontId::proportional(12.0),
                                ui.visuals().weak_text_color(),
                            );
                        }
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&hv.file_name).text_style(theme::ts_item()).strong());
                            if show_kind {
                                widgets::caption(ui, sec.label());
                            }
                            // 어디가 맞았는지: 작은 색 글자로 (배지는 제목보다 눈에 띄어서 쓰지 않는다)
                            if let Some(loc) = locator_label(hv) {
                                ui.label(RichText::new(loc).small().color(Tone::Info.fg(ui.visuals())));
                            }
                            if hv.keyword_match() {
                                ui.label(RichText::new("검색어 포함").small().color(Tone::Ok.fg(ui.visuals())))
                                    .on_hover_text("파일 이름이나 내용에 검색어가 그대로 들어 있습니다");
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                widgets::score_bar(ui, hv.hit.score, ScoreStyle::Row);
                            });
                        });
                        ui.add(egui::Label::new(RichText::new(&hv.hit.path).small().weak()).truncate());
                        if let Some(h) = &hv.highlight {
                            ui.add(egui::Label::new(highlight_job(ui, h, 13.0)).wrap());
                        } else if !hv.snippet.is_empty() && hv.hit.modality == Modality::Text {
                            ui.add(egui::Label::new(RichText::new(&hv.snippet).size(13.0)).wrap());
                        }
                    });
                });
            })
            .response
            .interact(egui::Sense::click());
        self.after_hit_response(ctx, sec, hv, &resp);
        ui.add_space(2.0);
    }
}

/// 긴 경로는 앞부분을 줄여서 표시 (`…\사진\2024`)
fn short_path(p: &str) -> String {
    let p = p.trim_end_matches('\\');
    if p.chars().count() <= 32 {
        return p.to_string();
    }
    let parts: Vec<&str> = p.split('\\').filter(|s| !s.is_empty()).collect();
    match parts.len() {
        0 => p.to_string(),
        1 => crate::util::truncate_chars(p, 32),
        n => format!("…\\{}\\{}", parts[n - 2], parts[n - 1]),
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
            fmt.color = ui.visuals().strong_text_color();
            fmt.background = palette(ui.visuals()).highlight_bg;
        }
        job.append(&part, 0.0, fmt);
    }
    job
}

/// "전체" 목록 최대 개수
const UNIFIED_LIMIT: usize = 60;

/// 모든 섹션을 합쳐 순위 점수(보정 일치도 + 키워드 가산점) 순으로. 같은 파일은 가장 높은 것 하나만.
/// "고양이 영상"처럼 종류를 말한 질의는 그 종류에 작은 가산점.
pub fn unified<'a>(visible: &[(Section, Vec<&'a HitView>)], hints: &[Section]) -> Vec<(Section, &'a HitView)> {
    // 표시된 일치도 순서 그대로. 종류를 말한 질의(hints)는 같은 점수일 때만 그 종류를 앞으로.
    let mut all: Vec<(Section, &HitView)> =
        visible.iter().flat_map(|(s, h)| h.iter().map(move |hv| (*s, *hv))).collect();
    all.sort_by(|a, b| {
        b.1.hit
            .score
            .total_cmp(&a.1.hit.score)
            .then_with(|| hints.contains(&b.0).cmp(&hints.contains(&a.0)))
    });
    let mut seen = std::collections::HashSet::new();
    all.retain(|(_, hv)| seen.insert(hv.hit.file_id));
    // 같은 파일이 여러 폴더에 있으면(사진 폴더 + Immich 라이브러리 등) 한 번만: 이름이 같고 점수가 사실상 같으면 복사본
    let mut kept: Vec<(Section, &HitView)> = Vec::with_capacity(all.len());
    for (s, hv) in all {
        let dup = kept.iter().any(|(ks, k)| {
            *ks == s && k.file_name.eq_ignore_ascii_case(&hv.file_name) && (k.hit.cosine - hv.hit.cosine).abs() < 0.003
        });
        if !dup {
            kept.push((s, hv));
        }
    }
    kept.truncate(UNIFIED_LIMIT);
    kept
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

pub fn locator_label(hv: &HitView) -> Option<String> {
    match (hv.hit.modality, hv.hit.kind) {
        (Modality::Video, _) => Some(format!("{}~ 구간", fmt_ts(hv.hit.locator))),
        (Modality::Image, Kind::Video) => Some(format!("{} 장면", fmt_ts(hv.hit.locator))),
        (Modality::Audio, _) => Some(format!("{}부터", fmt_ts(hv.hit.locator))),
        (Modality::Text, Kind::Doc) if crate::extract::pdf::page_of(hv.hit.locator).is_some() => {
            crate::extract::pdf::page_of(hv.hit.locator).map(|p| format!("{p}페이지"))
        }
        (Modality::Text, _) if hv.hit.locator > 0 => Some(format!("{}번째 구간", hv.hit.locator + 1)),
        _ => None,
    }
}
