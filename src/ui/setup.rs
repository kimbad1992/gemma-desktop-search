//! 첫 실행 마법사 (포터블): 설치 없이 이 폴더에서 바로 쓰도록, 처음 한 번만
//! PC 확인 → 필요한 파일 준비 → 이 PC에 맞추기 → 검색할 폴더 → 동작 확인.
//! 끝나면 다시 뜨지 않는다 (설정 탭에서 다시 열 수 있음).
//!
//! 사양 확인·준비물 받기·추천값 적용은 설정 화면과 같은 코드를 쓴다 (`app.rs`, `probe.rs`).
//! 준비물은 받을 수도 있고, 이미 받아 둔 llama.cpp 폴더와 모델 파일을 지정할 수도 있다 (`precheck.rs`가 검사).

use super::settings_panel::{find_mmproj, path_row, status_row, verdict_line, Pick};
use crate::app::GemmaApp;
use crate::download::Pack;
use crate::embed::selftest::Check;
use crate::precheck::Verdict;
use crate::sysinfo::Level;
use crate::ui::theme::{space, Tone};
use crate::ui::widgets::{self, Btn};
use eframe::egui::{self, RichText};
use std::path::Path;

/// 마법사 상태
pub struct SetupState {
    /// 받을 llama.cpp 판 (추천값으로 시작, 바꿀 수 있음)
    pub pack: Option<Pack>,
    pub apply_plan: bool,
    pub start_index: bool,
    /// "위치 직접 지정" 칸을 처음부터 펼쳐 둘지 (프로그램 폴더 밖의 파일을 이미 쓰고 있을 때)
    pub own_files: bool,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            pack: None,
            apply_plan: true,
            start_index: true,
            own_files: false,
        }
    }
}

fn level_tone(lv: Level) -> Tone {
    match lv {
        Level::Ok => Tone::Ok,
        Level::Warn => Tone::Warn,
        Level::Bad => Tone::Danger,
    }
}

impl GemmaApp {
    /// 마법사 열기 (사양 확인을 백그라운드로 시작)
    pub fn open_setup(&mut self, ctx: &egui::Context) {
        self.setup_open = true;
        self.setup = SetupState::default();
        // 프로그램 폴더 밖의 llama.cpp·모델을 쓰고 있으면 직접 지정한 것이므로 그 칸을 펼쳐 보여 준다
        let app_dir = crate::config::app_dir().display().to_string().to_lowercase();
        let outside = |p: &str| {
            let p = p.trim().to_lowercase();
            !p.is_empty() && !p.starts_with(&app_dir)
        };
        self.setup.own_files = outside(&self.cfg.llama_path) || outside(&self.cfg.model_path);
        let ffmpeg = self.cfg.ffmpeg().is_some();
        self.probe.start(ffmpeg, ctx);
    }

    fn setup_model_ok(&self) -> bool {
        Path::new(self.cfg.model_path.trim()).is_file()
    }

    fn setup_llama_ok(&self) -> bool {
        crate::server::resolve_binary(&self.cfg.llama_path).is_ok()
    }

    fn finish_setup(&mut self) {
        if self.setup.apply_plan {
            if let Some(plan) = self.probe.plan().cloned() {
                self.apply_plan(&plan);
            }
        }
        self.cfg.setup_done = true;
        match self.cfg.save(&self.data_dir) {
            Ok(()) => self.saved_cfg = self.cfg.clone(),
            Err(e) => self.notify(format!("설정 저장 실패: {e}"), true),
        }
        self.setup_open = false;
        self.invalidate_health();
        self.tab = crate::app::Tab::Search;
        if self.setup.start_index && !self.cfg.roots.is_empty() {
            self.start_index_when_ready();
            self.tab = crate::app::Tab::Index;
        }
        // 단축키가 실제로 등록됐을 때만 안내한다 (안 됐으면 상태 표시줄의 경고가 이유를 알려준다)
        let hotkey = if self.hotkey_available() {
            format!(" 어디서든 {}로 검색할 수 있습니다.", crate::shell::HOTKEY_LABEL)
        } else {
            String::new()
        };
        self.notify(format!("처음 설정을 마쳤습니다.{hotkey}"), false);
    }

    pub fn setup_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // 사양 확인 결과는 app.poll이 받아 둔다. 추천된 llama.cpp 판을 기본 선택으로.
        let recommended = self.probe.plan().map(|p| p.llama);
        if let Some(llama) = recommended {
            self.setup.pack.get_or_insert(llama);
        }
        let plan = self.probe.plan().cloned();
        let model_ok = self.setup_model_ok();
        let llama_ok = self.setup_llama_ok();
        // 있는 파일이 실제로 쓸 만한지: 모델은 종류를 읽고, llama.cpp는 실행해 본다 (끝나기 전에는 None)
        let model_verdict = self.file_checks.model(&self.cfg);
        let engine_verdict = self.file_checks.engine(&self.cfg, &ctx);
        let files_ok = model_ok
            && model_verdict.usable()
            && llama_ok
            && engine_verdict.as_ref().is_some_and(Verdict::usable);
        let ffmpeg_ok = self.cfg.ffmpeg().is_some();
        let ready = files_ok && !self.cfg.roots.is_empty();
        let own_files_open = self.setup.own_files;

        widgets::page(ui, "setup_page", 760.0, |ui| {
            ui.horizontal(|ui| {
                widgets::page_title(ui, "Gemma Search 처음 설정");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, Btn::Ghost, "나중에 하기")
                        .on_hover_text("설정 탭의 '처음 설정 다시 열기'로 언제든 돌아올 수 있습니다")
                        .clicked()
                    {
                        self.setup_open = false;
                    }
                });
            });
            widgets::help(
                ui,
                &format!(
                    "설치 없이 이 폴더({})에서 바로 씁니다. 필요한 파일을 준비하고 검색할 폴더만 고르면 됩니다.",
                    crate::config::app_dir().display()
                ),
            );

            // ── 1. PC 확인 ──
            widgets::step_section(ui, 1, "PC 확인", plan.is_some(), None, |ui| match &plan {
                None => {
                    ui.horizontal(|ui| {
                        widgets::spinner(ui);
                        ui.label("그래픽카드·메모리·디스크를 확인하는 중…");
                    });
                }
                Some(plan) => {
                    ui.label(RichText::new(plan.plain_summary()).strong());
                    for (level, name, text) in &plan.checks {
                        widgets::status_line(ui, level_tone(*level), &format!("{name}: {text}"));
                    }
                    if plan.checks.iter().any(|c| c.0 == Level::Bad) {
                        widgets::tone_note(ui, Tone::Danger, "표시된 항목은 실행이 어렵습니다. 그래도 계속할 수는 있습니다.");
                    }
                    if widgets::button(ui, Btn::Ghost, "다시 확인").clicked() {
                        self.probe.start(ffmpeg_ok, &ctx);
                    }
                }
            });

            // ── 2. 필요한 파일 ──
            widgets::step_section(
                ui,
                2,
                "필요한 파일 준비",
                files_ok,
                Some("AI 모델(EmbeddingGemma 2)과 그 모델을 돌리는 프로그램(llama.cpp)이 필요합니다. 받으면 이 프로그램 폴더 아래에 놓이고, 폴더째 옮겨도 그대로 씁니다."),
                |ui| {
                    // AI 모델
                    if !model_ok {
                        widgets::status_line(ui, Tone::Warn, &format!("받을 것: {}", Pack::Model.plain_label()));
                    } else if model_verdict.level == Level::Ok {
                        widgets::status_line(ui, Tone::Ok, "AI 모델이 있습니다");
                    } else {
                        let text = format!("AI 모델: {}", model_verdict.text);
                        widgets::status_line(ui, level_tone(model_verdict.level), &text);
                    }
                    // llama.cpp
                    if !llama_ok {
                        ui.horizontal(|ui| {
                            ui.colored_label(Tone::Warn.fg(ui.visuals()), Tone::Warn.icon());
                            ui.label("받을 것:");
                            let current = self.setup.pack.unwrap_or(Pack::LlamaCuda13);
                            egui::ComboBox::from_id_salt("setup_pack")
                                .selected_text(current.plain_label())
                                .show_ui(ui, |ui| {
                                    for p in [Pack::LlamaCuda13, Pack::LlamaCuda12, Pack::LlamaVulkan, Pack::LlamaCpu] {
                                        let label = if recommended == Some(p) {
                                            format!("{} · 추천", p.plain_label())
                                        } else {
                                            p.plain_label().to_string()
                                        };
                                        ui.selectable_value(&mut self.setup.pack, Some(p), label);
                                    }
                                });
                        });
                    } else {
                        match &engine_verdict {
                            None => {
                                ui.horizontal(|ui| {
                                    widgets::spinner(ui);
                                    ui.label("llama.cpp를 실행해 보는 중…");
                                });
                            }
                            Some(v) if v.level == Level::Ok => {
                                widgets::status_line(ui, Tone::Ok, &format!("llama.cpp가 있습니다 · {}", v.text));
                            }
                            Some(v) => {
                                widgets::status_line(ui, level_tone(v.level), &format!("llama.cpp: {}", v.text));
                            }
                        }
                    }
                    if ffmpeg_ok {
                        widgets::status_line(ui, Tone::Ok, "영상·소리 도구(ffmpeg)가 있습니다");
                    } else {
                        ui.horizontal(|ui| {
                            ui.colored_label(Tone::Warn.fg(ui.visuals()), Tone::Warn.icon());
                            ui.label("영상·소리 도구(ffmpeg)가 없습니다. 없어도 되지만 영상·소리는 이름으로만 찾습니다.");
                        });
                        ui.horizontal(|ui| {
                            if widgets::button(ui, Btn::Secondary, "ffmpeg 설치")
                                .on_hover_text("명령 창이 열리고 winget으로 설치합니다. 끝나면 '다시 찾기'를 누르세요.")
                                .clicked()
                            {
                                crate::platform::install_ffmpeg();
                            }
                            if widgets::button(ui, Btn::Ghost, "다시 찾기").clicked() {
                                if let Some(p) = crate::util::find_tool(&crate::platform::exe_name("ffmpeg")) {
                                    self.cfg.ffmpeg_path = p.display().to_string();
                                }
                                if let Some(p) = crate::util::find_tool(&crate::platform::exe_name("ffprobe")) {
                                    self.cfg.ffprobe_path = p.display().to_string();
                                }
                            }
                        });
                    }
                    ui.add_space(space::XS);
                    let downloading = self.download_progress(ui);
                    if !downloading
                        && !(model_ok && llama_ok)
                        && widgets::button(ui, Btn::Primary, "필요한 것 모두 받기").clicked()
                    {
                        let llama = self.setup.pack.unwrap_or(Pack::LlamaCuda13);
                        self.download_missing(&ctx, llama);
                    }

                    // 이미 받아 둔 것이 있으면 받지 않고 그 위치를 지정한다
                    ui.add_space(space::S);
                    let (mut llama_changed, mut model_changed, mut recheck) = (false, false, false);
                    widgets::collapsing_section(
                        ui,
                        "setup_own_files",
                        "이미 받아 둔 llama.cpp와 모델이 있나요? 위치 직접 지정",
                        own_files_open,
                        |ui| {
                            widgets::help(
                                ui,
                                "가지고 있는 llama.cpp 폴더와 EmbeddingGemma 2 모델 파일(.gguf)을 고르면 새로 받지 않고 그대로 씁니다. 고르면 바로 검사해서, 쓸 수 있으면 ✔로 알려 줍니다.",
                            );
                            widgets::caption(
                                ui,
                                "llama.cpp(llama-server)만 쓸 수 있습니다. vLLM·Ollama 같은 다른 서버는 지원하지 않습니다.",
                            );
                            ui.add_space(space::XS);
                            llama_changed = path_row(
                                ui,
                                "llama.cpp 폴더",
                                &mut self.cfg.llama_path,
                                Pick::FileOrFolder("실행 파일", &["exe"]),
                                "llama-server.exe가 들어 있는 폴더",
                            );
                            status_row(ui, |ui| {
                                verdict_line(ui, engine_verdict.as_ref(), "실행해 보는 중…");
                                // 빠진 DLL을 채워 넣은 뒤 다시 볼 수 있게
                                if llama_ok && engine_verdict.as_ref().is_some_and(|v| v.level != Level::Ok) {
                                    recheck = widgets::small_button(ui, Btn::Secondary, "다시 검사").clicked();
                                }
                            });
                            model_changed = path_row(
                                ui,
                                "모델 파일 (.gguf)",
                                &mut self.cfg.model_path,
                                Pick::File("GGUF", &["gguf"]),
                                "embeddinggemma-2-*.gguf",
                            );
                            status_row(ui, |ui| verdict_line(ui, Some(&model_verdict), ""));
                            if model_ok && model_verdict.usable() {
                                let has = self.cfg.has_mmproj() && Path::new(self.cfg.mmproj_path.trim()).is_file();
                                status_row(ui, |ui| {
                                    if has {
                                        widgets::tone_note(ui, Tone::Ok, "사진·영상·소리용 파일(mmproj)도 찾았습니다");
                                    } else {
                                        widgets::tone_note(
                                            ui,
                                            Tone::Warn,
                                            "같은 폴더에 mmproj-*.gguf가 없어 글만 내용으로 찾습니다",
                                        );
                                    }
                                });
                            }
                        },
                    );
                    if model_changed {
                        // 모델 옆에 있는 사진·소리용 파일을 함께 잡는다. 없고 예전 것도 못 쓰면 비운다(글만 검색).
                        match find_mmproj(&self.cfg.model_path) {
                            Some(found) => self.cfg.mmproj_path = found,
                            None if !Path::new(self.cfg.mmproj_path.trim()).is_file() => self.cfg.mmproj_path.clear(),
                            None => {}
                        }
                    }
                    if recheck {
                        self.file_checks.recheck();
                    }
                    if llama_changed || model_changed || recheck {
                        self.invalidate_health();
                        ui.ctx().request_repaint();
                    }
                },
            );

            // ── 3. 이 PC에 맞추기 ──
            widgets::step_section(
                ui,
                3,
                "이 PC에 맞추기",
                plan.is_some() && self.setup.apply_plan,
                None,
                |ui| match &plan {
                    Some(plan) => {
                        widgets::toggle_row(
                            ui,
                            &mut self.setup.apply_plan,
                            "이 PC에 맞춘 설정 쓰기",
                            "그래픽카드와 메모리에 맞춰 가장 빠르고 안정적인 값으로 정합니다",
                        );
                        widgets::caption(ui, &plan.summary);
                    }
                    None => widgets::help(ui, "PC 확인이 끝나면 추천 설정이 나옵니다."),
                },
            );

            // ── 4. 검색할 폴더 ──
            widgets::step_section(
                ui,
                4,
                "검색할 폴더",
                !self.cfg.roots.is_empty(),
                Some("사진·문서·영상이 있는 폴더를 고르세요."),
                |ui| {
                    let mut remove = None;
                    for (i, root) in self.cfg.roots.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.colored_label(Tone::Ok.fg(ui.visuals()), Tone::Ok.icon());
                            ui.label(root);
                            if widgets::button(ui, Btn::Ghost, "빼기").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        self.cfg.roots.remove(i);
                    }
                    if widgets::button(ui, Btn::Secondary, "폴더 추가…").clicked() {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            let s = dir.display().to_string();
                            if !self.cfg.roots.contains(&s) {
                                self.cfg.roots.push(s);
                            }
                        }
                    }
                },
            );

            // ── 5. 동작 확인 (선택) ──
            let state = self.server.state();
            let caps = self.server.capabilities();
            widgets::step_section(
                ui,
                5,
                "동작 확인 (선택)",
                caps.is_some(),
                Some("llama.cpp 서버를 켜서 모델이 글·사진·소리를 실제로 처리하는지 시험해 봅니다."),
                |ui| {
                    ui.horizontal(|ui| {
                        let can = files_ok && !state.is_running();
                        if widgets::button_enabled(ui, Btn::Secondary, can, "서버를 켜고 확인").clicked() {
                            if self.setup.apply_plan {
                                if let Some(plan) = &plan {
                                    self.apply_plan(plan);
                                }
                            }
                            self.start_server_auto();
                        }
                        widgets::tone_badge(
                            ui,
                            widgets::server_tone(&state),
                            state.label(),
                            false,
                            state.is_running() && !state.is_ready(),
                        );
                    });
                    if let Some(caps) = &caps {
                        for (name, c) in [("글", &caps.text), ("사진", &caps.image), ("소리", &caps.audio)] {
                            let tone = match c {
                                Check::Ok(_) => Tone::Ok,
                                _ if c.usable() => Tone::Warn,
                                _ => Tone::Danger,
                            };
                            widgets::status_line(ui, tone, &format!("{name}: {}", c.detail()));
                        }
                    }
                },
            );

            // ── 완료 ──
            ui.add_space(space::M);
            widgets::card(ui, |ui| {
                widgets::toggle_row(
                    ui,
                    &mut self.setup.start_index,
                    "끝나면 바로 검색 준비 시작",
                    "고른 폴더의 파일을 읽어 검색할 수 있게 준비합니다 (인덱싱). 파일이 많으면 오래 걸립니다",
                );
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    if widgets::button_enabled(ui, Btn::Primary, ready, "완료").clicked() {
                        self.finish_setup();
                    }
                    if !ready {
                        widgets::help(ui, "필요한 파일을 준비하고 폴더를 하나 이상 고르면 끝낼 수 있습니다.");
                    }
                });
            });
        });
    }
}
