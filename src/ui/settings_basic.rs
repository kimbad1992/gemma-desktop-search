//! 설정 > 일반: 전문 지식 없이도 쓸 수 있게 쉬운 말로 쓴 화면.
//!
//! 이 화면만으로 검색을 시작할 수 있어야 한다: 지금 상태(무엇이 빠졌는지와 해결 버튼),
//! 검색할 폴더, 살펴볼 파일 종류, PC에 맞춘 성능, 앱 동작. 전문 항목은 `settings_advanced.rs`에 있다.

use crate::app::{GemmaApp, SettingsView};
use crate::sysinfo::Level;
use crate::ui::theme::{space, Tone};
use crate::ui::widgets::{self, Btn};
use eframe::egui::{self, RichText};

/// "지금 상태" 카드에서 누른 해결 버튼
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fix {
    None,
    Download,
    Index,
    InstallFfmpeg,
    FindFfmpeg,
}

/// "지금 상태"의 항목 이름 칸 폭
const HEALTH_LABEL_W: f32 = 120.0;

/// "지금 상태"의 한 줄: 항목 이름 | 상태 | (해결 버튼)
fn health_row(ui: &mut egui::Ui, name: &str, tone: Tone, text: &str, action: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(HEALTH_LABEL_W, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_width(HEALTH_LABEL_W);
                ui.label(RichText::new(name).weak());
            },
        );
        ui.colored_label(tone.fg(ui.visuals()), tone.icon());
        ui.label(text);
        action(ui);
    });
}

impl GemmaApp {
    pub fn settings_basic(&mut self, ui: &mut egui::Ui) {
        self.basic_status(ui);
        self.basic_folders(ui);
        self.basic_kinds(ui);
        self.basic_performance(ui);
        self.basic_behavior(ui);
        self.basic_update(ui);

        ui.add_space(space::M);
        ui.horizontal(|ui| {
            if widgets::button(ui, Btn::Ghost, "처음 설정 다시 열기")
                .on_hover_text("PC 사양 확인부터 폴더 고르기까지 차례로 다시 안내합니다")
                .clicked()
            {
                let ctx = ui.ctx().clone();
                self.open_setup(&ctx);
            }
        });
        widgets::caption(ui, &format!("설정과 검색 자료가 저장되는 곳: {}", self.data_dir.display()));
    }

    /// 지금 바로 검색할 수 있는지, 아니면 무엇이 빠졌는지
    fn basic_status(&mut self, ui: &mut egui::Ui) {
        let h = self.health();
        let downloading = self.dl.lock().running;
        let probing = self.probe.running();
        let indexing = self.indexer.is_active();
        let warnings = self.shell_warnings();
        let recommended = self.probe.plan().map(|p| p.llama);
        let mut fix = Fix::None;
        let mut goto_advanced = false;

        widgets::section(ui, "지금 상태", None, |ui| {
            let limited = h.mmproj != Some(true) || !h.ffmpeg;
            if !h.ready() {
                widgets::callout(ui, Tone::Warn, "검색하려면 아래 준비가 필요합니다", "");
            } else if h.indexed_files == 0 {
                widgets::callout(
                    ui,
                    Tone::Info,
                    "준비가 끝났습니다",
                    "검색 자료를 만들면 바로 검색할 수 있습니다.",
                );
            } else if limited {
                widgets::callout(
                    ui,
                    Tone::Ok,
                    "바로 검색할 수 있습니다",
                    "다만 일부 종류는 파일 이름으로만 찾습니다. 아래에서 확인하세요.",
                );
            } else {
                widgets::callout(ui, Tone::Ok, "바로 검색할 수 있습니다", "");
            }
            ui.add_space(space::XS);

            // 필요한 파일: AI 모델과 그 모델을 돌리는 프로그램(llama.cpp)
            let (tone, text) = match (h.engine, h.model) {
                (true, true) => (Tone::Ok, "AI 모델과 llama.cpp가 준비됐습니다"),
                (false, false) => (Tone::Danger, "AI 모델과 llama.cpp가 없습니다"),
                (false, true) => (Tone::Danger, "llama.cpp(AI 모델을 돌리는 프로그램)가 없습니다"),
                (true, false) => (Tone::Danger, "AI 모델이 없습니다"),
            };
            health_row(ui, "필요한 파일", tone, text, |ui| {
                if h.engine && h.model {
                } else if downloading {
                    ui.label(RichText::new("받는 중…").weak());
                } else if probing {
                    ui.label(RichText::new("PC 사양 확인 중…").weak());
                } else if widgets::button(ui, Btn::Primary, "필요한 것 모두 받기")
                    .on_hover_text("이 PC에 맞는 것을 골라 프로그램 폴더 아래에 받습니다")
                    .clicked()
                {
                    fix = Fix::Download;
                }
            });

            // 사진·영상·소리를 이해하는 데 필요한 모델 파일 (mmproj)
            if h.model && h.mmproj != Some(true) {
                health_row(
                    ui,
                    "사진·영상·소리",
                    Tone::Warn,
                    "필요한 파일이 없어 글만 내용으로 찾습니다 (고급 설정의 mmproj)",
                    |_| {},
                );
            }

            let folders_text;
            let (tone, text) = if h.folders == 0 {
                (Tone::Danger, "아래에서 폴더를 추가하세요")
            } else if h.missing_folders > 0 {
                folders_text = format!("{}개 중 {}개를 찾을 수 없습니다", h.folders, h.missing_folders);
                (Tone::Warn, folders_text.as_str())
            } else {
                folders_text = format!("{}개", h.folders);
                (Tone::Ok, folders_text.as_str())
            };
            health_row(ui, "검색할 폴더", tone, text, |_| {});

            if indexing {
                health_row(ui, "검색 자료", Tone::Info, "만드는 중입니다 (인덱스 탭에서 진행 상황)", |_| {});
            } else if h.indexed_files > 0 {
                let text = format!("파일 {}개를 찾을 수 있습니다", crate::util::fmt_num(h.indexed_files as u64));
                health_row(ui, "검색 자료", Tone::Ok, &text, |_| {});
            } else {
                health_row(ui, "검색 자료", Tone::Warn, "아직 없습니다", |ui| {
                    if widgets::button_enabled(ui, Btn::Secondary, h.ready(), "지금 만들기")
                        .on_hover_text("폴더의 파일을 읽어 검색할 수 있게 준비합니다 (인덱싱)")
                        .clicked()
                    {
                        fix = Fix::Index;
                    }
                });
            }

            if h.ffmpeg {
                health_row(ui, "영상·소리 도구", Tone::Ok, "있음", |_| {});
            } else {
                health_row(ui, "영상·소리 도구", Tone::Warn, "없음. 영상·소리는 이름으로만 찾습니다", |ui| {
                    if widgets::button(ui, Btn::Secondary, "설치")
                        .on_hover_text("명령 창이 열리고 ffmpeg를 설치합니다 (winget). 끝나면 '다시 찾기'를 누르세요.")
                        .clicked()
                    {
                        fix = Fix::InstallFfmpeg;
                    }
                    if widgets::button(ui, Btn::Ghost, "다시 찾기").clicked() {
                        fix = Fix::FindFfmpeg;
                    }
                });
            }

            // 무엇을 받게 되는지 미리 알려 준다
            if !(h.engine && h.model) && !downloading {
                use crate::download::Pack;
                let mut parts: Vec<&str> = Vec::new();
                if !h.model {
                    parts.push(Pack::Model.plain_label());
                }
                if !h.engine {
                    parts.push(match recommended {
                        Some(p) => p.plain_label(),
                        None => "llama.cpp (PC 사양을 확인해 맞는 것을 고릅니다)",
                    });
                }
                widgets::caption(ui, &format!("받을 것: {}", parts.join(" / ")));
                // 이미 받아 둔 것이 있으면 받지 않고 고급 설정에서 위치를 지정한다
                ui.horizontal(|ui| {
                    widgets::caption(ui, "이미 받아 둔 llama.cpp와 모델이 있으면");
                    goto_advanced = ui
                        .link(RichText::new("위치를 직접 지정").small())
                        .on_hover_text("고급 설정의 'llama.cpp 서버 · 경로'로 갑니다. 지정하면 쓸 수 있는지 바로 검사합니다.")
                        .clicked();
                });
            }
            // 트레이 아이콘·단축키가 안 되는 이유
            for w in &warnings {
                ui.add_space(space::XS);
                widgets::callout(ui, Tone::Warn, "", w);
            }
            if downloading {
                ui.add_space(space::XS);
                self.download_progress(ui);
            }
        });

        if goto_advanced {
            self.settings_view = SettingsView::Advanced;
        }
        let ctx = ui.ctx().clone();
        match fix {
            Fix::None => {}
            Fix::Download => match self.probe.plan().map(|p| p.llama) {
                Some(llama) => self.download_missing(&ctx, llama),
                None => {
                    // 어떤 판이 맞는지 알아야 하므로 사양부터 확인하고, 끝나면 받기 시작
                    self.download_when_probed = true;
                    let ffmpeg = self.cfg.ffmpeg().is_some();
                    self.probe.start(ffmpeg, &ctx);
                }
            },
            Fix::Index => self.start_index_when_ready(),
            Fix::InstallFfmpeg => {
                let _ = std::process::Command::new("cmd")
                    .args(["/c", "start", "ffmpeg 설치", "cmd", "/k", "winget install --id Gyan.FFmpeg -e"])
                    .spawn();
            }
            Fix::FindFfmpeg => {
                if let Some(p) = crate::util::find_tool("ffmpeg.exe") {
                    self.cfg.ffmpeg_path = p.display().to_string();
                }
                if let Some(p) = crate::util::find_tool("ffprobe.exe") {
                    self.cfg.ffprobe_path = p.display().to_string();
                }
                self.invalidate_health();
                if self.cfg.ffmpeg().is_none() {
                    self.notify("ffmpeg를 찾지 못했습니다. 설치가 끝났는지 확인하세요.", true);
                }
            }
        }
    }

    /// 검색할 폴더 목록
    fn basic_folders(&mut self, ui: &mut egui::Ui) {
        let h = self.health();
        let changed = self.cfg.roots != self.saved_cfg.roots;
        let indexing = self.indexer.is_active();
        let (mut remove, mut add, mut apply) = (None, false, false);

        widgets::section(
            ui,
            "검색할 폴더",
            Some("여기에 넣은 폴더 안의 파일을 찾을 수 있습니다."),
            |ui| {
                for (i, root) in self.cfg.roots.iter().enumerate() {
                    let exists = h.root_exists.get(i).copied().unwrap_or(true);
                    ui.horizontal(|ui| {
                        let tone = if exists { Tone::Ok } else { Tone::Danger };
                        ui.colored_label(tone.fg(ui.visuals()), tone.icon());
                        ui.label(root);
                        if !exists {
                            widgets::tone_label(ui, Tone::Danger, "(찾을 수 없음)");
                        }
                        if widgets::button(ui, Btn::Ghost, "빼기")
                            .on_hover_text("목록에서만 뺍니다. 폴더와 파일은 그대로 있습니다.")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                }
                if self.cfg.roots.is_empty() {
                    widgets::help(ui, "사진·문서·영상이 들어 있는 폴더를 추가하세요.");
                }
                ui.add_space(space::XS);
                add = widgets::button(ui, Btn::Secondary, "폴더 추가…").clicked();

                if changed {
                    ui.add_space(space::XS);
                    widgets::callout_ui(ui, Tone::Info, "폴더를 바꿨습니다", |ui| {
                        ui.label("저장한 뒤 검색 준비(인덱싱)를 다시 해야 검색에 반영됩니다.");
                        apply = widgets::button_enabled(ui, Btn::Secondary, !indexing, "저장하고 지금 하기").clicked();
                    });
                }
            },
        );

        if let Some(i) = remove {
            self.cfg.roots.remove(i);
            self.invalidate_health();
        }
        if add {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                let s = dir.display().to_string();
                if !self.cfg.roots.contains(&s) {
                    self.cfg.roots.push(s);
                    self.invalidate_health();
                }
            }
        }
        if apply {
            self.save_settings();
            self.start_index_when_ready();
        }
    }

    /// 내용까지 살펴볼 파일 종류
    fn basic_kinds(&mut self, ui: &mut egui::Ui) {
        widgets::section(ui, "내용까지 살펴볼 파일 종류", None, |ui| {
            let c = &mut self.cfg;
            widgets::toggle_row(ui, &mut c.index_text, "글 파일·코드", "메모, 텍스트, 소스 코드를 내용으로 찾습니다");
            widgets::toggle_row(ui, &mut c.index_docs, "문서 (PDF·Word)", "문서 안에 적힌 글로 찾습니다");
            widgets::toggle_row(ui, &mut c.index_images, "사진·그림", "사진에 무엇이 찍혔는지로 찾습니다");
            widgets::toggle_row(
                ui,
                &mut c.index_video,
                "영상",
                "영상 속 장면으로 찾습니다. 준비에 시간이 오래 걸립니다",
            );
            widgets::toggle_row(
                ui,
                &mut c.index_audio,
                "소리·음악 (실험 기능)",
                "들리는 소리로 찾습니다. 아직 정확하지 않을 수 있습니다",
            );
            ui.add_space(space::XS);
            widgets::help(
                ui,
                "꺼 둔 종류도 파일 이름으로는 항상 찾을 수 있습니다. 새로 켠 종류는 다음 검색 준비 때 반영됩니다.",
            );
        });
    }

    /// PC 사양에 맞춘 성능 설정
    fn basic_performance(&mut self, ui: &mut egui::Ui) {
        let probing = self.probe.running();
        let plan = self.probe.plan().cloned();
        let applied = plan.as_ref().map(|p| self.plan_applied(p)).unwrap_or(false);
        let restart = self.server_restart_needed();
        let indexing = self.indexer.is_active();
        // (사양 확인 시작, 끝나면 바로 적용할지) / 추천값 적용 / 서버 다시 시작
        let (mut probe, mut apply, mut do_restart) = (None, false, false);

        widgets::section(
            ui,
            "성능",
            Some("그래픽카드와 메모리를 확인해 이 PC에서 가장 빠르고 안정적인 값으로 맞춥니다."),
            |ui| {
                match &plan {
                    _ if probing => {
                        ui.horizontal(|ui| {
                            widgets::spinner(ui);
                            ui.label("PC 사양을 확인하는 중…");
                        });
                    }
                    None => {
                        if widgets::button(ui, Btn::Secondary, "이 PC에 맞게 자동 설정").clicked() {
                            probe = Some(true);
                        }
                    }
                    Some(plan) => {
                        ui.label(plan.plain_summary());
                        // 주의할 점만 바로 보여주고, 전체는 접어 둔다
                        for (level, name, text) in plan.checks.iter().filter(|c| c.0 != Level::Ok) {
                            let tone = if *level == Level::Bad { Tone::Danger } else { Tone::Warn };
                            widgets::status_line(ui, tone, &format!("{name}: {text}"));
                        }
                        ui.add_space(space::XS);
                        if applied {
                            widgets::status_line(ui, Tone::Ok, "이 PC에 맞게 설정되어 있습니다");
                        } else {
                            widgets::callout_ui(ui, Tone::Info, "지금 설정이 추천값과 다릅니다", |ui| {
                                apply = widgets::button(ui, Btn::Secondary, "추천값으로 맞추기").clicked();
                            });
                        }
                        ui.add_space(space::XS);
                        widgets::collapsing_section(ui, "basic_perf_detail", "확인한 내용 자세히", false, |ui| {
                            for (level, name, text) in &plan.checks {
                                let tone = match level {
                                    Level::Ok => Tone::Ok,
                                    Level::Warn => Tone::Warn,
                                    Level::Bad => Tone::Danger,
                                };
                                widgets::status_line(ui, tone, &format!("{name}: {text}"));
                            }
                        });
                        if widgets::button(ui, Btn::Ghost, "다시 확인").clicked() {
                            probe = Some(false);
                        }
                    }
                }
                if restart {
                    ui.add_space(space::XS);
                    widgets::callout_ui(ui, Tone::Info, "바꾼 값은 서버를 다시 시작해야 적용됩니다", |ui| {
                        if indexing {
                            ui.label("인덱싱이 끝난 뒤에 다시 시작할 수 있습니다.");
                        }
                        do_restart = widgets::button_enabled(ui, Btn::Secondary, !indexing, "서버 다시 시작").clicked();
                    });
                }
            },
        );

        if let Some(apply_when_done) = probe {
            self.apply_plan_when_probed = apply_when_done;
            let ffmpeg = self.cfg.ffmpeg().is_some();
            let ctx = ui.ctx().clone();
            self.probe.start(ffmpeg, &ctx);
        }
        if apply {
            if let Some(plan) = &plan {
                self.apply_plan_and_persist(plan);
                self.notify("이 PC에 맞게 설정했습니다", false);
            }
        }
        if do_restart {
            self.restart_server();
        }
    }

    /// 앱 동작: 트레이, 자동 시작, 단축키, 알림
    fn basic_behavior(&mut self, ui: &mut egui::Ui) {
        widgets::section(ui, "앱 동작", None, |ui| {
            let c = &mut self.cfg;
            widgets::toggle_row(
                ui,
                &mut c.global_hotkey,
                "어디서든 빠른 검색",
                &format!(
                    "{}를 누르면 작은 검색창이 뜹니다. 바꾸면 앱을 다시 켠 뒤 적용됩니다",
                    crate::shell::HOTKEY_LABEL
                ),
            );
            widgets::toggle_row(
                ui,
                &mut c.close_to_tray,
                "창을 닫아도 계속 실행",
                "닫으면 작업 표시줄 오른쪽 아이콘으로 숨습니다. 끝내려면 그 아이콘을 우클릭 → 종료",
            );
            widgets::toggle_row(
                ui,
                &mut c.start_with_windows,
                "컴퓨터를 켜면 자동으로 시작",
                "Windows에 로그인하면 창 없이 조용히 켜집니다",
            );
            widgets::toggle_row(
                ui,
                &mut c.index_on_start,
                "켤 때 새 파일 자동 반영",
                "앱을 켤 때마다 새로 생기거나 바뀐 파일을 검색 자료에 넣습니다",
            );
            widgets::toggle_row(
                ui,
                &mut c.watch_folders,
                "폴더 감시",
                "검색 폴더의 파일이 생기거나 바뀌거나 지워지면 그 파일만 자동으로 검색 자료에 반영합니다",
            );
            if c.watch_folders {
                // 네트워크 폴더는 바로 반영된다고 약속할 수 없다: 한계를 늘 보이게 둔다
                let network = c.roots.iter().filter(|r| crate::watch::is_network(r)).count();
                if network > 0 {
                    let every = if c.watch_rescan_hours > 0 {
                        format!("{}시간마다", c.watch_rescan_hours)
                    } else {
                        "앱을 켤 때".to_string()
                    };
                    widgets::callout(
                        ui,
                        Tone::Warn,
                        &format!("네트워크 폴더 {network}개는 바로 반영되지 않을 수 있습니다"),
                        &format!(
                            "NAS에 따라 변경 알림이 오지 않거나 일부가 빠집니다. 그래서 {every} 폴더를 훑어 바뀐 파일을 찾습니다. \
                             반영이 그만큼 늦을 수 있고, 훑는 동안 NAS를 읽습니다. 급하면 인덱스 화면의 '지금 확인'을 누르세요."
                        ),
                    );
                }
                widgets::collapsing_section(ui, "basic_watch_notes", "폴더 감시에서 알아 둘 점", false, |ui| {
                    for line in [
                        "파일이 바뀌면 서버가 자동으로 켜지고 그래픽카드를 씁니다. 전체 화면 앱(게임·영상·발표)을 쓰는 동안은 미룹니다.",
                        "복사나 다운로드가 끝나 15초쯤 조용해진 뒤에 반영합니다. 계속 쓰이는 파일은 10분에 한 번만 반영합니다.",
                        "앱이 꺼져 있던 동안 바뀐 파일은 다음에 켰을 때 폴더를 훑어서 찾습니다.",
                        "옮기거나 이름만 바꾼 파일은 내용을 다시 읽지 않고 경로만 고칩니다.",
                        "저장한 뒤에 적용됩니다.",
                    ] {
                        widgets::status_line(ui, Tone::Info, line);
                    }
                });
            }
            widgets::toggle_row(
                ui,
                &mut c.index_overlay,
                "작업 중 작은 알림 창",
                "검색 준비를 하는 동안 화면 오른쪽 아래에 진행 상황을 보여 줍니다",
            );
            if c.index_overlay {
                widgets::collapsing_section(ui, "basic_overlay_notes", "알림 창에서 주의할 점", false, |ui| {
                    for line in [
                        "전체 화면 게임·영상·발표 중에는 자동으로 숨지만, 창 모드 게임 위에는 뜰 수 있습니다.",
                        "일부 게임의 안티치트, 화면 녹화·방송, 원격 데스크톱 프로그램과 충돌하거나 화면에 같이 찍힐 수 있습니다.",
                        "문제가 생기면 이 옵션을 끄세요. 꺼도 인덱스 탭에서 진행 상황을 볼 수 있습니다.",
                    ] {
                        widgets::status_line(ui, Tone::Warn, line);
                    }
                });
            }
        });
    }

    /// 업데이트: 지금 버전, 새 버전이 있으면 "업데이트 가능", 누르면 확인하고 받아서 바꾼 뒤 다시 시작
    fn basic_update(&mut self, ui: &mut egui::Ui) {
        use crate::update::Stage;
        use widgets::Icon;
        let stage = self.updater.lock().clone();
        let ctx = ui.ctx().clone();
        let available = self.update_available.as_ref().map(|(tag, _)| tag.clone());
        let (mut start, mut restart) = (false, false);
        widgets::section(
            ui,
            "업데이트",
            Some("누르면 새 버전이 있는지 확인하고, 있으면 받아서 지금 프로그램과 바꾼 뒤 다시 시작합니다. 설정과 검색 자료는 그대로입니다."),
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("지금 버전 v{}", env!("CARGO_PKG_VERSION")))
                            .text_style(crate::ui::theme::ts_strong()),
                    );
                    if let (Some(tag), false) = (&available, matches!(stage, Stage::Ready { .. })) {
                        widgets::tone_tag(ui, Tone::Info, &format!("업데이트 가능 {tag}"));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if matches!(stage, Stage::Ready { .. }) {
                            restart = widgets::icon_button(ui, Btn::Primary, Icon::RotateCw, "다시 시작").clicked();
                        } else if available.is_some() {
                            start = widgets::icon_button_enabled(ui, Btn::Primary, !stage.busy(), Icon::Download, "업데이트")
                                .clicked();
                        } else {
                            start =
                                widgets::icon_button_enabled(ui, Btn::Secondary, !stage.busy(), Icon::RefreshCw, "업데이트 확인")
                                    .clicked();
                        }
                    });
                });
                match &stage {
                    Stage::Idle => {}
                    Stage::Checking | Stage::Installing { .. } => {
                        ui.horizontal(|ui| {
                            widgets::spinner(ui);
                            ui.label(stage.text());
                        });
                    }
                    Stage::Downloading { done, total, .. } => {
                        let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                        widgets::progress_accent(ui, frac, 6.0, true);
                        widgets::caption(
                            ui,
                            &format!(
                                "{} · {:.1} / {:.1}MB",
                                stage.text(),
                                *done as f64 / 1048576.0,
                                *total as f64 / 1048576.0
                            ),
                        );
                    }
                    Stage::UpToDate | Stage::Ready { .. } => widgets::status_line(ui, Tone::Ok, &stage.text()),
                    Stage::Failed { message, page } => {
                        widgets::callout_ui(ui, Tone::Danger, "업데이트하지 못했습니다", |ui| {
                            ui.label(message);
                            if let Some(page) = page {
                                ui.hyperlink_to("릴리스 페이지에서 직접 받기", page);
                            }
                        });
                    }
                }
                ui.add_space(space::XS);
                widgets::toggle_row(
                    ui,
                    &mut self.cfg.check_updates,
                    "새 버전 알림",
                    "하루 한 번 새 버전이 있는지 확인해 알려 줍니다. 내 파일 정보는 보내지 않습니다",
                );
            },
        );
        if start {
            self.start_update(&ctx);
        }
        if restart {
            self.restart_for_update(&ctx);
        }
    }
}
