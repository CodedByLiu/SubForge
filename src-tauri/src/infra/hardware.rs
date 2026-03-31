use serde::Serialize;
use sysinfo::System;

#[derive(Debug, Clone, Serialize)]
pub struct GpuInfoDto {
    pub name: String,
    /// 显存总量（MB），无法检测时为 null
    pub memory_total_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HardwareInfoDto {
    pub cpu_brand: String,
    pub cpu_logical_cores: u32,
    pub cpu_physical_cores: u32,
    pub memory_total_mb: u64,
    pub memory_available_mb: u64,
    pub gpus: Vec<GpuInfoDto>,
    pub nvidia_nvml_available: bool,
    /// 按 §6.2 规则，结合 use_gpu 与主卡显存
    pub whisper_recommended_models: Vec<String>,
    pub whisper_note: String,
}

fn collect_nvml_gpus() -> (Vec<GpuInfoDto>, bool) {
    match nvml_wrapper::Nvml::init() {
        Ok(nvml) => {
            let count = match nvml.device_count() {
                Ok(c) => c,
                Err(_) => return (Vec::new(), true),
            };
            let mut out = Vec::new();
            for i in 0..count {
                if let Ok(dev) = nvml.device_by_index(i) {
                    let name = dev.name().unwrap_or_else(|_| "NVIDIA GPU".into());
                    let mem_mb = dev
                        .memory_info()
                        .ok()
                        .map(|m| m.total / (1024 * 1024));
                    out.push(GpuInfoDto {
                        name,
                        memory_total_mb: mem_mb,
                    });
                }
            }
            (out, true)
        }
        Err(_) => (Vec::new(), false),
    }
}

#[cfg(windows)]
fn fallback_gpu_names_from_os() -> Vec<String> {
    let script =
        "Get-CimInstance Win32_VideoController | Where-Object { $_.Name } | ForEach-Object { $_.Name }";
    let Ok(out) = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            script,
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(not(windows))]
fn fallback_gpu_names_from_os() -> Vec<String> {
    Vec::new()
}

/// 主卡显存（MB）：取第一块 NVML 显卡
fn primary_vram_mb(gpus: &[GpuInfoDto]) -> Option<u64> {
    gpus.iter().find_map(|g| g.memory_total_mb)
}

/// §6.2 推荐档位（文案与规格一致，仅供参考）
pub fn recommend_whisper_models(use_gpu: bool, primary_vram_mb: Option<u64>) -> (Vec<String>, String) {
    if !use_gpu {
        return (
            vec![
                "tiny".into(),
                "base".into(),
                "small".into(),
            ],
            "未启用 GPU 或按 CPU 推理：推荐 tiny～base，视机器可适当使用 small".into(),
        );
    }
    let Some(v) = primary_vram_mb else {
        return (
            vec!["tiny".into(), "base".into(), "small".into()],
            "已启用 GPU，但未检测到显存信息：保守推荐 tiny～small".into(),
        );
    };
    if v < 4096 {
        (
            vec!["base".into(), "small".into()],
            format!("显存约 {}MB（小于 4GB）：推荐 base 或 small", v),
        )
    } else if v <= 8192 {
        (
            vec!["small".into(), "medium".into()],
            format!("显存约 {}MB（4～8GB）：推荐 small 或 medium", v),
        )
    } else {
        (
            vec!["medium".into(), "large-v3".into()],
            format!("显存约 {}MB（大于 8GB）：可尝试 medium 或 large-v3", v),
        )
    }
}

pub fn gather_hardware_info(use_gpu: bool) -> HardwareInfoDto {
    let mut sys = System::new_all();
    sys.refresh_all();

    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".into());
    let cpu_logical_cores = sys.cpus().len() as u32;
    let cpu_physical_cores = sys
        .physical_core_count()
        .map(|n| n as u32)
        .unwrap_or(cpu_logical_cores);

    let memory_total_mb = sys.total_memory() / 1024 / 1024;
    let memory_available_mb = sys.available_memory() / 1024 / 1024;

    let (mut gpus, nvml_ok) = collect_nvml_gpus();
    if gpus.is_empty() {
        for name in fallback_gpu_names_from_os() {
            if name.eq_ignore_ascii_case("Microsoft Basic Render Driver") {
                continue;
            }
            gpus.push(GpuInfoDto {
                name,
                memory_total_mb: None,
            });
        }
    }

    let vram = primary_vram_mb(&gpus);
    let (whisper_recommended_models, whisper_note) = recommend_whisper_models(use_gpu, vram);

    HardwareInfoDto {
        cpu_brand,
        cpu_logical_cores,
        cpu_physical_cores,
        memory_total_mb,
        memory_available_mb,
        gpus,
        nvidia_nvml_available: nvml_ok,
        whisper_recommended_models,
        whisper_note,
    }
}
