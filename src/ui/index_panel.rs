//! 인덱스 탭: 진행 상태를 한눈에 (상태 배지, 진행 막대, 카운터, 종류별 표, 속도/ETA, 오류 목록)

use crate::app::GemmaApp;
use crate::extract::Kind;
use crate::index::progress::RunState;
use crate::ui::widgets;
use crate::util::{fmt_dur, fmt_num};
use eframe::egui::{self, RichText};

impl GemmaApp {
    pub fn index_tab(&mut self, ui: &mut egui::Ui) {
        let p = self.indexer.progress.lock().clone();
        let blocker = self.index_blocker();

        // ── 상태 + 제어 버튼 ──
        ui.horizontal(|ui| {
            let label = match p.state {
                RunState::Running if p.full_rebuild && p.scanning => "전체 재구축 중 (폴더 탐색 중)",
                RunState::Running if p.full_rebuild => "전체 재구축 중",
                RunState::Running if p.scanning => "인덱싱 진행 중 (폴더 탐색 중)",
                s => s.label(),
            };
            widgets::badge(ui, widgets::run_color(p.state), label, true, p.state == RunState::Running);
            ui.add_space(16.0);

            let active = p.state.is_active();
            let start = ui
                .add_enabled(blocker.is_none(), egui::Button::new(RichText::new("▶ 인덱싱 시작 (변경분)").size(15.0)));
            let start = match &blocker {
                Some(b) if !active => start.on_disabled_hover_text(b),
                _ => start.on_hover_text("새 파일과 바뀐 파일만 처리합니다"),
            };
            if start.clicked() {
                self.start_index(false);
            }
            if ui
                .add_enabled(blocker.is_none(), egui::Button::new("전체 재구축"))
                .on_hover_text("인덱스를 지우고 처음부터 다시 만듭니다")
                .clicked()
            {
                self.confirm_rebuild = true;
            }
            match p.state {
                RunState::Running => {
                    if ui.button(RichText::new("❚❚ 일시정지").size(15.0)).clicked() {
                        self.indexer.pause();
                    }
                }
                RunState::Paused => {
                    if ui.button(RichText::new("▶ 재개").size(15.0)).clicked() {
                        self.indexer.resume();
                    }
                }
                _ => {}
            }
            if ui
                .add_enabled(matches!(p.state, RunState::Running | RunState::Paused), egui::Button::new(RichText::new("■ 중지").size(15.0)))
                .on_hover_text("처리 중인 파일까지만 마치고 멈춥니다. 다음 실행에서 이어서 처리됩니다.")
                .clicked()
            {
                self.indexer.stop();
            }
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("대상 폴더:").weak());
            if self.cfg.roots.is_empty() {
                ui.colored_label(widgets::ORANGE, "없음");
            } else {
                ui.label(self.cfg.roots.join("   ·   "));
            }
            if ui.link("설정에서 변경").clicked() {
                self.tab = crate::app::Tab::Settings;
            }
        });

        if p.state == RunState::Failed && !p.message.is_empty() {
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(widgets::RED.gamma_multiply(0.12))
                .stroke(egui::Stroke::new(1.0, widgets::RED))
                .corner_radius(6)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.colored_label(widgets::RED, RichText::new(&p.message).strong());
                });
        }

        // ── 진행 막대 ──
        ui.add_space(8.0);
        let bar = match p.fraction() {
            Some(f) => egui::ProgressBar::new(f).text(
                RichText::new(format!(
                    "{:.1}%  ({} / {})",
                    f * 100.0,
                    fmt_num(p.processed),
                    fmt_num(p.discovered)
                ))
                .size(14.0),
            ),
            None if p.state.is_active() => egui::ProgressBar::new(if p.discovered > 0 {
                (p.processed as f32 / p.discovered as f32).clamp(0.0, 1.0)
            } else {
                0.0
            })
            .animate(true)
            .text(
                RichText::new(format!(
                    "폴더 탐색 중… 발견 {}개 · 처리 {}개",
                    fmt_num(p.discovered),
                    fmt_num(p.processed)
                ))
                .size(14.0),
            ),
            None => egui::ProgressBar::new(0.0).text("대기"),
        };
        let bar = if p.state == RunState::Paused {
            bar.fill(widgets::YELLOW)
        } else if p.state == RunState::Completed {
            bar.fill(widgets::BLUE)
        } else {
            bar
        };
        ui.add(bar.desired_height(26.0).animate(p.state == RunState::Running));

        // ── 카운터 카드 ──
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            widgets::stat_card(ui, "발견한 파일", &fmt_num(p.discovered), ui.visuals().text_color());
            widgets::stat_card(ui, "처리 완료", &fmt_num(p.processed), widgets::GREEN);
            widgets::stat_card(ui, "변경 없음(건너뜀)", &fmt_num(p.unchanged), widgets::GRAY);
            widgets::stat_card(ui, "이름만 인덱싱", &fmt_num(p.name_only), widgets::GRAY);
            widgets::stat_card(
                ui,
                "오류",
                &fmt_num(p.errors),
                if p.errors > 0 { widgets::RED } else { widgets::GRAY },
            );
            widgets::stat_card(ui, "벡터(청크)", &fmt_num(p.chunks), widgets::BLUE);
            if p.removed > 0 {
                widgets::stat_card(ui, "삭제 반영", &fmt_num(p.removed), widgets::ORANGE);
            }
            if p.inaccessible > 0 {
                widgets::stat_card(ui, "접근 불가", &fmt_num(p.inaccessible), widgets::ORANGE);
            }
        });

        // ── 속도 / 시간 ──
        ui.add_space(6.0);
        let (fps, cps) = p.rates();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("경과 {}", fmt_dur(p.elapsed())));
            ui.separator();
            ui.label(format!("{fps:.1} 파일/초 · {cps:.1} 벡터/초"));
            ui.separator();
            match p.eta() {
                Some(eta) => ui.label(format!("남은 시간 약 {}", fmt_dur(eta))),
                None if p.scanning => ui.label("남은 시간: 탐색이 끝나면 계산"),
                None => ui.label("남은 시간 -"),
            };
        });
        if !p.current.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new("현재:").weak());
                ui.add(egui::Label::new(RichText::new(&p.current).monospace().size(12.0)).truncate());
            });
        }

        // ── 종류별 ──
        ui.add_space(8.0);
        let caps = self.server.capabilities();
        let media_ok = self.cfg.ffmpeg().is_some() && self.cfg.ffprobe().is_some();
        widgets::section_frame(ui, |ui| {
            egui::Grid::new("kinds").num_columns(5).spacing([22.0, 6.0]).striped(true).show(ui, |ui| {
                ui.label(RichText::new("종류").strong());
                ui.label(RichText::new("이번 실행: 발견").strong());
                ui.label(RichText::new("인덱싱됨").strong());
                ui.label(RichText::new("전체 인덱스").strong());
                ui.label(RichText::new("내용 인덱싱").strong());
                ui.end_row();
                let stats = self.db_stats.lock().clone();
                for k in Kind::ALL {
                    ui.label(k.label());
                    ui.label(fmt_num(p.discovered_by_kind[k.idx()]));
                    ui.label(fmt_num(p.indexed_by_kind[k.idx()]));
                    ui.label(fmt_num(stats.files_by_kind[k.idx()]));
                    let (ok, why) = content_status(self, k, caps.as_ref(), media_ok);
                    if ok {
                        ui.colored_label(widgets::GREEN, why);
                    } else {
                        ui.colored_label(widgets::GRAY, why);
                    }
                    ui.end_row();
                }
            });
        });

        // ── 전체 인덱스 요약 ──
        let stats = self.db_stats.lock().clone();
        let mem = self.store.read().memory_mb();
        ui.label(
            RichText::new(format!(
                "전체 인덱스: 파일 {}개 · 벡터 {}개 · 오류 {}개 · DB {:.1}MB (벡터 {}) · 검색용 메모리 {:.1}MB",
                fmt_num(stats.files),
                fmt_num(stats.chunks),
                fmt_num(stats.errors),
                stats.file_bytes as f64 / 1048576.0,
                if stats.f16 { "FP16" } else { "FP32" },
                mem
            ))
            .weak(),
        );

        // ── 폴더별 ──
        let folders = self.folders.lock().clone();
        if !folders.list.is_empty() {
            ui.add_space(4.0);
            egui::CollapsingHeader::new(format!("폴더별 인덱스 ({}개 폴더)", folders.list.len()))
                .default_open(folders.list.len() > 1)
                .show(ui, |ui| {
                    egui::Grid::new("folders").num_columns(3).spacing([22.0, 4.0]).striped(true).show(ui, |ui| {
                        for f in &folders.list {
                            ui.label(&f.path);
                            ui.label(format!("{}개", fmt_num(f.files)));
                            if f.current {
                                ui.colored_label(widgets::GREEN, "인덱싱 대상");
                            } else {
                                ui.colored_label(widgets::GRAY, "예전 대상 (결과 유지)");
                            }
                            ui.end_row();
                        }
                        if folders.other > 0 {
                            ui.label(RichText::new("기타 위치").italics());
                            ui.label(format!("{}개", fmt_num(folders.other)));
                            ui.colored_label(widgets::GRAY, "폴더 기록 이전 항목");
                            ui.end_row();
                        }
                    });
                });
        }

        // ── 오류 목록 ──
        if !p.error_list.is_empty() {
            ui.add_space(6.0);
            egui::CollapsingHeader::new(
                RichText::new(format!(
                    "오류 {}개 · 경고 {}개 (목록)",
                    fmt_num(p.errors),
                    fmt_num(p.warnings)
                ))
                .color(if p.errors > 0 { widgets::RED } else { widgets::ORANGE }),
            )
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    for (path, msg) in &p.error_list {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(path).monospace().size(12.0));
                            ui.label(RichText::new(msg).color(widgets::RED).size(12.0));
                        });
                    }
                });
            });
        }
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
        Kind::Text if c.index_text => (true, "✔ 텍스트"),
        Kind::Doc if c.index_docs => (true, "✔ 텍스트 추출"),
        Kind::Image if !c.index_images => (false, "꺼짐"),
        Kind::Image => match img {
            Some(true) => (true, "✔ 이미지 임베딩"),
            Some(false) => (false, "이미지 진단 실패 → 이름만"),
            None => (false, "서버 진단 대기"),
        },
        Kind::Video if !c.index_video => (false, "꺼짐"),
        Kind::Video if !media_ok => (false, "ffmpeg 없음 → 이름만"),
        Kind::Video => match (img, aud) {
            (Some(true), Some(true)) if c.index_audio => (true, "✔ 프레임 + 오디오"),
            (Some(true), _) => (true, "✔ 프레임"),
            (Some(false), _) => (false, "이미지 진단 실패 → 이름만"),
            _ => (false, "서버 진단 대기"),
        },
        Kind::Audio if !c.index_audio => (false, "꺼짐"),
        Kind::Audio if !media_ok => (false, "ffmpeg 없음 → 이름만"),
        Kind::Audio => match aud {
            Some(true) => (true, "✔ 오디오 (실험적)"),
            Some(false) => (false, "오디오 진단 실패 → 이름만"),
            None => (false, "서버 진단 대기"),
        },
        Kind::Other => (false, "이름만"),
        _ => (false, "꺼짐"),
    }
}
