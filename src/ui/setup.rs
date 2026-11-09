//! 첫 실행 마법사 (포터블): 설치 없이 이 폴더에서 바로 쓰도록, 처음 한 번만
//! 사양 확인 → 준비물(모델·llama.cpp) 받기 → 서버 설정 → 폴더 → 동작 확인.
//! 끝나면 다시 뜨지 않는다 (설정 탭에서 다시 열 수 있음).

use crate::app::GemmaApp;
use crate::download::Pack;
use crate::sysinfo::{Level, Plan, SysInfo};
use crate::ui::widgets;
use eframe::egui::{self, RichText};
use std::path::Path;

/// 마법사 상태
pub struct SetupState {
    pub probe: Option<(SysInfo, Plan)>,
    pub probe_rx: Option<crossbeam_channel::Receiver<(SysInfo, Plan)>>,
    /// 받을 llama.cpp 판 (추천값으로 시작, 바꿀 수 있음)
    pub pack: Option<Pack>,
    /// 차례로 받을 준비물
    pub queue: Vec<Pack>,
    pub apply_plan: bool,
    pub start_index: bool,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            probe: None,
            probe_rx: None,
            pack: None,
            queue: Vec::new(),
            apply_plan: true,
            start_index: true,
        }
    }
}

fn level_mark(ui: &mut egui::Ui, lv: Level) {
    let (t, c) = match lv {
        Level::Ok => ("✔", widgets::GREEN),
        Level::Warn => ("⚠", widgets::ORANGE),
        Level::Bad => ("✖", widgets::RED),
    };
    ui.label(RichText::new(t).color(c).strong());
}

fn step(ui: &mut egui::Ui, n: u32, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(format!("{n}. {title}")).size(17.0).strong());
}

impl GemmaApp {
    /// 마법사 열기 (사양 확인을 백그라운드로 시작)
    pub fn open_setup(&mut self, ctx: &egui::Context) {
        self.setup_open = true;
        self.setup = SetupState::default();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ffmpeg = self.cfg.ffmpeg().is_some();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let info = crate::sysinfo::probe(&crate::config::app_dir(), ffmpeg);
            let plan = crate::sysinfo::recommend(&info);
            let _ = tx.send((info, plan));
            ctx.request_repaint();
        });
        self.setup.probe_rx = Some(rx);
    }

    fn setup_model_ok(&self) -> bool {
        Path::new(self.cfg.model_path.trim()).is_file()
    }

    fn setup_llama_ok(&self) -> bool {
        crate::server::resolve_binary(&self.cfg.llama_path).is_ok()
    }

    /// 추천 서버 설정을 설정에 반영
    fn apply_plan(&mut self, plan: &Plan) {
        self.cfg.n_gpu_layers = plan.n_gpu_layers;
        self.cfg.ctx_size = plan.ctx_size;
        self.cfg.parallel = plan.parallel;
        self.cfg.batch_size = plan.batch_size;
        self.cfg.ubatch_size = plan.ubatch_size;
        self.cfg.workers = plan.workers;
    }

    /// 받기가 끝날 때마다 다음 준비물 (app.poll에서 부름)
    pub fn setup_next_download(&mut self, ctx: &egui::Context, last_ok: bool) {
        if !last_ok {
            self.setup.queue.clear();
            return;
        }
        if !self.setup.queue.is_empty() {
            let next = self.setup.queue.remove(0);
            crate::download::start(next, crate::config::app_dir(), self.dl.clone(), self.dl_cancel.clone(), ctx.clone());
        }
    }

    fn finish_setup(&mut self) {
        if self.setup.apply_plan {
            if let Some((_, plan)) = self.setup.probe.clone() {
                self.apply_plan(&plan);
            }
        }
        self.cfg.setup_done = true;
        match self.cfg.save(&self.data_dir) {
            Ok(()) => self.saved_cfg = self.cfg.clone(),
            Err(e) => self.notify(format!("설정 저장 실패: {e}"), true),
        }
        self.setup_open = false;
        self.tab = crate::app::Tab::Search;
        if self.setup.start_index && !self.cfg.roots.is_empty() {
            if self.server.state().is_ready() {
                self.start_index(false);
            } else {
                if !self.server.state().is_running() {
                    self.start_server_auto();
                }
                self.auto_index = true;
            }
            self.tab = crate::app::Tab::Index;
        }
        self.notify("처음 설정을 마쳤습니다. 어디서든 Ctrl+Shift+Space로 검색할 수 있습니다.", false);
    }

    pub fn setup_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some(rx) = &self.setup.probe_rx {
            if let Ok((info, plan)) = rx.try_recv() {
                self.setup.pack.get_or_insert(plan.llama);
                self.setup.probe = Some((info, plan));
                self.setup.probe_rx = None;
            }
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Gemma Search 처음 설정").size(22.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("나중에 하기")
                        .on_hover_text("설정 탭의 '처음 설정 다시 열기'로 언제든 돌아올 수 있습니다")
                        .clicked()
                    {
                        self.setup_open = false;
                    }
                });
            });
            ui.label(
                RichText::new(format!(
                    "설치 없이 이 폴더({})에서 바로 씁니다. 필요한 것을 확인하고 받은 뒤 폴더째 옮겨도 그대로 동작합니다.",
                    crate::config::app_dir().display()
                ))
                .weak(),
            );

            // ── 1. 사양 ──
            step(ui, 1, "사양 확인");
            match &self.setup.probe {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("그래픽카드·메모리·디스크를 확인하는 중…");
                    });
                }
                Some((_, plan)) => {
                    let plan = plan.clone();
                    egui::Grid::new("setup_checks").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                        for (lv, k, v) in &plan.checks {
                            level_mark(ui, *lv);
                            ui.label(RichText::new(k).strong());
                            ui.label(v);
                            ui.end_row();
                        }
                    });
                    if plan.checks.iter().any(|c| c.0 == Level::Bad) {
                        ui.colored_label(widgets::RED, "✖ 표시된 항목은 실행이 어렵습니다. 그래도 계속할 수는 있습니다.");
                    }
                    if ui.small_button("다시 확인").clicked() {
                        self.open_setup(&ctx);
                    }
                }
            }

            // ── 2. 준비물 ──
            step(ui, 2, "준비물 받기");
            let dl = self.dl.lock().clone();
            let model_ok = self.setup_model_ok();
            let llama_ok = self.setup_llama_ok();
            ui.horizontal(|ui| {
                level_mark(ui, if model_ok { Level::Ok } else { Level::Warn });
                ui.label(if model_ok {
                    format!("모델 있음: {}", self.cfg.model_path)
                } else {
                    "모델 없음 — EmbeddingGemma 2 Q8 (865MB)".to_string()
                });
            });
            ui.horizontal(|ui| {
                level_mark(ui, if llama_ok { Level::Ok } else { Level::Warn });
                ui.label(if llama_ok {
                    format!("llama.cpp 있음: {}", self.cfg.llama_path)
                } else {
                    "llama.cpp 없음".to_string()
                });
                if !llama_ok {
                    let current = self.setup.pack.unwrap_or(Pack::LlamaCuda13);
                    egui::ComboBox::from_id_salt("setup_pack")
                        .selected_text(current.label())
                        .show_ui(ui, |ui| {
                            for p in [Pack::LlamaCuda13, Pack::LlamaCuda12, Pack::LlamaVulkan, Pack::LlamaCpu] {
                                ui.selectable_value(&mut self.setup.pack, Some(p), p.label());
                            }
                        });
                }
            });
            let ffmpeg_ok = self.cfg.ffmpeg().is_some();
            ui.horizontal(|ui| {
                level_mark(ui, if ffmpeg_ok { Level::Ok } else { Level::Warn });
                ui.label(if ffmpeg_ok { "ffmpeg 있음".to_string() } else { "ffmpeg 없음 (영상·오디오·HEIC용, 선택)".to_string() });
                if !ffmpeg_ok {
                    if ui
                        .button("winget으로 설치")
                        .on_hover_text("명령 창이 열리고 winget install Gyan.FFmpeg를 실행합니다. 끝나면 '다시 찾기'를 누르세요.")
                        .clicked()
                    {
                        let _ = std::process::Command::new("cmd")
                            .args(["/c", "start", "ffmpeg 설치", "cmd", "/k", "winget install --id Gyan.FFmpeg -e"])
                            .spawn();
                    }
                    if ui.button("다시 찾기").clicked() {
                        if let Some(p) = crate::util::find_tool("ffmpeg.exe") {
                            self.cfg.ffmpeg_path = p.display().to_string();
                        }
                        if let Some(p) = crate::util::find_tool("ffprobe.exe") {
                            self.cfg.ffprobe_path = p.display().to_string();
                        }
                    }
                }
            });
            if dl.running {
                ui.horizontal(|ui| {
                    let frac = if dl.total > 0 { dl.done as f32 / dl.total as f32 } else { 0.0 };
                    ui.add(egui::ProgressBar::new(frac).desired_width(420.0).text(format!(
                        "{} · {:.0}/{:.0}MB{}",
                        dl.file,
                        dl.done as f64 / 1048576.0,
                        dl.total as f64 / 1048576.0,
                        if self.setup.queue.is_empty() { String::new() } else { format!(" (남은 {}개)", self.setup.queue.len()) }
                    )));
                    if ui.button("취소").clicked() {
                        self.dl_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                        self.setup.queue.clear();
                    }
                });
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            } else if !(model_ok && llama_ok) {
                let mut todo = Vec::new();
                if !model_ok {
                    todo.push(Pack::Model);
                }
                if !llama_ok {
                    todo.push(self.setup.pack.unwrap_or(Pack::LlamaCuda13));
                }
                if ui.button(RichText::new("필요한 것 모두 받기").size(15.0)).clicked() {
                    let first = todo.remove(0);
                    self.setup.queue = todo;
                    crate::download::start(first, crate::config::app_dir(), self.dl.clone(), self.dl_cancel.clone(), ctx.clone());
                }
            }

            // ── 3. 서버 설정 ──
            step(ui, 3, "서버 설정");
            if let Some((_, plan)) = &self.setup.probe {
                let summary = plan.summary.clone();
                ui.checkbox(&mut self.setup.apply_plan, format!("이 PC에 맞춘 설정 쓰기 — {summary}"));
            } else {
                ui.label(RichText::new("사양 확인이 끝나면 추천 설정이 나옵니다").weak());
            }

            // ── 4. 폴더 ──
            step(ui, 4, "검색할 폴더");
            let mut remove = None;
            for (i, r) in self.cfg.roots.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("✕").clicked() {
                        remove = Some(i);
                    }
                    ui.label(r);
                });
            }
            if let Some(i) = remove {
                self.cfg.roots.remove(i);
            }
            if ui.button("폴더 추가…").clicked() {
                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                    let s = d.display().to_string();
                    if !self.cfg.roots.contains(&s) {
                        self.cfg.roots.push(s);
                    }
                }
            }
            if self.cfg.roots.is_empty() {
                ui.label(RichText::new("사진·문서·영상이 있는 폴더를 고르세요. NAS 같은 네트워크 폴더도 됩니다.").weak());
            }

            // ── 5. 동작 확인 ──
            step(ui, 5, "동작 확인");
            let st = self.server.state();
            ui.horizontal(|ui| {
                let can = model_ok && llama_ok && !st.is_running();
                if ui
                    .add_enabled(can, egui::Button::new("서버를 켜고 확인"))
                    .on_hover_text("추천 설정으로 서버를 켜고 텍스트·이미지·오디오 임베딩을 시험합니다")
                    .clicked()
                {
                    if self.setup.apply_plan {
                        if let Some((_, plan)) = self.setup.probe.clone() {
                            self.apply_plan(&plan);
                        }
                    }
                    self.start_server_auto();
                }
                widgets::badge(ui, widgets::server_color(&st), st.label(), false, st.is_running() && !st.is_ready());
            });
            if let Some(caps) = self.server.capabilities() {
                for (name, c) in [("텍스트", &caps.text), ("이미지", &caps.image), ("오디오", &caps.audio)] {
                    ui.horizontal(|ui| {
                        level_mark(ui, if matches!(c, crate::embed::selftest::Check::Ok(_)) {
                            Level::Ok
                        } else if c.usable() {
                            Level::Warn
                        } else {
                            Level::Bad
                        });
                        ui.label(format!("{name}: {}", c.detail()));
                    });
                }
            }

            // ── 완료 ──
            ui.add_space(14.0);
            ui.separator();
            let ready = model_ok && llama_ok && !self.cfg.roots.is_empty();
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.setup.start_index, "끝나면 바로 인덱싱 시작");
                if ui
                    .add_enabled(ready, egui::Button::new(RichText::new("완료").size(16.0)))
                    .clicked()
                {
                    self.finish_setup();
                }
                if !ready {
                    ui.label(RichText::new("모델·llama.cpp를 받고 폴더를 하나 이상 고르면 끝낼 수 있습니다").weak());
                }
            });
            ui.add_space(20.0);
        });
    }
}
