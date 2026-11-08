//! 앱 설정 (data\config.toml)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 설정 파일 형식 버전. 기본값을 바꿀 때 예전 기본값 그대로인 설정만 옮긴다.
pub const CONFIG_VERSION: u32 = 2;

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
    /// 이 점수 미만 결과는 화면에서 숨김 (0 = 끔)
    pub min_score: f32,
    /// 검색에서 뺀 폴더 키 (새 폴더는 기본 포함되도록 '제외 목록'으로 저장)
    pub search_excluded_folders: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            llama_path: r"F:\Tools\llama.cpp-embed".into(),
            model_path: r"F:\Tools\models\embeddinggemma-2\embeddinggemma-2-BF16.gguf".into(),
            mmproj_path: r"F:\Tools\models\embeddinggemma-2\mmproj-embeddinggemma-2-BF16.gguf".into(),
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

            ffmpeg_path: crate::util::find_tool("ffmpeg.exe")
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            ffprobe_path: crate::util::find_tool("ffprobe.exe")
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
            video_max_segments: 20,
            video_frames: 12,
            audio_segment_secs: 30,
            audio_max_segments: 20,
            workers: 4,

            search_dim: 768,
            top_k: 100,
            min_score: 0.0,
            search_excluded_folders: Vec::new(),
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
        cfg
    }

    /// 예전 기본값 그대로인 항목만 새 기본값으로 옮긴다 (사용자가 바꾼 값은 유지)
    fn migrate(&mut self) {
        if self.config_version < 2 {
            // 슬롯당 2048 → 8192: 영상 구간 임베딩에 필요
            if self.ctx_size == 16384 && self.parallel == 8 {
                self.ctx_size = 65536;
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

    /// 인덱스에 기록해 두는 모델 구성 서명. 바뀌면 재구축 필요.
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
        let _ = std::fs::remove_dir_all(dir);
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

/// 데이터 폴더: exe 옆의 `data`. cargo로 실행 중(target\debug|release)이면 프로젝트 루트의 `data`.
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("GEMMA_SEARCH_DATA") {
        return PathBuf::from(d);
    }
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
    dir.join("data")
}
