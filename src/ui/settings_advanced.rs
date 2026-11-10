//! 설정 > 고급: 전문 항목. 용어는 llama.cpp의 것을 그대로 쓴다.
//!
//! 준비물 수동 받기, 경로, 서버 옵션, 외부 도구, 인덱싱 세부값, 검색 세부값.
//! 폴더·파일 종류·앱 동작처럼 누구나 바꾸는 항목은 `settings_basic.rs`에 있다.

use super::settings_panel::{find_mmproj, path_row, status, status_row, verdict_line, Pick};
use crate::app::GemmaApp;
use crate::ui::theme::{space, Tone};
use crate::ui::widgets::{self, Btn};
use eframe::egui::{self, RichText};
use std::path::Path;

impl GemmaApp {
    pub fn settings_advanced(&mut self, ui: &mut egui::Ui) {
        self.advanced_banners(ui);
        self.advanced_downloads(ui);
        self.advanced_paths(ui);
        self.advanced_server(ui);
        self.advanced_tools(ui);
        self.advanced_index(ui);
        self.advanced_search(ui);
        ui.add_space(space::M);
        widgets::caption(ui, &format!("데이터 폴더: {}", self.data_dir.display()));
    }

    /// 바꾼 값이 언제 적용되는지 알린다
    fn advanced_banners(&mut self, ui: &mut egui::Ui) {
        if self.server_restart_needed() {
            let indexing = self.indexer.is_active();
            let mut restart = false;
            widgets::callout_ui(ui, Tone::Info, "서버 설정을 바꿨습니다", |ui| {
                ui.label(if indexing {
                    "다시 시작해야 적용됩니다. 인덱싱이 끝난 뒤에 다시 시작할 수 있습니다."
                } else {
                    "다시 시작해야 적용됩니다."
                });
                restart = widgets::button_enabled(ui, Btn::Secondary, !indexing, "서버 다시 시작").clicked();
            });
            if restart {
                self.restart_server();
            }
        }
        let (c, s) = (&self.cfg, &self.saved_cfg);
        let index_changed = c.chunk_chars != s.chunk_chars
            || c.chunk_overlap != s.chunk_overlap
            || c.max_chunks_per_file != s.max_chunks_per_file
            || c.max_text_mb != s.max_text_mb
            || c.max_media_mb != s.max_media_mb
            || c.video_segment_secs != s.video_segment_secs
            || c.video_max_segments != s.video_max_segments
            || c.audio_segment_secs != s.audio_segment_secs
            || c.audio_max_segments != s.audio_max_segments;
        if index_changed {
            widgets::callout(
                ui,
                Tone::Info,
                "인덱싱 설정을 바꿨습니다",
                "새로 추가되거나 바뀐 파일부터 적용됩니다. 이미 인덱싱한 파일까지 바꾸려면 인덱스 탭의 전체 재구축이 필요합니다.",
            );
        }
    }

    /// 준비물 수동 받기: 모델과 llama.cpp 판을 직접 고른다
    fn advanced_downloads(&mut self, ui: &mut egui::Ui) {
        use crate::download::{Pack, LLAMA_BUILD};
        let app_dir = crate::config::app_dir();
        let running = self.dl.lock().running;
        let model_ok = Path::new(self.cfg.model_path.trim()).is_file();
        let llama_ok = crate::server::resolve_binary(&self.cfg.llama_path).is_ok();
        let recommended = self.probe.plan().map(|p| p.llama);
        let mut start = None;

        widgets::section(
            ui,
            "준비물 받기 (수동)",
            Some(&format!(
                "모델과 llama.cpp를 프로그램 폴더({}) 아래에 받고 경로를 맞춥니다. 폴더째 옮겨도 그대로 씁니다.",
                app_dir.display()
            )),
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    status(ui, model_ok, "모델 있음", "모델 없음");
                    ui.add_enabled_ui(!running, |ui| {
                        if ui.button(Pack::Model.label()).clicked() {
                            start = Some(Pack::Model);
                        }
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    status(ui, llama_ok, "llama.cpp 있음", "llama.cpp 없음");
                    ui.add_enabled_ui(!running, |ui| {
                        for p in [Pack::LlamaCuda13, Pack::LlamaCuda12, Pack::LlamaVulkan, Pack::LlamaCpu] {
                            let label = if recommended == Some(p) {
                                format!("{} · 추천", p.label())
                            } else {
                                p.label().to_string()
                            };
                            if ui
                                .button(label)
                                .on_hover_text(format!("검증한 빌드 {LLAMA_BUILD}로 고정해서 받습니다"))
                                .clicked()
                            {
                                start = Some(p);
                            }
                        }
                    });
                });
                if recommended.is_none() {
                    widgets::caption(ui, "어느 판이 맞는지는 일반 설정의 '성능'에서 PC 사양을 확인하면 표시됩니다.");
                }
                self.download_progress(ui);
                widgets::caption(ui, "ffmpeg(영상·오디오·HEIC)는 따로 설치하세요: winget install Gyan.FFmpeg");
            },
        );

        if let Some(pack) = start {
            self.dl_queue.clear();
            crate::download::start(pack, app_dir, self.dl.clone(), self.dl_cancel.clone(), crate::app::waker(ui.ctx()));
        }
    }

    /// llama.cpp 실행 파일과 모델 경로
    fn advanced_paths(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut find = false;
        let mut recheck = false;
        // 지정한 것이 실제로 쓸 만한지: llama.cpp는 실행해 보고(끝나기 전에는 None), 모델은 종류를 읽는다
        let ctx = ui.ctx().clone();
        let engine_verdict = self.file_checks.engine(&self.cfg, &ctx);
        let model_verdict = self.file_checks.model(&self.cfg);
        widgets::section(ui, "llama.cpp 서버 · 경로", Some("바꾸면 서버를 다시 시작해야 적용됩니다."), |ui| {
            changed |= path_row(
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
            if crate::server::resolve_binary(&self.cfg.llama_path).is_ok() {
                status_row(ui, |ui| {
                    verdict_line(ui, engine_verdict.as_ref(), "실행해 보는 중…");
                    recheck = widgets::small_button(ui, widgets::Btn::Secondary, "다시 검사")
                        .on_hover_text("빠진 DLL을 채워 넣었거나 파일을 바꾼 뒤에 누르세요")
                        .clicked();
                });
            }

            changed |= path_row(
                ui,
                "EmbeddingGemma 2 모델",
                &mut self.cfg.model_path,
                Pick::File("GGUF", &["gguf"]),
                "embeddinggemma-2-*.gguf",
            );
            status_row(ui, |ui| verdict_line(ui, Some(&model_verdict), ""));

            changed |= path_row(
                ui,
                "mmproj (이미지·오디오)",
                &mut self.cfg.mmproj_path,
                Pick::File("GGUF", &["gguf"]),
                "비우면 텍스트 전용",
            );
            status_row(ui, |ui| {
                if !self.cfg.has_mmproj() {
                    widgets::tone_note(ui, Tone::Warn, "비어 있음 → 텍스트 전용 (이미지·영상·오디오는 이름만)");
                } else {
                    let ok = Path::new(self.cfg.mmproj_path.trim()).is_file();
                    status(ui, ok, "파일 있음", "파일을 찾을 수 없습니다");
                }
                find = widgets::small_button(ui, widgets::Btn::Secondary, "모델 폴더에서 찾기").clicked();
            });

            changed |= path_row(
                ui,
                "추가 DLL 경로",
                &mut self.cfg.extra_dll_dir,
                Pick::Folder,
                r"선택 사항: CUDA DLL이 llama 폴더에 없을 때 (예: F:\Tools\cuda\bin)",
            );
        });
        if find {
            match find_mmproj(&self.cfg.model_path) {
                Some(found) => {
                    self.cfg.mmproj_path = found;
                    changed = true;
                }
                None => self.notify("모델 폴더에 mmproj-*.gguf 파일이 없습니다", true),
            }
        }
        if recheck {
            self.file_checks.recheck();
        }
        if changed {
            self.invalidate_health();
        }
    }

    /// 서버 실행 옵션
    fn advanced_server(&mut self, ui: &mut egui::Ui) {
        let plan = self.probe.plan().cloned();
        let probing = self.probe.running();
        let (mut fill, mut probe) = (false, false);
        widgets::section(ui, "llama.cpp 서버 · 옵션", Some("바꾸면 서버를 다시 시작해야 적용됩니다."), |ui| {
            let c = &mut self.cfg;
            egui::Grid::new("adv_server").num_columns(4).spacing([14.0, 8.0]).show(ui, |ui| {
                ui.label("포트");
                ui.add(egui::DragValue::new(&mut c.port).range(1024..=65535));
                ui.label("GPU 레이어 (-ngl)");
                ui.add(egui::DragValue::new(&mut c.n_gpu_layers).range(0..=999))
                    .on_hover_text("0이면 CPU만 사용. NaN 문제가 있으면 0으로 비교해 보세요");
                ui.end_row();

                ui.label("컨텍스트 (-c)");
                ui.add(egui::DragValue::new(&mut c.ctx_size).range(512..=131072).speed(64));
                ui.label("슬롯 (-np)");
                ui.add(egui::DragValue::new(&mut c.parallel).range(1..=32));
                ui.end_row();

                ui.label("배치 (-b)");
                ui.add(egui::DragValue::new(&mut c.batch_size).range(256..=32768).speed(64));
                ui.label("마이크로배치 (-ub)");
                ui.add(egui::DragValue::new(&mut c.ubatch_size).range(256..=32768).speed(64));
                ui.end_row();

                ui.label("Flash Attention (-fa)");
                egui::ComboBox::from_id_salt("fa")
                    .selected_text(match c.flash_attn.as_str() {
                        "on" => "켜기",
                        "auto" => "자동",
                        _ => "끄기 (권장)",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut c.flash_attn, "off".to_string(), "끄기 (권장)");
                        ui.selectable_value(&mut c.flash_attn, "auto".to_string(), "자동");
                        ui.selectable_value(&mut c.flash_attn, "on".to_string(), "켜기");
                    })
                    .response
                    .on_hover_text(
                        "Windows + RTX 40 + Gemma 4 계열(mmproj)에서 Flash Attention이 켜져 있으면\n\
                         'CUDA illegal memory access'로 llama-server가 죽는 문제가 있습니다 (llama.cpp #30436).\n\
                         속도 차이는 거의 없으니 끄기를 권장합니다.",
                    );
                ui.end_row();
            });

            let max_in = c.max_input_tokens();
            if max_in < 600 {
                widgets::tone_label(
                    ui,
                    Tone::Danger,
                    format!("입력 하나의 최대 토큰이 {max_in}개뿐입니다. 이미지는 약 280토큰이 필요합니다. 컨텍스트를 늘리거나 슬롯을 줄이세요."),
                );
            } else {
                widgets::caption(
                    ui,
                    &format!("입력 하나당 최대 {max_in}토큰 (= min(컨텍스트/슬롯, 배치, 마이크로배치)) · 슬롯 수만큼 동시 처리"),
                );
            }
            ui.horizontal(|ui| {
                match &plan {
                    _ if probing => {
                        widgets::spinner(ui);
                        ui.label("PC 사양을 확인하는 중…");
                    }
                    Some(p) => {
                        if ui
                            .button("추천값으로 채우기")
                            .on_hover_text(format!("{}\n저장을 눌러야 파일에 기록됩니다.", p.summary))
                            .clicked()
                        {
                            fill = true;
                        }
                    }
                    None => {
                        if ui
                            .button("이 PC의 추천값 확인")
                            .on_hover_text("그래픽카드·메모리를 확인해 추천값을 계산합니다")
                            .clicked()
                        {
                            probe = true;
                        }
                    }
                }
            });

            ui.add_space(space::XS);
            ui.horizontal(|ui| {
                ui.label("추가 인자");
                ui.add(
                    egui::TextEdit::singleline(&mut c.extra_args)
                        .hint_text("예: -fa on -t 8")
                        .desired_width(400.0),
                );
            });
            ui.checkbox(&mut c.auto_start_server, "앱 시작 시 서버 자동 실행");
            ui.checkbox(
                &mut c.auto_recover,
                "인덱싱 중 서버가 비정상 종료되면 자동으로 다시 띄우고 이어서 인덱싱 (최대 5회)",
            )
            .on_hover_text(
                "충돌 때 처리 중이던 파일을 기록해 두고, 같은 파일이 두 번 충돌에 걸리면 그 파일의 내용 인덱싱은 건너뜁니다.\n서버 로그: data\\logs\\llama-server.log",
            );
            ui.checkbox(&mut c.server_on_demand, "검색할 때 서버가 꺼져 있으면 자동으로 켜기");
            ui.horizontal(|ui| {
                ui.label("자동으로 켠 서버를 쓰지 않으면");
                ui.add(egui::DragValue::new(&mut c.idle_stop_minutes).range(0..=240).suffix("분"));
                ui.label("뒤에 끄기 (0 = 끄지 않음)");
            })
            .response
            .on_hover_text("검색이나 인덱싱 때문에 앱이 켠 서버만 끕니다. 서버 탭에서 직접 켠 서버는 그대로 둡니다. 꺼도 다음 검색 때 다시 켭니다 (2초 안팎).");
        });
        if fill {
            if let Some(p) = &plan {
                self.apply_plan(p);
            }
        }
        if probe {
            let ffmpeg = self.cfg.ffmpeg().is_some();
            let ctx = ui.ctx().clone();
            self.probe.start(ffmpeg, &ctx);
        }
    }

    /// ffmpeg·ffprobe 경로
    fn advanced_tools(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        widgets::section(ui, "외부 도구 (영상·오디오, HEIC)", None, |ui| {
            changed |= path_row(ui, "ffmpeg.exe", &mut self.cfg.ffmpeg_path, Pick::File("exe", &["exe"]), "PATH에서 자동 탐색");
            changed |= path_row(ui, "ffprobe.exe", &mut self.cfg.ffprobe_path, Pick::File("exe", &["exe"]), "PATH에서 자동 탐색");
            status_row(ui, |ui| {
                let ok = self.cfg.ffmpeg().is_some() && self.cfg.ffprobe().is_some();
                status(ui, ok, "ffmpeg/ffprobe 사용 가능", "없으면 영상·오디오는 이름만 인덱싱");
                if widgets::small_button(ui, widgets::Btn::Secondary, "자동 탐색").clicked() {
                    if let Some(p) = crate::util::find_tool(&crate::platform::exe_name("ffmpeg")) {
                        self.cfg.ffmpeg_path = p.display().to_string();
                    }
                    if let Some(p) = crate::util::find_tool(&crate::platform::exe_name("ffprobe")) {
                        self.cfg.ffprobe_path = p.display().to_string();
                    }
                    changed = true;
                }
            });
        });
        if changed {
            self.invalidate_health();
        }
    }

    /// 인덱싱 세부값
    fn advanced_index(&mut self, ui: &mut egui::Ui) {
        let mut added = false;
        widgets::section(
            ui,
            "인덱싱",
            Some("다음 인덱싱부터 적용됩니다. 이미 인덱싱한 파일은 그대로 둡니다."),
            |ui| {
                widgets::subheading(ui, "대상 폴더 경로 직접 입력");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_root)
                            .hint_text(r"예: D:\문서 또는 \\서버\사진")
                            .desired_width(360.0),
                    );
                    if ui.button("추가").clicked() && !self.new_root.trim().is_empty() {
                        let s = self.new_root.trim().trim_end_matches(crate::paths::is_sep).to_string();
                        // "C:"처럼 드라이브만 남았으면 구분자를 다시 붙인다
                        let s = if s.ends_with(':') { format!("{s}{}", crate::paths::SEP) } else { s };
                        if !self.cfg.roots.contains(&s) {
                            self.cfg.roots.push(s);
                            added = true;
                        }
                        self.new_root.clear();
                    }
                });
                widgets::caption(
                    ui,
                    &format!("지금 대상 폴더 {}개. 목록은 일반 설정의 '검색할 폴더'에서 고칩니다.", self.cfg.roots.len()),
                );

                ui.add_space(space::S);
                widgets::subheading(ui, "텍스트");
                let c = &mut self.cfg;
                ui.horizontal_wrapped(|ui| {
                    ui.label("청크");
                    ui.add(egui::DragValue::new(&mut c.chunk_chars).range(200..=4000).suffix("자"));
                    ui.label("겹침");
                    ui.add(egui::DragValue::new(&mut c.chunk_overlap).range(0..=1000).suffix("자"));
                    ui.label("파일당 최대");
                    ui.add(egui::DragValue::new(&mut c.max_chunks_per_file).range(1..=5000).suffix("개"));
                    ui.label("최대 크기");
                    ui.add(egui::DragValue::new(&mut c.max_text_mb).range(1..=1024).suffix("MB"));
                });

                ui.add_space(space::S);
                widgets::subheading(ui, "영상·오디오");
                let window = c.video_window_secs();
                ui.horizontal_wrapped(|ui| {
                    ui.label("영상 구간");
                    ui.add(egui::DragValue::new(&mut c.video_segment_secs).range(4..=58).suffix("초"))
                        .on_hover_text(format!(
                            "영상을 이 길이로 나눠 구간마다 영상 그대로 임베딩합니다 (1초에 1프레임, 모델 카드 방식).\n\
                             지금 서버 설정으로 실제 적용되는 길이: {window}초 (입력 하나에 들어가는 프레임 수에 맞춰 줄어듦)"
                        ));
                    ui.label("영상당 최대");
                    ui.add(egui::DragValue::new(&mut c.video_max_segments).range(1..=2000).suffix("구간"));
                    ui.label("오디오 구간");
                    ui.add(egui::DragValue::new(&mut c.audio_segment_secs).range(5..=300).suffix("초"));
                    ui.label("파일당 최대");
                    ui.add(egui::DragValue::new(&mut c.audio_max_segments).range(1..=2000).suffix("구간"))
                        .on_hover_text("오디오는 창을 겹쳐서(30초 창이면 25초마다) 읽습니다. 이보다 많으면 전체에 고르게 분산.");
                    ui.label("최대 크기");
                    ui.add(egui::DragValue::new(&mut c.max_media_mb).range(1..=65536).suffix("MB"));
                });

                ui.add_space(space::S);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("작업 스레드").strong());
                    ui.add(egui::DragValue::new(&mut c.workers).range(1..=16));
                });

                ui.add_space(space::S);
                ui.checkbox(&mut c.watch_folders, "폴더 감시: 파일이 바뀌면 그 파일만 자동으로 인덱싱");
                ui.horizontal(|ui| {
                    ui.label("폴더를 훑어 확인하는 주기");
                    ui.add(egui::DragValue::new(&mut c.watch_rescan_hours).range(0..=168).suffix("시간"));
                    ui.label("(0 = 켤 때 한 번만)");
                })
                .response
                .on_hover_text("변경 알림이 오지 않는 네트워크 폴더와 놓친 알림을 찾으려고, 폴더를 훑어 인덱스와 비교합니다. 서버 없이 크기와 수정 시각만 봅니다.");

                ui.add_space(space::S);
                widgets::subheading(ui, "제외할 폴더 이름");
                widgets::caption(ui, "한 줄에 하나, 대소문자 무시. 이름이 같은 폴더는 어디에 있든 건너뜁니다.");
                if ui
                    .add(
                        egui::TextEdit::multiline(&mut self.exclude_text)
                            .desired_rows(5)
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
            },
        );
        if added {
            self.invalidate_health();
        }
    }

    /// 검색 세부값
    fn advanced_search(&mut self, ui: &mut egui::Ui) {
        let before = self.cfg.search_dim;
        let indexing = self.indexer.is_active();
        widgets::section(ui, "검색", None, |ui| {
            ui.horizontal(|ui| {
                ui.label("검색 차원 (Matryoshka)");
                ui.add_enabled_ui(!indexing, |ui| {
                    egui::ComboBox::from_id_salt("dim")
                        .selected_text(format!("{}", self.cfg.search_dim))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.cfg.search_dim, 768, "768 (최고 품질)");
                            ui.selectable_value(&mut self.cfg.search_dim, 512, "512");
                            ui.selectable_value(&mut self.cfg.search_dim, 256, "256 (메모리 1/3)");
                        });
                });
            });
            widgets::caption(ui, "DB에는 768차원 원본이 저장되므로 바꿔도 재인덱싱이 필요 없습니다.");
            ui.horizontal(|ui| {
                ui.label("섹션별 최대 결과 수");
                ui.add(egui::DragValue::new(&mut self.cfg.top_k).range(5..=500))
                    .on_hover_text("검색할 때 섹션마다 가져오는 최대 개수. 최소 관련도 필터는 이 안에서 적용됩니다.");
            });
        });
        if self.cfg.search_dim != before {
            self.reload_store();
        }
    }
}
