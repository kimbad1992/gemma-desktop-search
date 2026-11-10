//! 설정 탭의 틀: 일반 | 고급 전환, 저장·되돌리기, 두 화면이 같이 쓰는 부품.
//!
//! 본문은 `settings_basic.rs`(쉬운 말, 이것만으로 검색을 시작할 수 있음)와
//! `settings_advanced.rs`(전문 항목)에 있다. 저장·되돌리기는 두 화면의 설정 전체에 한 번에 적용된다.

use crate::app::{GemmaApp, SettingsView};
use crate::ui::theme::{self, space, Tone};
use crate::ui::widgets::{self, Btn};
use eframe::egui;
use std::path::Path;

pub(super) enum Pick {
    File(&'static str, &'static [&'static str]),
    Folder,
    FileOrFolder(&'static str, &'static [&'static str]),
}

/// ✔ 좋음 / ✖ 문제
pub(super) fn status(ui: &mut egui::Ui, ok: bool, ok_text: &str, bad_text: &str) {
    let (tone, text) = if ok { (Tone::Ok, ok_text) } else { (Tone::Danger, bad_text) };
    // 긴 경로가 카드 폭을 밀어내지 않도록 자르고, 전체는 마우스를 올리면 보인다
    let job = widgets::icon_text_job(ui, tone.glyph(), text, tone.fg(ui.visuals()));
    ui.add(egui::Label::new(job).truncate()).on_hover_text(text);
}

fn start_dir(value: &str) -> Option<std::path::PathBuf> {
    let p = Path::new(value.trim());
    if p.is_dir() {
        Some(p.to_path_buf())
    } else {
        p.parent().filter(|d| d.is_dir()).map(Path::to_path_buf)
    }
}

const LABEL_W: f32 = widgets::FIELD_LABEL_W;

fn row_label(ui: &mut egui::Ui, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(LABEL_W, ui.spacing().interact_size.y),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(LABEL_W);
            ui.label(label);
        },
    );
}

/// 라벨 아래 들여쓴 상태 줄
pub(super) fn status_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        // 글 한 줄(또는 낮은 버튼)이라 줄 높이를 낮춘다
        ui.spacing_mut().interact_size.y = 24.0;
        ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
        add(ui);
    });
}

/// 라벨 | 텍스트 입력 | 찾아보기 버튼
pub(super) fn path_row(ui: &mut egui::Ui, label: &str, value: &mut String, pick: Pick, hint: &str) -> bool {
    let mut changed = false;
    let start = start_dir(value);
    ui.horizontal(|ui| {
        row_label(ui, label);
        let buttons_w = match pick {
            Pick::FileOrFolder(..) => 160.0,
            _ => 100.0,
        };
        let w = (ui.available_width() - buttons_w).clamp(200.0, 760.0);
        changed |= ui
            .add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(w))
            .changed();
        let dialog = || {
            let d = rfd::FileDialog::new();
            match &start {
                Some(s) => d.set_directory(s),
                None => d,
            }
        };
        match pick {
            Pick::File(name, exts) => {
                if ui.button("찾아보기…").clicked() {
                    if let Some(p) = dialog().add_filter(name, exts).pick_file() {
                        *value = p.display().to_string();
                        changed = true;
                    }
                }
            }
            Pick::Folder => {
                if ui.button("폴더…").clicked() {
                    if let Some(p) = dialog().pick_folder() {
                        *value = p.display().to_string();
                        changed = true;
                    }
                }
            }
            Pick::FileOrFolder(name, exts) => {
                if ui.button("폴더…").clicked() {
                    if let Some(p) = dialog().pick_folder() {
                        *value = p.display().to_string();
                        changed = true;
                    }
                }
                if ui.button("파일…").clicked() {
                    if let Some(p) = dialog().add_filter(name, exts).pick_file() {
                        *value = p.display().to_string();
                        changed = true;
                    }
                }
            }
        }
    });
    changed
}

impl GemmaApp {
    pub fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let dirty = self.cfg != self.saved_cfg;
        let (mut save, mut revert) = (false, false);
        // 고정 머리: 화면 전환과 저장·되돌리기 (본문과 같은 폭으로 가운데 정렬)
        ui.add_space(space::XS);
        widgets::centered(ui, theme::PAGE_W, |ui| {
            ui.horizontal(|ui| {
                widgets::page_title(ui, "설정");
                ui.add_space(space::M);
                widgets::segmented(
                    ui,
                    &mut self.settings_view,
                    &[(SettingsView::Basic, "일반"), (SettingsView::Advanced, "고급")],
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    save = widgets::button_enabled(ui, Btn::Primary, dirty, "저장").clicked();
                    revert = widgets::button_enabled(ui, Btn::Secondary, dirty, "되돌리기").clicked();
                    if dirty {
                        widgets::tone_label(ui, Tone::Warn, "저장하지 않은 변경 사항이 있습니다");
                    }
                });
            });
        });
        if save {
            self.save_settings();
        }
        if revert {
            self.revert_settings();
        }
        ui.add_space(space::XS);
        ui.separator();

        widgets::page(ui, "settings_page", theme::PAGE_W, |ui| match self.settings_view {
            SettingsView::Basic => self.settings_basic(ui),
            SettingsView::Advanced => self.settings_advanced(ui),
        });
    }

    /// 설정 전체를 파일에 쓴다 (실행 중인 동작은 이미 화면의 값을 쓰고 있다)
    pub fn save_settings(&mut self) {
        match self.cfg.save(&self.data_dir) {
            Ok(()) => {
                if let Err(e) = crate::autostart::sync(self.cfg.start_with_windows) {
                    self.notify(format!("로그인 시 실행 설정 실패: {e}"), true);
                } else {
                    self.notify("설정을 저장했습니다", false);
                }
                self.saved_cfg = self.cfg.clone();
            }
            Err(e) => self.notify(format!("설정 저장 실패: {e}"), true),
        }
    }

    /// 저장하지 않은 변경을 버린다
    pub fn revert_settings(&mut self) {
        let dim_changed = self.cfg.search_dim != self.saved_cfg.search_dim;
        self.cfg = self.saved_cfg.clone();
        self.exclude_text = self.cfg.exclude_dirs.join("\n");
        self.invalidate_health();
        if dim_changed {
            self.reload_store();
        }
    }

    /// 준비물을 받는 중이면 진행 막대와 취소 버튼을 그린다. 받는 중이었으면 true.
    pub fn download_progress(&mut self, ui: &mut egui::Ui) -> bool {
        let st = self.dl.lock().clone();
        if !st.running {
            return false;
        }
        let frac = if st.total > 0 { st.done as f32 / st.total as f32 } else { 0.0 };
        let accent = theme::palette(ui.visuals()).accent;
        widgets::progress(ui, frac, accent, 8.0, false);
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} · {:.0}/{:.0}MB{}",
                st.file,
                st.done as f64 / 1048576.0,
                st.total as f64 / 1048576.0,
                if self.dl_queue.is_empty() {
                    String::new()
                } else {
                    format!(" (남은 {}개)", self.dl_queue.len())
                }
            ));
            if widgets::button(ui, Btn::Secondary, "취소").clicked() {
                self.dl_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                self.dl_queue.clear();
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        true
    }
}

/// 모델 파일과 같은 폴더에서 맞는 mmproj 파일을 찾는다
/// 검사 결과 한 줄 (✔ 초록 / ⚠ / ✖ 이유). 아직 확인 중이면 돌아가는 표시.
pub(super) fn verdict_line(ui: &mut egui::Ui, v: Option<&crate::precheck::Verdict>, pending: &str) {
    use crate::sysinfo::Level;
    match v {
        None => {
            widgets::spinner(ui);
            ui.label(egui::RichText::new(pending).weak());
        }
        Some(v) => {
            let tone = match v.level {
                Level::Ok => Tone::Ok,
                Level::Warn => Tone::Warn,
                Level::Bad => Tone::Danger,
            };
            let job = widgets::icon_text_job(ui, tone.glyph(), &v.text, tone.fg(ui.visuals()));
            ui.add(egui::Label::new(job).wrap());
        }
    }
}

pub(super) fn find_mmproj(model_path: &str) -> Option<String> {
    let dir = Path::new(model_path.trim()).parent()?;
    let mut found: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            n.starts_with("mmproj") && n.ends_with(".gguf")
        })
        .collect();
    // 모델과 같은 양자화(Q8 모델 → Q8 mmproj)를 먼저, 없으면 Q8 > BF16 > F32
    let model = Path::new(model_path.trim())
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let quant = ["q8_0", "bf16", "f32", "q4"].into_iter().find(|q| model.contains(q));
    found.sort_by_key(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if quant.is_some_and(|q| n.contains(q)) {
            0
        } else if n.contains("q8") {
            1
        } else if n.contains("bf16") {
            2
        } else if n.contains("f32") {
            3
        } else {
            4
        }
    });
    found.first().map(|p| p.display().to_string())
}
