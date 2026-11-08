//! 텍스트 디코딩(UTF-8/UTF-16/CP949 등 자동 판별)과 청크 분할

/// 바이트를 문자열로. 바이너리로 보이면 None.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Some(String::from_utf8_lossy(&bytes[3..]).into_owned());
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(bytes);
        return Some(s.into_owned());
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(bytes);
        return Some(s.into_owned());
    }
    if looks_binary(bytes) {
        return None;
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Some(s.to_string());
    }
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(bytes, true);
    let enc = det.guess(Some(b"kr"), chardetng::Utf8Detection::Allow);
    let (s, _, _) = enc.decode(bytes);
    Some(s.into_owned())
}

fn looks_binary(b: &[u8]) -> bool {
    let sample = &b[..b.len().min(16 * 1024)];
    if sample.is_empty() {
        return false;
    }
    if sample.contains(&0) {
        return true;
    }
    let ctrl = sample
        .iter()
        .filter(|&&c| c < 0x09 || (c > 0x0D && c < 0x20 && c != 0x1B))
        .count();
    ctrl * 100 > sample.len() * 3
}

/// 공백 정리: CRLF → LF, 3줄 이상 빈 줄 축약, 줄 끝 공백 제거
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.replace("\r\n", "\n").replace('\r', "\n").lines() {
        let l = line.trim_end();
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank = 0;
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// 약 `chunk_chars`자 단위로 자르되, 가능하면 줄바꿈/문장 끝/공백에서 끊는다.
pub fn chunk_text(text: &str, chunk_chars: usize, overlap: usize, max_chunks: usize) -> Vec<String> {
    let chunk_chars = chunk_chars.max(100);
    let overlap = overlap.min(chunk_chars / 2);
    // 필요한 만큼만 처리 (거대한 로그 파일 대비)
    let limit = chunk_chars.saturating_mul(max_chunks.max(1)).saturating_mul(2);
    let head: String = text.chars().take(limit).collect();
    let text = tidy(&head);
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < len && out.len() < max_chunks {
        let mut end = (start + chunk_chars).min(len);
        if end < len {
            let min_end = start + chunk_chars * 6 / 10;
            let brk = (min_end..end)
                .rev()
                .find(|&i| chars[i] == '\n')
                .or_else(|| {
                    (min_end..end)
                        .rev()
                        .find(|&i| matches!(chars[i], '.' | '!' | '?' | '。' | '！' | '？'))
                })
                .or_else(|| (min_end..end).rev().find(|&i| chars[i].is_whitespace()));
            if let Some(b) = brk {
                end = b + 1;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim();
        if !piece.is_empty() {
            out.push(piece.to_string());
        }
        if end >= len {
            break;
        }
        start = end.saturating_sub(overlap).max(start + 1);
    }
    out
}

/// 청크를 대략 반으로 (토큰 한도 초과 시)
pub fn split_half(s: &str) -> (String, String) {
    let chars: Vec<char> = s.chars().collect();
    let mid = chars.len() / 2;
    let brk = (mid.saturating_sub(mid / 3)..mid + mid / 3)
        .find(|&i| i < chars.len() && chars[i].is_whitespace())
        .unwrap_or(mid);
    (
        chars[..brk].iter().collect::<String>().trim().to_string(),
        chars[brk..].iter().collect::<String>().trim().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_cp949() {
        // "안녕하세요 반갑습니다" in CP949 (EUC-KR)
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("안녕하세요 반갑습니다. 한글 인코딩 판별 테스트입니다. 검색 프로그램");
        assert!(std::str::from_utf8(&bytes).is_err());
        let s = decode_text(&bytes).unwrap();
        assert!(s.contains("안녕하세요"), "{s}");
    }

    #[test]
    fn decodes_utf16_bom() {
        let mut b = vec![0xFF, 0xFE];
        for u in "검색".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&b).unwrap(), "검색");
    }

    #[test]
    fn rejects_binary() {
        let b: Vec<u8> = (0..2000u32).map(|i| (i % 256) as u8).collect();
        assert!(decode_text(&b).is_none());
    }

    #[test]
    fn chunks_cover_text_with_overlap() {
        let text = (0..200)
            .map(|i| format!("문장 번호 {i} 입니다."))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_text(&text, 300, 50, 100);
        assert!(chunks.len() > 5);
        assert!(chunks.iter().all(|c| c.chars().count() <= 300));
        assert!(chunks.last().unwrap().contains("199"));
        // 최대 개수 제한
        assert_eq!(chunk_text(&text, 300, 50, 3).len(), 3);
    }

    #[test]
    fn half_split() {
        let (a, b) = split_half("하나 둘 셋 넷 다섯 여섯");
        assert!(!a.is_empty() && !b.is_empty());
    }
}
