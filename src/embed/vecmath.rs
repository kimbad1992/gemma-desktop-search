//! 벡터 연산: 정규화, 내적, Matryoshka 절단, 바이트 직렬화

pub fn norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

pub fn normalize(v: &mut [f32]) {
    let n = norm(v);
    if n > 0.0 && n.is_finite() {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    // 컴파일러 자동 벡터화가 잘 되도록 단순 루프
    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    let mut s3 = 0.0f32;
    let n = a.len().min(b.len());
    let chunks = n / 4;
    for i in 0..chunks {
        let j = i * 4;
        s0 += a[j] * b[j];
        s1 += a[j + 1] * b[j + 1];
        s2 += a[j + 2] * b[j + 2];
        s3 += a[j + 3] * b[j + 3];
    }
    for j in chunks * 4..n {
        s0 += a[j] * b[j];
    }
    s0 + s1 + s2 + s3
}

/// 앞쪽 `dim`개만 남기고 다시 L2 정규화 (잘라낸 단위 벡터는 길이가 1이 아니므로)
pub fn truncate_normalized(v: &[f32], dim: usize) -> Vec<f32> {
    let mut t = v[..dim.min(v.len())].to_vec();
    normalize(&mut t);
    t
}

pub fn to_bytes(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

pub fn from_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// 절반 정밀도(FP16) 저장. 정규화된 벡터(성분 -1~1)는 상대 오차 약 0.05%로,
/// 코사인 점수 변화가 0.0005 미만이다.
pub fn to_f16_bytes(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 2);
    for x in v {
        b.extend_from_slice(&half::f16::from_f32(*x).to_le_bytes());
    }
    b
}

pub fn from_f16_bytes(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(2)
        .map(|c| half::f16::from_le_bytes([c[0], c[1]]).to_f32())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_renormalizes() {
        let mut v: Vec<f32> = (1..=768).map(|i| (i as f32).sin()).collect();
        normalize(&mut v);
        assert!((norm(&v) - 1.0).abs() < 1e-5);
        let t = truncate_normalized(&v, 256);
        assert_eq!(t.len(), 256);
        assert!((norm(&t) - 1.0).abs() < 1e-5, "절단 후 재정규화");
        // 잘린 벡터는 재정규화 전에는 길이가 1보다 작다
        assert!(norm(&v[..256]) < 0.99);
    }

    #[test]
    fn bytes_roundtrip() {
        let v = vec![0.5f32, -1.25, 3.0e-7, f32::MAX];
        assert_eq!(from_bytes(&to_bytes(&v)), v);
    }

    #[test]
    fn f16_storage_keeps_scores() {
        // 실제 임베딩처럼 정규화된 768차원 벡터 여러 개로 점수 오차 확인
        let mk = |seed: u32| {
            let mut s = seed;
            let mut v: Vec<f32> = (0..768)
                .map(|_| {
                    s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((s >> 8) as f32 / (1u32 << 24) as f32) - 0.5
                })
                .collect();
            normalize(&mut v);
            v
        };
        let q = mk(1);
        let mut max_err = 0f32;
        for seed in 2..200 {
            let d = mk(seed);
            let d16 = from_f16_bytes(&to_f16_bytes(&d));
            assert_eq!(to_f16_bytes(&d).len(), 768 * 2);
            max_err = max_err.max((dot(&q, &d) - dot(&q, &d16)).abs());
        }
        assert!(max_err < 5e-4, "FP16 점수 오차 {max_err}");
    }

    #[test]
    fn dot_matches_naive() {
        let a: Vec<f32> = (0..771).map(|i| i as f32 * 0.01).collect();
        let b: Vec<f32> = (0..771).map(|i| (i as f32 * 0.3).cos()).collect();
        let naive: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        assert!((dot(&a, &b) - naive).abs() < 1e-2);
    }
}
