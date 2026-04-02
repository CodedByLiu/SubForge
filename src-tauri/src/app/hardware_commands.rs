use futures_util::StreamExt;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_RANGE, RANGE};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;
use tokio::time::{sleep, Duration};

use crate::app::state::{AppRoot, WhisperDownloadLock};
use crate::infra::config_store;
use crate::infra::hardware::{gather_hardware_info, HardwareInfoDto};
use crate::infra::whisper_models::{self, WhisperModelsListDto};
use crate::infra::whisper_runtime::{self, WhisperRuntimeProgress};

const MAX_DOWNLOAD_RETRIES: u32 = 3;

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

    emit_download_progress(
        &app,
        WhisperDownloadProgress {
            model_id: model_id.clone(),
            percent: 1,
            bytes_received: 0,
            bytes_total: None,
            phase: "preparing".into(),
            message: "正在准备下载环境…".into(),
        },
    );

    if cfg.whisper.whisper_cli_path.trim().is_empty() {
        let app_dir = root.0.clone();
        let app_for_runtime = app.clone();
        let model_id_for_runtime = model_id.clone();
        tauri::async_runtime::spawn_blocking(move || {
            whisper_runtime::ensure_managed_whisper_cli(&app_dir, |progress| {
                emit_download_progress(
                    &app_for_runtime,
                    map_runtime_prepare_progress(&model_id_for_runtime, progress),
                );
            })
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

    emit_download_progress(
        &app,
        WhisperDownloadProgress {
            model_id: model_id.clone(),
            percent: 12,
            bytes_received: 0,
            bytes_total: None,
            phase: "connecting".into(),
            message: "正在连接下载源…".into(),
        },
    );

    let result = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(7200))
            .build()
            .map_err(|e| e.to_string())?;

        let mut file = tokio::fs::File::create(&dest)
            .await
            .map_err(|e| format!("无法写入文件: {e}"))?;
        let mut received = 0u64;
        let mut total_exact: Option<u64> = None;
        let mut progress_total = Some(ent.size_bytes_estimate);
        let mut last_pct = 0u32;
        let mut retry_count = 0u32;

        loop {
            if retry_count > 0 {
                emit_download_progress(
                    &app,
                    WhisperDownloadProgress {
                        model_id: model_id.clone(),
                        percent: compute_progress(received, progress_total),
                        bytes_received: received,
                        bytes_total: progress_total,
                        phase: "retrying".into(),
                        message: format!(
                            "网络中断，正在从 {} 继续下载（重试 {}/{}）…",
                            format_bytes(received),
                            retry_count,
                            MAX_DOWNLOAD_RETRIES
                        ),
                    },
                );
                sleep(Duration::from_millis(800)).await;
            }

            let response = start_download_request(&client, &url, received).await?;
            let status = response.status();

            if received == 0 {
                if !status.is_success() {
                    return Err(format!("HTTP {}：请检查镜像地址或网络", status));
                }
            } else if status != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(format!(
                    "下载源未返回断点续传响应（HTTP {}），无法继续下载",
                    status
                ));
            }

            if total_exact.is_none() {
                total_exact = resolve_total_bytes(received, &response);
                progress_total = total_exact.or(Some(ent.size_bytes_estimate));
                emit_download_progress(
                    &app,
                    WhisperDownloadProgress {
                        model_id: model_id.clone(),
                        percent: 14,
                        bytes_received: received,
                        bytes_total: progress_total,
                        phase: "downloading".into(),
                        message: if total_exact.is_some() {
                            "正在下载…".into()
                        } else {
                            "正在下载…（按模型目录中的约计大小估算进度）".into()
                        },
                    },
                );
            }

            let mut stream = response.bytes_stream();
            let mut stream_error = None;

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => {
                        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
                        received += chunk.len() as u64;

                        let pct = compute_progress(received, progress_total);
                        if pct >= last_pct.saturating_add(2) || progress_total == Some(received) {
                            last_pct = pct;
                            emit_download_progress(
                                &app,
                                WhisperDownloadProgress {
                                    model_id: model_id.clone(),
                                    percent: pct,
                                    bytes_received: received,
                                    bytes_total: progress_total,
                                    phase: "downloading".into(),
                                    message: format!(
                                        "已下载 {} / {}",
                                        format_bytes(received),
                                        format_total(progress_total)
                                    ),
                                },
                            );
                        }
                    }
                    Err(e) => {
                        stream_error = Some(format!("下载流错误: {e}"));
                        break;
                    }
                }
            }

            file.flush().await.map_err(|e| e.to_string())?;

            if let Some(expected) = total_exact {
                if received >= expected {
                    break;
                }
            } else if stream_error.is_none() {
                break;
            }

            if retry_count >= MAX_DOWNLOAD_RETRIES {
                if let Some(err) = stream_error {
                    return Err(err);
                }
                if let Some(expected) = total_exact {
                    return Err(format!(
                        "下载提前结束：已接收 {} / {}",
                        format_bytes(received),
                        format_bytes(expected)
                    ));
                }
                return Err("下载提前结束".into());
            }

            retry_count += 1;
        }

        let meta = tokio::fs::metadata(&dest)
            .await
            .map_err(|e| e.to_string())?;
        if meta.len() < 1024 * 1024 {
            return Err("下载文件过小，可能是错误页面".into());
        }

        emit_download_progress(
            &app,
            WhisperDownloadProgress {
                model_id: model_id.clone(),
                percent: 100,
                bytes_received: meta.len(),
                bytes_total: total_exact.or(Some(meta.len())),
                phase: "done".into(),
                message: "下载完成".into(),
            },
        );

        Ok::<(), String>(())
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&dest).await;
    }

    result
}

async fn start_download_request(
    client: &reqwest::Client,
    url: &str,
    start: u64,
) -> Result<reqwest::Response, String> {
    let mut request = client
        .get(url)
        .header("User-Agent", "SubForge/0.1 (whisper.cpp ggml)")
        .header(ACCEPT_ENCODING, "identity");

    if start > 0 {
        request = request.header(RANGE, format!("bytes={start}-"));
    }

    request
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {e}"))
}

fn emit_download_progress(app: &AppHandle, progress: WhisperDownloadProgress) {
    let _ = app.emit("whisper-model-progress", &progress);
}

fn map_runtime_prepare_progress(
    model_id: &str,
    progress: WhisperRuntimeProgress,
) -> WhisperDownloadProgress {
    let percent = match progress.phase.as_str() {
        "done" => 12,
        _ => ((progress.percent.max(1) as u64 * 12) / 100).clamp(1, 11) as u32,
    };

    WhisperDownloadProgress {
        model_id: model_id.to_string(),
        percent,
        bytes_received: progress.bytes_received,
        bytes_total: progress.bytes_total,
        phase: "preparing".into(),
        message: format!("正在准备 Whisper CLI：{}", progress.message),
    }
}

fn resolve_total_bytes(start: u64, response: &reqwest::Response) -> Option<u64> {
    if let Some(content_range) = response.headers().get(CONTENT_RANGE) {
        if let Ok(text) = content_range.to_str() {
            if let Some((_, total)) = text.rsplit_once('/') {
                if total != "*" {
                    if let Ok(parsed) = total.parse::<u64>() {
                        return Some(parsed);
                    }
                }
            }
        }
    }

    response.content_length().map(|n| n + start)
}

fn compute_progress(received: u64, total: Option<u64>) -> u32 {
    total
        .map(|t| {
            if t == 0 {
                14
            } else {
                ((received.saturating_mul(85)) / t).min(85) as u32 + 14
            }
        })
        .unwrap_or(14)
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
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
