//! 앱 설정 (data\config.toml)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 설정 파일 형식 버전. 기본값을 바꿀 때 예전 기본값 그대로인 설정만 옮긴다.
pub const CONFIG_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// 0 = 버전 기록 이전 설정 파일
    pub config_version: u32,

    // ── llama-server ──
    /// llama.cpp 폴더 또는 llama-server.exe / llama.exe 경로
    pub llama_path: String,
    pub model_path: String,
    /// 비워 두면 텍스트 전용 모드
    pub mmproj_path: String,
    /// CUDA DLL 등이 llama 폴더에 없을 때 PATH 앞에 붙일 폴더
    pub extra_dll_dir: String,
    pub port: u16,
    pub n_gpu_layers: i32,
    pub ctx_size: u32,
    pub parallel: u32,
    pub batch_size: u32,
    pub ubatch_size: u32,
    /// "off" / "auto" / "on". Windows + RTX 40 + Gemma 4 계열(+mmproj)에서 FA가
    /// CUDA illegal memory access로 서버를 죽이는 문제(llama.cpp #30436)가 있어 기본 off.
    pub flash_attn: String,
    pub extra_args: String,
    pub auto_start_server: bool,
    /// 인덱싱 중 서버가 비정상 종료되면 자동으로 다시 띄우고 이어서 인덱싱
    pub auto_recover: bool,

    // ── 외부 도구 ──
    pub ffmpeg_path: String,
    pub ffprobe_path: String,

    // ── 인덱싱 ──
    pub roots: Vec<String>,
    /// 폴더 이름(대소문자 무시)이 일치하면 건너뜀
    pub exclude_dirs: Vec<String>,
    pub index_text: bool,
    pub index_docs: bool,
    pub index_images: bool,
    pub index_video: bool,
    pub index_audio: bool,
    pub max_text_mb: u64,
    pub max_media_mb: u64,
    pub chunk_chars: usize,
    pub chunk_overlap: usize,
    pub max_chunks_per_file: usize,
    /// 영상을 이 길이(초)의 구간으로 나눠 구간마다 영상 그대로 임베딩 (모델 카드: 1fps, 입력당 최대 약 58프레임)
    pub video_segment_secs: u32,
    /// 영상 하나에서 임베딩할 최대 구간 수 (긴 영상은 고르게 분산)
    pub video_max_segments: usize,
    /// 서버가 영상 입력을 거부할 때만 쓰는 대체 방식(프레임 이미지)의 프레임 수
    pub video_frames: usize,
    pub audio_segment_secs: u32,
    pub audio_max_segments: usize,
    pub workers: usize,

    // ── 검색 ──
    /// 768 / 512 / 256 (Matryoshka). DB에는 항상 원본 768차원을 저장
    pub search_dim: usize,
    pub top_k: usize,
    /// 이 관련도 미만 결과는 화면에서 숨김 (관련도 = 이 검색에서 대부분의 파일보다 표준편차 몇 배만큼 튀는지)
    pub min_relevance: f32,
    /// 검색에서 뺀 폴더 키 (새 폴더는 기본 포함되도록 '제외 목록'으로 저장)
    pub search_excluded_folders: Vec<String>,
    /// 어느 앱에서든 Ctrl+Shift+Space로 검색창 띄우기
    pub global_hotkey: bool,

    // ── 상주 ──
    /// 창을 닫으면 끝내지 않고 트레이로 (트레이 메뉴의 "종료"로 끝냄)
    pub close_to_tray: bool,
    /// 트레이로 숨겼다는 안내를 이미 보여줌
    pub tray_notice_shown: bool,
    /// Windows에 로그인하면 트레이로 시작
    pub start_with_windows: bool,
    /// 앱을 켤 때 변경분 인덱싱
    pub index_on_start: bool,
    /// 검색할 때 서버가 꺼져 있으면 자동으로 켠다
    pub server_on_demand: bool,
    /// 앱이 켠 서버를 이 시간(분) 동안 검색·인덱싱이 없으면 끈다 (0 = 끄지 않음). 서버 탭에서 직접 켠 서버는 끄지 않는다.
    pub idle_stop_minutes: u32,
    /// 하루 한 번 GitHub에서 새 버전 확인
    pub check_updates: bool,
    /// 마지막 확인 시각 (유닉스 초)
    pub last_update_check: u64,
    /// 첫 실행 마법사를 마쳤는지
    pub setup_done: bool,
    /// 인덱싱 중 앱 창을 보고 있지 않으면 화면 오른쪽 아래에 상태 창
    pub index_overlay: bool,
    /// 폴더 감시: 검색 폴더의 파일이 생기거나 바뀌거나 지워지면 그 파일만 자동으로 인덱싱
    pub watch_folders: bool,
    /// 폴더를 훑어 인덱스와 비교하는 주기(시간). 변경 알림이 오지 않는 네트워크 폴더와 놓친 알림을 찾는다.
    /// 0 = 켤 때 한 번만
    pub watch_rescan_hours: u32,
    /// 서비스판(`gemma-search serve`)이 접속을 받는 주소. 다른 기기에서 접속하게 하려면 `0.0.0.0:8787`.
    pub service_bind: String,
    /// 서비스판의 접속 토큰. 다른 기기에서 접속하게 열면 있어야 하고, 비어 있으면 그때 만든다.
    pub service_token: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            llama_path: default_path(&[app_dir().join("llama.cpp"), PathBuf::from(r"F:\Tools\llama.cpp-embed")]),
            // ggml-org의 Q8_0 (310MB + 555MB). BF16과 벡터가 사실상 같다 (실측 코사인 0.997 이상)
            // 포터블: 프로그램 폴더의 models\ 아래 (설정 → 준비물 받기). 없으면 예전 위치.
            model_path: default_path(&[
                model_dir().join("embeddinggemma-2-Q8_0.gguf"),
                PathBuf::from(r"F:\Tools\models\embeddinggemma-2\embeddinggemma-2-Q8_0.gguf"),
            ]),
            mmproj_path: default_path(&[
                model_dir().join("mmproj-embeddinggemma-2-Q8_0.gguf"),
                PathBuf::from(r"F:\Tools\models\embeddinggemma-2\mmproj-embeddinggemma-2-Q8_0.gguf"),
            ]),
            extra_dll_dir: String::new(),
            port: 8090,
            n_gpu_layers: 99,
            // RTX 4090 실측: np 4/ub 2048 ≈ 56 벡터/초, np 8/ub 8192 ≈ 91 벡터/초, np 16 ≈ 73 (GPU 포화)
            // 슬롯당 8192(모델 최대 컨텍스트)가 되도록 65536 — 영상 30초 구간(약 2,600토큰)이 들어가야 함
            ctx_size: 65536,
            parallel: 8,
            batch_size: 8192,
            ubatch_size: 8192,
            flash_attn: "off".into(),
            extra_args: String::new(),
            auto_start_server: false,
            auto_recover: true,

            ffmpeg_path: crate::util::find_tool(&crate::platform::exe_name("ffmpeg"))
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            ffprobe_path: crate::util::find_tool(&crate::platform::exe_name("ffprobe"))
                .map(|p| p.display().to_string())
                .unwrap_or_default(),

            roots: Vec::new(),
            exclude_dirs: [
                ".git",
                ".svn",
                ".hg",
                "node_modules",
                "target",
                "__pycache__",
                ".venv",
                "venv",
                ".cache",
                "$Recycle.Bin",
                "System Volume Information",
                "Windows",
                // 키·인증서가 있는 폴더는 아예 읽지 않는다
                ".ssh",
                ".gnupg",
                ".aws",
                ".azure",
                ".kube",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            index_text: true,
            index_docs: true,
            index_images: true,
            index_video: true,
            index_audio: true,
            max_text_mb: 20,
            max_media_mb: 4096,
            chunk_chars: 1000,
            chunk_overlap: 150,
            max_chunks_per_file: 200,
            video_segment_secs: 30,
            // 긴 영상·오디오도 끝까지 (30초 구간 기준 영상 2시간, 오디오 약 3시간). 더 길면 고르게 분산.
            video_max_segments: 240,
            video_frames: 12,
            audio_segment_secs: 30,
            audio_max_segments: 400,
            workers: 4,

            search_dim: 768,
            top_k: 100,
            // 실측: 맞는 게 없는 검색의 1위는 4.9~5.5, 맞는 검색은 대체로 6~10
            min_relevance: 5.5,
            search_excluded_folders: Vec::new(),
            global_hotkey: true,
            close_to_tray: true,
            tray_notice_shown: false,
            start_with_windows: false,
            index_on_start: false,
            server_on_demand: true,
            idle_stop_minutes: 10,
            check_updates: true,
            last_update_check: 0,
            setup_done: false,
            index_overlay: true,
            watch_folders: false,
            watch_rescan_hours: 6,
            service_bind: "127.0.0.1:8787".into(),
            service_token: String::new(),
        }
    }
}

impl Config {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("config.toml");
        let mut cfg: Config = match std::fs::read_to_string(&path) {
            Ok(s) => {
                // 버전 키가 없는 예전 파일은 0으로 읽어야 이전 기본값을 옮길 수 있다
                let had_version = s.lines().any(|l| l.trim_start().starts_with("config_version"));
                let mut c: Config = toml::from_str(&s).unwrap_or_else(|e| {
                    eprintln!("config.toml 파싱 실패, 기본값 사용: {e}");
                    Self::default()
                });
                if !had_version {
                    c.config_version = 0;
                }
                c
            }
            Err(_) => return Self::default(),
        };
        cfg.migrate();
        cfg.reanchor_portable(&app_dir());
        cfg
    }

    /// 포터블 폴더를 통째로 옮긴 경우: 설정에는 절대 경로가 저장돼 있어 예전 위치를 가리킨다.
    /// 저장된 경로가 없어졌고 지금 프로그램 폴더 아래에 같은 꼬리 경로가 있으면 그쪽으로 바꾼다.
    fn reanchor_portable(&mut self, base: &Path) {
        for p in [&mut self.llama_path, &mut self.model_path, &mut self.mmproj_path] {
            if let Some(moved) = reanchor(p.as_str(), base) {
                *p = moved;
            }
        }
    }

    /// 예전 기본값 그대로인 항목만 새 기본값으로 옮긴다 (사용자가 바꾼 값은 유지)
    fn migrate(&mut self) {
        if self.config_version < 2 {
            // 슬롯당 2048 → 8192: 영상 구간 임베딩에 필요
            if self.ctx_size == 16384 && self.parallel == 8 {
                self.ctx_size = 65536;
            }
        }
        if self.config_version < 3 {
            // 긴 영상·오디오를 끝까지 읽도록 구간 수 기본값 상향, 키·인증서 폴더 제외 추가
            if self.video_max_segments == 20 {
                self.video_max_segments = 240;
            }
            if self.audio_max_segments == 20 {
                self.audio_max_segments = 400;
            }
            for d in SENSITIVE_DIRS {
                if !self.exclude_dirs.iter().any(|x| x.eq_ignore_ascii_case(d)) {
                    self.exclude_dirs.push(d.to_string());
                }
            }
        }
        self.config_version = CONFIG_VERSION;
    }

    pub fn save(&self, data_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let s = toml::to_string_pretty(self)?;
        std::fs::write(data_dir.join("config.toml"), s)?;
        Ok(())
    }

    /// 슬롯 하나가 처리할 수 있는 최대 입력 토큰 수 (여유분 포함)
    pub fn max_input_tokens(&self) -> usize {
        let slot = (self.ctx_size / self.parallel.max(1)).max(256);
        (slot.min(self.ubatch_size).min(self.batch_size) as usize).saturating_sub(16)
    }

    /// 영상 구간 길이(초): 설정값과, 입력 하나(슬롯)에 들어가는 프레임 수 중 작은 쪽.
    /// 1fps 기준 프레임당 약 85토큰(실측) — 여유 있게 150으로 잡는다.
    pub fn video_window_secs(&self) -> u32 {
        let by_ctx = (self.max_input_tokens().saturating_sub(64) / 150) as u32;
        self.video_segment_secs.clamp(4, 58).min(by_ctx.max(4))
    }

    pub fn ffmpeg(&self) -> Option<PathBuf> {
        existing(&self.ffmpeg_path)
    }

    pub fn ffprobe(&self) -> Option<PathBuf> {
        existing(&self.ffprobe_path)
    }

    pub fn has_mmproj(&self) -> bool {
        !self.mmproj_path.trim().is_empty()
    }

    /// llama.cpp 빌드 서명 (실행 파일 이름:크기). 바뀌면 벡터가 그대로인지 호환성 검사.
    pub fn server_signature(&self) -> String {
        match crate::server::resolve_binary(&self.llama_path) {
            Ok((exe, _)) => {
                let name = exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                // llama-server.exe는 작은 실행 파일이고 실제 코드는 DLL에 있다 → ggml/llama DLL 크기도 함께
                let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
                let size = |n: &str| std::fs::metadata(dir.join(n)).map(|m| m.len()).unwrap_or(0);
                let exe_size = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
                format!("{name}:{exe_size}|llama:{}|cuda:{}", size("llama.dll"), size("ggml-cuda.dll"))
            }
            Err(_) => "-".into(),
        }
    }

    /// 인덱스에 기록해 두는 모델 구성 서명. 바뀌면 호환성 검사 → 벡터가 달라졌으면 재구축 필요.
    pub fn model_signature(&self) -> String {
        fn sig(p: &str) -> String {
            let p = p.trim();
            if p.is_empty() {
                return "-".into();
            }
            let path = Path::new(p);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            format!("{name}:{size}")
        }
        format!(
            "{}|{}|p{}",
            sig(&self.model_path),
            sig(&self.mmproj_path),
            crate::embed::prompt::PROMPT_VERSION
        )
    }
}

/// 후보 중 실제로 있는 첫 경로, 없으면 첫 후보 (프로그램 폴더 기준)
fn default_path(candidates: &[PathBuf]) -> String {
    candidates
        .iter()
        .find(|p| p.exists())
        .or(candidates.first())
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// `stored`가 이제 없는 경로일 때, 그 꼬리 부분을 긴 것부터 `base` 아래에서 찾는다.
/// 예: `C:\old\app\models\m\x.gguf`(없음), base `D:\new\app` → `D:\new\app\models\m\x.gguf`(있으면).
/// 경로가 그대로 있거나, 비어 있거나, `base` 아래에도 없으면 None.
fn reanchor(stored: &str, base: &Path) -> Option<String> {
    let stored = stored.trim();
    if stored.is_empty() || Path::new(stored).exists() {
        return None;
    }
    // 드라이브·루트를 뺀 폴더·파일 이름들
    let parts: Vec<_> = Path::new(stored)
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .collect();
    (0..parts.len()).find_map(|start| {
        let candidate = parts[start..].iter().fold(base.to_path_buf(), |dir, part| dir.join(part));
        candidate.exists().then(|| candidate.display().to_string())
    })
}

/// 키·인증서가 들어 있는 폴더 (기본 제외)
const SENSITIVE_DIRS: &[&str] = &[".ssh", ".gnupg", ".aws", ".azure", ".kube"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_old_default_context_only() {
        let dir = std::env::temp_dir().join(format!("gs-cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("config.toml"), "ctx_size = 16384\nparallel = 8\n").unwrap();
        assert_eq!(Config::load(&dir).ctx_size, 65536, "예전 기본값은 새 기본값으로");
        std::fs::write(dir.join("config.toml"), "ctx_size = 32768\nparallel = 8\n").unwrap();
        assert_eq!(Config::load(&dir).ctx_size, 32768, "사용자가 바꾼 값은 유지");
        std::fs::write(dir.join("config.toml"), "config_version = 2\nctx_size = 16384\nparallel = 8\n").unwrap();
        assert_eq!(Config::load(&dir).ctx_size, 16384, "이미 새 버전이면 손대지 않음");
        std::fs::write(
            dir.join("config.toml"),
            "config_version = 2\nvideo_max_segments = 20\naudio_max_segments = 50\nexclude_dirs = [\".git\"]\n",
        )
        .unwrap();
        let c = Config::load(&dir);
        assert_eq!((c.video_max_segments, c.audio_max_segments), (240, 50), "예전 기본값만 상향");
        assert!(c.exclude_dirs.iter().any(|d| d == ".ssh"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reanchors_paths_after_portable_folder_moved() {
        let base = std::env::temp_dir().join(format!("gs-move-{}", std::process::id()));
        let model = base.join("models").join("m").join("x.gguf");
        std::fs::create_dir_all(model.parent().unwrap()).unwrap();
        std::fs::write(&model, b"").unwrap();
        std::fs::create_dir_all(base.join("llama.cpp")).unwrap();
        // 옮기기 전 위치(이제 없음)를 가리키는 경로는 새 프로그램 폴더 아래로
        let old = std::env::temp_dir().join(format!("gs-gone-{}", std::process::id())).join("app");
        let old_model = old.join("models").join("m").join("x.gguf").display().to_string();
        assert_eq!(reanchor(&old_model, &base), Some(model.display().to_string()));
        assert_eq!(
            reanchor(&old.join("llama.cpp").display().to_string(), &base),
            Some(base.join("llama.cpp").display().to_string())
        );
        // 그대로 있는 경로, 빈 값, 새 위치에도 없는 경로는 건드리지 않는다
        assert_eq!(reanchor(&model.display().to_string(), &base), None);
        assert_eq!(reanchor("  ", &base), None);
        assert_eq!(reanchor(&old.join("models").join("m").join("y.gguf").display().to_string(), &base), None);
        let _ = std::fs::remove_dir_all(base);
    }
}

fn existing(p: &str) -> Option<PathBuf> {
    let p = p.trim();
    if p.is_empty() {
        return None;
    }
    let pb = PathBuf::from(p);
    pb.is_file().then_some(pb)
}

/// 포터블 준비물 가운데 모델이 놓이는 폴더: 프로그램 폴더의 `models/embeddinggemma-2`
fn model_dir() -> PathBuf {
    app_dir().join("models").join("embeddinggemma-2")
}

/// 다른 프로세스가 데이터 폴더를 잠가 두었을 때의 안내 첫머리 (`instance_lock`의 오류가 이것으로 시작한다)
pub const LOCK_BUSY: &str = "다른 Gemma Search가 이미 이 데이터 폴더를 쓰고 있습니다";

/// 한 데이터 폴더를 한 프로세스만 쓰게 하는 잠금 (창이 있는 앱, 서비스판, `--index-once`가 같은 인덱스를 함께 고치지 않게).
/// 돌려받은 파일을 들고 있는 동안 잠겨 있고, 프로세스가 끝나면 풀린다.
pub fn instance_lock(data_dir: &Path) -> Result<std::fs::File, String> {
    let path = data_dir.join("instance.lock");
    let file = std::fs::create_dir_all(data_dir)
        .and_then(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
        })
        .map_err(|e| format!("데이터 폴더에 쓸 수 없습니다 ({}): {e}", path.display()))?;
    match file.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "{LOCK_BUSY} ({}).\n창이 있는 앱, 서비스(gemma-search serve), --index-once는 같은 데이터 폴더로 함께 실행할 수 없습니다.",
            data_dir.display()
        )),
        // 잠금을 지원하지 않는 파일 시스템이면 잠그지 않고 쓴다
        Ok(()) | Err(std::fs::TryLockError::Error(_)) => Ok(file),
    }
}

/// 데이터 폴더: exe 옆의 `data`. cargo로 실행 중(target\debug|release)이면 프로젝트 루트의 `data`.
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GEMMA_SEARCH_DATA") {
        return PathBuf::from(d);
    }
    app_dir().join("data")
}

/// 프로그램 폴더: exe가 있는 폴더 (cargo로 실행 중이면 프로젝트 루트). 포터블 준비물(모델·llama.cpp)도 여기 아래.
pub fn app_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    let mut dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let is_target = |d: &Path| {
        let name = d.file_name().map(|n| n.to_string_lossy().to_lowercase());
        matches!(name.as_deref(), Some("debug") | Some("release"))
            && d.parent()
                .and_then(|p| p.file_name())
                .map(|n| n.eq_ignore_ascii_case("target"))
                .unwrap_or(false)
    };
    if is_target(&dir) {
        if let Some(root) = dir.parent().and_then(Path::parent) {
            dir = root.to_path_buf();
        }
    }
    dir
}
