//! 인덱스 탭: 지금 상태와 주 동작, 이번 실행의 통계, 종류별·전체 인덱스, 폴더·오류 목록, 전체 재구축

use crate::app::GemmaApp;
use crate::extract::Kind;
use crate::index::progress::RunState;
use crate::ui::theme::{self, space, Tone};
use crate::ui::widgets::{self, Btn};
use crate::util::{fmt_dur, fmt_num};
use eframe::egui::{self, RichText};

/// 머리 카드에서 누른 버튼
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    None,
    Start,
    Pause,
    Resume,
    Stop,
    Settings,
}

/// 이름 | 값 한 줄
fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.label(RichText::new(key).weak());
    ui.label(value);
    ui.end_row();
}

impl GemmaApp {
    pub fn index_tab(&mut self, ui: &mut egui::Ui) {
        widgets::page(ui, "index_page", theme::DASH_W, |ui| {
            self.index_header(ui);
            self.index_run_stats(ui);
            self.index_watch(ui);
            if ui.available_width() >= 900.0 {
                ui.columns(2, |cols| {
                    self.index_kinds(&mut cols[0]);
                    self.index_totals(&mut cols[1]);
                });
            } else {
                self.index_kinds(ui);
                self.index_totals(ui);
            }
            self.index_lists(ui);
            self.index_rebuild(ui);
        });
    }

    /// 상태 배지, 주 동작 버튼, 진행 막대, 시작할 수 없는 이유
    fn index_header(&mut self, ui: &mut egui::Ui) {
        let p = self.indexer.progress.lock().clone();
        let loading = self.store_loading.load(std::sync::atomic::Ordering::SeqCst);
        let no_roots = self.cfg.roots.is_empty();
        let waiting = self.auto_index && !p.state.is_active();
        let mut act = Act::None;

        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                let label = match p.state {
                    RunState::Running if p.targeted => "바뀐 파일 반영 중",
                    RunState::Running if p.full_rebuild && p.scanning => "전체 재구축 중 (폴더 탐색 중)",
                    RunState::Running if p.full_rebuild => "전체 재구축 중",
                    RunState::Running if p.scanning => "인덱싱 진행 중 (폴더 탐색 중)",
                    s => s.label(),
                };
                widgets::tone_badge(ui, widgets::run_tone(p.state), label, true, p.state == RunState::Running);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // 오른쪽부터 놓인다
                    match p.state {
                        RunState::Running => {
                            if widgets::icon_button(ui, Btn::Secondary, widgets::Icon::Square, "중지")
                                .on_hover_text("처리 중인 파일까지만 마치고 멈춥니다. 다음에 이어서 처리됩니다.")
                                .clicked()
                            {
                                act = Act::Stop;
                            }
                            if widgets::icon_button(ui, Btn::Secondary, widgets::Icon::Pause, "일시정지").clicked() {
                                act = Act::Pause;
                            }
                        }
                        RunState::Paused => {
                            if widgets::icon_button(ui, Btn::Secondary, widgets::Icon::Square, "중지").clicked() {
                                act = Act::Stop;
                            }
                            if widgets::icon_button(ui, Btn::Primary, widgets::Icon::Play, "재개").clicked() {
                                act = Act::Resume;
                            }
                        }
                        RunState::Stopping => {}
                        _ => {
                            let can = !loading && !no_roots && !waiting;
                            if widgets::icon_button_enabled(ui, Btn::Primary, can, widgets::Icon::Play, "인덱싱 시작")
                                .on_hover_text("새 파일과 바뀐 파일만 처리합니다. 서버가 꺼져 있으면 켜고 시작합니다.")
                                .clicked()
                            {
                                act = Act::Start;
                            }
                        }
                    }
                });
            });

            // 진행 상황
            if p.state.is_active() {
                ui.add_space(space::XS);
                let frac = p.fraction().unwrap_or_else(|| {
                    if p.discovered > 0 {
                        (p.processed as f32 / p.discovered as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    }
                });
                if p.state == RunState::Running {
                    widgets::progress_accent(ui, frac, 8.0, true);
                } else {
                    let color = widgets::run_tone(p.state).fg(ui.visuals());
                    widgets::progress(ui, frac, color, 8.0, false);
                }
                let (fps, cps) = p.rates();
                let mut parts = Vec::new();
                match p.fraction() {
                    Some(f) => parts.push(format!("{:.1}%", f * 100.0)),
                    None => parts.push("폴더 탐색 중".to_string()),
                }
                parts.push(format!("{} / {}", fmt_num(p.processed), fmt_num(p.discovered)));
                match p.eta() {
                    Some(eta) => parts.push(format!("남은 시간 약 {}", fmt_dur(eta))),
                    None if p.scanning => parts.push("남은 시간은 탐색이 끝나면 계산".to_string()),
                    None => {}
                }
                parts.push(format!("{fps:.1} 파일/초 · {cps:.1} 벡터/초"));
                parts.push(format!("경과 {}", fmt_dur(p.elapsed())));
                ui.label(parts.join(" · "));
                if !p.current.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&p.current).monospace().weak()).truncate())
                        .on_hover_text(&p.current);
                }
            } else {
                widgets::help(ui, "폴더의 파일을 읽어 검색할 수 있게 준비합니다. 한 번 한 뒤에는 새 파일과 바뀐 파일만 처리합니다.");
            }

            // 시작할 수 없는 이유와 해결 방법
            if waiting {
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    widgets::spinner(ui);
                    ui.label("서버가 준비되면 바로 시작합니다…");
                });
            } else if !p.state.is_active() && no_roots {
                ui.add_space(space::XS);
                widgets::callout_ui(ui, Tone::Warn, "검색할 폴더가 없습니다", |ui| {
                    if widgets::button(ui, Btn::Secondary, "설정에서 폴더 추가").clicked() {
                        act = Act::Settings;
                    }
                });
            } else if !p.state.is_active() && loading {
                ui.add_space(space::XS);
                widgets::callout(ui, Tone::Info, "인덱스를 불러오는 중입니다", "끝나면 시작할 수 있습니다.");
            }
            if p.state == RunState::Failed && !p.message.is_empty() {
                ui.add_space(space::XS);
                widgets::callout(ui, Tone::Danger, "인덱싱이 실패했습니다", &p.message);
            }
            if self.recover_attempts > 0 {
                ui.add_space(space::XS);
                widgets::callout(
                    ui,
                    Tone::Warn,
                    &format!("서버 자동 복구 {}/{}회", self.recover_attempts, crate::app::MAX_RECOVER_ATTEMPTS),
                    "llama-server가 비정상 종료되어 다시 띄우고 이어서 인덱싱했습니다.",
                );
            }
        });

        match act {
            Act::None => {}
            Act::Start => {
                self.recover_attempts = 0;
                self.start_index_when_ready();
            }
            Act::Pause => self.indexer.pause(),
            Act::Resume => self.indexer.resume(),
            Act::Stop => self.indexer.stop(),
            Act::Settings => self.tab = crate::app::Tab::Settings,
        }
    }

    /// 이번 실행의 통계 카드 (한 번도 돌리지 않았으면 그리지 않는다)
    fn index_run_stats(&mut self, ui: &mut egui::Ui) {
        let p = self.indexer.progress.lock().clone();
        if p.state == RunState::Idle {
            return;
        }
        ui.add_space(space::S);
        widgets::section_title(ui, "이번 실행");
        ui.horizontal_wrapped(|ui| {
            let v = ui.visuals().clone();
            let muted = Tone::Muted.fg(&v);
            widgets::stat_card(ui, "발견한 파일", &fmt_num(p.discovered), Tone::Neutral.fg(&v));
            widgets::stat_card(ui, "처리 완료", &fmt_num(p.processed), Tone::Ok.fg(&v));
            widgets::stat_card(ui, "변경 없음(건너뜀)", &fmt_num(p.unchanged), muted);
            widgets::stat_card(ui, "이름만 인덱싱", &fmt_num(p.name_only), muted);
            widgets::stat_card(ui, "오류", &fmt_num(p.errors), if p.errors > 0 { Tone::Danger.fg(&v) } else { muted });
            widgets::stat_card(ui, "벡터(청크)", &fmt_num(p.chunks), Tone::Info.fg(&v));
            if p.moved > 0 {
                widgets::stat_card(ui, "옮김 반영", &fmt_num(p.moved), Tone::Info.fg(&v));
            }
            if p.removed > 0 {
                widgets::stat_card(ui, "삭제 반영", &fmt_num(p.removed), Tone::Warn.fg(&v));
            }
            if p.inaccessible > 0 {
                widgets::stat_card(ui, "접근 불가", &fmt_num(p.inaccessible), Tone::Warn.fg(&v));
            }
        });
    }

    /// 종류별 표: 이번 실행과 전체 인덱스, 내용까지 인덱싱하는지
    fn index_kinds(&mut self, ui: &mut egui::Ui) {
        let p = self.indexer.progress.lock().clone();
        let caps = self.server.capabilities();
        let media_ok = self.cfg.ffmpeg().is_some() && self.cfg.ffprobe().is_some();
        let stats = self.db_stats.lock().clone();
        widgets::section(ui, "종류별", None, |ui| {
            egui::Grid::new("kinds").num_columns(5).spacing([18.0, 8.0]).striped(true).show(ui, |ui| {
                for head in ["종류", "발견", "인덱싱됨", "전체", "내용 인덱싱"] {
                    ui.label(RichText::new(head).text_style(theme::ts_strong()).strong());
                }
                ui.end_row();
                for k in Kind::ALL {
                    ui.label(k.label());
                    ui.label(fmt_num(p.discovered_by_kind[k.idx()]));
                    ui.label(fmt_num(p.indexed_by_kind[k.idx()]));
                    ui.label(fmt_num(stats.files_by_kind[k.idx()]));
                    let (ok, why) = content_status(self, k, caps.as_ref(), media_ok);
                    if ok {
                        widgets::tone_note(ui, Tone::Ok, why);
                    } else {
                        widgets::tone_label(ui, Tone::Muted, why);
                    }
                    ui.end_row();
                }
            });
            widgets::caption(ui, "발견·인덱싱됨은 이번 실행, 전체는 지금 인덱스에 들어 있는 파일 수입니다.");
        });
    }

    /// 폴더 감시 상태: 폴더별로 변경 알림을 받는지, 네트워크 폴더의 한계, 대기 중인 변경, 다음 확인 시각
    fn index_watch(&mut self, ui: &mut egui::Ui) {
        let Some(w) = &self.watcher else { return };
        let snap = w.snapshot();
        let hours = self.saved_cfg.watch_rescan_hours;
        let (scan_now, ()) = widgets::section_with(
            ui,
            "폴더 감시",
            |ui| {
                widgets::small_button(ui, Btn::Secondary, "지금 확인")
                    .on_hover_text("폴더를 훑어 인덱스와 비교하고, 바뀐 파일이 있으면 반영합니다. 서버 없이 크기와 수정 시각만 봅니다.")
                    .clicked()
            },
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    if snap.scanning {
                        widgets::spinner(ui);
                        ui.label("폴더를 훑어 바뀐 파일을 찾는 중…");
                    } else if snap.pending > 0 {
                        widgets::tone_badge(ui, Tone::Info, &format!("바뀐 경로 {}개 대기", fmt_num(snap.pending as u64)), false, false);
                        widgets::caption(ui, "조용해지면 반영합니다 (전체 화면 앱을 쓰는 동안은 미룹니다)");
                    } else {
                        widgets::tone_badge(ui, Tone::Ok, "감시 중", false, false);
                    }
                    if let Some((at, n)) = snap.last_scan {
                        widgets::caption(ui, &format!("마지막 확인 {} 전 · 바뀐 것 {}개", fmt_dur(at.elapsed()), fmt_num(n as u64)));
                    }
                    if let (Some(t), false) = (snap.next_scan, snap.scanning) {
                        let left = t.saturating_duration_since(std::time::Instant::now());
                        widgets::caption(ui, &format!("다음 확인 {} 뒤", fmt_dur(left)));
                    }
                });
                ui.add_space(space::XS);
                for r in &snap.roots {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().interact_size.y = 22.0;
                        let (tone, text) = match (&r.error, r.network) {
                            _ if r.registering => (Tone::Muted, "확인 중…"),
                            (Some(_), _) => (Tone::Danger, "변경 알림 없음 · 주기적으로만 확인"),
                            (None, true) => (Tone::Warn, "네트워크 · 늦게 반영될 수 있음"),
                            (None, false) => (Tone::Ok, "바로 반영"),
                        };
                        let tag = widgets::tone_tag(ui, tone, text);
                        if let Some(e) = &r.error {
                            tag.on_hover_text(e);
                        }
                        ui.add(egui::Label::new(&r.path).truncate());
                    });
                }
                if let Some(note) = &snap.scan_note {
                    widgets::tone_note(ui, Tone::Warn, note);
                }
                if snap.roots.iter().any(|r| r.network || r.error.is_some()) {
                    let every = if hours > 0 { format!("{hours}시간마다") } else { "앱을 켤 때".to_string() };
                    widgets::caption(
                        ui,
                        &format!("네트워크 폴더는 NAS에 따라 변경 알림이 오지 않거나 빠집니다. {every} 폴더를 훑어 확인하므로, 반영이 그만큼 늦을 수 있습니다."),
                    );
                }
            },
        );
        if scan_now {
            w.scan_now();
        }
    }

    /// 전체 인덱스 요약과 대상 폴더
    fn index_totals(&mut self, ui: &mut egui::Ui) {
        let stats = self.db_stats.lock().clone();
        let mem = self.store.read().memory_mb();
        let mut go_settings = false;
        widgets::section(ui, "전체 인덱스", None, |ui| {
            egui::Grid::new("index_totals").num_columns(2).spacing([18.0, 8.0]).show(ui, |ui| {
                kv(ui, "파일", &format!("{}개", fmt_num(stats.files)));
                kv(ui, "벡터", &format!("{}개", fmt_num(stats.chunks)));
                kv(ui, "오류", &format!("{}개", fmt_num(stats.errors)));
                kv(
                    ui,
                    "DB 크기",
                    &format!(
                        "{:.1}MB (벡터 {})",
                        stats.file_bytes as f64 / 1048576.0,
                        if stats.f16 { "FP16" } else { "FP32" }
                    ),
                );
                kv(ui, "검색용 메모리", &format!("{mem:.1}MB"));
            });
            ui.add_space(space::S);
            ui.horizontal(|ui| {
                widgets::subheading(ui, "대상 폴더");
                if ui.link("설정에서 변경").clicked() {
                    go_settings = true;
                }
            });
            if self.cfg.roots.is_empty() {
                widgets::tone_label(ui, Tone::Warn, "없음");
            }
            for root in &self.cfg.roots {
                ui.add(egui::Label::new(root).truncate()).on_hover_text(root);
            }
        });
        if go_settings {
            self.tab = crate::app::Tab::Settings;
        }
    }

    /// 접어 두는 목록: 폴더별 인덱스, 오류·경고
    fn index_lists(&mut self, ui: &mut egui::Ui) {
        let folders = self.folders.lock().clone();
        if !folders.list.is_empty() {
            ui.add_space(space::S);
            widgets::collapsing_section(
                ui,
                "index_folders",
                format!("폴더별 인덱스 ({}개 폴더)", folders.list.len()),
                folders.list.len() > 1,
                |ui| {
                    egui::Grid::new("folders").num_columns(3).spacing([18.0, 6.0]).striped(true).show(ui, |ui| {
                        for f in &folders.list {
                            ui.label(&f.path);
                            ui.label(format!("{}개", fmt_num(f.files)));
                            if f.current {
                                widgets::tone_label(ui, Tone::Ok, "인덱싱 대상");
                            } else {
                                widgets::tone_label(ui, Tone::Muted, "예전 대상 (결과 유지)");
                            }
                            ui.end_row();
                        }
                        if folders.other > 0 {
                            ui.label(RichText::new("기타 위치").italics());
                            ui.label(format!("{}개", fmt_num(folders.other)));
                            widgets::tone_label(ui, Tone::Muted, "폴더 기록 이전 항목");
                            ui.end_row();
                        }
                    });
                },
            );
        }

        let p = self.indexer.progress.lock().clone();
        if !p.error_list.is_empty() {
            ui.add_space(space::XS);
            let tone = if p.errors > 0 { Tone::Danger } else { Tone::Warn };
            let title = RichText::new(format!("오류 {}개 · 경고 {}개", fmt_num(p.errors), fmt_num(p.warnings)))
                .color(tone.fg(ui.visuals()));
            widgets::collapsing_section(ui, "index_errors", title, false, |ui| {
                let danger = Tone::Danger.fg(ui.visuals());
                egui::ScrollArea::vertical()
                    .id_salt("index_error_list")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (path, msg) in &p.error_list {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(path).monospace());
                                ui.label(RichText::new(msg).color(danger));
                            });
                        }
                    });
            });
        }
    }

    /// 전체 재구축: 되돌릴 수 없는 동작이라 맨 아래에 따로 둔다
    fn index_rebuild(&mut self, ui: &mut egui::Ui) {
        let blocker = self.index_blocker();
        widgets::section(ui, "인덱스 다시 만들기", None, |ui| {
            widgets::help(
                ui,
                "인덱스를 지우고 처음부터 다시 만듭니다. 모델을 바꿨거나 인덱싱 설정을 기존 파일에도 적용하려는 경우에만 필요합니다.",
            );
            ui.add_space(space::XS);
            let r = widgets::button_enabled(ui, Btn::Danger, blocker.is_none(), "전체 재구축…");
            let r = match &blocker {
                Some(b) => r.on_disabled_hover_text(b),
                None => r,
            };
            if r.clicked() {
                self.confirm_rebuild = true;
            }
        });
    }
}

fn content_status(
    app: &GemmaApp,
    k: Kind,
    caps: Option<&crate::embed::selftest::Capabilities>,
    media_ok: bool,
) -> (bool, &'static str) {
    let c = &app.cfg;
    let img = caps.map(|c| c.image.usable());
    let aud = caps.map(|c| c.audio.usable());
    match k {
        Kind::Text if c.index_text => (true, "텍스트"),
        Kind::Doc if c.index_docs => (true, "텍스트 추출"),
        Kind::Image if !c.index_images => (false, "꺼짐"),
        Kind::Image => match img {
            Some(true) => (true, "이미지 임베딩"),
            Some(false) => (false, "이미지 진단 실패 → 이름만"),
            None => (false, "서버 진단 대기"),
        },
        Kind::Video if !c.index_video => (false, "꺼짐"),
        Kind::Video if !media_ok => (false, "ffmpeg 없음 → 이름만"),
        Kind::Video => match (img, aud) {
            (Some(true), Some(true)) if c.index_audio => (true, "영상 구간 + 오디오"),
            (Some(true), _) => (true, "영상 구간"),
            (Some(false), _) => (false, "이미지 진단 실패 → 이름만"),
            _ => (false, "서버 진단 대기"),
        },
        Kind::Audio if !c.index_audio => (false, "꺼짐"),
        Kind::Audio if !media_ok => (false, "ffmpeg 없음 → 이름만"),
        Kind::Audio => match aud {
            Some(true) => (true, "오디오 (실험적)"),
            Some(false) => (false, "오디오 진단 실패 → 이름만"),
            None => (false, "서버 진단 대기"),
        },
        Kind::Other => (false, "이름만"),
        _ => (false, "꺼짐"),
    }
}
