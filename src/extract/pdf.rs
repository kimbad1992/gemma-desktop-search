//! PDF 텍스트 추출. 파서가 멈추거나 panic해도 워커가 묶이지 않도록
//! 전용 스레드에서 돌리고 시간 초과 시 그 스레드를 버린다.

use anyhow::{bail, Result};
use std::path::Path;
use std::time::Duration;

pub fn extract_pdf(path: &Path, timeout: Duration) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("pdf-extract".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pdf_extract::extract_text_from_mem(&bytes)
            }));
            let _ = tx.send(r);
        })?;
    match rx.recv_timeout(timeout) {
        Ok(Ok(Ok(text))) => Ok(text),
        Ok(Ok(Err(e))) => bail!("PDF 텍스트 추출 실패: {e}"),
        Ok(Err(_)) => bail!("PDF 구조를 해석하지 못했습니다 (파서 내부 오류)"),
        Err(_) => bail!("PDF 추출 시간 초과 ({}초)", timeout.as_secs()),
    }
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
}
