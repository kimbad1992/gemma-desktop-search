//! llama-server `/v1/embeddings` 클라이언트.
//!
//! 실측(b12201)으로 확인한 요청 형식:
//! - 텍스트: `"input": ["..."]`
//! - 이미지: `"input": [{"content":[{"type":"image_url","image_url":{"url":"data:image/jpeg;base64,..."}}]}]`
//! - 오디오: `"input": [{"content":[{"type":"input_audio","input_audio":{"data":"<b64 wav>","format":"wav"}}]}]`
//! 응답은 `data[i].index`로 순서를 맞춘다.

pub use super::target::EmbedTarget;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fmt;
use std::time::Duration;

#[derive(Clone)]
pub enum EmbedInput {
    Text(String),
    /// JPEG 바이트
    Image(Vec<u8>),
    /// 16kHz mono 16-bit WAV 바이트
    Audio(Vec<u8>),
    /// 영상 구간 (mp4 바이트). 서버가 ffmpeg로 디코딩해 1fps로 샘플링하고 한 벡터로 임베딩
    Video(Vec<u8>),
}

impl EmbedInput {
    /// llama-server `/v1/embeddings`의 `input` 항목 하나 (빌려주는 기기에 일감을 보낼 때도 이 모양 그대로 보낸다)
    pub fn to_json(&self) -> Value {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        match self {
            EmbedInput::Text(s) => Value::String(s.clone()),
            EmbedInput::Image(jpeg) => json!({
                "content": [{
                    "type": "image_url",
                    "image_url": { "url": format!("data:image/jpeg;base64,{}", b64(jpeg)) }
                }]
            }),
            EmbedInput::Audio(wav) => json!({
                "content": [{
                    "type": "input_audio",
                    "input_audio": { "data": b64(wav), "format": "wav" }
                }]
            }),
            EmbedInput::Video(mp4) => json!({
                "content": [{
                    "type": "input_video",
                    "input_video": { "data": format!("data:video/mp4;base64,{}", b64(mp4)) }
                }]
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub enum EmbedError {
    /// 서버에 연결할 수 없음 → 인덱싱 중단 사유
    Transport(String),
    /// 서버가 요청을 거부/실패 (입력별 문제일 수 있음)
    Server(String),
    /// 응답 형식 이상, NaN 등
    Invalid(String),
}

impl fmt::Display for EmbedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmbedError::Transport(m) => write!(f, "서버 연결 실패: {m}"),
            EmbedError::Server(m) => write!(f, "서버 오류: {m}"),
            EmbedError::Invalid(m) => write!(f, "잘못된 임베딩: {m}"),
        }
    }
}

impl std::error::Error for EmbedError {}

impl From<ureq::Error> for EmbedError {
    fn from(e: ureq::Error) -> Self {
        match e {
            ureq::Error::ConnectionFailed | ureq::Error::HostNotFound | ureq::Error::Io(_) => {
                EmbedError::Transport(e.to_string())
            }
            ureq::Error::Timeout(_) => EmbedError::Server(format!("시간 초과: {e}")),
            other => EmbedError::Server(other.to_string()),
        }
    }
}

#[derive(Deserialize)]
struct EmbResponse {
    data: Vec<EmbItem>,
}

#[derive(Deserialize)]
struct EmbItem {
    index: usize,
    /// NaN이 null로 오는 경우까지 잡기 위해 Option
    embedding: Vec<Option<f32>>,
}

#[derive(Deserialize)]
struct TokResponse {
    tokens: Vec<Value>,
}

#[derive(Deserialize)]
struct PropsResponse {
    #[serde(default)]
    build_info: String,
    #[serde(default)]
    model_path: String,
    #[serde(default)]
    total_slots: u32,
    #[serde(default)]
    default_generation_settings: Option<SlotSettings>,
}

#[derive(Deserialize)]
struct SlotSettings {
    #[serde(default)]
    n_ctx: u32,
}

/// `GET /props`에서 쓰는 것
#[derive(Debug, Clone, Default)]
pub struct ServerProps {
    /// llama.cpp 빌드 (예: b12201-a2b301831)
    pub build: String,
    /// 불러온 모델 파일 이름 (소문자)
    pub model: String,
    /// 슬롯 수 (-np)
    pub slots: u32,
    /// 슬롯 하나의 컨텍스트 (-c를 슬롯 수로 나눈 값)
    pub slot_ctx: u32,
}

impl ServerProps {
    /// 인덱스에 남기는 서버 서명: llama.cpp 빌드와 모델 파일. 같은 빌드면 CUDA·Vulkan처럼 백엔드가 달라도 같다.
    /// 이 기기 서버, 외부 서버, 빌려주는 기기가 모두 이 모양을 쓰므로 서로 섞어 계산할 수 있다.
    pub fn signature(&self) -> String {
        let or_q = |s: &str| if s.is_empty() { "?".to_string() } else { s.to_string() };
        format!("llama:{}|{}", or_q(&self.build), or_q(&self.model))
    }
}

#[derive(Clone)]
pub struct EmbedClient {
    agent: ureq::Agent,
    quick: ureq::Agent,
    base: String,
    /// "Bearer <키>" (llama-server를 --api-key로 켠 경우)
    auth: Option<String>,
}

impl EmbedClient {
    pub fn new(target: &EmbedTarget) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .build()
            .into();
        let quick: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(1500)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            quick,
            base: target.base.clone(),
            auth: target.key.as_ref().map(|k| format!("Bearer {k}")),
        }
    }

    fn get(&self, agent: &ureq::Agent, path: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        let r = agent.get(&format!("{}{path}", self.base));
        match &self.auth {
            Some(a) => r.header("Authorization", a),
            None => r,
        }
    }

    fn post(&self, path: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let r = self.agent.post(&format!("{}{path}", self.base));
        match &self.auth {
            Some(a) => r.header("Authorization", a),
            None => r,
        }
    }

    /// GET /health 상태 코드 (200 준비, 503 로딩)
    pub fn health(&self) -> Result<u16, EmbedError> {
        let resp = self.get(&self.quick, "/health").call()?;
        Ok(resp.status().as_u16())
    }

    /// GET /props: 빌드, 모델, 슬롯. 다른 곳의 서버를 쓸 때 인덱스에 기록할 서명과 입력 크기 확인에 쓴다.
    /// 접속 키가 틀렸으면 401이 온다.
    pub fn props(&self) -> Result<ServerProps, EmbedError> {
        let mut resp = self.get(&self.quick, "/props").call()?;
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(EmbedError::Server(format!("props HTTP {status}: {}", error_message(&text))));
        }
        let p: PropsResponse = resp.body_mut().read_json().map_err(|e| EmbedError::Invalid(e.to_string()))?;
        let model = p
            .model_path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_lowercase();
        Ok(ServerProps {
            build: p.build_info,
            model,
            slots: p.total_slots,
            slot_ctx: p.default_generation_settings.map(|s| s.n_ctx).unwrap_or(0),
        })
    }

    /// 여러 입력을 한 요청으로 임베딩. 결과는 L2 정규화된 벡터, 입력 순서대로.
    pub fn embed(&self, inputs: &[EmbedInput]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let values: Vec<Value> = inputs.iter().map(EmbedInput::to_json).collect();
        self.embed_values(&values)
    }

    /// `embed`와 같고, 입력을 이미 llama-server 모양(`EmbedInput::to_json`)으로 받는다.
    /// 빌려주는 기기는 소유 기기가 보낸 이 모양을 그대로 자기 서버에 넘긴다.
    pub fn embed_values(&self, inputs: &[Value]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({
            "input": inputs,
            "encoding_format": "float",
        });
        let mut resp = self.post("/v1/embeddings").send_json(&body)?;
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(EmbedError::Server(format!(
                "HTTP {status}: {}",
                error_message(&text)
            )));
        }
        let parsed: EmbResponse = resp
            .body_mut()
            .with_config()
            .limit(512 * 1024 * 1024)
            .read_json()
            .map_err(|e| EmbedError::Invalid(format!("응답 파싱 실패: {e}")))?;

        let mut out: Vec<Option<Vec<f32>>> = vec![None; inputs.len()];
        for item in parsed.data {
            if item.index >= out.len() {
                return Err(EmbedError::Invalid(format!("잘못된 index {}", item.index)));
            }
            let mut v = Vec::with_capacity(item.embedding.len());
            for x in item.embedding {
                match x {
                    Some(x) if x.is_finite() => v.push(x),
                    _ => {
                        return Err(EmbedError::Invalid(
                            "NaN/무한대 값 포함 (fp16 문제일 수 있음: -ngl 0 으로 비교해 보세요)".into(),
                        ))
                    }
                }
            }
            if v.is_empty() {
                return Err(EmbedError::Invalid("빈 벡터".into()));
            }
            super::vecmath::normalize(&mut v);
            out[item.index] = Some(v);
        }
        let dim = out.iter().flatten().next().map(|v| v.len()).unwrap_or(0);
        out.into_iter()
            .enumerate()
            .map(|(i, v)| match v {
                Some(v) if v.len() == dim => Ok(v),
                Some(_) => Err(EmbedError::Invalid("차원 불일치".into())),
                None => Err(EmbedError::Invalid(format!("입력 {i}의 결과 없음"))),
            })
            .collect()
    }

    /// POST /tokenize 로 토큰 수 확인
    pub fn count_tokens(&self, text: &str) -> Result<usize, EmbedError> {
        let mut resp = self
            .post("/tokenize")
            .send_json(&json!({ "content": text, "add_special": true }))?;
        let status = resp.status().as_u16();
        if status != 200 {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(EmbedError::Server(format!("tokenize HTTP {status}: {}", error_message(&text))));
        }
        let t: TokResponse = resp
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_json()
            .map_err(|e| EmbedError::Invalid(e.to_string()))?;
        Ok(t.tokens.len())
    }
}

fn error_message(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        if let Some(m) = v.pointer("/error/message").and_then(Value::as_str) {
            return m.to_string();
        }
    }
    crate::util::truncate_chars(body.trim(), 300)
}
