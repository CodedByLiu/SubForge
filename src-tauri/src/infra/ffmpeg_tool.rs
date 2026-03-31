use std::path::{Path, PathBuf};

use which::which;

pub fn resolve_ffmpeg(preferred: &str) -> Result<PathBuf, String> {
    let p = preferred.trim();
    if !p.is_empty() {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
        return Err(format!("ffmpeg 路径不存在: {}", pb.display()));
    }
    which("ffmpeg")
        .or_else(|_| which("ffmpeg.exe"))
        .map_err(|_| "未找到 ffmpeg（请安装并加入 PATH，或在配置中填写可执行文件路径）".into())
}

pub fn extract_mono_16k_wav(ffmpeg: &Path, video: &Path, wav_out: &Path) -> Result<(), String> {
    if let Some(parent) = wav_out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建临时目录失败: {e}"))?;
    }
    let out = std::process::Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(video)
        .arg("-vn")
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-f")
        .arg("wav")
        .arg(wav_out)
        .output()
        .map_err(|e| format!("启动 ffmpeg 失败: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg: String = err.chars().take(4000).collect();
        return Err(format!("ffmpeg 抽取音频失败: {msg}"));
    }
    Ok(())
}
