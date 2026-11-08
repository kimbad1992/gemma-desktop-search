//! 앱 설정 (data\config.toml)

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
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
    pub extra_args: String,
    pub auto_start_server: bool,

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            llama_path: r"F:\Tools\llama.cpp-embed".into(),
            model_path: r"F:\Tools\models\embeddinggemma-2\embeddinggemma-2-BF16.gguf".into(),
            mmproj_path: r"F:\Tools\models\embeddinggemma-2\mmproj-embeddinggemma-2-BF16.gguf".into(),
            extra_dll_dir: String::new(),
            port: 8090,
            n_gpu_layers: 99,
            // RTX 4090 실측: np 4/ub 2048 ≈ 56 벡터/초, np 8/ub 8192 ≈ 91 벡터/초, np 16 ≈ 73 (GPU 포화)
            ctx_size: 16384,
            parallel: 8,
            batch_size: 8192,
            ubatch_size: 8192,
            extra_args: String::new(),
            auto_start_server: false,

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
            video_frames: 12,
            audio_segment_secs: 30,
            audio_max_segments: 20,
            workers: 4,

            search_dim: 768,
            top_k: 100,
            min_score: 0.0,
        }
    }
}

impl Config {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                eprintln!("config.toml 파싱 실패, 기본값 사용: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
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
