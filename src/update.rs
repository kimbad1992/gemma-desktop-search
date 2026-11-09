//! 새 버전 확인: 하루 한 번 GitHub의 최신 릴리스 태그를 본다 (파일 정보는 보내지 않음, 설정에서 끌 수 있음)

pub const REPO: &str = "kimbad1992/gemma-desktop-search";

/// "v0.4.0" → (0, 4, 0)
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut it = s.split(['.', '-']).map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

/// 지금 버전보다 새 릴리스가 있으면 (태그, 릴리스 페이지 주소)
pub fn check() -> Result<Option<(String, String)>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .into();
    let v: serde_json::Value = agent
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", "gemma-desktop-search")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or_default().to_string();
    let url = v["html_url"].as_str().unwrap_or_default().to_string();
    let (Some(latest), Some(mine)) = (parse_version(&tag), parse_version(env!("CARGO_PKG_VERSION"))) else {
        return Ok(None);
    };
    Ok((latest > mine).then_some((tag, url)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.4.0"), Some((0, 4, 0)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert!(parse_version("v0.10.0") > parse_version("v0.9.3"));
        assert_eq!(parse_version("latest"), None);
    }
}
