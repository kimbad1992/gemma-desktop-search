//! 메시: 원격 라이브러리 (메시 3단계, `docs/MESH-DESIGN.md` 7장)
//!
//! 접속 기기(노트북 앱)에서 네트워크 폴더(`\\leeubuntu\공유\사진`, 그 공유에 연결한 드라이브 문자)를 검색 폴더로
//! 추가하면 이렇게 된다 (설계 7.4).
//! 1. 경로의 서버 이름·주소를 메시 기기(참여 코드로 넣은 기기)들이 알린 이름·주소와 대조해 소유 기기를 찾는다
//!    (`network_location`, `find_owner`). 메시에 없는 서버면 지금처럼 이 기기가 직접 인덱싱한다.
//! 2. 공유 이름, 그 아래 경로, 표본 파일 몇 개(상대경로·크기·수정 시각)를 소유 기기에 보낸다. 소유 기기는 공유
//!    설정으로 공유 이름을 자기 폴더로 바꾸고(`share.rs`), 표본이 같은 크기·시각으로 있는지 본다(`verify_samples`).
//! 3. 그 폴더가 이미 라이브러리(소유 기기의 검색 폴더)이거나 그 안이면 그 라이브러리를 쓰고, 아니면 라이브러리를
//!    만들어 인덱싱을 시작한다(`plan_attach`). 판정을 소유 기기 쪽 경로로 하므로, 다른 기기가 같은 폴더를 어떤
//!    주소로 추가해도 같은 라이브러리가 된다.
//! 4. 추가한 기기는 그 인덱싱 동안 계산을 빌려준다(`RemoteLibrary::lend_while_indexing`). 검색은 소유 기기의
//!    API로 하고, 결과의 소유 기기 경로를 추가한 경로로 바꿔 바로 연다(`map_to_local`).
//!
//! 보안(첫 조각): 메시 기기 사이의 인증은 서비스판의 접속 토큰이고, 길은 집 LAN이나 Tailscale 같은 VPN 안으로
//! 한정한다. 기기 인증서와 TLS(설계 6.3)는 다음 조각이다. 참여 코드에는 토큰이 들어 있으므로 비밀번호처럼 다룬다.

pub mod cli;
pub mod client;
pub mod monitor;
pub mod share;

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::Path;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// 참여 코드의 머리말 (형식이 바뀌면 숫자를 올린다)
pub const JOIN_PREFIX: &str = "GSJ1-";
/// 접속 기기가 보내는 표본 파일 수
pub const SAMPLE_COUNT: usize = 8;
/// 수정 시각 비교의 여유 (초). 파일 시스템마다 시각을 저장하는 정밀도가 다르다 (FAT은 2초).
const MTIME_SLACK: i64 = 2;

/// 메시 기기: 참여 코드로 넣은 다른 기기의 서비스(`gemma-search serve`). 설정 파일에 저장한다.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MeshDevice {
    /// 기기 ID (그 기기 설정의 `device_id`). 이름이나 주소가 바뀌어도 같은 기기로 알아본다.
    pub id: String,
    /// 컴퓨터 이름
    pub name: String,
    /// 서비스 포트
    pub port: u16,
    /// 닿을 수 있는 주소들 (IP, 컴퓨터 이름). 앞의 것부터 시도한다.
    pub addrs: Vec<String>,
    /// 그 서비스의 접속 토큰
    pub token: String,
}

impl MeshDevice {
    /// 시도할 서비스 주소들 (`http://주소:포트`). `first`(지난번에 닿은 주소)를 맨 앞에.
    pub fn bases(&self, first: Option<&str>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(f) = first.filter(|f| !f.is_empty()) {
            out.push(f.to_string());
        }
        for a in &self.addrs {
            let b = base_of(a, self.port);
            if !out.contains(&b) {
                out.push(b);
            }
        }
        out
    }
}

/// `100.90.136.114`, 8787 → `http://100.90.136.114:8787` (IPv6는 대괄호로 감싼다)
pub fn base_of(addr: &str, port: u16) -> String {
    let a = addr.trim().trim_matches(['[', ']']);
    if a.contains(':') {
        format!("http://[{a}]:{port}")
    } else {
        format!("http://{a}:{port}")
    }
}

/// 이 기기의 검색 대상에 넣은 원격 라이브러리 (설정 파일에 저장한다)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RemoteLibrary {
    /// 소유 기기의 ID (`MeshDevice::id`)
    pub device_id: String,
    /// 소유 기기가 알려 준 라이브러리 ID
    pub library_id: String,
    /// 소유 기기 쪽 라이브러리 폴더
    pub library_root: String,
    /// 이 기기가 추가한 폴더에 해당하는 소유 기기 쪽 폴더 (라이브러리 폴더이거나 그 아래). 검색을 이 폴더로 거른다.
    pub owner_folder: String,
    /// 이 기기에서 그 폴더를 가리키는 경로 (`\\leeubuntu\공유\사진`). 결과를 열 때 이 아래로 바꾼다.
    pub local_folder: String,
    /// 검색에 넣는지 (검색 화면의 폴더 고르기)
    pub enabled: bool,
    /// 이 기기가 추가해서 소유 기기가 인덱싱을 시작했다: 그 인덱싱이 끝날 때까지 계산을 빌려준다
    pub lend_while_indexing: bool,
}

impl Default for RemoteLibrary {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            library_id: String::new(),
            library_root: String::new(),
            owner_folder: String::new(),
            local_folder: String::new(),
            enabled: true,
            lend_while_indexing: false,
        }
    }
}

impl RemoteLibrary {
    /// 화면에 보일 이름: 추가한 폴더의 마지막 이름
    pub fn name(&self) -> String {
        last_name(&self.local_folder).unwrap_or_else(|| self.local_folder.clone())
    }
}

/// 경로의 마지막 이름 (`\\nas\공유\사진` → `사진`)
pub fn last_name(path: &str) -> Option<String> {
    split_parts(path).last().map(|s| s.to_string())
}

// ── 참여 코드 ──

/// 참여 코드: 소유 기기의 ID·이름·포트·주소들·접속 토큰을 한 줄로 묶은 것 (`GSJ1-…`)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JoinCode {
    pub id: String,
    pub name: String,
    pub port: u16,
    pub addrs: Vec<String>,
    pub token: String,
}

impl JoinCode {
    pub fn encode(&self) -> String {
        use base64::Engine;
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!("{JOIN_PREFIX}{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json))
    }

    /// 붙여 넣은 글에서 읽는다 (앞뒤 공백, 줄바꿈으로 갈라진 코드도 받는다)
    pub fn decode(text: &str) -> Result<Self, String> {
        use base64::Engine;
        let s: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let body = s
            .get(..JOIN_PREFIX.len())
            .filter(|p| p.eq_ignore_ascii_case(JOIN_PREFIX))
            .map(|_| &s[JOIN_PREFIX.len()..])
            .ok_or_else(|| format!("참여 코드는 {JOIN_PREFIX}로 시작합니다. 소유 기기에서 `gemma-search join-code`로 확인하세요."))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body.trim_end_matches('='))
            .map_err(|_| "참여 코드가 잘렸거나 틀렸습니다. 처음부터 끝까지 다시 복사해 넣으세요.".to_string())?;
        let code: JoinCode =
            serde_json::from_slice(&bytes).map_err(|_| "참여 코드를 읽지 못했습니다. 다시 복사해 넣으세요.".to_string())?;
        if code.id.is_empty() || code.port == 0 || code.addrs.is_empty() {
            return Err("참여 코드에 기기 ID나 주소가 없습니다".to_string());
        }
        Ok(code)
    }
}

/// 새 기기 ID (16자리 16진수). 표준 라이브러리의 해시 키가 OS 난수로 정해지는 것을 쓴다.
pub fn new_device_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:016x}", h.finish())
}

/// 설정에 기기 ID가 없으면 만든다. 만들었으면 true (저장해야 한다).
pub fn ensure_device_id(cfg: &mut crate::config::Config) -> bool {
    if cfg.device_id.trim().is_empty() {
        cfg.device_id = new_device_id();
        return true;
    }
    false
}

/// 이 기기가 다른 기기에 알릴 주소들: IP(루프백·링크 로컬 제외)와 컴퓨터 이름
pub fn local_addrs() -> Vec<String> {
    let mut ips = crate::platform::local_addresses();
    ips.retain(|ip| !ip.is_loopback() && !is_link_local(ip) && !ip.is_unspecified());
    // IPv4를 앞에 (사람이 읽기 쉽고, Tailscale·집 LAN 모두 IPv4로 닿는다)
    ips.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    ips.dedup();
    let mut out: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
    let name = crate::lend::machine_name();
    if !name.is_empty() && !out.iter().any(|a| a.eq_ignore_ascii_case(&name)) {
        out.push(name);
    }
    out
}

fn is_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

// ── 네트워크 경로 → 서버·공유 ──

/// `\\서버\공유\아래\경로`를 나눈 것
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncPath {
    pub server: String,
    pub share: String,
    pub rest: Vec<String>,
}

impl UncPath {
    /// `\\서버\공유\…`, `//서버/공유/…`, `\\?\UNC\서버\공유\…`
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s
            .strip_prefix(r"\\?\UNC\")
            .or_else(|| s.strip_prefix(r"\\"))
            .or_else(|| s.strip_prefix("//"))?;
        let mut it = s.split(['\\', '/']).filter(|p| !p.is_empty());
        let server = it.next()?.to_string();
        let share = it.next()?.to_string();
        // `\\?\C:\…`처럼 UNC가 아닌 긴 경로
        if server == "?" || server == "." {
            return None;
        }
        Some(Self { server, share, rest: it.map(str::to_string).collect() })
    }

    /// Windows 모양: `\\서버\공유\아래\경로`
    pub fn to_unc(&self) -> String {
        let mut s = format!(r"\\{}\{}", self.server, self.share);
        for p in &self.rest {
            s.push('\\');
            s.push_str(p);
        }
        s
    }
}

/// 이 기기의 경로가 네트워크 공유 위에 있으면 그 서버·공유·아래 경로.
/// Windows: UNC 경로, 네트워크 드라이브 문자(연결된 `\\서버\공유`로 바꿈). Linux: SMB(cifs) 마운트 아래.
pub fn network_location(path: &str) -> Option<UncPath> {
    let p = path.trim();
    if let Some(u) = UncPath::parse(p) {
        return Some(u);
    }
    #[cfg(windows)]
    {
        let b = p.as_bytes();
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            let remote = crate::platform::mapped_drive_target(b[0] as char)?;
            let mut u = UncPath::parse(&remote)?;
            u.rest.extend(split_parts(&p[2..]).map(str::to_string));
            return Some(u);
        }
        None
    }
    #[cfg(unix)]
    {
        let (source, mountpoint) = crate::platform::smb_mount_of(p)?;
        let mut u = UncPath::parse(&source)?;
        let below = p.strip_prefix(mountpoint.trim_end_matches('/')).unwrap_or("");
        u.rest.extend(split_parts(below).map(str::to_string));
        Some(u)
    }
}

/// 구분자(`\`, `/`)로 나눈 이름들 (빈 것은 뺀다)
pub fn split_parts(path: &str) -> impl Iterator<Item = &str> {
    path.split(['\\', '/']).filter(|s| !s.is_empty())
}

// ── 소유 기기 찾기 ──

/// 이름의 첫 마디, 소문자 (`leeubuntu.tail1234.ts.net` → `leeubuntu`)
fn host_label(name: &str) -> String {
    name.trim().split('.').next().unwrap_or("").to_lowercase()
}

/// 경로의 서버(`server`)가 이 메시 기기인지. `server_ips`는 서버 이름을 풀어 얻은 주소들(없으면 빈 것).
pub fn host_matches(server: &str, d: &MeshDevice, server_ips: &[IpAddr]) -> bool {
    let server = server.trim().trim_matches(['[', ']']);
    if server.is_empty() {
        return false;
    }
    let device_ips: Vec<IpAddr> = d.addrs.iter().filter_map(|a| a.trim_matches(['[', ']']).parse().ok()).collect();
    if let Ok(ip) = server.parse::<IpAddr>() {
        return device_ips.contains(&ip);
    }
    let label = host_label(server);
    let names = d.addrs.iter().filter(|a| a.parse::<IpAddr>().is_err()).chain(std::iter::once(&d.name));
    for n in names {
        if !n.trim().is_empty() && host_label(n) == label {
            return true;
        }
    }
    server_ips.iter().any(|ip| device_ips.contains(ip))
}

/// 서버 이름을 주소로 푼다 (`timeout` 안에 못 풀면 빈 것)
pub fn resolve_host(host: &str, timeout: Duration) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    let host = host.trim().trim_matches(['[', ']']).to_string();
    if let Ok(ip) = host.parse::<IpAddr>() {
        return vec![ip];
    }
    let (tx, rx) = crossbeam_channel::bounded(1);
    let _ = std::thread::Builder::new().name("resolve".into()).spawn(move || {
        let ips: Vec<IpAddr> = (host.as_str(), 445)
            .to_socket_addrs()
            .map(|it| it.map(|a| a.ip()).collect())
            .unwrap_or_default();
        let _ = tx.send(ips);
    });
    rx.recv_timeout(timeout).unwrap_or_default()
}

/// 경로의 서버가 어느 메시 기기인지: 이름·주소를 먼저 바로 견주고, 없으면 이름을 풀어 주소로 견준다
pub fn find_owner(server: &str, devices: &[MeshDevice]) -> Option<usize> {
    if let Some(i) = devices.iter().position(|d| host_matches(server, d, &[])) {
        return Some(i);
    }
    let ips = resolve_host(server, Duration::from_secs(3));
    if ips.is_empty() {
        return None;
    }
    devices.iter().position(|d| host_matches(server, d, &ips))
}

// ── 표본 파일 ──

/// 표본 파일: 폴더 안의 상대경로(`/`로 이음), 크기, 수정 시각(유닉스 초)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sample {
    pub rel: String,
    pub size: u64,
    pub mtime: i64,
}

fn mtime_secs(m: &std::fs::Metadata) -> Option<i64> {
    let t = m.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// 폴더 안에서 표본 파일을 `max`개까지 고른다 (얕은 곳부터, 이름 순서. 네트워크 폴더라 `budget` 안에서만 훑는다)
pub fn collect_samples(dir: &Path, max: usize, budget: Duration) -> Vec<Sample> {
    let t0 = Instant::now();
    let mut out = Vec::new();
    let walk = walkdir::WalkDir::new(dir)
        .max_depth(4)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
    for (seen, entry) in walk.enumerate() {
        if out.len() >= max || seen > 5000 || t0.elapsed() > budget {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Some(mtime) = mtime_secs(&meta) else { continue };
        let Ok(rel) = entry.path().strip_prefix(dir) else { continue };
        let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        out.push(Sample { rel: rel.join("/"), size: meta.len(), mtime });
    }
    out
}

/// 표본이 `folder` 아래에 같은 크기·수정 시각으로 있는지: (맞은 수, 본 수)
pub fn verify_samples(folder: &Path, samples: &[Sample]) -> (usize, usize) {
    let mut matched = 0;
    for s in samples {
        let parts: Vec<&str> = s.rel.split('/').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() || parts.iter().any(|p| *p == ".." || *p == "." || p.contains(['\\', ':'])) {
            continue;
        }
        let path = parts.iter().fold(folder.to_path_buf(), |p, part| p.join(part));
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if meta.is_file() && meta.len() == s.size && mtime_secs(&meta).is_some_and(|t| (t - s.mtime).abs() <= MTIME_SLACK) {
            matched += 1;
        }
    }
    (matched, samples.len())
}

/// 표본 대조를 통과했는지: 과반이 맞아야 한다 (그사이 바뀐 파일 하나쯤은 넘어간다). 빈 폴더는 표본이 없다.
pub fn samples_ok(matched: usize, total: usize) -> bool {
    total == 0 || matched * 2 > total
}

// ── 라이브러리와 경로 ──

/// 라이브러리 ID: 소유 기기 ID와 폴더로 정한다 (같은 폴더면 늘 같다)
pub fn library_id(device_id: &str, root: &str) -> String {
    use sha2::{Digest, Sha256};
    let key: Vec<String> = split_parts(root).map(|p| p.to_lowercase()).collect();
    let d = Sha256::digest(format!("{device_id}\n{}", key.join("/")).as_bytes());
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// `path`가 `root`이거나 그 아래면 그 안의 상대 이름들 (`root` 자체면 빈 것). 아니면 None.
/// 구분자는 `\`와 `/`를 같게 본다. `ignore_case`면 대소문자를 가리지 않는다 (Windows 경로).
pub fn within(path: &str, root: &str, ignore_case: bool) -> Option<Vec<String>> {
    let p: Vec<&str> = split_parts(path).collect();
    let r: Vec<&str> = split_parts(root).collect();
    if r.is_empty() || p.len() < r.len() {
        return None;
    }
    // 앞이 `/`인 경로와 아닌 경로는 다르다 (`/mnt/a`와 `mnt/a`)
    let rooted = |s: &str| s.trim_start().starts_with(['/', '\\']);
    if rooted(path) != rooted(root) {
        return None;
    }
    let same = |a: &str, b: &str| if ignore_case { a.to_lowercase() == b.to_lowercase() } else { a == b };
    if !r.iter().zip(&p).all(|(a, b)| same(a, b)) {
        return None;
    }
    Some(p[r.len()..].iter().map(|s| s.to_string()).collect())
}

/// 소유 기기의 경로를 이 기기의 경로로: `owner_folder` 아래의 상대경로를 `local_folder` 아래로 옮긴다
pub fn map_to_local(owner_path: &str, owner_folder: &str, local_folder: &str) -> Option<String> {
    let rel = within(owner_path, owner_folder, false).or_else(|| within(owner_path, owner_folder, true))?;
    let sep = std::path::MAIN_SEPARATOR;
    let mut s = local_folder.trim_end_matches(['\\', '/']).to_string();
    for p in rel {
        s.push(sep);
        s.push_str(&p);
    }
    Some(s)
}

/// 접속 기기가 고른 폴더를 소유 기기가 어떻게 받을지
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// 이미 라이브러리(검색 폴더)이거나 그 안: 그 라이브러리 폴더
    Existing(String),
    /// 새 라이브러리로 만들고 인덱싱한다
    New,
}

/// 받은 폴더(`folder`)와 지금 라이브러리 폴더들(`roots`)로 정한다. 여러 라이브러리 안이면 가장 깊은 것.
pub fn plan_attach(roots: &[String], folder: &str, ignore_case: bool) -> Plan {
    roots
        .iter()
        .filter_map(|r| within(folder, r, ignore_case).map(|_| (split_parts(r).count(), r)))
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, r)| Plan::Existing(r.clone()))
        .unwrap_or(Plan::New)
}

// ── 소유 기기 API가 주고받는 것 ──

/// `GET /api/mesh/info`: 기기 정보
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub port: u16,
    pub addrs: Vec<String>,
}

/// 라이브러리 하나 (`GET /api/libraries`, 붙이기 답)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryInfo {
    pub id: String,
    pub name: String,
    /// 소유 기기 쪽 폴더
    pub path: String,
    pub files: u64,
    /// 경로 대응용 표본 파일 (설계 7.3)
    pub samples: Vec<Sample>,
}

/// `POST /api/libraries/attach`: 접속 기기가 고른 네트워크 폴더
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AttachRequest {
    /// 공유 이름 (`\\서버\공유\…`의 `공유`)
    pub share: String,
    /// 공유 아래 경로의 이름들
    pub sub: Vec<String>,
    pub samples: Vec<Sample>,
    /// 보낸 기기의 이름 (로그용)
    pub from: String,
}

/// 붙이기 답
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AttachReply {
    pub library: LibraryInfo,
    /// 접속 기기가 고른 폴더의 소유 기기 쪽 경로 (라이브러리 폴더이거나 그 아래)
    pub folder: String,
    /// 이번에 새로 만든 라이브러리인지
    pub created: bool,
    /// 소유 기기가 인덱싱 중이거나 곧 시작한다
    pub indexing: bool,
    /// 표본 대조: 맞은 수, 본 수
    pub matched: usize,
    pub checked: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, addrs: &[&str]) -> MeshDevice {
        MeshDevice {
            id: "dev1".into(),
            name: name.into(),
            port: 8787,
            addrs: addrs.iter().map(|s| s.to_string()).collect(),
            token: "t".into(),
        }
    }

    #[test]
    fn join_code_round_trip() {
        let c = JoinCode {
            id: "0123456789abcdef".into(),
            name: "leeubuntu".into(),
            port: 8787,
            addrs: vec!["100.90.136.114".into(), "192.168.0.100".into(), "leeubuntu".into()],
            token: "secret".into(),
        };
        let s = c.encode();
        assert!(s.starts_with(JOIN_PREFIX));
        assert_eq!(JoinCode::decode(&s).unwrap(), c);
        // 줄바꿈으로 갈라지거나 앞뒤에 공백이 붙어도, 머리말이 소문자여도 읽는다
        let broken = format!("  gsj1-{}\n{}  ", &s[JOIN_PREFIX.len()..20], &s[20..]);
        assert_eq!(JoinCode::decode(&broken).unwrap(), c);
        assert!(JoinCode::decode("hello").is_err());
        assert!(JoinCode::decode(&s[..s.len() - 6]).is_err());
    }

    #[test]
    fn parses_unc_paths() {
        let u = UncPath::parse(r"\\leeubuntu\공유\사진\2024").unwrap();
        assert_eq!((u.server.as_str(), u.share.as_str()), ("leeubuntu", "공유"));
        assert_eq!(u.rest, vec!["사진", "2024"]);
        assert_eq!(u.to_unc(), r"\\leeubuntu\공유\사진\2024");
        assert_eq!(UncPath::parse("//100.90.136.114/storage/").unwrap().rest, Vec::<String>::new());
        assert_eq!(UncPath::parse(r"\\?\UNC\nas\media\a").unwrap().server, "nas");
        assert_eq!(UncPath::parse(r"\\?\C:\Users"), None);
        assert_eq!(UncPath::parse(r"\\nas"), None);
        assert_eq!(UncPath::parse(r"C:\Users"), None);
    }

    #[test]
    fn matches_the_owner_by_name_or_address() {
        let d = device("leeubuntu", &["192.168.0.100", "100.90.136.114", "leeubuntu"]);
        assert!(host_matches("leeubuntu", &d, &[]));
        assert!(host_matches("LEEUBUNTU", &d, &[]));
        assert!(host_matches("leeubuntu.tail1234.ts.net", &d, &[]));
        assert!(host_matches("100.90.136.114", &d, &[]));
        assert!(host_matches("[100.90.136.114]", &d, &[]));
        assert!(!host_matches("192.168.0.99", &d, &[]));
        assert!(!host_matches("desktop", &d, &[]));
        // 이름을 풀어 얻은 주소로도 찾는다
        let ts: IpAddr = "100.90.136.114".parse().unwrap();
        assert!(host_matches("homeserver", &d, &[ts]));
        // 이름이 Windows식 대문자여도
        let w = device("DESKTOP-4090", &["192.168.0.99"]);
        assert!(host_matches("desktop-4090", &w, &[]));
        assert_eq!(find_owner("100.90.136.114", &[w.clone(), d.clone()]), Some(1));
        assert_eq!(find_owner("192.168.0.99", &[w, d]), Some(0));
        assert_eq!(base_of("fd7a:115c::1", 8787), "http://[fd7a:115c::1]:8787");
        let d2 = device("x", &["10.0.0.1", "x"]);
        assert_eq!(d2.bases(Some("http://x:8787")), vec!["http://x:8787", "http://10.0.0.1:8787"]);
    }

    #[test]
    fn relative_paths_and_mapping() {
        assert_eq!(within("/mnt/storage/공유/사진/a.jpg", "/mnt/storage/공유", false).unwrap(), vec!["사진", "a.jpg"]);
        assert_eq!(within("/mnt/storage/공유", "/mnt/storage/공유/", false).unwrap(), Vec::<String>::new());
        assert_eq!(within("/mnt/storage/공유2/a", "/mnt/storage/공유", false), None);
        assert_eq!(within("/mnt/Storage/a", "/mnt/storage", false), None);
        assert!(within("/mnt/Storage/a", "/mnt/storage", true).is_some());
        assert_eq!(within("mnt/storage/a", "/mnt/storage", false), None);
        assert!(within(r"D:\Photos\2024\a.jpg", r"d:\photos", true).is_some());
        let local = map_to_local("/mnt/storage/공유/사진/2024/a.jpg", "/mnt/storage/공유/사진", r"\\leeubuntu\공유\사진").unwrap();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(local, format!(r"\\leeubuntu\공유\사진{sep}2024{sep}a.jpg"));
        assert_eq!(map_to_local("/mnt/other/a.jpg", "/mnt/storage", r"\\x\y"), None);
    }

    #[test]
    fn attach_plan_uses_the_owner_side_path() {
        let roots = vec!["/mnt/storage/Media/사진".to_string(), "/mnt/storage/Media".to_string()];
        // 같은 폴더를 이름·Tailscale·드라이브 문자 어느 것으로 추가해도 소유 기기 쪽 경로는 같다
        assert_eq!(plan_attach(&roots, "/mnt/storage/Media/사진", false), Plan::Existing("/mnt/storage/Media/사진".into()));
        assert_eq!(plan_attach(&roots, "/mnt/storage/Media/사진/2024", false), Plan::Existing("/mnt/storage/Media/사진".into()));
        assert_eq!(plan_attach(&roots, "/mnt/storage/Media/영상", false), Plan::Existing("/mnt/storage/Media".into()));
        assert_eq!(plan_attach(&roots, "/mnt/storage/Docs", false), Plan::New);
        assert_eq!(plan_attach(&[], "/x", false), Plan::New);
        assert_eq!(library_id("dev", "/mnt/a/"), library_id("dev", "/mnt/a"));
        assert_ne!(library_id("dev", "/mnt/a"), library_id("dev2", "/mnt/a"));
        assert_eq!(library_id("dev", "/mnt/a").len(), 16);
    }

    #[test]
    fn samples_are_checked_by_size_and_time() {
        let dir = std::env::temp_dir().join(format!("gs-mesh-samples-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("하위")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        std::fs::write(dir.join("하위").join("b.jpg"), b"12345678").unwrap();
        std::fs::write(dir.join(".hidden").join("c"), b"x").unwrap();
        let samples = collect_samples(&dir, 8, Duration::from_secs(5));
        let rels: Vec<&str> = samples.iter().map(|s| s.rel.as_str()).collect();
        assert_eq!(rels, vec!["a.txt", "하위/b.jpg"], "숨은 폴더는 보지 않는다");
        assert_eq!(verify_samples(&dir, &samples), (2, 2));
        let mut wrong = samples.clone();
        wrong[0].size += 1;
        wrong[1].mtime -= 100;
        assert_eq!(verify_samples(&dir, &wrong), (0, 2));
        // 폴더 밖을 가리키는 표본은 보지 않는다
        let escape = vec![Sample { rel: "../x".into(), size: 1, mtime: 0 }];
        assert_eq!(verify_samples(&dir.join("하위"), &escape), (0, 1));
        assert!(samples_ok(2, 3) && samples_ok(0, 0) && !samples_ok(1, 2) && !samples_ok(0, 1));
        let _ = std::fs::remove_dir_all(dir);
    }
}
