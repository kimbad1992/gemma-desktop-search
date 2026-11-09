//! 결과를 일치한 자리에서 열기: 영상·오디오는 그 시점, PDF는 그 페이지.
//! 그 형식의 기본 앱이 위치를 받는 앱(VLC, mpv, MPC-HC/BE, 팟플레이어 / Edge, Chrome, Firefox, SumatraPDF, Acrobat)일
//! 때만 위치를 넘기고, 그 밖의 앱은 처음부터 연다.

use crate::extract::{Kind, Modality};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    Start,
    /// 초
    Seconds(f64),
    /// 1부터
    Page(i64),
}

impl Place {
    /// 검색 결과 청크가 가리키는 자리
    pub fn of(path: &str, kind: Kind, modality: Modality, locator: i64) -> Place {
        match modality {
            Modality::Video | Modality::Audio => Place::Seconds(locator.max(0) as f64 / 1000.0),
            Modality::Image if kind == Kind::Video => Place::Seconds(locator.max(0) as f64 / 1000.0),
            Modality::Text if is_pdf(path) => {
                crate::extract::pdf::page_of(locator).map(Place::Page).unwrap_or(Place::Start)
            }
            _ => Place::Start,
        }
    }
}

pub fn is_pdf(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// 확장자(".mp4")에 연결된 기본 앱 실행 파일
pub fn default_app(ext: &str) -> Option<PathBuf> {
    use windows_sys::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};
    let wide = |s: &str| -> Vec<u16> { std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect() };
    let assoc = wide(ext);
    let verb = wide("open");
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let hr = unsafe {
        AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, assoc.as_ptr(), verb.as_ptr(), buf.as_mut_ptr(), &mut len)
    };
    if hr != 0 {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    p.is_file().then_some(p)
}

fn hms(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// 파일 경로 → file:/// URL (브라우저가 PDF를 #page=N으로 열 수 있게)
fn file_url(path: &str) -> String {
    let p = path.replace('\\', "/");
    let enc: String = p
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '%' => "%25".to_string(),
            '?' => "%3F".to_string(),
            c => c.to_string(),
        })
        .collect();
    if let Some(unc) = enc.strip_prefix("//") {
        format!("file://{unc}")
    } else {
        format!("file:///{enc}")
    }
}

/// 실행할 명령 (exe, 인자). 위치를 넘길 수 없는 앱이면 None.
pub fn place_command(app: &Path, path: &str, place: Place) -> Option<Vec<String>> {
    let name = app.file_name()?.to_string_lossy().to_lowercase();
    let p = path.to_string();
    match place {
        Place::Start => None,
        Place::Seconds(t) => {
            if name.starts_with("vlc") {
                Some(vec![format!("--start-time={t:.1}"), p])
            } else if name.starts_with("mpv") {
                Some(vec![format!("--start={t:.1}"), p])
            } else if name.starts_with("mpc-hc") || name.starts_with("mpc-be") {
                Some(vec![p, "/start".into(), format!("{}", (t * 1000.0) as u64)])
            } else if name.starts_with("potplayer") {
                Some(vec![p, format!("/seek={}", hms(t))])
            } else {
                None
            }
        }
        Place::Page(n) => {
            if ["msedge", "chrome", "firefox", "brave", "vivaldi", "whale"]
                .iter()
                .any(|b| name.starts_with(b))
            {
                Some(vec![format!("{}#page={n}", file_url(path))])
            } else if name.starts_with("sumatrapdf") {
                Some(vec!["-page".into(), n.to_string(), p])
            } else if name.starts_with("acrobat") || name.starts_with("acrord32") {
                Some(vec!["/A".into(), format!("page={n}"), p])
            } else {
                None
            }
        }
    }
}

/// 연다. 반환: 위치를 넘겼으면 그 설명 ("VLC · 01:30부터")
pub fn open_at(path: &str, place: Place) -> Option<String> {
    if place != Place::Start {
        let ext = Path::new(path)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        if let Some(app) = default_app(&ext) {
            if let Some(args) = place_command(&app, path, place) {
                // 플레이어·뷰어는 앱을 닫아도 남아 있어야 하므로 Job에 넣지 않는다
                if Command::new(&app).args(&args).spawn().is_ok() {
                    let app_name = app.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    return Some(match place {
                        Place::Seconds(t) => format!("{app_name} · {}부터", crate::util::fmt_ts((t * 1000.0) as i64)),
                        Place::Page(n) => format!("{app_name} · {n}페이지"),
                        Place::Start => app_name,
                    });
                }
            }
        }
    }
    crate::util::open_path(path);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_for_known_apps() {
        let vlc = Path::new(r"C:\Program Files\VideoLAN\VLC\vlc.exe");
        assert_eq!(
            place_command(vlc, r"D:\v.mp4", Place::Seconds(90.0)).unwrap(),
            vec!["--start-time=90.0", r"D:\v.mp4"]
        );
        let pot = Path::new(r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe");
        assert_eq!(place_command(pot, "a.mp4", Place::Seconds(3725.0)).unwrap()[1], "/seek=01:02:05");
        let edge = Path::new(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe");
        assert_eq!(
            place_command(edge, r"\\nas\공유\보고서 1.pdf", Place::Page(4)).unwrap(),
            vec!["file://nas/공유/보고서%201.pdf#page=4"]
        );
        assert_eq!(
            place_command(edge, r"C:\a\b.pdf", Place::Page(2)).unwrap(),
            vec!["file:///C:/a/b.pdf#page=2"]
        );
        let other = Path::new(r"C:\x\Photos.exe");
        assert!(place_command(other, "a.mp4", Place::Seconds(5.0)).is_none());
        assert!(place_command(vlc, "a.mp4", Place::Start).is_none());
    }

    #[test]
    fn place_from_hit() {
        assert_eq!(Place::of("a.mp4", Kind::Video, Modality::Video, 30_000), Place::Seconds(30.0));
        assert_eq!(Place::of(r"C:\a.pdf", Kind::Doc, Modality::Text, 3002), Place::Page(3));
        assert_eq!(Place::of(r"C:\a.pdf", Kind::Doc, Modality::Text, 2), Place::Start, "예전 청크");
        assert_eq!(Place::of("a.txt", Kind::Text, Modality::Text, 2), Place::Start);
    }
}
