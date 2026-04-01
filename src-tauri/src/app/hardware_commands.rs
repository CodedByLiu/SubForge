use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;

use crate::app::state::{AppRoot, WhisperDownloadLock};
use crate::infra::config_store;
use crate::infra::hardware::{gather_hardware_info, HardwareInfoDto};
use crate::infra::whisper_models::{self, WhisperModelsListDto};
use crate::infra::whisper_runtime;

#[derive(Clone, Serialize)]
pub struct WhisperDownloadProgress {
    pub model_id: String,
    pub percent: u32,
    pub bytes_received: u64,
    pub bytes_total: Option<u64>,
    pub phase: String,
    pub message: String,
}

#[tauri::command]
pub fn get_hardware_info(
    _root: State<'_, AppRoot>,
    use_whisper_gpu: bool,
) -> Result<HardwareInfoDto, String> {
    Ok(gather_hardware_info(use_whisper_gpu))
}

#[tauri::command]
pub fn list_whisper_models(root: State<'_, AppRoot>) -> Result<WhisperModelsListDto, String> {
    let cfg = config_store::load_config(&root.0).map_err(|e| e.to_string())?;
    whisper_models::list_installed_and_catalog(
        &root.0,
        &cfg.whisper.mirror_url,
        cfg.whisper.prefer_mirror,
        &cfg.whisper.download_url,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_whisper_model(root: State<'_, AppRoot>, model_id: String) -> Result<(), String> {
    whisper_models::delete_model_file(&root.0, model_id.trim()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn download_whisper_model(
    app: AppHandle,
    root: State<'_, AppRoot>,
    lock: State<'_, WhisperDownloadLock>,
    model_id: String,
) -> Result<(), String> {
    let model_id = model_id.trim().to_string();
    let _guard = lock.0.lock().await;

    let ent = whisper_models::entry_for_id(&model_id).ok_or_else(|| "未知模型".to_string())?;
    let cfg = config_store::load_config(&root.0).map_err(|e| e.to_string())?;
    if cfg.whisper.whisper_cli_path.trim().is_empty() {
        let app_dir = root.0.clone();
        tauri::async_runtime::spawn_blocking(move || {
            whisper_runtime::ensure_managed_whisper_cli(&app_dir, |_| {})
        })
        .await
        .map_err(|e| format!("准备 Whisper CLI 失败: {e}"))?
        .map_err(|e| format!("准备 Whisper CLI 失败: {e}"))?;
    }
    let base = whisper_models::resolve_download_base(
        &cfg.whisper.mirror_url,
        cfg.whisper.prefer_mirror,
        &cfg.whisper.download_url,
    );
    let url = whisper_models::build_file_url(&base, ent.file_name);
    let dest = whisper_models::model_file_path(&root.0, &model_id).map_err(|e| e.to_string())?;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }

    let emit = |p: WhisperDownloadProgress| {
        let _ = app.emit("whisper-model-progress", &p);
    };

    emit(WhisperDownloadProgress {
        model_id: model_id.clone(),
        percent: 0,
        bytes_received: 0,
        bytes_total: None,
        phase: "connecting".into(),
        message: "正在连接…".into(),
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(7200))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .get(&url)
        .header("User-Agent", "SubForge/0.1 (whisper.cpp ggml)")
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}：请检查镜像地址或网络", response.status()));
    }

    let total = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&dest)
        .await
        .map_err(|e| format!("无法写入文件: {e}"))?;

    let mut received: u64 = 0;
    let mut last_pct: u32 = 0;

    emit(WhisperDownloadProgress {
        model_id: model_id.clone(),
        percent: 0,
        bytes_received: 0,
        bytes_total: total,
        phase: "downloading".into(),
        message: "正在下载…".into(),
    });

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("下载流错误: {e}"))?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        received += chunk.len() as u64;

        let pct = total
            .map(|t| {
                if t == 0 {
                    0
                } else {
                    ((received * 100) / t).min(100) as u32
                }
            })
            .unwrap_or(0);

        if pct >= last_pct.saturating_add(2) || received == total.unwrap_or(0) {
            last_pct = pct;
            emit(WhisperDownloadProgress {
                model_id: model_id.clone(),
                percent: pct,
                bytes_received: received,
                bytes_total: total,
                phase: "downloading".into(),
                message: format!(
                    "已下载 {} / {}",
                    format_bytes(received),
                    format_total(total)
                ),
            });
        }
    }

    file.flush().await.map_err(|e| e.to_string())?;

    let meta = tokio::fs::metadata(&dest)
        .await
        .map_err(|e| e.to_string())?;
    if meta.len() < 1024 * 1024 {
        let _ = tokio::fs::remove_file(&dest).await;
        return Err("下载文件过小，可能为错误页面，已删除".into());
    }

    emit(WhisperDownloadProgress {
        model_id: model_id.clone(),
        percent: 100,
        bytes_received: meta.len(),
        bytes_total: Some(meta.len()),
        phase: "done".into(),
        message: "下载完成".into(),
    });

    Ok(())
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", n as f64 / 1024.0 / 1024.0 / 1024.0)
    }
}

fn format_total(t: Option<u64>) -> String {
    match t {
        Some(n) => format_bytes(n),
        None => "未知".into(),
    }
}
