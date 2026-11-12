// 움직이는 내용 위에 뜨는 유리의 배경 흐림 (src/ui/backdrop.rs)
// 화면 전체를 덮는 삼각형 하나로 원본 텍스처를 읽어 출력 텍스처를 채운다.

struct Params {
    // 원본 텍셀 하나의 크기 (1 / 원본 크기)
    src_texel: vec2<f32>,
    // 흐림 방향: (1, 0) 가로, (0, 1) 세로
    dir: vec2<f32>,
    // 가우스 표준편차와 반경 (원본 텍셀 단위)
    sigma: f32,
    radius: f32,
    // 채도 배율 (CSS saturate). 1이면 그대로
    saturation: f32,
    // 1이면 원본 값이 선형(sRGB 화면 형식)이라 감마로 바꿔 쓴다
    to_gamma: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> p: Params;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> VsOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VsOut;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

fn gamma_from_linear(rgb: vec3<f32>) -> vec3<f32> {
    let cutoff = rgb < vec3<f32>(0.0031308);
    let lower = rgb * vec3<f32>(12.92);
    let higher = vec3<f32>(1.055) * pow(rgb, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(higher, lower, cutoff);
}

// 반으로 줄이기: 출력 픽셀의 중심이 원본 2×2 텍셀이 만나는 꼭짓점이라, 쌍선형 한 번이 넷의 평균이다
@fragment
fn fs_down(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(src, samp, in.uv, 0.0);
    if p.to_gamma > 0.5 {
        c = vec4<f32>(gamma_from_linear(c.rgb), c.a);
    }
    return vec4<f32>(c.rgb, 1.0);
}

// 한 방향 가우스 흐림. 마지막(세로) 단계에서 채도를 올린다.
@fragment
fn fs_blur(in: VsOut) -> @location(0) vec4<f32> {
    let r = i32(p.radius);
    let two_s2 = 2.0 * p.sigma * p.sigma;
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var k = -r; k <= r; k = k + 1) {
        let x = f32(k);
        let w = exp(-(x * x) / two_s2);
        acc = acc + w * textureSampleLevel(src, samp, in.uv + p.dir * p.src_texel * x, 0.0).rgb;
        wsum = wsum + w;
    }
    var c = acc / wsum;
    let l = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = clamp(mix(vec3<f32>(l), c, p.saturation), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(c, 1.0);
}
