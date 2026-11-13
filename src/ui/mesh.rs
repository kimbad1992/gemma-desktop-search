//! 원격 라이브러리의 화면과 앱 연결 (메시 3단계, `crate::mesh`, `MESH-DESIGN.md` 7장)
//!
//! - 설정 > 일반 > 내 다른 기기: 참여 코드 넣기, 기기 목록과 연결 상태, 빼기, 놀 때 계산 빌려주기.
//! - 설정 > 일반 > 검색할 폴더: 네트워크 폴더를 추가하면 소유 기기를 찾아 원격 라이브러리로 붙인다
//!   (`mesh_add_folder`). 모르는 서버면 그 서버에 Gemma Search가 있는지 물어보고, 있으면 그 자리에서 참여 코드를
//!   받아 참여와 붙이기를 이어서 한다. 원격 라이브러리 줄에 소유 기기의 인덱싱 진행·"연결 안 됨"이 보이고, 빼면 이
//!   기기의 검색 대상에서만 빠진다(라이브러리와 인덱스는 소유 기기에 남는다).
//! - 검색: 내 인덱스와 원격 라이브러리에 동시에 묻는다 (`GemmaApp::start_search`가 `remote_queries`를 쓴다).
//!   닿지 않는 것으로 알려진 기기는 묻지 않고 "연결 안 됨"으로 보여 검색이 기다리지 않게 한다.
//!   원격 결과의 썸네일은 소유 기기에서 받아 온다 (`request_remote_thumb`).
//! - 계산 빌려주기 (`auto_lend`): 이 기기가 추가해서 시작된 인덱싱은 늘 돕고, "놀 때 빌려주기"를 켜면 내 다른
//!   기기가 인덱싱할 때마다 돕는다. 쉬는 조건(전체 화면 앱, 모자란 그래픽 메모리)은 빌려주기가 따른다.

use crate::app::GemmaApp;
use crate::mesh::client::{AddOutcome, Hello, RemoteQuery};
use crate::mesh::monitor::{DeviceState, Monitor};
use crate::mesh::{AttachReply, JoinCode, MeshDevice, RemoteLibrary};
use crate::search::query::{Outcome, RemoteNote};
use crate::ui::theme::{space, Tone};
use crate::ui::widgets::{self, Btn};
use crate::util::fmt_num;
use crossbeam_channel::{Receiver, Sender};
use eframe::egui::{self, RichText};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// 소유 기기의 인덱싱이 끝나고 이만큼 조용하면 자동으로 시작한 빌려주기를 끝낸다
const LEND_DONE_AFTER: Duration = Duration::from_secs(60);

/// 참여 코드로 참여하고 붙인 결과
type JoinAdd = Result<(MeshDevice, RemoteLibrary, AttachReply), String>;

/// 모르는 서버의 네트워크 폴더: 참여 코드를 받으면 그 기기의 라이브러리로 붙인다
pub struct PendingJoin {
    pub path: String,
    pub server: String,
    /// 그 서버에서 찾은 Gemma Search (못 찾았으면 None)
    pub hello: Option<Hello>,
    pub code: String,
    pub error: Option<String>,
    rx: Option<Receiver<JoinAdd>>,
}

/// 앱이 들고 있는 원격 라이브러리 상태
pub struct MeshUi {
    pub monitor: Monitor,
    /// 참여 코드 입력칸
    pub join_input: String,
    join_rx: Option<Receiver<Result<MeshDevice, String>>>,
    pub join_error: Option<String>,
    /// 네트워크 폴더를 붙이는 중: (경로, 결과)
    add_rx: Option<(String, Receiver<Result<AddOutcome, String>>)>,
    /// 원격 라이브러리로 붙이지 못함: (경로, 까닭). 이 PC에서 직접 인덱싱할지 묻는다.
    pub add_failed: Option<(String, String)>,
    /// 모르는 서버의 폴더: 참여 코드를 기다린다
    pub pending: Option<PendingJoin>,
    /// 원격 결과의 썸네일 번호(음수) → (기기 ID, 소유 기기의 조각 번호)
    thumb_refs: HashMap<i64, (String, i64)>,
    thumb_pending: HashSet<i64>,
    thumb_tx: Sender<(i64, Option<egui::ColorImage>)>,
    thumb_rx: Receiver<(i64, Option<egui::ColorImage>)>,
    /// 지금 결과에 원격 결과가 섞여 있다: 결과마다 출처를 보인다
    pub show_source: bool,
    /// 원격 라이브러리 인덱싱 동안 자동으로 시작한 빌려주기의 소유 기기 ID
    pub lend_auto: Option<String>,
    /// 자동 빌려주기를 할 수 없었던 기기 (서명이 다르다, 준비물이 없다 등): 다시 시도하지 않는다
    lend_auto_skip: HashSet<String>,
}

impl MeshUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let (thumb_tx, thumb_rx) = crossbeam_channel::unbounded();
        Self {
            monitor: Monitor::start(crate::app::waker(ctx)),
            join_input: String::new(),
            join_rx: None,
            join_error: None,
            add_rx: None,
            add_failed: None,
            pending: None,
            thumb_refs: HashMap::new(),
            thumb_pending: HashSet::new(),
            thumb_tx,
            thumb_rx,
            show_source: false,
            lend_auto: None,
            lend_auto_skip: HashSet::new(),
        }
    }

    /// 네트워크 폴더를 붙이는 중이면 그 경로
    pub fn adding(&self) -> Option<&str> {
        self.add_rx.as_ref().map(|(p, _)| p.as_str())
    }

    /// 폴더 추가를 새로 받을 수 있는지 (붙이는 중이거나 참여 코드를 기다리는 중이면 아니다)
    pub fn can_add(&self) -> bool {
        self.add_rx.is_none() && self.pending.is_none()
    }
}

/// 그 기기가 닿지 않는 것으로 알려졌는지 (아직 모르면 false: 물어본다)
pub fn known_offline(st: Option<&DeviceState>) -> bool {
    st.is_some_and(|s| s.reachable == Some(false))
}

/// 그 기기가 지금 인덱싱 중(또는 곧 시작)이라 계산을 빌려줄 만한지
pub fn owner_needs_help(st: Option<&DeviceState>) -> bool {
    st.is_some_and(|s| s.reachable == Some(true) && s.indexing.busy())
}

/// 그 기기의 인덱싱이 끝나고 `after` 넘게 조용했는지 (자동 빌려주기를 끝낸다)
pub fn owner_done(st: Option<&DeviceState>, after: Duration) -> bool {
    st.is_some_and(|s| s.reachable == Some(true) && !s.indexing.busy() && s.idle_since.is_some_and(|t| t.elapsed() > after))
}

/// 원격 라이브러리 한 줄의 상태 글과 색
pub fn library_status(lib: &RemoteLibrary, device: Option<&MeshDevice>, st: Option<&DeviceState>) -> (Tone, String) {
    let Some(d) = device else {
        return (Tone::Danger, "메시 기기 목록에 없는 기기".to_string());
    };
    let Some(st) = st.filter(|s| s.reachable.is_some()) else {
        return (Tone::Muted, format!("{} · 확인하는 중", d.name));
    };
    if st.reachable == Some(false) {
        return (Tone::Warn, format!("{} · {}", d.name, if st.error.is_empty() { "연결 안 됨" } else { &st.error }));
    }
    let i = &st.indexing;
    if i.busy() {
        let what = if i.discovered > 0 {
            let pct = if i.scanning { String::new() } else { format!(" ({:.0}%)", (i.fraction * 100.0).clamp(0.0, 100.0)) };
            format!("{}에서 인덱싱 중 · {} / {}{pct}", d.name, fmt_num(i.processed), fmt_num(i.discovered))
        } else if i.active {
            format!("{}에서 인덱싱 중 · 파일을 찾는 중", d.name)
        } else {
            format!("{}에서 곧 인덱싱합니다", d.name)
        };
        return (Tone::Info, what);
    }
    let files = st.libraries.iter().find(|l| l.id == lib.library_id).map(|l| l.files);
    match files {
        Some(n) => (Tone::Ok, format!("{} · 파일 {}개", d.name, fmt_num(n))),
        None => (Tone::Ok, d.name.clone()),
    }
}

impl GemmaApp {
    /// 원격 라이브러리 설정을 바로 저장한다 (설정 화면의 저장 단추와 상관없이)
    fn save_mesh(&mut self) {
        let mut s = self.saved_cfg.clone();
        s.mesh_devices = self.cfg.mesh_devices.clone();
        s.remote_libraries = self.cfg.remote_libraries.clone();
        s.lend_mesh_idle = self.cfg.lend_mesh_idle;
        if let Err(e) = s.save(&self.data_dir) {
            self.notify(format!("설정을 저장하지 못했습니다: {e}"), true);
            return;
        }
        self.saved_cfg = s;
    }

    /// 메시 기기를 넣거나 (같은 ID면) 고친다
    fn upsert_device(&mut self, d: MeshDevice) {
        match self.cfg.mesh_devices.iter_mut().find(|x| x.id == d.id) {
            Some(x) => *x = d,
            None => self.cfg.mesh_devices.push(d),
        }
    }

    /// 라이브러리를 붙였다: 목록에 넣고 저장, 상태를 자주 묻게 하고 알린다
    fn on_attached(&mut self, device: MeshDevice, library: RemoteLibrary, reply: AttachReply) {
        let lib_name = library.name();
        let dup = self
            .cfg
            .remote_libraries
            .iter_mut()
            .find(|l| l.device_id == library.device_id && l.owner_folder == library.owner_folder);
        match dup {
            Some(l) => {
                l.local_folder = library.local_folder.clone();
                l.enabled = true;
                l.lend_while_indexing |= library.lend_while_indexing;
            }
            None => self.cfg.remote_libraries.push(library),
        }
        self.save_mesh();
        if reply.indexing {
            self.mesh.monitor.expect_indexing(&device.id);
        }
        self.mesh.monitor.poke(120);
        let msg = if reply.created {
            format!("{}에서 '{lib_name}' 인덱싱을 시작했습니다. 끝난 파일부터 바로 검색되고, 그동안 이 PC도 계산을 돕습니다.", device.name)
        } else {
            format!("'{lib_name}'은(는) {}에 이미 인덱싱돼 있어 다시 하지 않고 바로 검색합니다.", device.name)
        };
        self.notify(msg, false);
    }

    /// 이 PC가 네트워크로 직접 인덱싱할 폴더로 넣는다 (저장은 검색할 폴더의 "저장하고 지금 하기")
    fn add_local_root(&mut self, path: String) {
        if !self.cfg.roots.contains(&path) {
            self.cfg.roots.push(path);
            self.invalidate_health();
        }
    }

    /// 매 프레임 (app.poll): 상태 확인 스레드에 기기 목록 주기, 참여·붙이기 결과, 원격 썸네일, 자동 빌려주기
    pub fn mesh_logic(&mut self, ctx: &egui::Context) {
        self.mesh.monitor.set_devices(&self.cfg.mesh_devices);
        // 참여 코드 결과 (내 다른 기기 칸)
        if let Some(r) = self.mesh.join_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.mesh.join_rx = None;
            match r {
                Ok(d) => {
                    let name = d.name.clone();
                    self.upsert_device(d);
                    self.save_mesh();
                    self.mesh.join_input.clear();
                    self.mesh.join_error = None;
                    self.mesh.monitor.poke(10);
                    self.notify(format!("{name}을(를) 내 다른 기기로 넣었습니다. 그 기기의 공유 폴더를 검색 폴더로 추가해 보세요."), false);
                }
                Err(e) => self.mesh.join_error = Some(e),
            }
        }
        // 네트워크 폴더 붙이기 결과
        let added = self.mesh.add_rx.as_ref().and_then(|(p, rx)| rx.try_recv().ok().map(|r| (p.clone(), r)));
        if let Some((path, r)) = added {
            self.mesh.add_rx = None;
            match r {
                Ok(AddOutcome::Attached { device, library, reply }) => self.on_attached(device, library, reply),
                // 네트워크 공유가 아니다: 지금처럼 이 기기가 직접 인덱싱한다
                Ok(AddOutcome::NotMesh(_)) => self.add_local_root(path),
                // 모르는 서버: Gemma Search가 있으면 참여 코드를 받는다
                Ok(AddOutcome::Detected { server, hello }) => {
                    self.mesh.pending = Some(PendingJoin { path, server, hello: Some(hello), code: String::new(), error: None, rx: None });
                }
                // 모르는 서버이고 Gemma Search도 없다: 메시를 쓰지 않으면 지금처럼 바로 직접 인덱싱, 쓰면 고르게 한다
                Ok(AddOutcome::NoService { server }) => {
                    if self.cfg.mesh_devices.is_empty() {
                        self.add_local_root(path);
                    } else {
                        self.mesh.pending = Some(PendingJoin { path, server, hello: None, code: String::new(), error: None, rx: None });
                    }
                }
                Err(e) => self.mesh.add_failed = Some((path, e)),
            }
        }
        // 참여 코드로 참여하고 붙인 결과 (모르는 서버의 폴더)
        let joined = self.mesh.pending.as_mut().and_then(|p| p.rx.as_ref().and_then(|rx| rx.try_recv().ok()));
        if let Some(r) = joined {
            match r {
                Ok((device, library, reply)) => {
                    self.mesh.pending = None;
                    self.upsert_device(device.clone());
                    self.on_attached(device, library, reply);
                }
                Err(e) => {
                    if let Some(p) = self.mesh.pending.as_mut() {
                        p.rx = None;
                        p.error = Some(e);
                    }
                }
            }
        }
        // 원격 썸네일
        while let Ok((id, img)) = self.mesh.thumb_rx.try_recv() {
            self.mesh.thumb_pending.remove(&id);
            let tex = img.map(|ci| ctx.load_texture(format!("rthumb{id}"), ci, egui::TextureOptions::LINEAR));
            self.thumbs.insert(id, tex);
        }
        self.auto_lend(ctx);
    }

    /// 폴더 고르기에서 고른 폴더: 네트워크 폴더면 그 폴더가 있는 기기를 찾기 시작하고 true
    /// (메시 기기면 원격 라이브러리로 붙이고, 모르는 서버면 Gemma Search가 있는지 물어본다).
    /// 아니면 false (부른 쪽이 지금처럼 이 기기의 검색 폴더에 넣는다).
    pub fn mesh_add_folder(&mut self, path: &str) -> bool {
        if !self.mesh.can_add() || !crate::platform::is_network_path(path) {
            return false;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let devices = self.cfg.mesh_devices.clone();
        let p = path.to_string();
        let _ = std::thread::Builder::new().name("mesh-add".into()).spawn(move || {
            let _ = tx.send(crate::mesh::client::add_network_folder(&p, &devices));
        });
        self.mesh.add_rx = Some((path.to_string(), rx));
        self.mesh.add_failed = None;
        true
    }

    /// 지금 검색할 원격 라이브러리들(기기마다 한 묶음)과, 닿지 않는 것으로 알려진 기기의 "연결 안 됨".
    /// 꺼진 기기에 묻느라 검색이 기다리지 않게 그 기기는 묻지 않고, 상태 확인만 바로 다시 하게 한다.
    pub fn remote_queries(&self) -> (Vec<RemoteQuery>, Vec<RemoteNote>) {
        let (mut queries, mut offline) = (Vec::new(), Vec::new());
        for (index, d) in self.cfg.mesh_devices.iter().enumerate() {
            let libraries: Vec<RemoteLibrary> = self
                .cfg
                .remote_libraries
                .iter()
                .filter(|l| l.enabled && l.device_id == d.id)
                .cloned()
                .collect();
            if libraries.is_empty() {
                continue;
            }
            if known_offline(self.mesh.monitor.state(&d.id).as_ref()) {
                offline.push(RemoteNote { name: d.name.clone(), error: Some("연결 안 됨".to_string()) });
                self.mesh.monitor.poke(0);
                continue;
            }
            queries.push(RemoteQuery { index, device: d.clone(), base: self.mesh.monitor.base(&d.id), libraries });
        }
        (queries, offline)
    }

    /// 새 검색 결과가 왔다: 원격 결과의 썸네일을 받을 곳을 적어 둔다
    pub fn mesh_register_outcome(&mut self, o: &Outcome) {
        self.mesh.show_source = o.has_remote();
        if self.mesh.thumb_refs.len() > 4000 {
            self.mesh.thumb_refs.clear();
        }
        for (_, hits) in &o.sections {
            for hv in hits {
                if let (Some(r), Some(id), Some(remote_thumb)) = (&hv.remote, hv.thumb_chunk, hv.remote.as_ref().and_then(|r| r.thumb)) {
                    self.mesh.thumb_refs.insert(id, (r.device_id.clone(), remote_thumb));
                }
            }
        }
    }

    /// 원격 결과의 썸네일을 소유 기기에서 받기 시작한다 (`thumb_texture`가 음수 번호일 때 부른다)
    pub fn request_remote_thumb(&mut self, ctx: &egui::Context, id: i64) {
        if self.mesh.thumb_pending.contains(&id) {
            return;
        }
        let Some((device_id, chunk)) = self.mesh.thumb_refs.get(&id).cloned() else {
            return;
        };
        let Some(device) = self.cfg.mesh_devices.iter().find(|d| d.id == device_id).cloned() else {
            return;
        };
        self.mesh.thumb_pending.insert(id);
        let base = self.mesh.monitor.base(&device_id);
        let tx = self.mesh.thumb_tx.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("mesh-thumb".into()).spawn(move || {
            let img = crate::mesh::client::thumb(&device, base.as_deref(), chunk)
                .ok()
                .and_then(|jpeg| image::load_from_memory(&jpeg).ok())
                .map(|img| {
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
                });
            let _ = tx.send((id, img));
            ctx.request_repaint();
        });
    }

    /// 계산 빌려주기 (설계 7.4의 4, 8장): 이 기기가 추가해서 시작된 인덱싱은 늘 돕고, "놀 때 빌려주기"를 켜면
    /// 내 다른 기기가 인덱싱할 때마다 돕는다. 그 기기의 인덱싱이 끝나고 조용해지면 끝낸다.
    fn auto_lend(&mut self, ctx: &egui::Context) {
        let state = |app: &Self, id: &str| app.mesh.monitor.state(id);
        // 이 기기가 추가해 시작된 인덱싱이 끝났으면 그 표시를 지운다
        let flagged: HashSet<String> = self
            .cfg
            .remote_libraries
            .iter()
            .filter(|l| l.lend_while_indexing)
            .map(|l| l.device_id.clone())
            .collect();
        let finished: Vec<String> = flagged
            .iter()
            .filter(|id| owner_done(state(self, id).as_ref(), LEND_DONE_AFTER))
            .cloned()
            .collect();
        if !finished.is_empty() {
            for l in &mut self.cfg.remote_libraries {
                if finished.contains(&l.device_id) {
                    l.lend_while_indexing = false;
                }
            }
            self.save_mesh();
        }
        // 자동으로 시작한 빌려주기: 그 기기의 인덱싱이 끝났으면 끝내고, 세션이 끝났으면(실패 포함) 정리한다
        if let Some(id) = self.mesh.lend_auto.clone() {
            if self.lend.is_none() {
                let failed = self.lend_last.as_ref().is_some_and(|s| s.lock().phase == crate::lend::Phase::Failed);
                if failed {
                    // 서명이 다르다 등: 그 기기에는 다시 시도하지 않는다
                    self.mesh.lend_auto_skip.insert(id);
                }
                self.mesh.lend_auto = None;
            } else if owner_done(state(self, &id).as_ref(), LEND_DONE_AFTER) {
                // 받은 일감을 마저 처리하고 끝난다. 끝날 때까지는 새로 시작하지 않는다(`self.lend`가 남아 있다).
                self.stop_lending();
                self.mesh.lend_auto = None;
                let name = self.cfg.mesh_devices.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_default();
                self.notify(format!("{name}의 인덱싱이 끝나 계산 빌려주기를 마칩니다"), false);
            }
            return;
        }
        if self.lend.is_some() {
            // 사용자가 직접 빌려주는 중이다
            return;
        }
        // 시작: 도울 기기 가운데 인덱싱 중인 기기가 있으면
        let idle_lending = self.cfg.lend_mesh_idle;
        let target = self.cfg.mesh_devices.iter().find(|d| {
            (idle_lending || flagged.contains(&d.id))
                && !self.mesh.lend_auto_skip.contains(&d.id)
                && owner_needs_help(state(self, &d.id).as_ref())
        });
        let Some(d) = target.cloned() else { return };
        let ready = crate::server::resolve_binary(&self.cfg.llama_path).is_ok()
            && std::path::Path::new(self.cfg.model_path.trim()).is_file();
        if !ready {
            self.mesh.lend_auto_skip.insert(d.id.clone());
            self.notify(
                format!("이 PC에 llama.cpp나 모델이 없어 {}의 인덱싱에 계산을 빌려주지 않습니다 (검색은 됩니다)", d.name),
                false,
            );
            return;
        }
        let base = self.mesh.monitor.base(&d.id).unwrap_or_else(|| d.bases(None).first().cloned().unwrap_or_default());
        let token = Some(d.token.clone()).filter(|t| !t.trim().is_empty());
        match self.start_lending_to(ctx, base, token) {
            Ok(()) => {
                self.mesh.lend_auto = Some(d.id.clone());
                self.notify(format!("{}에서 인덱싱하는 동안 이 PC의 그래픽으로 계산을 돕습니다", d.name), false);
            }
            Err(e) => {
                self.mesh.lend_auto_skip.insert(d.id.clone());
                self.notify(format!("계산 빌려주기를 시작하지 못했습니다: {e}"), true);
            }
        }
    }

    /// 설정 > 일반 > 검색할 폴더 안: 원격 라이브러리 줄들과 붙이는 중·참여 코드·실패 안내
    pub(crate) fn remote_library_rows(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        for (i, lib) in self.cfg.remote_libraries.iter().enumerate() {
            let device = self.cfg.mesh_devices.iter().find(|d| d.id == lib.device_id);
            let st = self.mesh.monitor.state(&lib.device_id);
            let (tone, status) = library_status(lib, device, st.as_ref());
            ui.horizontal(|ui| {
                ui.colored_label(tone.fg(ui.visuals()), tone.icon());
                ui.label(&lib.local_folder).on_hover_text(format!(
                    "원격 라이브러리: {}의 {}\n그 기기가 인덱싱하고 이 PC는 검색만 합니다.",
                    device.map(|d| d.name.as_str()).unwrap_or("?"),
                    lib.owner_folder
                ));
                widgets::tone_label(ui, tone, status);
                if widgets::button(ui, Btn::Ghost, "빼기")
                    .on_hover_text("이 PC의 검색 대상에서만 뺍니다. 그 기기의 라이브러리와 인덱스는 그대로 남습니다.")
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.cfg.remote_libraries.remove(i);
            self.save_mesh();
        }
        if let Some(p) = self.mesh.adding() {
            ui.horizontal(|ui| {
                widgets::spinner(ui);
                ui.label(format!("{p}: 그 폴더가 있는 기기를 찾는 중…"));
            });
        }
        self.pending_join_ui(ui);
        let mut local = None;
        let mut close = false;
        if let Some((path, why)) = &self.mesh.add_failed {
            ui.add_space(space::XS);
            widgets::callout_ui(ui, Tone::Warn, "원격 라이브러리로 붙이지 못했습니다", |ui| {
                ui.label(format!("{path}\n{why}"));
                ui.horizontal(|ui| {
                    if widgets::button(ui, Btn::Secondary, "이 PC에서 직접 인덱싱")
                        .on_hover_text("이 PC가 네트워크로 파일을 읽어 인덱싱합니다 (다른 기기에서는 검색되지 않습니다)")
                        .clicked()
                    {
                        local = Some(path.clone());
                    }
                    if widgets::button(ui, Btn::Ghost, "닫기").clicked() {
                        close = true;
                    }
                });
            });
        }
        if let Some(p) = local {
            self.add_local_root(p);
            close = true;
        }
        if close {
            self.mesh.add_failed = None;
        }
    }

    /// 모르는 서버의 폴더를 추가했을 때: 그 서버의 Gemma Search 참여 코드를 받아 참여하고 붙인다
    fn pending_join_ui(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.mesh.pending.as_mut() else { return };
        let (mut submit, mut local, mut cancel) = (false, false, false);
        let joining = p.rx.is_some();
        let (tone, title, body) = match &p.hello {
            Some(h) => (
                Tone::Info,
                format!("{}에서 Gemma Search가 돌고 있습니다", h.name),
                format!(
                    "그 기기가 이 폴더를 인덱싱하고 이 PC는 검색만 하게 하려면(인덱싱 동안은 이 PC도 계산을 돕습니다) \
                     그 기기의 참여 코드를 넣으세요. 그 기기에서 `gemma-search join-code`로 봅니다.{}",
                    if h.join { "" } else { "\n그 기기는 지금 다른 기기의 참여를 받지 않게 열려 있습니다(--bind 0.0.0.0:8787로 켜야 합니다)." }
                ),
            ),
            None => (
                Tone::Warn,
                format!("{}에서 Gemma Search를 찾지 못했습니다", p.server),
                "없거나, 다른 기기가 닿게 열려 있지 않습니다(그 기기에서 `gemma-search serve --bind 0.0.0.0:8787`). \
                 다른 포트로 띄웠으면 그 기기의 참여 코드를 넣으세요. 아니면 이 PC가 직접 인덱싱합니다."
                    .to_string(),
            ),
        };
        ui.add_space(space::XS);
        widgets::callout_ui(ui, tone, &title, |ui| {
            ui.label(RichText::new(&p.path).weak());
            ui.label(body);
            ui.horizontal(|ui| {
                ui.add_enabled(
                    !joining,
                    egui::TextEdit::singleline(&mut p.code).hint_text("참여 코드 (GSJ1-…)").desired_width(340.0),
                );
                if joining {
                    widgets::spinner(ui);
                    ui.label("참여하고 붙이는 중…");
                } else if widgets::button_enabled(ui, Btn::Primary, !p.code.trim().is_empty(), "참여하고 추가").clicked() {
                    submit = true;
                }
            });
            if let Some(e) = &p.error {
                widgets::tone_label(ui, Tone::Danger, e);
            }
            ui.horizontal(|ui| {
                if widgets::button_enabled(ui, Btn::Secondary, !joining, "이 PC에서 직접 인덱싱")
                    .on_hover_text("이 PC가 네트워크로 파일을 읽어 인덱싱합니다 (다른 기기에서는 검색되지 않습니다)")
                    .clicked()
                {
                    local = true;
                }
                if widgets::button_enabled(ui, Btn::Ghost, !joining, "취소").clicked() {
                    cancel = true;
                }
            });
        });
        if submit {
            match JoinCode::decode(&p.code) {
                Err(e) => p.error = Some(e),
                Ok(code) if p.hello.as_ref().is_some_and(|h| !h.id.is_empty() && h.id != code.id) => {
                    p.error = Some(format!(
                        "이 참여 코드는 {}의 것이 아닙니다 (그 기기에서 gemma-search join-code로 확인하세요)",
                        p.hello.as_ref().map(|h| h.name.as_str()).unwrap_or("그 기기")
                    ));
                }
                Ok(code) => {
                    p.error = None;
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    let path = p.path.clone();
                    let _ = std::thread::Builder::new().name("mesh-join-add".into()).spawn(move || {
                        let _ = tx.send(crate::mesh::client::join_and_add(&code, &path));
                    });
                    p.rx = Some(rx);
                }
            }
        }
        if local {
            if let Some(p) = self.mesh.pending.take() {
                self.add_local_root(p.path);
            }
        } else if cancel {
            self.mesh.pending = None;
        }
    }

    /// 설정 > 일반 > 내 다른 기기
    pub(crate) fn basic_mesh(&mut self, ui: &mut egui::Ui) {
        let mut remove = None;
        let mut join = false;
        let mut idle_changed = false;
        widgets::section(
            ui,
            "내 다른 기기",
            Some(
                "홈서버처럼 Gemma Search 서비스(gemma-search serve)가 도는 내 기기를 넣어 두면, 그 기기의 공유 폴더를 \
                 검색할 폴더로 추가할 때 그 기기가 인덱싱하고 이 PC에서 검색합니다. 처음 보는 기기의 폴더를 추가하면 \
                 그 자리에서 참여 코드를 물어봅니다.",
            ),
            |ui| {
                for (i, d) in self.cfg.mesh_devices.iter().enumerate() {
                    let st = self.mesh.monitor.state(&d.id);
                    let (tone, text) = match st.as_ref().and_then(|s| s.reachable.map(|r| (r, s))) {
                        None => (Tone::Muted, "확인하는 중".to_string()),
                        Some((false, s)) => (Tone::Warn, if s.error.is_empty() { "연결 안 됨".to_string() } else { s.error.clone() }),
                        Some((true, s)) => {
                            let mut t = format!("연결됨 · v{} · 파일 {}개", s.version, fmt_num(s.files));
                            if s.indexing.busy() {
                                t += " · 인덱싱 중";
                            }
                            (Tone::Ok, t)
                        }
                    };
                    let libs = self.cfg.remote_libraries.iter().filter(|l| l.device_id == d.id).count();
                    let lending = self.mesh.lend_auto.as_deref() == Some(d.id.as_str()) && self.lend.is_some();
                    ui.horizontal(|ui| {
                        ui.colored_label(tone.fg(ui.visuals()), tone.icon());
                        ui.label(RichText::new(&d.name).strong()).on_hover_text(format!(
                            "주소: {}\n포트 {}",
                            d.addrs.join(", "),
                            d.port
                        ));
                        widgets::tone_label(ui, tone, text);
                        if lending {
                            widgets::tone_label(ui, Tone::Info, "· 이 PC가 계산을 돕는 중");
                        }
                        if libs > 0 {
                            widgets::caption(ui, &format!("원격 라이브러리 {libs}개"));
                        }
                        if widgets::button(ui, Btn::Ghost, "빼기")
                            .on_hover_text("이 기기와 그 기기의 원격 라이브러리를 이 PC에서 뺍니다 (그 기기에는 그대로 남습니다)")
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                }
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    let joining = self.mesh.join_rx.is_some();
                    ui.add_enabled(
                        !joining,
                        egui::TextEdit::singleline(&mut self.mesh.join_input)
                            .hint_text("참여 코드 (GSJ1-…): 그 기기에서 gemma-search join-code")
                            .desired_width(360.0),
                    );
                    if joining {
                        widgets::spinner(ui);
                    } else if widgets::button_enabled(ui, Btn::Secondary, !self.mesh.join_input.trim().is_empty(), "넣기").clicked() {
                        join = true;
                    }
                });
                if let Some(e) = &self.mesh.join_error {
                    widgets::tone_label(ui, Tone::Danger, e);
                }
                ui.add_space(space::XS);
                idle_changed = widgets::toggle_row(
                    ui,
                    &mut self.cfg.lend_mesh_idle,
                    "놀 때 계산 빌려주기",
                    "내 다른 기기가 인덱싱하면, 이 PC가 쉬고 있는 동안 그래픽으로 임베딩을 돕습니다. 전체 화면 앱을 쓰거나 \
                     그래픽 메모리가 모자라면(다른 LLM 서버 등) 돕지 않습니다. 꺼 두어도 이 PC가 추가해 시작된 인덱싱은 돕습니다.",
                )
                .changed();
                widgets::help(
                    ui,
                    "인증은 그 기기의 접속 토큰입니다(참여 코드에 들어 있습니다). 파일 내용과 검색이 오가므로 집 LAN이나 \
                     Tailscale 같은 VPN 안에서만 쓰세요.",
                );
            },
        );
        if idle_changed {
            self.save_mesh();
            self.mesh.monitor.poke(0);
            // 끄면 놀 때 빌려주기로 시작한 빌려주기를 끝낸다 (이 PC가 추가해 시작된 인덱싱은 계속 돕는다)
            if !self.cfg.lend_mesh_idle {
                if let Some(id) = self.mesh.lend_auto.clone() {
                    let flagged = self.cfg.remote_libraries.iter().any(|l| l.device_id == id && l.lend_while_indexing);
                    if !flagged && self.lend.is_some() {
                        self.stop_lending();
                    }
                }
            }
        }
        if let Some(i) = remove {
            let d = self.cfg.mesh_devices.remove(i);
            self.cfg.remote_libraries.retain(|l| l.device_id != d.id);
            self.save_mesh();
            if self.mesh.lend_auto.as_deref() == Some(d.id.as_str()) {
                self.stop_lending();
                self.mesh.lend_auto = None;
            }
        }
        if join {
            match JoinCode::decode(&self.mesh.join_input) {
                Err(e) => self.mesh.join_error = Some(e),
                Ok(code) => {
                    self.mesh.join_error = None;
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    let _ = std::thread::Builder::new().name("mesh-join".into()).spawn(move || {
                        let _ = tx.send(crate::mesh::client::join(&code));
                    });
                    self.mesh.join_rx = Some(rx);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::monitor::Indexing;
    use std::time::Instant;

    fn st(reachable: Option<bool>, busy: bool, idle_for: Option<u64>) -> DeviceState {
        DeviceState {
            reachable,
            indexing: Indexing { active: busy, ..Default::default() },
            idle_since: idle_for.map(|s| Instant::now() - Duration::from_secs(s)),
            ..Default::default()
        }
    }

    #[test]
    fn lending_and_search_follow_the_owner_state() {
        // 모르는 기기는 묻는다 (꺼진 것으로 알려진 기기만 검색에서 뺀다)
        assert!(!known_offline(None));
        assert!(!known_offline(Some(&st(None, false, None))));
        assert!(known_offline(Some(&st(Some(false), false, None))));
        assert!(!known_offline(Some(&st(Some(true), false, None))));
        // 닿고 인덱싱 중일 때만 돕는다
        assert!(owner_needs_help(Some(&st(Some(true), true, None))));
        assert!(!owner_needs_help(Some(&st(Some(false), true, None))));
        assert!(!owner_needs_help(Some(&st(Some(true), false, Some(5)))));
        assert!(!owner_needs_help(None));
        // 끝나고 1분 넘게 조용해야 끝낸다 (인덱싱 사이의 짧은 틈에 끊지 않게)
        let after = Duration::from_secs(60);
        assert!(owner_done(Some(&st(Some(true), false, Some(90))), after));
        assert!(!owner_done(Some(&st(Some(true), false, Some(10))), after));
        assert!(!owner_done(Some(&st(Some(true), true, None)), after));
        assert!(!owner_done(Some(&st(Some(false), false, Some(90))), after), "닿지 않으면 끝났는지 모른다");
        assert_eq!(crate::sysinfo::lend_vram_need_mb(8192), 3700);
        assert_eq!(crate::sysinfo::lend_vram_need_mb(2048), 1800);
    }
}
