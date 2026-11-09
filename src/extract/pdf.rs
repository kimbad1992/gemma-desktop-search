//! PDF 텍스트 추출. 파서가 멈추거나 panic해도 워커가 묶이지 않도록
//! 전용 스레드에서 돌리고 시간 초과 시 그 스레드를 버린다.

use anyhow::{bail, Result};
use std::path::Path;
use std::time::Duration;

/// 페이지별 텍스트 (1페이지부터 순서대로)
pub fn extract_pdf_pages(path: &Path, timeout: Duration) -> Result<Vec<String>> {
    let bytes = std::fs::read(path)?;
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("pdf-extract".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pdf_extract::extract_text_from_mem_by_pages(&bytes)
            }));
            let _ = tx.send(r);
        })?;
    match rx.recv_timeout(timeout) {
        Ok(Ok(Ok(pages))) => Ok(pages),
        Ok(Ok(Err(e))) => bail!("PDF 텍스트 추출 실패: {e}"),
        Ok(Err(_)) => bail!("PDF 구조를 해석하지 못했습니다 (파서 내부 오류)"),
        Err(_) => bail!("PDF 추출 시간 초과 ({}초)", timeout.as_secs()),
    }
}

/// PDF 청크의 locator: 페이지(1부터) × 1000 + 페이지 안 순번. 예전(페이지 없는) PDF 청크는 1000 미만.
pub const PAGE_LOCATOR: i64 = 1000;

pub fn page_of(locator: i64) -> Option<i64> {
    (locator >= PAGE_LOCATOR).then_some(locator / PAGE_LOCATOR)
}

/// 암호가 걸려 텍스트를 꺼낼 수 없는 PDF인지 (고칠 수 없는 경우라 오류 대신 '이름만'으로 분류)
pub fn is_encrypted_error(e: &anyhow::Error) -> bool {
    let m = e.to_string().to_lowercase();
    m.contains("decrypt") || m.contains("password") || m.contains("encrypt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_encryption_errors() {
        let e = anyhow::anyhow!(
            "PDF 텍스트 추출 실패: PDF error: decryption error: the supplied password is incorrect"
        );
        assert!(is_encrypted_error(&e));
        assert!(!is_encrypted_error(&anyhow::anyhow!("PDF 추출 시간 초과 (90초)")));
    }

    #[test]
    fn page_locators() {
        assert_eq!(page_of(3 * PAGE_LOCATOR + 2), Some(3));
        assert_eq!(page_of(5), None, "예전 청크 번호");
    }
}
