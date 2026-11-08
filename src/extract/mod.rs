//! 파일 종류 판별과 내용 추출

pub mod docx;
pub mod image;
pub mod media;
pub mod pdf;
pub mod text;

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Other = 0,
    Text = 1,
    Doc = 2,
    Image = 3,
    Video = 4,
    Audio = 5,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Text,
        Kind::Doc,
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Other,
    ];

    pub fn from_u8(v: u8) -> Kind {
        match v {
            1 => Kind::Text,
            2 => Kind::Doc,
            3 => Kind::Image,
            4 => Kind::Video,
            5 => Kind::Audio,
            _ => Kind::Other,
        }
    }

    pub fn idx(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Text => "텍스트·코드",
            Kind::Doc => "문서(PDF·DOCX)",
            Kind::Image => "이미지",
            Kind::Video => "영상",
            Kind::Audio => "오디오",
            Kind::Other => "기타(이름만)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Modality {
    Name = 0,
    Text = 1,
    Image = 2,
    Audio = 3,
    /// 영상 구간을 영상 그대로 임베딩 (모델 카드 방식)
    Video = 4,
}

impl Modality {
    pub fn from_u8(v: u8) -> Modality {
        match v {
            1 => Modality::Text,
            2 => Modality::Image,
            3 => Modality::Audio,
            4 => Modality::Video,
            _ => Modality::Name,
        }
    }
}

const TEXT_EXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml",
    "ini", "cfg", "conf", "xml", "html", "htm", "css", "scss", "sass", "less", "js", "mjs", "cjs",
    "ts", "tsx", "jsx", "py", "pyw", "ipynb", "rs", "go", "java", "kt", "kts", "c", "h", "cpp",
    "hpp", "cc", "cxx", "hh", "cs", "swift", "rb", "php", "pl", "lua", "sh", "bash", "zsh", "bat",
    "cmd", "ps1", "psm1", "sql", "r", "m", "mm", "scala", "dart", "vue", "svelte", "tex", "bib",
    "srt", "vtt", "smi", "ass", "properties", "gradle", "cmake", "makefile", "dockerfile", "proto",
    "graphql", "gql", "nfo", "org", "adoc", "asciidoc", "env", "gitignore", "editorconfig", "reg",
    "inf", "vb", "vbs", "fs", "fsx", "hs", "elm", "ex", "exs", "erl", "clj", "zig", "nim", "jl",
    "sol", "v", "sv", "vhd", "asm", "s", "glsl", "hlsl", "wgsl", "cu", "tf", "hcl",
];
const DOC_EXT: &[&str] = &["pdf", "docx"];
const IMAGE_EXT: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "webp", "bmp", "gif", "tif", "tiff", "heic", "heif", "avif",
];
const VIDEO_EXT: &[&str] = &[
    "mp4", "m4v", "mkv", "mov", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "m2ts", "mts", "3gp",
    "ogv",
];
const AUDIO_EXT: &[&str] = &[
    "mp3", "wav", "flac", "m4a", "aac", "ogg", "oga", "opus", "wma", "aiff", "aif",
];

pub fn classify(path: &Path) -> Kind {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| {
            // 확장자 없는 Makefile, Dockerfile 등
            path.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
    let e = ext.as_str();
    if TEXT_EXT.contains(&e) {
        Kind::Text
    } else if DOC_EXT.contains(&e) {
        Kind::Doc
    } else if IMAGE_EXT.contains(&e) {
        Kind::Image
    } else if VIDEO_EXT.contains(&e) {
        Kind::Video
    } else if AUDIO_EXT.contains(&e) {
        Kind::Audio
    } else {
        Kind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_examples() {
        assert_eq!(classify(Path::new(r"C:\a\b.RS")), Kind::Text);
        assert_eq!(classify(Path::new("x.ts")), Kind::Text);
        assert_eq!(classify(Path::new("report.pdf")), Kind::Doc);
        assert_eq!(classify(Path::new("IMG_0001.JPG")), Kind::Image);
        assert_eq!(classify(Path::new("clip.mkv")), Kind::Video);
        assert_eq!(classify(Path::new("song.flac")), Kind::Audio);
        assert_eq!(classify(Path::new("Makefile")), Kind::Text);
        assert_eq!(classify(Path::new("setup.exe")), Kind::Other);
    }
}
