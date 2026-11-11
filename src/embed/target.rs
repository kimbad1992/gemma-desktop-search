//! 임베딩을 맡길 곳: llama-server의 주소와 접속 키.
//!
//! 보통은 이 기기에서 앱이 켠 llama-server(`http://127.0.0.1:<포트>`)이고, 설정의 `embed_server`를 주면
//! 다른 곳에서 떠 있는 llama-server를 쓴다 (예: SSH 터널로 이어 둔 다른 PC의 서버).

use std::fmt;

#[derive(Clone, PartialEq, Eq)]
pub struct EmbedTarget {
    /// `http://주소:포트` (끝의 `/`는 뗀다). 요청은 여기에 `/health`, `/v1/embeddings`처럼 붙인다.
    pub base: String,
    /// llama-server를 `--api-key`로 켰을 때의 키 (Authorization: Bearer). 화면과 로그에는 내보내지 않는다.
    pub key: Option<String>,
}

impl EmbedTarget {
    /// 이 기기의 llama-server
    pub fn local(port: u16) -> Self {
        Self { base: format!("http://127.0.0.1:{port}"), key: None }
    }

    /// 사용자가 준 주소를 읽는다. `8090`(이 기기의 포트), `192.168.0.10:8090`, `http://host:8090`,
    /// `https://host/llama` 같은 모양을 받는다. 키가 비어 있으면 키 없이 접속한다.
    pub fn parse(addr: &str, key: &str) -> Result<Self, String> {
        let addr = addr.trim();
        if addr.is_empty() {
            return Err("서버 주소가 비어 있습니다".into());
        }
        let key = Some(key.trim().to_string()).filter(|k| !k.is_empty());
        if addr.bytes().all(|b| b.is_ascii_digit()) {
            return match addr.parse::<u16>() {
                Ok(p) if p > 0 => Ok(Self { key, ..Self::local(p) }),
                _ => Err(format!("포트 번호가 올바르지 않습니다: {addr}")),
            };
        }
        let full = if addr.contains("://") { addr.to_string() } else { format!("http://{addr}") };
        let uri: ureq::http::Uri = full.parse().map_err(|e| format!("주소를 읽을 수 없습니다 ({addr}): {e}"))?;
        match uri.scheme_str() {
            Some("http") | Some("https") => {}
            _ => return Err(format!("http:// 또는 https:// 주소여야 합니다: {addr}")),
        }
        let host = uri.host().unwrap_or("");
        if host.is_empty() {
            return Err(format!("주소에 기기 이름이나 IP가 없습니다: {addr}"));
        }
        if uri.query().is_some() {
            return Err(format!("주소에 ?로 시작하는 부분은 넣을 수 없습니다: {addr}"));
        }
        Ok(Self { base: full.trim_end_matches('/').to_string(), key })
    }

    /// 이 기기 안의 서버인지 (127.0.0.1, ::1, localhost)
    pub fn is_loopback(&self) -> bool {
        let Ok(uri) = self.base.parse::<ureq::http::Uri>() else { return false };
        let host = uri.host().unwrap_or("").trim_matches(['[', ']']);
        host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }
}

/// 화면과 로그용: 주소만 (키는 숨긴다)
impl fmt::Display for EmbedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.base)?;
        if self.key.is_some() {
            f.write_str(" (접속 키 사용)")?;
        }
        Ok(())
    }
}

/// `{:?}`로 찍어도 키가 새지 않게
impl fmt::Debug for EmbedTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EmbedTarget({self})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_usual_shapes() {
        let t = |s: &str| EmbedTarget::parse(s, "").map(|t| t.base);
        assert_eq!(t("8090").unwrap(), "http://127.0.0.1:8090");
        assert_eq!(t(" 192.168.0.10:8090 ").unwrap(), "http://192.168.0.10:8090");
        assert_eq!(t("http://desk.tailnet.ts.net:8090/").unwrap(), "http://desk.tailnet.ts.net:8090");
        assert_eq!(t("https://example.com/llama/").unwrap(), "https://example.com/llama");
        assert_eq!(t("[::1]:8090").unwrap(), "http://[::1]:8090");
        assert!(t("").is_err());
        assert!(t("0").is_err());
        assert!(t("99999").is_err());
        assert!(t("ftp://host:21").is_err());
        assert!(t("http://:8090").is_err());
        assert!(t("http://host:8090/?a=1").is_err());
    }

    #[test]
    fn key_is_kept_but_never_shown() {
        let t = EmbedTarget::parse("host:8090", " secret ").unwrap();
        assert_eq!(t.key.as_deref(), Some("secret"));
        assert!(!format!("{t} {t:?}").contains("secret"));
        assert_eq!(EmbedTarget::parse("host:8090", "  ").unwrap().key, None);
    }

    #[test]
    fn loopback_or_not() {
        assert!(EmbedTarget::local(8090).is_loopback());
        assert!(EmbedTarget::parse("localhost:1", "").unwrap().is_loopback());
        assert!(EmbedTarget::parse("[::1]:1", "").unwrap().is_loopback());
        assert!(!EmbedTarget::parse("192.168.0.10:8090", "").unwrap().is_loopback());
    }
}
