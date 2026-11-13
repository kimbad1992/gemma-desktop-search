//! 접속 기기 쪽: 메시 기기(소유 기기)의 서비스에 묻는다 (참여, 라이브러리 붙이기, 검색, 상태, 썸네일)
//!
//! 기기마다 주소를 여러 개 안다(집 LAN, Tailscale, 컴퓨터 이름). 지난번에 닿은 주소부터 차례로 시도하고,
//! 닿지 않는 주소는 짧게(`CONNECT_TIMEOUT`) 포기하고 다음으로 넘어간다.

use super::{
    map_to_local, network_location, AttachReply, AttachRequest, DeviceInfo, JoinCode, LibraryInfo, MeshDevice, RemoteLibrary,
    SAMPLE_COUNT,
};
use crate::extract::{Kind, Modality};
use crate::search::keyword::{MARK_END, MARK_START};
use crate::search::query::{HitView, RemoteHit};
use crate::search::store::{Hit, Section};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// 주소 하나에 접속을 기다리는 시간
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 검색 한 번을 기다리는 시간. 소유 기기는 llama-server가 꺼져 있으면 켜서 준비될 때까지(최대 2분) 기다린다.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(150);

/// 소유 기기의 답이 아닌 실패
#[derive(Debug, Clone)]
pub enum CallError {
    /// 어느 주소로도 닿지 않음
    Unreachable(String),
    /// 서비스가 오류로 답함: (HTTP 상태, 까닭)
    Refused(u16, String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Unreachable(m) => write!(f, "연결 안 됨 ({m})"),
            CallError::Refused(401, _) => write!(f, "접속 토큰이 맞지 않습니다 (참여 코드를 다시 넣으세요)"),
            CallError::Refused(code, m) if m.is_empty() => write!(f, "HTTP {code}"),
            CallError::Refused(_, m) => write!(f, "{m}"),
        }
    }
}

/// 한 기기의 서비스에 묻는 것
pub struct Owner {
    bases: Vec<String>,
    token: String,
    agent: ureq::Agent,
}

impl Owner {
    /// `first`: 지난번에 닿은 주소 (먼저 시도한다)
    pub fn new(device: &MeshDevice, first: Option<&str>, timeout: Duration) -> Self {
        Self::with_bases(device.bases(first), &device.token, timeout)
    }

    pub fn with_bases(bases: Vec<String>, token: &str, timeout: Duration) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        Self { bases, token: token.trim().to_string(), agent }
    }

    /// 요청을 보내고 (닿은 주소, 응답)을 돌려준다. 닿지 않으면 다음 주소로.
    fn send(&self, method: &str, path: &str, body: Option<&Value>) -> Result<(String, ureq::http::Response<ureq::Body>), CallError> {
        let auth = format!("Bearer {}", self.token);
        let mut last = String::from("주소가 없습니다");
        for base in &self.bases {
            let url = format!("{base}{path}");
            let r = if method == "GET" {
                let mut req = self.agent.get(&url);
                if !self.token.is_empty() {
                    req = req.header("Authorization", &auth);
                }
                req.call()
            } else {
                let mut req = self.agent.post(&url);
                if !self.token.is_empty() {
                    req = req.header("Authorization", &auth);
                }
                req.send_json(body.cloned().unwrap_or(Value::Null))
            };
            match r {
                Ok(resp) => return Ok((base.clone(), resp)),
                Err(e) => last = format!("{base}: {e}"),
            }
        }
        Err(CallError::Unreachable(last))
    }

    /// JSON으로 묻는다: (닿은 주소, 답)
    pub fn json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<(String, Value), CallError> {
        let (base, mut r) = self.send(method, path, body)?;
        let status = r.status().as_u16();
        let v: Value = r
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_json()
            .map_err(|e| CallError::Refused(status, format!("응답을 읽지 못했습니다: {e}")))?;
        if status != 200 {
            return Err(CallError::Refused(status, v["error"].as_str().unwrap_or("").to_string()));
        }
        Ok((base, v))
    }

    /// 바이트로 받는다 (썸네일)
    pub fn bytes(&self, path: &str) -> Result<Vec<u8>, CallError> {
        let (_, mut r) = self.send("GET", path, None)?;
        let status = r.status().as_u16();
        if status != 200 {
            return Err(CallError::Refused(status, String::new()));
        }
        r.body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| CallError::Refused(status, e.to_string()))
    }
}

/// 참여 코드로 그 기기에 닿아 보고, 닿으면 메시 기기로 돌려준다 (닿은 주소를 맨 앞에)
pub fn join(code: &JoinCode) -> Result<MeshDevice, String> {
    let device = MeshDevice {
        id: code.id.clone(),
        name: code.name.clone(),
        port: code.port,
        addrs: code.addrs.clone(),
        token: code.token.clone(),
    };
    let owner = Owner::new(&device, None, Duration::from_secs(8));
    let (base, v) = owner.json("GET", "/api/mesh/info", None).map_err(|e| match e {
        CallError::Unreachable(m) => format!(
            "{}에 닿지 않습니다 ({m}). 그 기기에서 서비스가 다른 기기도 받게 열려 있는지(--bind 0.0.0.0:{}), \
             이 PC와 같은 LAN이나 Tailscale에 있는지 확인하세요",
            code.name, code.port
        ),
        CallError::Refused(404, _) => format!("{}의 Gemma Search가 원격 라이브러리를 모르는 예전 버전입니다. 업데이트하세요", code.name),
        e => format!("{}: {e}", code.name),
    })?;
    let info: DeviceInfo = serde_json::from_value(v).map_err(|e| format!("기기 정보를 읽지 못했습니다: {e}"))?;
    if !info.id.is_empty() && info.id != code.id {
        return Err(format!("{}의 기기 ID가 참여 코드와 다릅니다. 그 기기에서 참여 코드를 다시 확인하세요", code.name));
    }
    // 닿은 주소를 앞에, 그 기기가 지금 알리는 주소도 더한다
    let reached = base.split_once("://").map(|(_, r)| r).unwrap_or(&base);
    let reached_host = reached.rsplit_once(':').map(|(h, _)| h.trim_matches(['[', ']'])).unwrap_or(reached).to_string();
    let mut addrs = vec![reached_host];
    for a in code.addrs.iter().chain(&info.addrs) {
        if !addrs.iter().any(|x| x.eq_ignore_ascii_case(a)) {
            addrs.push(a.clone());
        }
    }
    Ok(MeshDevice {
        id: code.id.clone(),
        name: if info.name.is_empty() { code.name.clone() } else { info.name },
        port: code.port,
        addrs,
        token: code.token.clone(),
    })
}

/// 서비스판의 기본 포트 (`serve --bind 0.0.0.0:8787`). 모르는 서버에 Gemma Search가 있는지 이 포트로 물어본다.
pub const DEFAULT_PORT: u16 = 8787;

/// 그 주소에서 도는 Gemma Search 서비스 (`GET /api/hello`, 토큰 없이 답한다)
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct Hello {
    pub app: String,
    pub version: String,
    /// 기기 ID (참여 코드의 ID와 같아야 한다)
    pub id: String,
    pub name: String,
    /// 다른 기기의 참여를 받는지 (접속 토큰이 있고 다른 기기가 닿게 열려 있다)
    pub join: bool,
    /// 답한 주소
    #[serde(skip)]
    pub base: String,
}

/// `/api/hello`의 답이 Gemma Search인지 보고 읽는다
pub fn hello_from(v: Value, base: &str) -> Option<Hello> {
    let mut h: Hello = serde_json::from_value(v).ok()?;
    (h.app == "gemma-search").then(|| {
        h.base = base.to_string();
        h
    })
}

/// 서버(`leeubuntu`, `100.90.136.114`)의 기본 포트에 Gemma Search가 있는지 묻는다 (닿지 않거나 아니면 None)
pub fn probe(host: &str) -> Option<Hello> {
    let base = super::base_of(host, DEFAULT_PORT);
    let owner = Owner::with_bases(vec![base.clone()], "", Duration::from_secs(5));
    let (_, v) = owner.json("GET", "/api/hello", None).ok()?;
    hello_from(v, &base)
}

/// 네트워크 폴더를 추가한 결과
#[derive(Debug, Clone)]
pub enum AddOutcome {
    /// 네트워크 공유 폴더가 아니다: 지금처럼 이 기기가 직접 인덱싱한다
    NotMesh(String),
    /// 메시에 넣지 않은 서버인데 Gemma Search가 돈다: 그 기기의 참여 코드를 넣으면 그 기기가 인덱싱한다
    Detected { server: String, hello: Hello },
    /// 메시에 넣지 않은 서버이고 Gemma Search도 찾지 못했다 (없거나, 다른 기기가 닿게 열려 있지 않거나, 다른 포트)
    NoService { server: String },
    /// 소유 기기의 라이브러리로 붙였다
    Attached { device: MeshDevice, library: RemoteLibrary, reply: AttachReply },
}

/// 네트워크 폴더(`\\서버\공유\…`, 드라이브 문자, cifs 마운트)를 추가한다 (설계 7.4의 1~3).
/// 소유 기기를 메시 기기 목록에서 찾으면 라이브러리로 붙인다. 목록에 없으면 그 서버에 Gemma Search가 있는지
/// 물어보고(`probe`) `Detected`/`NoService`로 알린다 (참여 코드를 받으면 `join_and_add`).
pub fn add_network_folder(path: &str, devices: &[MeshDevice]) -> Result<AddOutcome, String> {
    let Some(unc) = network_location(path) else {
        return Ok(AddOutcome::NotMesh("네트워크 공유 폴더가 아닙니다".to_string()));
    };
    if !Path::new(path).is_dir() {
        return Err(format!("이 PC에서 그 폴더를 열 수 없습니다: {path}"));
    }
    let Some(i) = super::find_owner(&unc.server, devices) else {
        return Ok(match probe(&unc.server) {
            Some(hello) => AddOutcome::Detected { server: unc.server.clone(), hello },
            None => AddOutcome::NoService { server: unc.server.clone() },
        });
    };
    let device = devices[i].clone();
    let (library, reply) = attach_to(&device, path, &unc)?;
    Ok(AddOutcome::Attached { device, library, reply })
}

/// 참여 코드로 그 기기를 메시에 넣고 바로 이 폴더를 그 기기의 라이브러리로 붙인다.
/// 그 기기가 정말 이 경로의 서버인지는 소유 기기가 공유 이름과 표본으로 확인한다. 붙였으면 경로의 서버 이름을
/// 그 기기의 주소에 더해 다음부터는 바로 알아본다.
pub fn join_and_add(code: &JoinCode, path: &str) -> Result<(MeshDevice, RemoteLibrary, AttachReply), String> {
    let unc = network_location(path).ok_or_else(|| "네트워크 공유 폴더가 아닙니다".to_string())?;
    if !Path::new(path).is_dir() {
        return Err(format!("이 PC에서 그 폴더를 열 수 없습니다: {path}"));
    }
    let mut device = join(code)?;
    let (library, reply) = attach_to(&device, path, &unc)
        .map_err(|e| format!("{e}\n(참여 코드가 이 폴더가 있는 기기의 것인지 확인하세요)"))?;
    if !super::host_matches(&unc.server, &device, &[]) {
        device.addrs.push(unc.server.clone());
    }
    Ok((device, library, reply))
}

/// 그 기기에 공유 이름·아래 경로·표본을 보내 라이브러리로 붙인다
fn attach_to(device: &MeshDevice, path: &str, unc: &super::UncPath) -> Result<(RemoteLibrary, AttachReply), String> {
    let samples = super::collect_samples(Path::new(path), SAMPLE_COUNT, Duration::from_secs(8));
    let req = AttachRequest {
        share: unc.share.clone(),
        sub: unc.rest.clone(),
        samples,
        from: crate::lend::machine_name(),
    };
    let body = serde_json::to_value(&req).map_err(|e| e.to_string())?;
    let owner = Owner::new(device, None, Duration::from_secs(60));
    let (_, v) = owner.json("POST", "/api/libraries/attach", Some(&body)).map_err(|e| match e {
        CallError::Refused(404, m) if m.is_empty() => {
            format!("{}의 Gemma Search가 원격 라이브러리를 모르는 예전 버전입니다. 업데이트하세요", device.name)
        }
        e => format!("{}: {e}", device.name),
    })?;
    let reply: AttachReply = serde_json::from_value(v).map_err(|e| format!("{}의 답을 읽지 못했습니다: {e}", device.name))?;
    // 결과를 열 때 쓸 이 기기의 경로: Windows는 UNC로(드라이브 문자 연결이 바뀌어도 열린다), Linux는 마운트 경로 그대로
    let local_folder = if cfg!(windows) { unc.to_unc() } else { path.trim().trim_end_matches('/').to_string() };
    let library = RemoteLibrary {
        device_id: device.id.clone(),
        library_id: reply.library.id.clone(),
        library_root: reply.library.path.clone(),
        owner_folder: reply.folder.clone(),
        local_folder,
        enabled: true,
        lend_while_indexing: reply.indexing,
    };
    Ok((library, reply))
}

/// 라이브러리 목록 (`GET /api/libraries`)
pub fn libraries(owner: &Owner) -> Result<Vec<LibraryInfo>, CallError> {
    let (_, v) = owner.json("GET", "/api/libraries", None)?;
    Ok(serde_json::from_value(v["libraries"].clone()).unwrap_or_default())
}

// ── 검색 ──

/// 한 기기에 묻는 검색
#[derive(Clone)]
pub struct RemoteQuery {
    /// 메시 기기 목록에서의 자리 (결과 번호가 겹치지 않게 쓴다)
    pub index: usize,
    pub device: MeshDevice,
    /// 지난번에 닿은 주소
    pub base: Option<String>,
    /// 그 기기에서 검색할 원격 라이브러리들
    pub libraries: Vec<RemoteLibrary>,
}

/// 한 기기의 검색 답
#[derive(Clone, Default)]
pub struct RemoteAnswer {
    pub device_name: String,
    pub hits: Vec<(Section, HitView)>,
    pub searched: usize,
    pub error: Option<String>,
    /// 닿지 않았다 (`error`가 "연결 안 됨")
    pub unreachable: bool,
}

/// 원격 결과의 번호: 이 기기 인덱스의 번호(양수)와 겹치지 않게 음수로, 기기마다 다르게
pub fn synthetic_id(index: usize, remote_id: i64) -> i64 {
    -((((index as i64) + 1) << 40) | (remote_id & ((1i64 << 40) - 1)))
}

fn modality_of(s: &str) -> Option<Modality> {
    Some(match s {
        "name" => Modality::Name,
        "text" => Modality::Text,
        "image" => Modality::Image,
        "audio" => Modality::Audio,
        "video" => Modality::Video,
        _ => return None,
    })
}

fn kind_of(s: &str) -> Kind {
    match s {
        "text" => Kind::Text,
        "doc" => Kind::Doc,
        "image" => Kind::Image,
        "video" => Kind::Video,
        "audio" => Kind::Audio,
        _ => Kind::Other,
    }
}

fn section_of(s: &str) -> Option<Section> {
    Some(match s {
        "docs" => Section::Docs,
        "images" => Section::Images,
        "videos" => Section::Videos,
        "audio" => Section::Audio,
        "names" => Section::Names,
        _ => return None,
    })
}

/// 서비스의 `highlight`([[글, 강조인지], …])를 강조 표식이 든 글로
fn highlight_text(v: &Value) -> Option<String> {
    let parts = v.as_array()?;
    let mut s = String::new();
    for p in parts {
        let text = p[0].as_str().unwrap_or("");
        if p[1].as_bool().unwrap_or(false) {
            s.push(MARK_START);
            s.push_str(text);
            s.push(MARK_END);
        } else {
            s.push_str(text);
        }
    }
    Some(s)
}

/// 검색 답(JSON)의 결과 하나를 이 기기의 결과로: 경로는 이 기기에서 여는 경로로 바꾼다
fn hit_from_json(q: &RemoteQuery, sec: Section, h: &Value) -> Option<HitView> {
    let owner_path = h["path"].as_str()?.to_string();
    let kind = kind_of(h["kind"].as_str().unwrap_or(""));
    let modality = h["modality"].as_str().and_then(modality_of).unwrap_or(match sec {
        Section::Names => Modality::Name,
        Section::Docs => Modality::Text,
        Section::Audio => Modality::Audio,
        Section::Images | Section::Videos => Modality::Image,
    });
    // 가장 깊게 맞는 라이브러리 폴더로 바꾼다
    let local = q
        .libraries
        .iter()
        .filter_map(|l| map_to_local(&owner_path, &l.owner_folder, &l.local_folder).map(|p| (l.owner_folder.len(), p)))
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)?;
    let file_id = h["file_id"].as_i64()?;
    let chunk_id = h["chunk_id"].as_i64().unwrap_or(0);
    let thumb = h["thumb"].as_i64();
    let f = |k: &str| h[k].as_f64().unwrap_or(0.0) as f32;
    let hit = Hit {
        file_id: synthetic_id(q.index, file_id),
        chunk_id: synthetic_id(q.index, chunk_id),
        score: f("score"),
        meaning: f("meaning"),
        cosine: f("cosine"),
        // 예전 서비스는 일치 비율을 주지 않는다: 단어로 걸린 결과("keyword")면 보이게
        coverage: h["coverage"]
            .as_f64()
            .map(|c| c as f32)
            .unwrap_or(if h["keyword"].as_bool().unwrap_or(false) { 1.0 } else { 0.0 }),
        modality,
        locator: h["loc"].as_i64().unwrap_or(0),
        path: local,
        kind,
    };
    Some(HitView {
        file_name: h["name"].as_str().map(str::to_string).unwrap_or_else(|| super::last_name(&owner_path).unwrap_or_default()),
        snippet: h["snippet"].as_str().unwrap_or("").to_string(),
        highlight: highlight_text(&h["highlight"]),
        thumb_chunk: thumb.map(|t| synthetic_id(q.index, t)),
        hit,
        remote: Some(Arc::new(RemoteHit {
            device_id: q.device.id.clone(),
            device_name: q.device.name.clone(),
            thumb,
            owner_path,
        })),
    })
}

/// 서비스의 검색 답을 결과들로 (단위 시험용으로 따로 둔다)
pub fn hits_from_answer(q: &RemoteQuery, v: &Value) -> Vec<(Section, HitView)> {
    let mut out = Vec::new();
    for s in v["sections"].as_array().into_iter().flatten() {
        let Some(sec) = s["key"].as_str().and_then(section_of) else { continue };
        for h in s["hits"].as_array().into_iter().flatten() {
            if let Some(hv) = hit_from_json(q, sec, h) {
                out.push((sec, hv));
            }
        }
    }
    out
}

/// 한 기기에 검색을 묻는다. `sections`: 찾을 종류, `path`: 경로 글자 거르기.
/// 관련도로 거르지 않고 받아 둔다(화면의 관련도 막대는 다시 검색하지 않고 바로 적용한다).
pub fn search(q: &RemoteQuery, text: &str, sections: &[bool; 5], path: &str) -> RemoteAnswer {
    let mut answer = RemoteAnswer {
        device_name: q.device.name.clone(),
        ..Default::default()
    };
    let enc = crate::service::url_encode;
    let mut url = format!("/api/search?q={}&min=-100", enc(text));
    if sections.iter().any(|on| !on) {
        let keys = ["docs", "images", "videos", "audio", "names"];
        let on: Vec<&str> = keys.iter().zip(sections).filter(|(_, on)| **on).map(|(k, _)| *k).collect();
        url += &format!("&kinds={}", enc(&on.join(",")));
    }
    if !path.trim().is_empty() {
        url += &format!("&path={}", enc(path.trim()));
    }
    for l in &q.libraries {
        url += &format!("&under={}", enc(&l.owner_folder));
    }
    let owner = Owner::new(&q.device, q.base.as_deref(), SEARCH_TIMEOUT);
    match owner.json("GET", &url, None) {
        Ok((_, v)) => {
            answer.searched = v["searched"].as_u64().unwrap_or(0) as usize;
            answer.hits = hits_from_answer(q, &v);
        }
        Err(e) => {
            answer.unreachable = matches!(e, CallError::Unreachable(_));
            answer.error = Some(if answer.unreachable { "연결 안 됨".to_string() } else { e.to_string() });
        }
    }
    answer
}

/// 원격 결과의 썸네일 (JPEG)
pub fn thumb(device: &MeshDevice, base: Option<&str>, chunk_id: i64) -> Result<Vec<u8>, CallError> {
    Owner::new(device, base, Duration::from_secs(20)).bytes(&format!("/api/thumb/{chunk_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remote_answers_become_local_results() {
        let q = RemoteQuery {
            index: 0,
            device: MeshDevice { id: "d1".into(), name: "leeubuntu".into(), port: 8787, addrs: vec![], token: String::new() },
            base: None,
            libraries: vec![RemoteLibrary {
                device_id: "d1".into(),
                owner_folder: "/mnt/storage/공유/사진".into(),
                local_folder: r"\\leeubuntu\공유\사진".into(),
                ..Default::default()
            }],
        };
        let v = json!({
            "searched": 1234,
            "sections": [
                { "key": "images", "hits": [
                    { "file_id": 7, "chunk_id": 70, "path": "/mnt/storage/공유/사진/2024/바다.jpg", "name": "바다.jpg",
                      "kind": "image", "modality": "image", "score": 8.4, "meaning": 8.4, "cosine": 0.61, "coverage": 0.0,
                      "loc": 0, "thumb": 70, "snippet": "", "highlight": null },
                    // 이 기기가 추가한 폴더 밖의 결과는 버린다
                    { "file_id": 8, "chunk_id": 80, "path": "/mnt/other/x.jpg", "kind": "image", "score": 9.0 }
                ]},
                { "key": "docs", "hits": [
                    { "file_id": 9, "chunk_id": 90, "path": "/mnt/storage/공유/사진/메모.txt", "kind": "text", "modality": "text",
                      "score": 6.1, "keyword": true, "loc": 2, "highlight": [["바다", true], [" 사진", false]] }
                ]}
            ]
        });
        let hits = hits_from_answer(&q, &v);
        assert_eq!(hits.len(), 2);
        let (sec, img) = &hits[0];
        assert_eq!(*sec, Section::Images);
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(img.hit.path, format!(r"\\leeubuntu\공유\사진{sep}2024{sep}바다.jpg"));
        assert!(img.hit.file_id < 0 && img.thumb_chunk.is_some_and(|t| t < 0));
        let r = img.remote.as_ref().unwrap();
        assert_eq!((r.thumb, r.device_name.as_str(), r.owner_path.as_str()), (Some(70), "leeubuntu", "/mnt/storage/공유/사진/2024/바다.jpg"));
        let (_, doc) = &hits[1];
        assert_eq!(doc.hit.locator, 2);
        assert!(doc.keyword_match(), "예전 서비스의 keyword 표시로 단어 일치를 안다");
        assert_eq!(doc.highlight.as_deref(), Some("\u{1}바다\u{2} 사진"));
        // 서비스가 토큰 없이 알리는 정보: Gemma Search일 때만 받는다
        let h = hello_from(json!({"app": "gemma-search", "version": "0.9.1", "id": "abc", "name": "leeubuntu", "join": true}), "http://x:8787").unwrap();
        assert_eq!((h.name.as_str(), h.join, h.base.as_str()), ("leeubuntu", true, "http://x:8787"));
        assert!(hello_from(json!({"app": "something-else"}), "b").is_none());
        assert!(hello_from(json!("not an object"), "b").is_none());
        // 기기마다 번호가 겹치지 않는다
        assert_ne!(synthetic_id(0, 7), synthetic_id(1, 7));
        assert!(synthetic_id(3, i64::MAX) < 0);
    }
}
