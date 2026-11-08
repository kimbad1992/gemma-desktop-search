//! 서버 준비 직후 모달리티 자가진단.
//! - 텍스트: 차원/유한값/관련 문서가 무관 문서보다 높은지
//! - 이미지: 빨강·파랑 이미지가 서로 다른 벡터인지(텍스트 전용 벡터가 조용히 오는지 감지), 교차 모달 순서, 배치 순서
//! - 오디오: 사인파와 노이즈가 서로 다른 벡터인지

use super::client::{EmbedClient, EmbedInput};
use super::prompt;
use super::vecmath::dot;
use std::time::Instant;

#[derive(Clone, Debug)]
pub enum Check {
    Ok(String),
    Warn(String),
    Fail(String),
    Skipped(String),
}

impl Check {
    pub fn usable(&self) -> bool {
        matches!(self, Check::Ok(_) | Check::Warn(_))
    }
    pub fn detail(&self) -> &str {
        match self {
            Check::Ok(s) | Check::Warn(s) | Check::Fail(s) | Check::Skipped(s) => s,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Capabilities {
    pub dim: usize,
    pub text: Check,
    pub image: Check,
    pub image_batch: bool,
    pub audio: Check,
    pub elapsed_ms: u128,
}

pub fn run(client: &EmbedClient, has_mmproj: bool) -> Capabilities {
    let t0 = Instant::now();
    let (text, dim) = check_text(client);
    let (image, image_batch, audio) = if !text.usable() {
        let s = Check::Skipped("텍스트 진단 실패로 건너뜀".into());
        (s.clone(), false, s)
    } else if !has_mmproj {
        let s = Check::Skipped("mmproj 미지정 (텍스트 전용 모드)".into());
        (s.clone(), false, s)
    } else {
        let (img, batch) = check_image(client);
        (img, batch, check_audio(client))
    };
    Capabilities {
        dim,
        text,
        image,
        image_batch,
        audio,
        elapsed_ms: t0.elapsed().as_millis(),
    }
}

fn check_text(client: &EmbedClient) -> (Check, usize) {
    let inputs = [
        prompt::query("How do I reset my password?"),
        prompt::document("none", "Select Forgot password to receive a reset link by email."),
        prompt::document("none", "The northern lights are caused by solar wind hitting the atmosphere."),
    ]
    .map(EmbedInput::Text);
    match client.embed(&inputs) {
        Err(e) => (Check::Fail(e.to_string()), 0),
        Ok(v) => {
            let dim = v[0].len();
            let rel = dot(&v[0], &v[1]);
            let irr = dot(&v[0], &v[2]);
            let msg = format!("{dim}차원 · 관련 {rel:.3} > 무관 {irr:.3}");
            if dim != 768 {
                (Check::Warn(format!("예상(768)과 다른 차원 — {msg}")), dim)
            } else if rel > irr {
                (Check::Ok(msg), dim)
            } else {
                (Check::Fail(format!("검색 순위가 뒤집힘 — {msg}")), dim)
            }
        }
    }
}

fn solid_jpeg(rgb: [u8; 3]) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(96, 96, image::Rgb(rgb)));
    crate::extract::image::encode_jpeg(&img, 92).unwrap_or_default()
}

fn check_image(client: &EmbedClient) -> (Check, bool) {
    let red = EmbedInput::Image(solid_jpeg([220, 20, 20]));
    let blue = EmbedInput::Image(solid_jpeg([20, 40, 220]));
    let r = client.embed(std::slice::from_ref(&red));
    let b = client.embed(std::slice::from_ref(&blue));
    let (r, b) = match (r, b) {
        (Ok(mut r), Ok(mut b)) => (r.remove(0), b.remove(0)),
        (Err(e), _) | (_, Err(e)) => return (Check::Fail(e.to_string()), false),
    };
    let same = dot(&r, &b);
    if same > 0.99 {
        return (
            Check::Fail(format!(
                "서로 다른 이미지가 같은 벡터(cos {same:.3}) — 서버가 이미지를 무시하고 있습니다"
            )),
            false,
        );
    }
    let q = [
        prompt::query("a solid red square"),
        prompt::query("a solid blue square"),
    ]
    .map(EmbedInput::Text);
    let order_ok;
    let detail;
    match client.embed(&q) {
        Ok(t) => {
            let (rr, rb) = (dot(&r, &t[0]), dot(&r, &t[1]));
            let (br, bb) = (dot(&b, &t[0]), dot(&b, &t[1]));
            order_ok = rr > rb && bb > br;
            detail = format!("빨강↔'red' {rr:.3} / 'blue' {rb:.3} · 서로 cos {same:.3}");
        }
        Err(e) => return (Check::Fail(e.to_string()), false),
    }
    // 한 요청에 여러 장 보내도 순서·내용이 유지되는지
    let batch_ok = match client.embed(&[red, blue]) {
        Ok(v) if v.len() == 2 => dot(&v[0], &r) > 0.99 && dot(&v[1], &b) > 0.99,
        _ => false,
    };
    let detail = format!("{detail} · 배치 {}", if batch_ok { "가능" } else { "불가(1장씩)" });
    if order_ok {
        (Check::Ok(detail), batch_ok)
    } else {
        (Check::Warn(format!("교차 모달 순서 이상 — {detail}")), batch_ok)
    }
}

fn check_audio(client: &EmbedClient) -> Check {
    let rate = 16_000u32;
    let n = rate as usize * 2;
    let mut sine = Vec::with_capacity(n * 2);
    let mut noise = Vec::with_capacity(n * 2);
    let mut seed: u32 = 0x1234_5678;
    for i in 0..n {
        let s = ((i as f32 / rate as f32) * 440.0 * std::f32::consts::TAU).sin() * 12_000.0;
        sine.extend_from_slice(&(s as i16).to_le_bytes());
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let r = ((seed >> 16) as i16 as f32) * 0.25;
        noise.extend_from_slice(&(r as i16).to_le_bytes());
    }
    let a = EmbedInput::Audio(crate::extract::media::wav_from_s16le(&sine, rate));
    let b = EmbedInput::Audio(crate::extract::media::wav_from_s16le(&noise, rate));
    match (
        client.embed(std::slice::from_ref(&a)),
        client.embed(std::slice::from_ref(&b)),
    ) {
        (Ok(x), Ok(y)) => {
            let c = dot(&x[0], &y[0]);
            if c > 0.99 {
                Check::Fail(format!("서로 다른 소리가 같은 벡터(cos {c:.3})"))
            } else {
                Check::Ok(format!("사인파↔노이즈 cos {c:.3} (실험적 기능)"))
            }
        }
        (Err(e), _) | (_, Err(e)) => Check::Fail(e.to_string()),
    }
}
