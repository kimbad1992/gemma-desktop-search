//! 허브 보정용 기준 질의 묶음.
//!
//! 교차 모달 검색에서는 어떤 질의에나 비슷하게 잘 걸리는 "허브" 항목(밋밋한 스크린샷, 짧은 설정 파일 등)이 생긴다.
//! 이 질의들로 항목마다 "아무 질의에나 걸리는 정도"를 재서 빼면 허브가 상위를 차지하지 않는다.
//! 실제 검색어와 겹치지 않게, 사람들이 흔히 찾는 여러 주제를 고르게 담았다.

/// 저장된 기준 질의 벡터를 다시 써도 되는지 가르는 값: 질의 묶음·접두사·모델이 같아야 한다
pub fn version(model_signature: &str) -> String {
    let mut h: u64 = 1469598103934665603;
    for s in BANK {
        for b in s.bytes() {
            h = (h ^ b as u64).wrapping_mul(1099511628211);
        }
        h = (h ^ 0xff).wrapping_mul(1099511628211);
    }
    format!("{h:016x}|p{}|{model_signature}", crate::embed::prompt::PROMPT_VERSION)
}

/// 기준 질의를 임베딩해서 저장소에 넣고 DB에 저장 (첫 검색 때 한 번, 1초 안팎)
pub fn ensure(
    client: &crate::embed::client::EmbedClient,
    store: &parking_lot::RwLock<crate::search::store::VectorStore>,
    db: &parking_lot::Mutex<crate::index::db::Db>,
) {
    if store.read().has_bank() {
        return;
    }
    let version = version(&db.lock().meta_get("model_signature").unwrap_or_default());
    let saved = db.lock().load_bank(&version, BANK.len());
    let vectors = match saved {
        Some(v) => v,
        None => {
            let mut out = Vec::with_capacity(BANK.len());
            for chunk in BANK.chunks(16) {
                let inputs: Vec<_> = chunk
                    .iter()
                    .map(|s| crate::embed::client::EmbedInput::Text(crate::embed::prompt::query(s)))
                    .collect();
                match client.embed(&inputs) {
                    Ok(v) => out.extend(v),
                    Err(_) => return, // 다음 검색 때 다시
                }
            }
            let _ = db.lock().save_bank(&version, &out);
            out
        }
    };
    store.write().set_bank(&vectors);
}

/// 저장된 기준 질의가 있으면 저장소에 넣는다 (앱 시작 때, 서버 없이)
pub fn load_saved(db: &crate::index::db::Db, store: &mut crate::search::store::VectorStore) {
    let version = version(&db.meta_get("model_signature").unwrap_or_default());
    if let Some(v) = db.load_bank(&version, BANK.len()) {
        store.set_bank(&v);
    }
}

pub const BANK: &[&str] = &[
    // 사진·장면
    "해변에서 찍은 가족 사진", "산 정상에서 본 풍경", "도시 야경", "강아지가 잔디밭에서 뛰는 모습", "고양이가 자는 모습",
    "결혼식 사진", "졸업식", "아기 돌잔치", "캠핑장 텐트", "벚꽃 길", "단풍", "눈 덮인 마을", "비 오는 거리", "일몰",
    "자동차 실내", "음식점 메뉴판", "라면", "커피와 디저트", "회식 자리", "놀이공원", "수영장", "등산하는 사람들",
    "운동회", "콘서트 무대", "공항 탑승구", "기차 창밖", "바닷가 일출", "강아지 산책", "꽃다발", "크리스마스 트리",
    "a person riding a bicycle", "a city street at night", "a plate of pasta", "children playing soccer",
    "a mountain lake", "a group photo of friends", "a cat on a sofa", "fireworks over a river", "a baby smiling",
    // 스크린샷·문서
    "로그인 화면 캡처", "쇼핑몰 주문 내역", "택배 배송 조회", "은행 이체 확인", "카카오톡 대화 캡처", "지도 길찾기 화면",
    "게임 화면", "유튜브 영상 캡처", "엑셀 표", "프로그램 오류 메시지", "설정 화면", "영수증 사진", "명함",
    "a chart showing sales growth", "an error dialog box", "a web page screenshot", "a spreadsheet table",
    "이력서", "자기소개서", "보고서 초안", "회의 안건", "여행 일정표", "가계부", "임대차 계약", "보험 약관", "건강검진 결과",
    "수업 필기", "시험 문제", "논문 요약", "제품 사용 설명서", "레시피", "소설의 한 장면", "시", "일기",
    "project proposal", "meeting notes", "user manual", "invoice", "tax return", "research paper abstract",
    "python function that parses json", "sql query", "html page layout", "configuration file",
    // 소리·영상
    "음악 연주", "노래 부르는 소리", "전화 통화 녹음", "강의 녹음", "아이 웃음소리", "빗소리", "박수 소리", "개 짖는 소리",
    "뉴스 앵커가 말하는 장면", "요리하는 영상", "춤추는 영상", "축구 경기 하이라이트", "드론 촬영 영상", "브이로그",
    "a podcast about technology", "a piano performance", "people talking in a cafe", "a dog barking",
    "a video of waves crashing", "a cooking tutorial", "a wedding video", "a birthday party video",
    // 일반·추상
    "행복한 순간", "중요한 서류", "옛날 사진", "여름 휴가", "가족 모임", "회사 일", "공부 자료", "취미 생활",
    "something funny", "important document", "old memories", "work stuff", "travel", "family",
];
