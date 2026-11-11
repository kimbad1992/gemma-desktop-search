//! PC 사양 확인(그래픽카드·메모리·디스크)을 백그라운드에서 돌리고 결과를 들고 있는다.
//! 첫 실행 마법사와 설정의 "성능"·"필요한 파일"이 같이 쓴다.

use crate::config::Config;
use crate::precheck::Verdict;
use crate::sysinfo::{Plan, SysInfo};
use crossbeam_channel::Receiver;
use eframe::egui;

#[derive(Default)]
pub struct ProbeState {
    /// 마지막 확인 결과 (사양, 그에 맞춘 추천)
    pub result: Option<(SysInfo, Plan)>,
    rx: Option<Receiver<(SysInfo, Plan)>>,
}

impl ProbeState {
    /// 사양 확인을 (다시) 시작한다. 끝나면 화면을 깨운다.
    pub fn start(&mut self, ffmpeg: bool, ctx: &egui::Context) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let info = crate::sysinfo::probe(&crate::config::prep_dir(), ffmpeg);
            let plan = crate::sysinfo::recommend(&info);
            let _ = tx.send((info, plan));
            ctx.request_repaint();
        });
        self.result = None;
        self.rx = Some(rx);
    }

    /// 결과가 도착했으면 받아 둔다. 이번에 새로 도착했으면 true.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(r) => {
                self.result = Some(r);
                self.rx = None;
                true
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.rx = None;
                false
            }
            Err(crossbeam_channel::TryRecvError::Empty) => false,
        }
    }

    /// 확인하는 중인지
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    pub fn plan(&self) -> Option<&Plan> {
        self.result.as_ref().map(|r| &r.1)
    }
}

/// 준비물이 실제로 쓸 만한지 확인한 결과를 들고 있는다 (`precheck`). 마법사와 고급 설정이 같이 쓴다.
/// llama.cpp는 실행해 보느라 시간이 걸리므로 백그라운드에서 돌리고, 경로가 바뀌면 다시 확인한다.
#[derive(Default)]
pub struct FileChecks {
    /// 확인했거나 확인 중인 대상 (llama 경로, DLL 경로, 찾은 실행 파일)
    engine_key: String,
    engine: Option<Verdict>,
    engine_rx: Option<Receiver<Verdict>>,
    /// (모델 경로와 크기, 결과)
    model: Option<(String, Verdict)>,
}

impl FileChecks {
    /// llama.cpp 실행 확인 결과. 아직 확인 중이면 None.
    pub fn engine(&mut self, cfg: &Config, ctx: &egui::Context) -> Option<Verdict> {
        // 찾은 실행 파일도 열쇠에 넣는다: 받기가 끝나 파일이 생기면 다시 확인한다
        let resolved = crate::server::resolve_binary(&cfg.llama_path)
            .map(|(p, _)| p.display().to_string())
            .unwrap_or_default();
        let key = format!("{}\n{}\n{resolved}", cfg.llama_path.trim(), cfg.extra_dll_dir.trim());
        if key != self.engine_key {
            self.engine_key = key;
            self.engine = None;
            let (tx, rx) = crossbeam_channel::bounded(1);
            let (llama, dll, ctx) = (cfg.llama_path.clone(), cfg.extra_dll_dir.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(crate::precheck::check_engine(&llama, &dll));
                ctx.request_repaint();
            });
            self.engine_rx = Some(rx);
        }
        if let Some(rx) = &self.engine_rx {
            match rx.try_recv() {
                Ok(v) => {
                    self.engine = Some(v);
                    self.engine_rx = None;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.engine = Some(Verdict::bad("확인하지 못했습니다"));
                    self.engine_rx = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }
        self.engine.clone()
    }

    /// 모델 파일 확인 결과 (파일 앞부분만 읽으므로 바로 끝난다)
    pub fn model(&mut self, cfg: &Config) -> Verdict {
        let path = cfg.model_path.trim();
        let len = std::fs::metadata(path).map(|m| m.len()).ok();
        let key = format!("{path}\n{len:?}");
        match &self.model {
            Some((k, v)) if *k == key => v.clone(),
            _ => {
                let v = crate::precheck::check_model(path);
                self.model = Some((key, v.clone()));
                v
            }
        }
    }

    /// 다음에 물을 때 처음부터 다시 확인한다 (빠진 DLL을 채워 넣은 뒤 등)
    pub fn recheck(&mut self) {
        *self = Self::default();
    }
}
