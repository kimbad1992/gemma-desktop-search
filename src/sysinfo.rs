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

fn ram_mb() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
        return 0;
    }
    m.ullTotalPhys / 1_048_576
}

fn free_disk_mb(dir: &Path) -> u64 {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut p = dir.to_path_buf();
    while !p.exists() {
        match p.parent() {
            Some(q) => p = q.to_path_buf(),
            None => break,
        }
    }
    let w: Vec<u16> = p.as_os_str().encode_wide_null();
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok == 0 {
        0
    } else {
        free / 1_048_576
    }
}

trait WideNull {
    fn encode_wide_null(&self) -> Vec<u16>;
}
impl WideNull for std::ffi::OsStr {
    fn encode_wide_null(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

/// PowerShell 한 번으로 Windows 버전과 그래픽카드 목록(레지스트리의 정확한 VRAM)을 얻는다
fn os_and_adapters() -> (String, u32, Vec<Gpu>) {
    let script = r#"
$cv = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
"OS`t$($cv.ProductName)`t$($cv.DisplayVersion)`t$($cv.CurrentBuildNumber)"
Get-ChildItem 'HKLM:\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}' -ErrorAction SilentlyContinue |
  Where-Object { $_.PSChildName -match '^\d{4}$' } | ForEach-Object {
    $p = Get-ItemProperty $_.PSPath -ErrorAction SilentlyContinue
    $mem = $p.'HardwareInformation.qwMemorySize'
    if (-not $mem) { $mem = $p.'HardwareInformation.MemorySize' }
    if ($mem -is [byte[]]) { $mem = [BitConverter]::ToUInt32($mem, 0) }
    if ($p.DriverDesc) { "GPU`t$($p.DriverDesc)`t$([uint64]$mem)" }
  }
"#;
    let mut cmd = hidden_command("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    let mut os = String::from("Windows");
    let mut build = 0;
    let mut gpus = Vec::new();
    if let Ok(out) = run_capture(cmd, Duration::from_secs(20)) {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let f: Vec<&str> = line.trim_end().split('\t').collect();
            match f.as_slice() {
                ["OS", name, ver, b] => {
                    build = b.trim().parse().unwrap_or(0);
                    // Windows 11도 ProductName은 "Windows 10 …"으로 남아 있다 → 빌드 번호로 판별
                    let name = if build >= 22000 { name.replace("Windows 10", "Windows 11") } else { name.to_string() };
                    os = format!("{name} {ver} (빌드 {build})");
                }
                ["GPU", name, mem] => {
                    let vram_mb = mem.trim().parse::<u64>().unwrap_or(0) / 1_048_576;
                    let vendor = vendor_of(name);
                    // 원격 데스크톱·가상 어댑터 등은 뺀다
                    if vendor != Vendor::Other || vram_mb >= 1024 {
                        gpus.push(Gpu { name: name.to_string(), vram_mb, driver: None, vendor });
                    }
                }
                _ => {}
            }
        }
    }
    (os, build, gpus)
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
    let (os, build, mut gpus) = os_and_adapters();
    let smi = nvidia_smi();
    if !smi.is_empty() {
        gpus.retain(|g| g.vendor != Vendor::Nvidia);
        for (name, mb, drv) in smi {
            gpus.insert(0, Gpu { name, vram_mb: mb, driver: Some(drv), vendor: Vendor::Nvidia });
        }
    }
    SysInfo {
        os,
        build,
        ram_mb: ram_mb(),
        free_disk_mb: free_disk_mb(app_dir),
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
    // Windows
    let os_ok = info.build >= 10240;
    checks.push((
        if os_ok { Level::Ok } else { Level::Bad },
        "Windows".to_string(),
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
        assert_eq!(driver_major("621.14"), Some(621));
        assert_eq!(vendor_of("Intel(R) Arc(TM) A770"), Vendor::Intel);
    }
}
