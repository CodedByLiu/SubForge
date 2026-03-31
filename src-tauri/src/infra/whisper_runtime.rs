use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use zip::ZipArchive;

use super::paths::{bin_dir, temp_dir};

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/ggml-org/whisper.cpp/releases/latest";

#[derive(Clone, Serialize)]
pub struct WhisperRuntimeProgress {
    pub phase: String,
    pub message: String,
    pub percent: u32,
    pub bytes_received: u64,
    pub bytes_total: Option<u64>,
}

pub fn managed_whisper_dir(app_dir: &Path) -> PathBuf {
    bin_dir(app_dir).join("whispercpp")
}

pub fn managed_whisper_cli_path(app_dir: &Path) -> PathBuf {
    managed_whisper_dir(app_dir).join(whisper_cli_file_name())
}

pub fn ensure_managed_whisper_cli(
    app_dir: &Path,
    mut on_progress: impl FnMut(WhisperRuntimeProgress),
) -> Result<PathBuf, String> {
    let managed_cli = managed_whisper_cli_path(app_dir);
    if managed_cli.exists() {
        on_progress(WhisperRuntimeProgress {
            phase: "done".into(),
            message: "Whisper CLI 已就绪".into(),
            percent: 100,
            bytes_received: 0,
            bytes_total: None,
        });
        return Ok(managed_cli);
    }

    if !cfg!(target_os = "windows") {
        return Err("当前仅实现 Windows 平台的 whisper.cpp 自动下载".into());
    }

    let managed_dir = managed_whisper_dir(app_dir);
    fs::create_dir_all(&managed_dir).map_err(|e| format!("创建目录失败: {e}"))?;

    on_progress(WhisperRuntimeProgress {
        phase: "connecting".into(),
        message: "正在获取 Whisper CLI 发布信息…".into(),
        percent: 0,
        bytes_received: 0,
        bytes_total: None,
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("初始化下载客户端失败: {e}"))?;

    let release_json = client
        .get(LATEST_RELEASE_API)
        .header("User-Agent", "SubForge/0.1 whisper-runtime")
        .send()
        .map_err(|e| format!("获取 whisper.cpp 发布信息失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("获取 whisper.cpp 发布信息失败: {e}"))?
        .json::<Value>()
        .map_err(|e| format!("解析 whisper.cpp 发布信息失败: {e}"))?;

    let asset_name = if cfg!(target_arch = "x86") {
        "whisper-bin-Win32.zip"
    } else {
        "whisper-bin-x64.zip"
    };

    let asset_url = release_json
        .get("assets")
        .and_then(|v| v.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                if asset.get("name").and_then(|v| v.as_str()) == Some(asset_name) {
                    asset
                        .get("browser_download_url")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| format!("未找到适用于当前系统的 whisper.cpp 包: {asset_name}"))?;

    on_progress(WhisperRuntimeProgress {
        phase: "downloading".into(),
        message: format!("正在下载 Whisper CLI：{asset_name}"),
        percent: 2,
        bytes_received: 0,
        bytes_total: None,
    });

    let mut response = client
        .get(&asset_url)
        .header("User-Agent", "SubForge/0.1 whisper-runtime")
        .send()
        .map_err(|e| format!("下载 whisper.cpp 包失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载 whisper.cpp 包失败: {e}"))?;

    let total = response.content_length();
    let zip_path = temp_dir(app_dir).join("whisper-runtime-download.zip");
    if let Some(parent) = zip_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建临时目录失败: {e}"))?;
    }
    let mut zip_file = File::create(&zip_path).map_err(|e| format!("创建临时文件失败: {e}"))?;
    let mut buf = [0u8; 64 * 1024];
    let mut received = 0u64;
    let mut last_pct = 0u32;
    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("读取 whisper.cpp 下载流失败: {e}"))?;
        if n == 0 {
            break;
        }
        zip_file
            .write_all(&buf[..n])
            .map_err(|e| format!("写入 whisper.cpp 压缩包失败: {e}"))?;
        received += n as u64;
        let pct = total
            .map(|t| if t == 0 { 0 } else { ((received * 88) / t).min(88) as u32 + 2 })
            .unwrap_or(0);
        if pct >= last_pct.saturating_add(2) || total == Some(received) {
            last_pct = pct;
            on_progress(WhisperRuntimeProgress {
                phase: "downloading".into(),
                message: format!("正在下载 Whisper CLI… {}", format_total(total, received)),
                percent: pct,
                bytes_received: received,
                bytes_total: total,
            });
        }
    }

    on_progress(WhisperRuntimeProgress {
        phase: "extracting".into(),
        message: "正在解压 Whisper CLI…".into(),
        percent: 92,
        bytes_received: received,
        bytes_total: total,
    });

    extract_release_zip(app_dir, &zip_path)?;
    let _ = fs::remove_file(&zip_path);

    if managed_cli.exists() {
        on_progress(WhisperRuntimeProgress {
            phase: "done".into(),
            message: "Whisper CLI 下载完成".into(),
            percent: 100,
            bytes_received: received,
            bytes_total: total,
        });
        Ok(managed_cli)
    } else {
        Err(format!(
            "whisper.cpp 自动安装完成，但未找到 {}",
            managed_cli.display()
        ))
    }
}

fn whisper_cli_file_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "whisper-cli.exe"
    } else {
        "whisper-cli"
    }
}

fn extract_release_zip(app_dir: &Path, zip_path: &Path) -> Result<(), String> {
    let zip_file = File::open(zip_path).map_err(|e| format!("打开压缩包失败: {e}"))?;
    let mut archive = ZipArchive::new(zip_file).map_err(|e| format!("打开压缩包失败: {e}"))?;
    let dest_root = managed_whisper_dir(app_dir);
    let temp_root = temp_dir(app_dir).join("whisper-runtime-extract");
    let _ = fs::remove_dir_all(&temp_root);
    fs::create_dir_all(&temp_root).map_err(|e| format!("创建临时目录失败: {e}"))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("读取压缩包条目失败: {e}"))?;
        let Some(enclosed) = entry.enclosed_name().map(|p| p.to_owned()) else {
            continue;
        };
        let parts = enclosed
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        if parts.is_empty() || parts[0] != "Release" {
            continue;
        }
        let relative = parts[1..].iter().collect::<PathBuf>();
        if relative.as_os_str().is_empty() {
            continue;
        }
        let out_path = temp_root.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| format!("创建目录失败: {e}"))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
        }
        let mut out_file = File::create(&out_path).map_err(|e| format!("写入文件失败: {e}"))?;
        io::copy(&mut entry, &mut out_file).map_err(|e| format!("解压文件失败: {e}"))?;
    }

    if dest_root.exists() {
        fs::remove_dir_all(&dest_root).map_err(|e| format!("替换旧运行时失败: {e}"))?;
    }
    fs::rename(&temp_root, &dest_root).map_err(|e| format!("安装运行时失败: {e}"))?;
    Ok(())
}

fn format_total(total: Option<u64>, received: u64) -> String {
    match total {
        Some(total) => format!("{}/{}", format_bytes(received), format_bytes(total)),
        None => format!("{} / 未知", format_bytes(received)),
    }
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    }
}
