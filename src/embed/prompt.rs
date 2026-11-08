//! EmbeddingGemma 2 모델 카드의 작업 접두사. 텍스트에만 붙이고, 이미지·오디오에는 붙이지 않는다.

/// 접두사 형식이 바뀌면 올린다 → 기존 인덱스는 재구축 필요로 표시됨
pub const PROMPT_VERSION: u32 = 1;

/// 검색 질의: `task: search result | query: {q}`
pub fn query(q: &str) -> String {
    format!("task: search result | query: {}", q.trim())
}

/// 문서: `title: {title} | text: {content}`
pub fn document(title: &str, text: &str) -> String {
    let title = title.trim();
    let title = if title.is_empty() { "none" } else { title };
    format!("title: {title} | text: {text}")
}
