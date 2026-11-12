//! 움직이는 내용 위에 뜨는 유리(펼침 메뉴)의 진짜 배경 흐림.
//!
//! 바탕(벽) 위의 유리는 미리 흐린 벽 텍스처를 잘라 깔면 되지만(`ui::glass`), 결과 목록 위에 펼친
//! 메뉴 밑에는 글자와 그림이 지나간다. egui에는 이미 칠한 화면을 읽어 흐리는 단계가 없으므로,
//! 한 프레임의 그림이 다 모인 뒤(`App::ui` 끝) 유리 밑에 있는 층들의 도형을 따로 한 번 더 그린다:
//! 절반 해상도로 그리기 → 1/4로 줄이기 → 가로·세로 가우스 흐림(+채도) → egui 텍스처.
//! 이 GPU 작업은 화면 그리기보다 먼저 제출되므로, 유리는 같은 프레임에 그 텍스처를 자기 자리만큼
//! 잘라 깐다 (한 프레임 늦는 일이 없다).
//!
//! egui가 보장하지 않는 동작에 기댄다: `ctx.graphics()`로 층의 도형 읽기, `layer_ids()`의 순서,
//! eframe보다 먼저 공유 렌더러의 `update_buffers` 부르기. egui 판을 올리면 펼침 메뉴가 흐리게
//! 보이는지 확인한다. GPU 오류가 나면 흐림을 끄고 진한 색으로 칠한다(`ui::glass::show_floating`).
//! 2026-11-11 시험: RTX 4090, 1280×860에서 GPU 약 0.13ms(Vulkan), CPU 약 0.3ms.

use eframe::egui::{self, epaint::ClippedShape, Color32, LayerId, Rect, Vec2};
use eframe::{egui_wgpu, wgpu};
use std::sync::{Mutex, OnceLock};

/// 흐림 세기 (CSS `blur(22px)`의 표준편차, 점 단위)
const BLUR_SIGMA_PT: f32 = 22.0;
/// 흐린 뒤 채도 (CSS `saturate(1.8)`)
const SATURATION: f32 = 1.8;
/// 한 프레임에 서로 다른 층 위에 놓일 수 있는 유리 수 (층마다 흐림 텍스처 하나)
const SLOTS: usize = 4;

// ── 화면 쪽: 이번 프레임에 흐림이 필요한 층 ──

struct Request {
    viewport: egui::ViewportId,
    frame: u64,
    layer: LayerId,
}

static REQUESTS: Mutex<Vec<Request>> = Mutex::new(Vec::new());
/// 층마다 쓰는 흐림 텍스처 (앱이 시작할 때 `Blur::new`가 등록한다. 없으면 흐리지 않는다)
static TEXTURES: OnceLock<Vec<egui::TextureId>> = OnceLock::new();
/// GPU 오류로 흐림을 껐는지
static BROKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `layer` 층에 놓일 유리 `rect` 밑을 흐린 텍스처와 그 UV. 쓸 수 없으면 None.
/// 같은 층의 유리끼리는 텍스처 하나를 나눠 쓴다.
pub fn live(ctx: &egui::Context, layer: LayerId, rect: Rect) -> Option<(egui::TextureId, Rect)> {
    if BROKEN.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let textures = TEXTURES.get()?;
    let (viewport, frame) = (ctx.viewport_id(), ctx.cumulative_frame_nr());
    let slot = {
        let mut list = REQUESTS.lock().ok()?;
        list.retain(|r| r.viewport != viewport || r.frame == frame);
        let mine: Vec<LayerId> = list.iter().filter(|r| r.viewport == viewport).map(|r| r.layer).collect();
        match mine.iter().position(|l| *l == layer) {
            Some(i) => i,
            None => {
                list.push(Request { viewport, frame, layer });
                mine.len()
            }
        }
    };
    let tex = *textures.get(slot)?;
    let cover = Sizes::of(ctx).cover;
    let o = ctx.viewport_rect().min;
    let uv = Rect::from_min_max(((rect.min - o) / cover).to_pos2(), ((rect.max - o) / cover).to_pos2());
    Some((tex, uv))
}

/// 흐림 텍스처의 크기와, 그 텍스처가 덮는 화면 넓이(점)
struct Sizes {
    ppp: f32,
    half: [u32; 2],
    quarter: [u32; 2],
    cover: Vec2,
}

impl Sizes {
    fn of(ctx: &egui::Context) -> Self {
        let ppp = ctx.pixels_per_point();
        let screen = ctx.viewport_rect();
        let full = [
            (screen.width() * ppp).round().max(1.0) as u32,
            (screen.height() * ppp).round().max(1.0) as u32,
        ];
        let half = [full[0].div_ceil(2), full[1].div_ceil(2)];
        let quarter = [half[0].div_ceil(2), half[1].div_ceil(2)];
        let cover = Vec2::new(half[0] as f32 * 2.0 / ppp, half[1] as f32 * 2.0 / ppp);
        Self { ppp, half, quarter, cover }
    }
}

// ── GPU 쪽: 밑의 층을 다시 그리고 흐리기 ──

pub struct Blur {
    pipe_down: wgpu::RenderPipeline,
    pipe_blur: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    slots: Vec<Slot>,
}

struct Slot {
    tex_id: egui::TextureId,
    /// 지금 텍스처를 만든 기준 (절반 해상도 크기)
    half: [u32; 2],
    targets: Option<Targets>,
    /// 줄이기 · 가로 · 세로 단계의 값
    params: [wgpu::Buffer; 3],
}

struct Targets {
    _textures: Vec<wgpu::Texture>,
    half: wgpu::TextureView,
    q: [wgpu::TextureView; 3],
    bind: [wgpu::BindGroup; 3],
}

const OUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Blur {
    pub fn new(rs: &egui_wgpu::RenderState) -> Self {
        let device = &rs.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glass_blur"),
            source: wgpu::ShaderSource::Wgsl(include_str!("backdrop.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glass_blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(32),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glass_blur"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: OUT_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipe_down = pipeline("fs_down");
        let pipe_blur = pipeline("fs_blur");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glass_blur"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        // 유리가 첫 프레임부터 텍스처 번호를 쓸 수 있게, 1×1 자리 텍스처로 미리 등록해 둔다
        let mut slots = Vec::new();
        for _ in 0..SLOTS {
            let placeholder = device.create_texture(&texture_desc([1, 1], OUT_FORMAT));
            let view = placeholder.create_view(&Default::default());
            let tex_id = rs
                .renderer
                .write()
                .register_native_texture(device, &view, wgpu::FilterMode::Linear);
            let buffer = || {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("glass_blur_params"),
                    size: 32,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            slots.push(Slot {
                tex_id,
                half: [0, 0],
                targets: None,
                params: [buffer(), buffer(), buffer()],
            });
        }
        let _ = TEXTURES.set(slots.iter().map(|s| s.tex_id).collect());
        Self {
            pipe_down,
            pipe_blur,
            layout,
            sampler,
            slots,
        }
    }

    /// 프레임의 맨 끝(모든 그림이 모인 뒤)에 부른다. 이번 프레임에 흐림을 청한 유리가 없으면 아무 일도 하지 않는다.
    pub fn run(&mut self, ctx: &egui::Context, rs: &egui_wgpu::RenderState) {
        let (viewport, frame) = (ctx.viewport_id(), ctx.cumulative_frame_nr());
        let layers: Vec<LayerId> = {
            let Ok(mut list) = REQUESTS.lock() else { return };
            let mine = list
                .iter()
                .filter(|r| r.viewport == viewport && r.frame == frame)
                .map(|r| r.layer)
                .collect();
            list.retain(|r| r.viewport != viewport);
            mine
        };
        if layers.is_empty() || BROKEN.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let sizes = Sizes::of(ctx);

        // egui가 화면에 내보낼 때와 같은 순서: Order 순, 그 안에서는 층 순서
        let mut order: Vec<LayerId> = ctx.memory(|m| m.layer_ids().collect());
        if !order.contains(&LayerId::background()) {
            order.insert(0, LayerId::background());
        }
        order.sort_by_key(|l| l.order as usize);

        let bg = ctx.global_style().visuals.panel_fill;
        for (slot, layer) in layers.iter().enumerate().take(SLOTS) {
            let Some(pos) = order.iter().position(|l| l == layer) else {
                continue; // 이번 프레임에 처음 생긴 층: 다음 프레임부터
            };
            let mut shapes: Vec<ClippedShape> = Vec::new();
            for l in &order[..pos] {
                let mut list: Vec<ClippedShape> = ctx.graphics(|g| {
                    g.get(*l)
                        .map(|p| p.all_entries().cloned().collect())
                        .unwrap_or_default()
                });
                if let Some(t) = ctx.layer_transform_to_global(*l) {
                    for s in &mut list {
                        s.transform(t);
                    }
                }
                shapes.extend(list);
            }
            let prims = ctx.tessellate(shapes, sizes.ppp);

            self.ensure(rs, slot, &sizes);
            let scope = rs.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut enc = rs.device.create_command_encoder(&Default::default());
            let extra = self.encode(rs, slot, &sizes, &prims, bg, &mut enc);
            // 층마다 따로 제출한다: 공유 렌더러의 정점 버퍼를 다음 층이 덮어쓰기 전에 이 층이 쓰도록
            rs.queue.submit(extra.into_iter().chain([enc.finish()]));
            if let Some(Some(err)) = poll_now(scope.pop()) {
                eprintln!("펼침 메뉴의 흐림을 끕니다 (GPU 오류): {err}");
                BROKEN.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    }

    /// 밑의 층 그리기 + 흐림 세 단계를 `enc`에 넣는다
    fn encode(
        &self,
        rs: &egui_wgpu::RenderState,
        slot: usize,
        sizes: &Sizes,
        prims: &[egui::ClippedPrimitive],
        bg: Color32,
        enc: &mut wgpu::CommandEncoder,
    ) -> Vec<wgpu::CommandBuffer> {
        let s = &self.slots[slot];
        let Some(tg) = &s.targets else { return Vec::new() };
        let desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: sizes.half,
            pixels_per_point: sizes.ppp * 0.5,
        };
        let extra = rs.renderer.write().update_buffers(&rs.device, &rs.queue, enc, prims, &desc);
        let srgb = rs.target_format.is_srgb();
        let clear = if srgb {
            let l = egui::Rgba::from(bg);
            wgpu::Color {
                r: l.r() as f64,
                g: l.g() as f64,
                b: l.b() as f64,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: bg.r() as f64 / 255.0,
                g: bg.g() as f64 / 255.0,
                b: bg.b() as f64 / 255.0,
                a: 1.0,
            }
        };
        let sigma = BLUR_SIGMA_PT * sizes.ppp / 4.0;
        let radius = (sigma * 3.0).ceil().min(64.0);
        let qt = [1.0 / sizes.quarter[0] as f32, 1.0 / sizes.quarter[1] as f32];
        let ht = [1.0 / sizes.half[0] as f32, 1.0 / sizes.half[1] as f32];
        let to_gamma = if srgb { 1.0 } else { 0.0 };
        let params: [[f32; 8]; 3] = [
            [ht[0], ht[1], 0.0, 0.0, 0.0, 0.0, 1.0, to_gamma],
            [qt[0], qt[1], 1.0, 0.0, sigma, radius, 1.0, 0.0],
            [qt[0], qt[1], 0.0, 1.0, sigma, radius, SATURATION, 0.0],
        ];
        for (buf, p) in s.params.iter().zip(params) {
            let mut bytes = [0u8; 32];
            for (i, v) in p.iter().enumerate() {
                bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            rs.queue.write_buffer(buf, 0, &bytes);
        }
        let renderer = rs.renderer.read();
        {
            let pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glass_source"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &tg.half,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.render(&mut pass.forget_lifetime(), prims, &desc);
        }
        for (i, pipe) in [&self.pipe_down, &self.pipe_blur, &self.pipe_blur].into_iter().enumerate() {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glass_blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &tg.q[i],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &tg.bind[i], &[]);
            pass.draw(0..3, 0..1);
        }
        extra
    }

    /// 창 크기가 바뀌면 텍스처를 새로 만들고, egui 텍스처 번호가 새 텍스처를 가리키게 한다
    fn ensure(&mut self, rs: &egui_wgpu::RenderState, slot: usize, sizes: &Sizes) {
        let s = &self.slots[slot];
        if s.targets.is_some() && s.half == sizes.half {
            return;
        }
        let device = &rs.device;
        let half_tex = device.create_texture(&texture_desc(sizes.half, rs.target_format));
        let q_tex: Vec<wgpu::Texture> = (0..3)
            .map(|_| device.create_texture(&texture_desc(sizes.quarter, OUT_FORMAT)))
            .collect();
        let half = half_tex.create_view(&Default::default());
        let q: [wgpu::TextureView; 3] = std::array::from_fn(|i| q_tex[i].create_view(&Default::default()));
        // 단계마다 읽는 쪽: 절반 → q0, q0 → q1, q1 → q2
        let sources = [&half, &q[0], &q[1]];
        let bind: [wgpu::BindGroup; 3] = std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("glass_blur"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(sources[i]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: s.params[i].as_entire_binding(),
                    },
                ],
            })
        });
        rs.renderer
            .write()
            .update_egui_texture_from_wgpu_texture(device, &q[2], wgpu::FilterMode::Linear, s.tex_id);
        let mut textures = vec![half_tex];
        textures.extend(q_tex);
        let s = &mut self.slots[slot];
        s.half = sizes.half;
        s.targets = Some(Targets {
            _textures: textures,
            half,
            q,
            bind,
        });
    }
}

fn texture_desc(size: [u32; 2], format: wgpu::TextureFormat) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("glass_blur"),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    }
}

/// 이미 끝나 있을 결과를 기다리지 않고 꺼낸다 (wgpu의 오류 범위는 네이티브에서 바로 준비된다)
fn poll_now<F: std::future::Future>(f: F) -> Option<F::Output> {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match f.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(v) => Some(v),
        std::task::Poll::Pending => None,
    }
}
