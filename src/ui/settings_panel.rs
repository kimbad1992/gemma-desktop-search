//! 설정 탭: llama.cpp/모델 경로, 서버 옵션, 도구 경로, 인덱싱 대상과 옵션, 검색 옵션

use crate::app::GemmaApp;
use crate::ui::widgets;
use eframe::egui::{self, RichText};
use std::path::Path;

enum Pick {
    File(&'static str, &'static [&'static str]),
    Folder,
    FileOrFolder(&'static str, &'static [&'static str]),
}

fn status(ui: &mut egui::Ui, ok: bool, ok_text: &str, bad_text: &str) {
    if ok {
        ui.colored_label(widgets::GREEN, format!("✔ {ok_text}"));
    } else {
        ui.colored_label(widgets::RED, format!("✖ {bad_text}"));
    }
}

fn start_dir(value: &str) -> Option<std::path::PathBuf> {
    let p = Path::new(value.trim());
    if p.is_dir() {
        Some(p.to_path_buf())
    } else {
        p.parent().filter(|d| d.is_dir()).map(Path::to_path_buf)
    }
}

const LABEL_W: f32 = 170.0;

fn row_label(ui: &mut egui::Ui, label: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(LABEL_W, 22.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(LABEL_W);
            ui.label(label);
        },
    );
}

/// 라벨 아래 들여쓴 상태 줄
fn status_row(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W + ui.spacing().item_spacing.x);
        add(ui);
    });
}

/// 라벨 | 텍스트 입력 | 찾아보기 버튼
fn path_row(ui: &mut egui::Ui, label: &str, value: &mut String, pick: Pick, hint: &str) -> bool {
    let mut changed = false;
    let start = start_dir(value);
    ui.horizontal(|ui| {
        row_label(ui, label);
        let buttons_w = match pick {
            Pick::FileOrFolder(..) => 140.0,
            _ => 90.0,
        };
        let w = (ui.available_width() - buttons_w).clamp(220.0, 760.0);
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
        ui.horizontal(|ui| {
            if ui
                .add_enabled(dirty, egui::Button::new(RichText::new("저장").size(15.0)))
                .clicked()
            {
                match self.cfg.save(&self.data_dir) {
                    Ok(()) => {
                        self.saved_cfg = self.cfg.clone();
                        self.notify(
                            format!("설정을 저장했습니다: {}", self.data_dir.join("config.toml").display()),
                            false,
                        );
                    }
                    Err(e) => self.notify(format!("설정 저장 실패: {e}"), true),
                }
            }
            if ui.add_enabled(dirty, egui::Button::new("되돌리기")).clicked() {
                let dim_changed = self.cfg.search_dim != self.saved_cfg.search_dim;
                self.cfg = self.saved_cfg.clone();
                self.exclude_text = self.cfg.exclude_dirs.join("\n");
                if dim_changed {
                    self.reload_store();
                }
            }
            if dirty {
                ui.colored_label(widgets::ORANGE, "저장하지 않은 변경 사항이 있습니다");
            }
            if self.server.state().is_running() {
                ui.label(RichText::new("· 서버 옵션은 서버를 다시 시작해야 적용됩니다").weak());
            }
        });
        ui.add_space(4.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.server_settings(ui);
            ui.add_space(10.0);
            self.tool_settings(ui);
            ui.add_space(10.0);
            self.index_settings(ui);
            ui.add_space(10.0);
            self.search_settings(ui);
            ui.add_space(10.0);
            ui.label(
                RichText::new(format!("데이터 폴더: {}", self.data_dir.display()))
                    .weak()
                    .small(),
            );
        });
    }

    fn server_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("llama.cpp 서버").size(17.0).strong());
        widgets::section_frame(ui, |ui| {
            path_row(
                ui,
                "llama.cpp 위치",
                &mut self.cfg.llama_path,
                Pick::FileOrFolder("실행 파일", &["exe"]),
                "llama.cpp 폴더 또는 llama-server.exe",
            );
            status_row(ui, |ui| match crate::server::resolve_binary(&self.cfg.llama_path) {
                Ok((p, serve)) => status(
                    ui,
                    true,
                    &format!("{}{}", p.display(), if serve { " (llama serve)" } else { "" }),
                    "",
                ),
                Err(e) => status(ui, false, "", &e.to_string()),
            });

            path_row(
                ui,
                "EmbeddingGemma 2 모델",
                &mut self.cfg.model_path,
                Pick::File("GGUF", &["gguf"]),
                "embeddinggemma-2-*.gguf",
            );
            let ok = Path::new(self.cfg.model_path.trim()).is_file();
            status_row(ui, |ui| status(ui, ok, "파일 있음", "파일을 찾을 수 없습니다"));

            path_row(
                ui,
                "mmproj (이미지·오디오)",
                &mut self.cfg.mmproj_path,
                Pick::File("GGUF", &["gguf"]),
                "비우면 텍스트 전용",
            );
            let mut find = false;
            status_row(ui, |ui| {
                if !self.cfg.has_mmproj() {
                    ui.colored_label(widgets::ORANGE, "⚠ 비어 있음 → 텍스트 전용 (이미지·영상·오디오는 이름만)");
                } else {
                    let ok = Path::new(self.cfg.mmproj_path.trim()).is_file();
                    status(ui, ok, "파일 있음", "파일을 찾을 수 없습니다");
                }
                find = ui.small_button("모델 폴더에서 찾기").clicked();
            });
            if find {
                match find_mmproj(&self.cfg.model_path) {
                    Some(found) => self.cfg.mmproj_path = found,
                    None => self.notify("모델 폴더에 mmproj-*.gguf 파일이 없습니다", true),
                }
            }

            path_row(
                ui,
                "추가 DLL 경로",
                &mut self.cfg.extra_dll_dir,
                Pick::Folder,
                r"선택 사항: CUDA DLL이 llama 폴더에 없을 때 (예: F:\Tools\cuda\bin)",
            );

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("포트");
                ui.add(egui::DragValue::new(&mut self.cfg.port).range(1024..=65535));
                ui.separator();
                ui.label("GPU 레이어(-ngl)");
                ui.add(egui::DragValue::new(&mut self.cfg.n_gpu_layers).range(0..=999))
                    .on_hover_text("0이면 CPU만 사용. NaN 문제가 있으면 0으로 비교해 보세요");
                ui.separator();
                ui.label("컨텍스트(-c)");
                ui.add(egui::DragValue::new(&mut self.cfg.ctx_size).range(512..=131072).speed(64));
                ui.label("슬롯(-np)");
                ui.add(egui::DragValue::new(&mut self.cfg.parallel).range(1..=32));
                ui.label("배치(-b)");
                ui.add(egui::DragValue::new(&mut self.cfg.batch_size).range(256..=32768).speed(64));
                ui.label("마이크로배치(-ub)");
                ui.add(egui::DragValue::new(&mut self.cfg.ubatch_size).range(256..=32768).speed(64));
            });
            let max_in = self.cfg.max_input_tokens();
            if max_in < 600 {
                ui.colored_label(
                    widgets::RED,
                    format!("입력 하나의 최대 토큰이 {max_in}개뿐입니다. 이미지는 약 280토큰이 필요합니다 — 컨텍스트를 늘리거나 슬롯을 줄이세요."),
                );
            } else {
                ui.label(
                    RichText::new(format!(
                        "입력 하나당 최대 {max_in}토큰 (= min(컨텍스트/슬롯, 배치, 마이크로배치)) · 슬롯 수만큼 동시 처리"
                    ))
                    .weak()
                    .small(),
                );
            }
            ui.horizontal(|ui| {
                ui.label("추가 인자");
                ui.add(
                    egui::TextEdit::singleline(&mut self.cfg.extra_args)
                        .hint_text("예: -fa on -t 8")
                        .desired_width(400.0),
                );
                ui.checkbox(&mut self.cfg.auto_start_server, "앱 시작 시 서버 자동 실행");
            });
        });
    }

    fn tool_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("외부 도구 (영상·오디오, HEIC)").size(17.0).strong());
        widgets::section_frame(ui, |ui| {
            path_row(ui, "ffmpeg.exe", &mut self.cfg.ffmpeg_path, Pick::File("exe", &["exe"]), "PATH에서 자동 탐색");
            path_row(ui, "ffprobe.exe", &mut self.cfg.ffprobe_path, Pick::File("exe", &["exe"]), "PATH에서 자동 탐색");
            status_row(ui, |ui| {
                let ok = self.cfg.ffmpeg().is_some() && self.cfg.ffprobe().is_some();
                status(ui, ok, "ffmpeg/ffprobe 사용 가능", "없으면 영상·오디오는 이름만 인덱싱");
                if ui.small_button("자동 탐색").clicked() {
                    if let Some(p) = crate::util::find_tool("ffmpeg.exe") {
                        self.cfg.ffmpeg_path = p.display().to_string();
                    }
                    if let Some(p) = crate::util::find_tool("ffprobe.exe") {
                        self.cfg.ffprobe_path = p.display().to_string();
                    }
                }
            });
        });
    }

    fn index_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("인덱싱").size(17.0).strong());
        widgets::section_frame(ui, |ui| {
            ui.label(RichText::new("대상 폴더").strong());
            let mut remove = None;
            for (i, r) in self.cfg.roots.iter().enumerate() {
                ui.horizontal(|ui| {
                    let exists = Path::new(r).is_dir();
                    widgets::dot(ui, if exists { widgets::GREEN } else { widgets::RED });
                    ui.label(r);
                    if !exists {
                        ui.colored_label(widgets::RED, "(없음)");
                    }
                    if ui.small_button("제거").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                self.cfg.roots.remove(i);
            }
            ui.horizontal(|ui| {
                if ui.button("폴더 추가…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        let s = p.display().to_string();
                        if !self.cfg.roots.contains(&s) {
                            self.cfg.roots.push(s);
                        }
                    }
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_root)
                        .hint_text(r"또는 경로 입력 (예: D:\문서)")
                        .desired_width(300.0),
                );
                if ui.button("추가").clicked() && !self.new_root.trim().is_empty() {
                    let s = self.new_root.trim().trim_end_matches('\\').to_string();
                    let s = if s.ends_with(':') { format!("{s}\\") } else { s };
                    if !self.cfg.roots.contains(&s) {
                        self.cfg.roots.push(s);
                    }
                    self.new_root.clear();
                }
            });

            ui.add_space(6.0);
            ui.label(RichText::new("내용까지 인덱싱할 종류").strong());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.cfg.index_text, "텍스트·코드");
                ui.checkbox(&mut self.cfg.index_docs, "PDF·DOCX");
                ui.checkbox(&mut self.cfg.index_images, "이미지");
                ui.checkbox(&mut self.cfg.index_video, "영상(프레임)");
                ui.checkbox(&mut self.cfg.index_audio, "오디오(실험적)");
            });
            ui.label(RichText::new("꺼진 종류와 기타 파일도 파일 이름은 항상 인덱싱됩니다.").weak().small());

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("텍스트 청크");
                ui.add(egui::DragValue::new(&mut self.cfg.chunk_chars).range(200..=4000).suffix("자"));
                ui.label("겹침");
                ui.add(egui::DragValue::new(&mut self.cfg.chunk_overlap).range(0..=1000).suffix("자"));
                ui.label("파일당 최대");
                ui.add(egui::DragValue::new(&mut self.cfg.max_chunks_per_file).range(1..=5000).suffix("개"));
                ui.label("텍스트 최대 크기");
                ui.add(egui::DragValue::new(&mut self.cfg.max_text_mb).range(1..=1024).suffix("MB"));
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("영상 프레임");
                ui.add(egui::DragValue::new(&mut self.cfg.video_frames).range(1..=64).suffix("장"));
                ui.label("오디오 구간");
                ui.add(egui::DragValue::new(&mut self.cfg.audio_segment_secs).range(5..=300).suffix("초"));
                ui.label("파일당 최대");
                ui.add(egui::DragValue::new(&mut self.cfg.audio_max_segments).range(1..=200).suffix("구간"));
                ui.label("미디어 최대 크기");
                ui.add(egui::DragValue::new(&mut self.cfg.max_media_mb).range(1..=65536).suffix("MB"));
                ui.label("작업 스레드");
                ui.add(egui::DragValue::new(&mut self.cfg.workers).range(1..=16));
            });

            ui.add_space(6.0);
            ui.label(RichText::new("제외할 폴더 이름 (한 줄에 하나, 대소문자 무시)").strong());
            if ui
                .add(
                    egui::TextEdit::multiline(&mut self.exclude_text)
                        .desired_rows(4)
                        .desired_width(400.0),
                )
                .changed()
            {
                self.cfg.exclude_dirs = self
                    .exclude_text
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
            }
        });
    }

    fn search_settings(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("검색").size(17.0).strong());
        widgets::section_frame(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("검색 차원 (Matryoshka)");
                let before = self.cfg.search_dim;
                let indexing = self.indexer.is_active();
                ui.add_enabled_ui(!indexing, |ui| {
                    egui::ComboBox::from_id_salt("dim")
                        .selected_text(format!("{}", self.cfg.search_dim))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.cfg.search_dim, 768, "768 (최고 품질)");
                            ui.selectable_value(&mut self.cfg.search_dim, 512, "512");
                            ui.selectable_value(&mut self.cfg.search_dim, 256, "256 (메모리 1/3)");
                        });
                });
                if self.cfg.search_dim != before {
                    self.reload_store();
                }
                ui.label(
                    RichText::new("DB에는 768차원 원본이 저장되므로 바꿔도 재인덱싱이 필요 없습니다")
                        .weak()
                        .small(),
                );
            });
            ui.horizontal(|ui| {
                ui.label("섹션별 최대 결과 수");
                ui.add(egui::DragValue::new(&mut self.cfg.top_k).range(5..=500))
                    .on_hover_text("검색할 때 섹션마다 가져오는 최대 개수. 최소 일치도 필터는 이 안에서 적용됩니다.");
            });
        });
    }
}

fn find_mmproj(model_path: &str) -> Option<String> {
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
    // BF16 > F16 > Q8 순으로 선호
    found.sort_by_key(|p| {
        let n = p.to_string_lossy().to_lowercase();
        if n.contains("bf16") {
            0
        } else if n.contains("f32") {
            1
        } else if n.contains("q8") {
            2
        } else {
            3
        }
    });
    found.first().map(|p| p.display().to_string())
}
