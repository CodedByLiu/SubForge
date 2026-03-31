use std::time::Duration;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::State;

use crate::domain::config::{
    AppConfigView, LlmTestResult, SaveConfigRequest, TestLlmRequest,
};
use crate::domain::task::STATUS_PENDING;

fn ok_llm_test(r: LlmTestResult) -> Result<LlmTestResult, String> {
    log::info!(
        target: "subforge_llm",
        "llm_connection_test ok={} code={}",
        r.ok,
        r.code
    );
    Ok(r)
}
use crate::infra::config_store;
use crate::infra::openai_compat::{chat_completions_url, truncate_detail};
use crate::infra::secrets;
use crate::infra::task_store;

use super::state::{AppRoot, TaskState};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub app_dir: String,
    pub version: String,
}

#[tauri::command]
pub fn get_app_info(root: State<'_, AppRoot>) -> Result<AppInfo, String> {
    Ok(AppInfo {
        app_dir: root.0.to_string_lossy().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[tauri::command]
pub fn get_config(root: State<'_, AppRoot>) -> Result<AppConfigView, String> {
    let cfg = config_store::load_config(&root.0).map_err(|e| e.to_string())?;
    let secrets = secrets::load_secrets(&root.0).map_err(|e| e.to_string())?;
    let api_key_configured = !secrets.llm_api_key.trim().is_empty();
    Ok(AppConfigView {
        config: cfg,
        api_key_configured,
    })
}

#[tauri::command]
pub fn save_config(
    root: State<'_, AppRoot>,
    ts: State<'_, TaskState>,
    req: SaveConfigRequest,
) -> Result<AppConfigView, String> {
    let SaveConfigRequest {
        config: cfg,
        llm_api_key,
        clear_llm_api_key,
    } = req;
    cfg.validate()?;
    if clear_llm_api_key {
        secrets::clear_llm_api_key(&root.0).map_err(|e| e.to_string())?;
    } else if let Some(key) = llm_api_key {
        if !key.trim().is_empty() {
            let mut s = secrets::load_secrets(&root.0).map_err(|e| e.to_string())?;
            s.llm_api_key = key;
            secrets::save_secrets(&root.0, &s).map_err(|e| e.to_string())?;
        }
    }
    config_store::save_config(&root.0, &cfg).map_err(|e| e.to_string())?;
    {
        let mut store = ts.0.lock().map_err(|e| e.to_string())?;
        let tnow = now_ms();
        for task in &mut store.tasks {
            if task.status == STATUS_PENDING {
                task.will_translate = cfg.will_run_translation();
                task.translator_engine_snapshot = cfg.translator.engine.clone();
                task.subtitle_mode_snapshot = cfg.subtitle.mode.clone();
                task.translate_source_lang_snapshot = cfg.translate.source_lang.clone();
                task.translate_target_lang_snapshot = cfg.translate.target_lang.clone();
                task.updated_at_ms = tnow;
            }
        }
        task_store::save_task_store_file(&root.0, &store).map_err(|e| e.to_string())?;
    }
    get_config(root)
}

#[tauri::command]
pub async fn test_llm_connection(
    root: State<'_, AppRoot>,
    req: TestLlmRequest,
) -> Result<LlmTestResult, String> {
    let TestLlmRequest {
        base_url,
        model,
        api_key,
        timeout_sec,
    } = req;
    let base = base_url.trim();
    if base.is_empty() {
        return ok_llm_test(LlmTestResult {
            ok: false,
            code: "invalid_input".into(),
            message: "Base URL 不能为空".into(),
            detail: None,
        });
    }
    if model.trim().is_empty() {
        return ok_llm_test(LlmTestResult {
            ok: false,
            code: "invalid_input".into(),
            message: "模型名称不能为空".into(),
            detail: None,
        });
    }
    let vault = secrets::load_secrets(&root.0).map_err(|e| e.to_string())?;
    let key_from_vault = vault.llm_api_key.clone();
    let effective_key = match api_key {
        Some(ref k) if !k.trim().is_empty() => k.clone(),
        _ => key_from_vault,
    };
    if effective_key.trim().is_empty() {
        return ok_llm_test(LlmTestResult {
            ok: false,
            code: "no_api_key".into(),
            message: "未配置 API Key：请在输入框填写或先保存配置".into(),
            detail: None,
        });
    }
    let url = chat_completions_url(base);
    let timeout = Duration::from_secs(timeout_sec.max(1) as u64);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "model": model.trim(),
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1
    });
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", effective_key.trim()))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;
    match resp {
        Ok(r) => {
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            if status.is_success() {
                ok_llm_test(LlmTestResult {
                    ok: true,
                    code: "ok".into(),
                    message: "连接成功".into(),
                    detail: None,
                })
            } else if status == 401 || status == 403 {
                ok_llm_test(LlmTestResult {
                    ok: false,
                    code: "auth".into(),
                    message: "鉴权失败：请检查 API Key 是否有效".into(),
                    detail: Some(truncate_detail(&text, 800)),
                })
            } else if status == 404 {
                ok_llm_test(LlmTestResult {
                    ok: false,
                    code: "not_found".into(),
                    message: "请求地址或模型不存在：请检查 Base URL 与模型名".into(),
                    detail: Some(truncate_detail(&text, 800)),
                })
            } else if status.as_u16() == 429 {
                ok_llm_test(LlmTestResult {
                    ok: false,
                    code: "rate_limit".into(),
                    message: "请求被限流".into(),
                    detail: Some(truncate_detail(&text, 800)),
                })
            } else {
                ok_llm_test(LlmTestResult {
                    ok: false,
                    code: "http_error".into(),
                    message: format!("HTTP {}", status.as_u16()),
                    detail: Some(truncate_detail(&text, 800)),
                })
            }
        }
        Err(e) => {
            if e.is_timeout() {
                ok_llm_test(LlmTestResult {
                    ok: false,
                    code: "timeout".into(),
                    message: "连接超时：请检查网络或增大超时时间".into(),
                    detail: None,
                })
            } else if e.is_connect() {
                ok_llm_test(LlmTestResult {
                    ok: false,
                    code: "network".into(),
                    message: "网络不可达：无法连接到服务器".into(),
                    detail: Some(e.to_string()),
                })
            } else {
                ok_llm_test(LlmTestResult {
                    ok: false,
                    code: "request_failed".into(),
                    message: "请求失败".into(),
                    detail: Some(e.to_string()),
                })
            }
        }
    }
}
