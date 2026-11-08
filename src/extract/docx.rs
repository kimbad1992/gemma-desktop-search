//! DOCX 텍스트 추출 (word/document.xml의 <w:t> 내용)

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

pub fn extract_docx(path: &Path) -> Result<String> {
    let f = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(f)).context("DOCX(zip) 열기 실패")?;
    let mut xml = String::new();
    zip.by_name("word/document.xml")
        .context("word/document.xml 없음")?
        .read_to_string(&mut xml)?;
    Ok(xml_to_text(&xml))
}

fn xml_to_text(xml: &str) -> String {
    let mut out = String::new();
    let mut in_text = false;
    let mut i = 0usize;
    while i < xml.len() {
        let rest = &xml[i..];
        if rest.starts_with('<') {
            let Some(end) = rest.find('>') else { break };
            let tag = &rest[1..end];
            let closing = tag.starts_with('/');
            let self_closing = tag.ends_with('/');
            let name = tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            match name {
                "w:t" => in_text = !closing && !self_closing,
                "w:tab" if !closing => out.push('\t'),
                "w:br" | "w:cr" if !closing => out.push('\n'),
                "w:p" if closing => out.push('\n'),
                _ => {}
            }
            i += end + 1;
        } else {
            let next = rest.find('<').unwrap_or(rest.len());
            if in_text {
                out.push_str(&unescape(&rest[..next]));
            }
            i += next;
        }
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(semi) = rest.find(';') else {
            break;
        };
        let ent = &rest[1..semi];
        let rep = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match rep {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_runs_and_paragraphs() {
        let xml = r#"<w:document><w:body><w:p><w:r><w:t>안녕</w:t></w:r><w:r><w:t xml:space="preserve"> 세상 &amp; 우주</w:t></w:r></w:p><w:p><w:r><w:tab/><w:t>둘째&#44608;</w:t></w:r></w:p></w:body></w:document>"#;
        let t = xml_to_text(xml);
        assert_eq!(t, "안녕 세상 & 우주\n\t둘째김\n");
    }
}
