//! 경로 문자열 비교. 인덱스(DB)와 폴더 감시는 경로를 문자열로 다룬다 (DB의 키가 경로 문자열이다).
//! 구분자와 대소문자 규칙을 여기 한곳에 둔다.
//!
//! - 구분자는 그 OS의 것을 따른다 (Windows는 `\`와 `/`, 그 밖은 `/`).
//! - 비교는 대소문자를 구분하지 않는다 (Windows와 macOS의 기본 파일 시스템 기준).

/// 그 OS에서 경로를 이어 붙일 때 쓰는 구분자
pub use std::path::MAIN_SEPARATOR as SEP;

/// 경로 구분자인지
pub fn is_sep(c: char) -> bool {
    std::path::is_separator(c)
}

/// 비교용 모양: 앞뒤 공백과 끝의 구분자를 떼고 소문자로
pub fn norm(path: &str) -> String {
    path.trim().trim_end_matches(is_sep).to_lowercase()
}

/// `path`가 `base` 폴더 아래인지 (둘 다 `norm`을 거친 것). 같은 경로는 "아래"가 아니다.
pub fn under(path: &str, base: &str) -> bool {
    path.starts_with(base) && path[base.len()..].starts_with(is_sep)
}

/// 구분자로 나눈 이름들 (빈 것은 뺀다)
pub fn parts(path: &str) -> impl Iterator<Item = &str> {
    path.split(is_sep).filter(|s| !s.is_empty())
}

/// 시험용: Windows 모양으로 적은 경로를 이 OS의 모양으로 바꾼다.
/// Windows는 그대로, 그 밖에서는 `C:\Docs\a` → `/C/Docs/a`, `\\nas\share` → `/nas/share`.
#[cfg(test)]
pub fn os_path(p: &str) -> String {
    if cfg!(windows) {
        return p.to_string();
    }
    let mut s = p.replace('\\', "/");
    if s.as_bytes().get(1) == Some(&b':') {
        s.remove(1);
    }
    format!("/{}", s.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_paths_without_case_or_trailing_separator() {
        let sep = SEP;
        let base = norm(&format!("  Root{sep}Docs{sep}"));
        assert_eq!(base, format!("root{sep}docs"));
        assert!(under(&norm(&format!("ROOT{sep}docs{sep}a.txt")), &base));
        assert!(under(&norm(&format!("root{sep}docs{sep}sub{sep}a.txt")), &base));
        // 이름이 같은 글자로 시작할 뿐인 폴더, 그리고 자기 자신은 아래가 아니다
        assert!(!under(&norm(&format!("root{sep}docs2{sep}a.txt")), &base));
        assert!(!under(&base, &base));
        let rel = format!("{sep}sub{sep}{sep}a.txt");
        assert_eq!(parts(&rel).collect::<Vec<_>>(), vec!["sub", "a.txt"]);
    }

    #[test]
    fn test_paths_follow_the_os() {
        if cfg!(windows) {
            assert_eq!(os_path(r"C:\Docs\a.txt"), r"C:\Docs\a.txt");
        } else {
            assert_eq!(os_path(r"C:\Docs\a.txt"), "/C/Docs/a.txt");
            assert_eq!(os_path(r"\\nas\share\Photos\"), "/nas/share/Photos/");
            assert_eq!(os_path(r"C:\Docs\~$report.docx"), "/C/Docs/~$report.docx");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_accepts_both_separators() {
        assert_eq!(norm(r"C:\Docs\"), r"c:\docs");
        assert_eq!(norm("C:/Docs/"), "c:/docs");
        assert!(under(r"c:\docs\a.txt", r"c:\docs"));
        assert!(under("c:/docs/a.txt", "c:/docs"));
        assert_eq!(parts(r"\\nas\share/photos").collect::<Vec<_>>(), vec!["nas", "share", "photos"]);
    }
}
