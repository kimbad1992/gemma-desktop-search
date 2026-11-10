//! 첫 실행 사양 확인: Windows 버전, 그래픽카드(VRAM·드라이버), 메모리, 남은 디스크, ffmpeg
//! 그리고 그에 맞는 llama.cpp 판과 서버 설정 추천.

use crate::download::Pack;
use crate::util::{hidden_command, run_capture};
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

#[derive(Clone, Debug)]
pub struct Gpu {
    pub name: String,
    pub vram_mb: u64,
    /// NVIDIA 드라이버 버전 (예: 621.14)
    pub driver: Option<String>,
    pub vendor: Vendor,
}

#[derive(Clone, Debug, Default)]
pub struct SysInfo {
    pub os: String,
    pub build: u32,
    pub ram_mb: u64,
    pub free_disk_mb: u64,
    pub gpus: Vec<Gpu>,
    pub ffmpeg: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Bad,
}

/// 사양에 맞춘 추천
#[derive(Clone, Debug)]
pub struct Plan {
    pub llama: Pack,
    pub n_gpu_layers: i32,
    pub ctx_size: u32,
    pub parallel: u32,
    pub batch_size: u32,
    pub ubatch_size: u32,
    pub workers: usize,
    /// (수준, 항목, 설명)
    pub checks: Vec<(Level, String, String)>,
    pub summary: String,
    /// 임베딩에 쓰는 장치: 그래픽카드 이름 또는 "CPU"
    pub device: String,
}

impl Plan {
    /// 쉬운 말로 된 한 줄 요약 (일반 설정)
    pub fn plain_summary(&self) -> String {
        if self.n_gpu_layers == 0 {
            "그래픽카드 없이 처리합니다. 사진·영상이 많으면 오래 걸립니다.".to_string()
        } else if self.ubatch_size >= 8192 {
            format!("{} 그래픽카드로 빠르게 처리합니다.", self.device)
        } else {
            format!(
                "{} 그래픽카드로 처리합니다. 그래픽 메모리가 적어 영상은 짧게 나눠 살펴봅니다.",
                self.device
            )
        }
    }
}

fn vendor_of(name: &str) -> Vendor {
    let n = name.to_lowercase();
    if n.contains("nvidia") || n.contains("geforce") || n.contains("rtx") || n.contains("quadro") {
        Vendor::Nvidia
    } else if n.contains("amd") || n.contains("radeon") {
        Vendor::Amd
    } else if n.contains("intel") || n.contains("arc") {
        Vendor::Intel
    } else {
        Vendor::Other
    }
}

/// "621.14" → 621
fn driver_major(v: &str) -> Option<u32> {
    v.split('.').next()?.trim().parse().ok()
}

/// NVIDIA 카드는 nvidia-smi로 VRAM과 드라이버 버전을 정확히
fn nvidia_smi() -> Vec<(String, u64, String)> {
    let mut cmd = hidden_command("nvidia-smi");
    cmd.args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"]);
    let Ok(out) = run_capture(cmd, Duration::from_secs(15)) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            (f.len() == 3).then(|| (f[0].to_string(), f[1].parse().unwrap_or(0), f[2].to_string()))
        })
        .collect()
}

pub fn probe(app_dir: &Path, ffmpeg: bool) -> SysInfo {
    let info = crate::platform::os_and_adapters();
    let mut gpus: Vec<Gpu> = info
        .adapters
        .into_iter()
        .filter_map(|(name, vram_bytes)| {
            let vram_mb = vram_bytes / 1_048_576;
            let vendor = vendor_of(&name);
            // 원격 데스크톱·가상 어댑터 등은 뺀다
            (vendor != Vendor::Other || vram_mb >= 1024).then_some(Gpu {
                name,
                vram_mb,
                driver: None,
                vendor,
            })
        })
        .collect();
    let smi = nvidia_smi();
    if !smi.is_empty() {
        gpus.retain(|g| g.vendor != Vendor::Nvidia);
        for (name, mb, drv) in smi {
            gpus.insert(0, Gpu { name, vram_mb: mb, driver: Some(drv), vendor: Vendor::Nvidia });
        }
    }
    SysInfo {
        os: info.name,
        build: info.build,
        ram_mb: crate::platform::total_memory_mb(),
        free_disk_mb: crate::platform::free_disk_mb(app_dir),
        gpus,
        ffmpeg,
    }
}

/// 서버 설정 (VRAM 실측: Q8 + mmproj 기준 -ub 8192 3.6GB, 4096 2.1GB, 2048 1.7GB)
fn server_for(vram_mb: u64) -> (u32, u32, u32, u32) {
    if vram_mb >= 6 * 1024 {
        (65536, 8, 8192, 8192)
    } else if vram_mb >= 4 * 1024 {
        (16384, 4, 4096, 4096)
    } else {
        (8192, 4, 2048, 2048)
    }
}

pub fn recommend(info: &SysInfo) -> Plan {
    let mut checks = Vec::new();
    // 운영체제: Windows는 10 이상이어야 한다
    let os_ok = !cfg!(windows) || info.build >= 10240;
    checks.push((
        if os_ok { Level::Ok } else { Level::Bad },
        if cfg!(windows) { "Windows" } else { "운영체제" }.to_string(),
        info.os.clone(),
    ));
    // 메모리: 검색용 벡터가 메모리에 올라간다 (벡터 10만 개 ≈ 300MB)
    let ram_gb = info.ram_mb as f64 / 1024.0;
    checks.push((
        if ram_gb >= 8.0 { Level::Ok } else if ram_gb >= 4.0 { Level::Warn } else { Level::Bad },
        "메모리".to_string(),
        format!(
            "{ram_gb:.0}GB{}",
            if ram_gb >= 8.0 { "" } else { " — 8GB 이상 권장 (인덱스가 크면 느려질 수 있음)" }
        ),
    ));

    // 그래픽카드 → llama.cpp 판과 서버 설정
    let best = info
        .gpus
        .iter()
        .filter(|g| g.vendor != Vendor::Other)
        .max_by_key(|g| (g.vendor == Vendor::Nvidia, g.vram_mb))
        .cloned();
    let (llama, ngl, (ctx, np, b, ub), gpu_line) = match &best {
        Some(g) if g.vendor == Vendor::Nvidia => {
            let drv = g.driver.as_deref().and_then(driver_major).unwrap_or(0);
            let (pack, note) = if drv >= 580 {
                (Pack::LlamaCuda13, "CUDA 13판")
            } else if drv >= 551 {
                (Pack::LlamaCuda12, "드라이버가 580 미만이라 CUDA 12판")
            } else {
                (Pack::LlamaVulkan, "드라이버가 오래돼 Vulkan판 (드라이버를 업데이트하면 CUDA판이 더 빠름)")
            };
            let level = if drv >= 551 { Level::Ok } else { Level::Warn };
            checks.push((
                level,
                "그래픽카드".to_string(),
                format!(
                    "{} · VRAM {:.1}GB · 드라이버 {} → {note}",
                    g.name,
                    g.vram_mb as f64 / 1024.0,
                    g.driver.clone().unwrap_or_else(|| "?".into())
                ),
            ));
            (pack, 99, server_for(g.vram_mb), g.name.clone())
        }
        Some(g) if g.vram_mb >= 2048 => {
            checks.push((
                Level::Ok,
                "그래픽카드".to_string(),
                format!("{} · VRAM {:.1}GB → Vulkan판", g.name, g.vram_mb as f64 / 1024.0),
            ));
            (Pack::LlamaVulkan, 99, server_for(g.vram_mb), g.name.clone())
        }
        other => {
            checks.push((
                Level::Warn,
                "그래픽카드".to_string(),
                match other {
                    Some(g) => format!("{} · VRAM {}MB — 부족해 CPU로 실행 (사진 한 장에 수 초, 큰 폴더는 몇 시간)", g.name, g.vram_mb),
                    None => "쓸 수 있는 그래픽카드가 없어 CPU로 실행 (사진 한 장에 수 초, 큰 폴더는 몇 시간)".into(),
                },
            ));
            (Pack::LlamaCpu, 0, (8192, 2, 2048, 2048), "CPU".to_string())
        }
    };
    // VRAM이 적으면 영상 구간이 짧아진다
    if ngl > 0 && ub < 8192 {
        checks.push((
            Level::Warn,
            "VRAM".to_string(),
            format!("VRAM이 6GB보다 적어 입력 크기를 {ub}토큰으로 줄입니다 (영상 구간이 30초보다 짧아짐)"),
        ));
    }
    // 디스크: 모델 0.9GB + llama.cpp (CUDA판은 풀면 약 1.5GB)
    let need_mb: u64 = 900 + if matches!(llama, Pack::LlamaCuda13 | Pack::LlamaCuda12) { 1600 } else { 200 };
    checks.push((
        if info.free_disk_mb >= need_mb + 1024 {
            Level::Ok
        } else if info.free_disk_mb >= need_mb {
            Level::Warn
        } else {
            Level::Bad
        },
        "디스크".to_string(),
        format!(
            "프로그램 폴더 드라이브에 {:.1}GB 남음 (준비물 약 {:.1}GB + 인덱스)",
            info.free_disk_mb as f64 / 1024.0,
            need_mb as f64 / 1024.0
        ),
    ));
    checks.push((
        if info.ffmpeg { Level::Ok } else { Level::Warn },
        "ffmpeg".to_string(),
        if info.ffmpeg {
            "있음 (영상·오디오·HEIC 사용 가능)".to_string()
        } else {
            "없음 — 영상·오디오·HEIC 사진은 이름만 색인됩니다. 아래 버튼으로 설치할 수 있습니다".to_string()
        },
    ));
    let summary = format!(
        "{gpu_line}: llama.cpp {} · 서버 -c {ctx} -np {np} -ub {ub}{}",
        match llama {
            Pack::LlamaCuda13 => "CUDA 13",
            Pack::LlamaCuda12 => "CUDA 12",
            Pack::LlamaVulkan => "Vulkan",
            _ => "CPU",
        },
        if ngl == 0 { " · GPU 안 씀" } else { "" }
    );
    Plan {
        llama,
        n_gpu_layers: ngl,
        ctx_size: ctx,
        parallel: np,
        batch_size: b,
        ubatch_size: ub,
        workers: if ngl == 0 { 2 } else { 4 },
        checks,
        summary,
        device: gpu_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(gpus: Vec<Gpu>) -> SysInfo {
        SysInfo {
            os: "Windows 11".into(),
            build: 26100,
            ram_mb: 32768,
            free_disk_mb: 100_000,
            gpus,
            ffmpeg: true,
        }
    }

    fn nv(vram: u64, drv: &str) -> Gpu {
        Gpu { name: "NVIDIA GeForce RTX".into(), vram_mb: vram, driver: Some(drv.into()), vendor: Vendor::Nvidia }
    }

    #[test]
    fn plans_by_gpu() {
        let p = recommend(&info(vec![nv(24564, "621.14")]));
        assert_eq!((p.llama, p.ubatch_size, p.n_gpu_layers), (Pack::LlamaCuda13, 8192, 99));
        let p = recommend(&info(vec![nv(8192, "560.94")]));
        assert_eq!(p.llama, Pack::LlamaCuda12);
        let p = recommend(&info(vec![nv(4096, "531.0")]));
        assert_eq!((p.llama, p.ubatch_size), (Pack::LlamaVulkan, 4096));
        let amd = Gpu { name: "AMD Radeon RX 7800".into(), vram_mb: 16384, driver: None, vendor: Vendor::Amd };
        assert_eq!(recommend(&info(vec![amd])).llama, Pack::LlamaVulkan);
        let p = recommend(&info(vec![]));
        assert_eq!((p.llama, p.n_gpu_layers), (Pack::LlamaCpu, 0));
        // 쉬운 말 요약: 장치 이름이 들어가고, 그래픽카드가 없으면 그렇게 말한다
        assert!(p.plain_summary().contains("그래픽카드 없이"));
        let p = recommend(&info(vec![nv(24564, "621.14")]));
        assert!(p.plain_summary().starts_with("NVIDIA GeForce RTX 그래픽카드로 빠르게"));
        assert!(recommend(&info(vec![nv(4096, "560.94")])).plain_summary().contains("짧게 나눠"));
        assert_eq!(driver_major("621.14"), Some(621));
        assert_eq!(vendor_of("Intel(R) Arc(TM) A770"), Vendor::Intel);
    }
}
