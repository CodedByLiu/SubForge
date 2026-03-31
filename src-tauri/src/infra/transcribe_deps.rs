use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::domain::config::AppConfig;

use super::ffmpeg_tool::resolve_ffmpeg;
use super::whisper_models::model_file_path;
use super::whisper_runtime;
use super::whisper_tool::resolve_whisper_cli;

#[derive(Debug, Serialize)]
pub struct TranscribeDepsCheck {
    pub ffmpeg_resolved: Option<String>,
    pub ffmpeg_ok: bool,
    pub ffmpeg_detail: String,
    pub whisper_resolved: Option<String>,
    pub whisper_ok: bool,
    pub whisper_detail: String,
    pub model_path: Option<String>,
    pub model_ok: bool,
    pub model_detail: String,
}

pub fn check(app_dir: &Path, cfg: &AppConfig) -> TranscribeDepsCheck {
    check_with_progress(app_dir, cfg, |_| {})
}

pub fn check_with_progress(
    app_dir: &Path,
    cfg: &AppConfig,
    mut on_progress: impl FnMut(whisper_runtime::WhisperRuntimeProgress),
) -> TranscribeDepsCheck {
    let mut r = TranscribeDepsCheck {
        ffmpeg_resolved: None,
        ffmpeg_ok: false,
        ffmpeg_detail: String::new(),
        whisper_resolved: None,
        whisper_ok: false,
        whisper_detail: String::new(),
        model_path: None,
        model_ok: false,
        model_detail: String::new(),
    };

    match resolve_ffmpeg(&cfg.whisper.ffmpeg_path) {
        Ok(p) => {
            r.ffmpeg_resolved = Some(p.to_string_lossy().to_string());
            match Command::new(&p).arg("-version").output() {
                Ok(o) if o.status.success() => {
                    r.ffmpeg_ok = true;
                    r.ffmpeg_detail = "ffmpeg 可执行".into();
                }
                Ok(o) => {
                    r.ffmpeg_detail = format!(
                        "执行失败: {}",
                        String::from_utf8_lossy(&o.stderr)
                            .chars()
                            .take(500)
                            .collect::<String>()
                    );
                }
                Err(e) => r.ffmpeg_detail = format!("无法运行: {e}"),
            }
        }
        Err(e) => r.ffmpeg_detail = e,
    }

    let mut runtime_prepare_err: Option<String> = None;
    if cfg.whisper.whisper_cli_path.trim().is_empty() {
        if let Err(e) = whisper_runtime::ensure_managed_whisper_cli(app_dir, &mut on_progress) {
            runtime_prepare_err = Some(e);
        }
    }

    match resolve_whisper_cli(app_dir, &cfg.whisper.whisper_cli_path) {
        Ok(p) => {
            r.whisper_resolved = Some(p.to_string_lossy().to_string());
            match Command::new(&p).arg("-h").output() {
                Ok(o) if o.status.success() || !o.stdout.is_empty() || !o.stderr.is_empty() => {
                    r.whisper_ok = true;
                    r.whisper_detail = "Whisper CLI 可执行".into();
                }
                Ok(o) => {
                    r.whisper_detail = format!(
                        "退出码 {:?}，stderr: {}",
                        o.status.code(),
                        String::from_utf8_lossy(&o.stderr)
                            .chars()
                            .take(400)
                            .collect::<String>()
                    );
                }
                Err(e) => r.whisper_detail = format!("无法运行: {e}"),
            }
        }
        Err(e) => {
            r.whisper_detail = runtime_prepare_err
                .map(|runtime_err| format!("Whisper CLI 自动安装失败: {runtime_err}；{e}"))
                .unwrap_or(e);
        }
    }

    match model_file_path(app_dir, cfg.whisper.model.trim()) {
        Ok(p) => {
            r.model_path = Some(p.to_string_lossy().to_string());
            if p.exists() {
                r.model_ok = true;
                r.model_detail = "模型文件已就绪".into();
            } else {
                r.model_detail = "模型文件尚未下载，请先下载对应 ggml 权重".into();
            }
        }
        Err(e) => r.model_detail = e.to_string(),
    }

    r
}
