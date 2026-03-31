use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::domain::config::AppConfig;
use crate::domain::task::{
    TaskRecord, TaskStoreFile, STATUS_EXTRACTING, STATUS_FAILED, STATUS_PAUSE_REQUESTED, STATUS_PAUSED,
    STATUS_PENDING, STATUS_QUEUED, STATUS_TRANSLATING, STATUS_TRANSCRIBING,
};
use crate::infra::config_store;
use crate::infra::task_store::{self, normalize_existing_path, video_extension_ok};

use super::state::{AppRoot, TaskState};

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn persist(root: &AppRoot, store: &TaskStoreFile) -> Result<(), String> {
    task_store::save_task_store_file(&root.0, store).map_err(|e| e.to_string())
}

fn eligible_for_start(status: &str) -> bool {
    matches!(status, STATUS_PENDING | STATUS_PAUSED | STATUS_FAILED)
}

fn build_snapshot_summary(cfg: &AppConfig, output_dir_mode: &str) -> String {
    let output = if output_dir_mode == "custom" {
        "统一输出目录"
    } else {
        "视频同目录"
    };
    format!(
        "mode={} translator={} whisper={} gpu={} src={} tgt={} output={}",
        cfg.subtitle.mode,
        cfg.translator.engine,
        cfg.whisper.model,
        if cfg.whisper.use_gpu { "on" } else { "off" },
        cfg.translate.source_lang,
        cfg.translate.target_lang,
        output
    )
}

fn apply_pending_config_defaults(task: &mut TaskRecord, cfg: &AppConfig) {
    task.will_translate = cfg.will_run_translation();
    task.translator_engine_snapshot = cfg.translator.engine.clone();
    task.subtitle_mode_snapshot = cfg.subtitle.mode.clone();
    task.translate_source_lang_snapshot = cfg.translate.source_lang.clone();
    task.translate_target_lang_snapshot = cfg.translate.target_lang.clone();
}

fn clear_run_artifacts(task: &mut TaskRecord) {
    task.translate_note = None;
    task.error_message = None;
    task.cancel_requested = false;
    task.original_preview = None;
    task.translated_preview = None;
    task.original_output_path = None;
    task.translated_output_path = None;
    task.bilingual_output_path = None;
}

fn has_snapshot(task: &TaskRecord) -> bool {
    !task.snapshot_id.trim().is_empty()
}

fn prepare_for_queue(task: &mut TaskRecord, tnow: i64) {
    clear_run_artifacts(task);
    task.status = STATUS_QUEUED.into();
    task.progress = 0;
    task.phase.clear();
    task.updated_at_ms = tnow;
}

fn resume_with_existing_snapshot(task: &mut TaskRecord, tnow: i64) {
    clear_run_artifacts(task);
    task.status = STATUS_QUEUED.into();
    task.progress = 0;
    task.phase.clear();
    task.updated_at_ms = tnow;
}

fn apply_run_snapshot(
    task: &mut TaskRecord,
    cfg: &AppConfig,
    output_dir_mode: &str,
    custom_output_dir: &str,
) {
    apply_pending_config_defaults(task, cfg);
    task.snapshot_id = Uuid::new_v4().to_string();
    task.snapshot_summary = build_snapshot_summary(cfg, output_dir_mode);
    task.snapshot_whisper_model = cfg.whisper.model.clone();
    task.snapshot_recognition_lang = cfg.whisper.recognition_lang.clone();
    task.snapshot_whisper_use_gpu = cfg.whisper.use_gpu;
    task.snapshot_subtitle_overwrite = cfg.subtitle.overwrite;
    task.snapshot_output_dir_mode = output_dir_mode.to_string();
    task.snapshot_custom_output_dir = custom_output_dir.to_string();
    task.snapshot_ffmpeg_path = cfg.whisper.ffmpeg_path.clone();
    task.snapshot_whisper_cli_path = cfg.whisper.whisper_cli_path.clone();
    task.snapshot_cpu_thread_limit = cfg.runtime.cpu_thread_limit;
    task.snapshot_translate_style = cfg.translate.style.clone();
    task.snapshot_translate_max_segment_chars = cfg.translate.max_segment_chars;
    task.snapshot_llm_base_url = cfg.llm.base_url.clone();
    task.snapshot_llm_model = cfg.llm.model.clone();
    task.snapshot_llm_timeout_sec = cfg.llm.timeout_sec;
    task.snapshot_llm_max_retries = cfg.llm.max_retries;
    task.snapshot_keep_proper_nouns = cfg.translate.keep_proper_nouns_in_source;
    task.snapshot_glossary_case_sensitive = cfg.translate.glossary_case_sensitive;
    task.snapshot_translate_glossary_json =
        serde_json::to_string(&cfg.translate.glossary).unwrap_or_else(|_| "[]".into());
    task.snapshot_translator_min_interval_ms = cfg.translator.min_request_interval_ms;
    task.snapshot_llm_translate_concurrency = cfg.llm.translate_concurrency;
    task.snapshot_task_auto_retry_max = cfg.runtime.task_auto_retry_max;
    task.retry_attempts = 0;
    clear_run_artifacts(task);
}

fn resolve_open_path(store: &TaskStoreFile, first_video: Option<&Path>) -> Result<PathBuf, String> {
    if store.output_dir_mode == "custom" {
        let p = store.custom_output_dir.trim();
        if p.is_empty() {
            return Err("未设置统一输出目录".into());
        }
        let pb = PathBuf::from(p);
        if !pb.exists() {
            return Err("输出目录不存在".into());
        }
        return Ok(pb);
    }
    let Some(v) = first_video else {
        return Err("任务列表为空".into());
    };
    v.parent()
        .map(|p| p.to_path_buf())
        .filter(|p| p.exists())
        .ok_or_else(|| "无法解析视频所在目录".into())
}

#[derive(Debug, Serialize)]
pub struct ImportVideosResult {
    pub added: u32,
    pub skipped_duplicates: u32,
    pub skipped_invalid: u32,
}

#[derive(Debug, Serialize)]
pub struct TaskRowDto {
    pub id: String,
    pub video_path: String,
    pub file_name: String,
    pub file_size: u64,
    pub duration_sec: Option<f64>,
    pub status: String,
    pub progress: u8,
    pub phase: String,
    pub will_translate: bool,
    pub retry_attempts: u32,
    pub cancel_requested: bool,
    pub snapshot_summary: String,
    pub original_status_display: String,
    pub translate_status_display: String,
    pub original_preview: Option<String>,
    pub translated_preview: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskListPanel {
    pub output_dir_mode: String,
    pub custom_output_dir: String,
    pub show_translate_column: bool,
    pub has_active_pipeline: bool,
    /// 前端可据此短轮询 list_tasks（排队/执行中）
    pub needs_progress_refresh: bool,
    pub tasks: Vec<TaskRowDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SetPanelOutputRequest {
    pub output_dir_mode: String,
    #[serde(default)]
    pub custom_output_dir: String,
}

#[tauri::command]
pub fn list_tasks(ts: State<'_, TaskState>) -> Result<TaskListPanel, String> {
    let store = ts.0.lock().map_err(|e| e.to_string())?;
    let show_translate_column = store.tasks.iter().any(|t| t.will_translate);
    let has_active_pipeline = store.tasks.iter().any(|t| t.is_active_pipeline());
    let needs_progress_refresh = store.tasks.iter().any(|t| {
        matches!(
            t.status.as_str(),
            STATUS_QUEUED
                | STATUS_EXTRACTING
                | STATUS_TRANSCRIBING
                | STATUS_TRANSLATING
                | STATUS_PAUSE_REQUESTED
        )
    });
    let tasks = store
        .tasks
        .iter()
        .map(|t| TaskRowDto {
            id: t.id.clone(),
            video_path: t.video_path.clone(),
            file_name: t.file_name.clone(),
            file_size: t.file_size,
            duration_sec: t.duration_sec,
            status: t.status.clone(),
            progress: t.progress,
            phase: t.phase.clone(),
            will_translate: t.will_translate,
            retry_attempts: t.retry_attempts,
            cancel_requested: t.cancel_requested,
            snapshot_summary: t.snapshot_summary.clone(),
            original_status_display: t.original_status_label(),
            translate_status_display: t.translate_status_label(),
            original_preview: t.original_preview.clone(),
            translated_preview: t.translated_preview.clone(),
            error_message: t.error_message.clone(),
        })
        .collect();
    Ok(TaskListPanel {
        output_dir_mode: store.output_dir_mode.clone(),
        custom_output_dir: store.custom_output_dir.clone(),
        show_translate_column,
        has_active_pipeline,
        needs_progress_refresh,
        tasks,
    })
}

#[tauri::command]
pub fn set_panel_output(
    root: State<'_, AppRoot>,
    ts: State<'_, TaskState>,
    req: SetPanelOutputRequest,
) -> Result<(), String> {
    if req.output_dir_mode != "video_dir" && req.output_dir_mode != "custom" {
        return Err("输出目录模式无效".into());
    }
    if req.output_dir_mode == "custom" && req.custom_output_dir.trim().is_empty() {
        return Err("自定义输出目录不能为空".into());
    }
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    store.output_dir_mode = req.output_dir_mode;
    store.custom_output_dir = req.custom_output_dir.trim().to_string();
    persist(&root, &store)
}

#[tauri::command]
pub fn import_videos(
    root: State<'_, AppRoot>,
    ts: State<'_, TaskState>,
    paths: Vec<String>,
) -> Result<ImportVideosResult, String> {
    let cfg = config_store::load_config(&root.0).map_err(|e| e.to_string())?;

    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    let mut existing: std::collections::HashSet<String> =
        store.tasks.iter().map(|t| t.video_path.clone()).collect();

    let mut added = 0u32;
    let mut skipped_duplicates = 0u32;
    let mut skipped_invalid = 0u32;
    let tnow = now_ms();

    for p in paths {
        let path = Path::new(&p);
        if !video_extension_ok(path) {
            skipped_invalid += 1;
            continue;
        }
        let canon = match normalize_existing_path(path) {
            Ok(c) => c,
            Err(_) => {
                skipped_invalid += 1;
                continue;
            }
        };
        let key = canon.to_string_lossy().to_string();
        if existing.contains(&key) {
            skipped_duplicates += 1;
            continue;
        }
        existing.insert(key.clone());
        let file_name = canon
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".into());
        let file_size = std::fs::metadata(&canon).map_err(|e| e.to_string())?.len();
        let mut rec = TaskRecord {
            id: Uuid::new_v4().to_string(),
            video_path: key,
            file_name,
            file_size,
            duration_sec: None,
            status: STATUS_PENDING.into(),
            progress: 0,
            phase: String::new(),
            will_translate: false,
            translator_engine_snapshot: String::new(),
            subtitle_mode_snapshot: String::new(),
            translate_source_lang_snapshot: String::new(),
            translate_target_lang_snapshot: String::new(),
            snapshot_id: String::new(),
            snapshot_summary: String::new(),
            snapshot_whisper_model: String::new(),
            snapshot_recognition_lang: String::new(),
            snapshot_whisper_use_gpu: false,
            snapshot_subtitle_overwrite: false,
            snapshot_output_dir_mode: String::new(),
            snapshot_custom_output_dir: String::new(),
            snapshot_ffmpeg_path: String::new(),
            snapshot_whisper_cli_path: String::new(),
            snapshot_cpu_thread_limit: 0,
            snapshot_translate_style: String::new(),
            snapshot_translate_max_segment_chars: 0,
            snapshot_llm_base_url: String::new(),
            snapshot_llm_model: String::new(),
            snapshot_llm_timeout_sec: 0,
            snapshot_llm_max_retries: 0,
            snapshot_keep_proper_nouns: false,
            snapshot_glossary_case_sensitive: false,
            snapshot_translate_glossary_json: String::new(),
            snapshot_translator_min_interval_ms: 0,
            snapshot_llm_translate_concurrency: 0,
            snapshot_task_auto_retry_max: 0,
            retry_attempts: 0,
            original_preview: None,
            translated_preview: None,
            original_output_path: None,
            translated_output_path: None,
            bilingual_output_path: None,
            translate_note: None,
            cancel_requested: false,
            error_message: None,
            created_at_ms: tnow,
            updated_at_ms: tnow,
        };
        apply_pending_config_defaults(&mut rec, &cfg);
        store.tasks.push(rec);
        added += 1;
    }
    persist(&root, &store)?;
    Ok(ImportVideosResult {
        added,
        skipped_duplicates,
        skipped_invalid,
    })
}

#[tauri::command]
pub fn delete_task(root: State<'_, AppRoot>, ts: State<'_, TaskState>, id: String) -> Result<(), String> {
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    let pos = store
        .tasks
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| "找不到任务".to_string())?;
    if store.tasks[pos].is_active_pipeline() {
        store.tasks[pos].cancel_requested = true;
        store.tasks[pos].updated_at_ms = now_ms();
        persist(&root, &store)?;
        return Ok(());
    }
    store.tasks.remove(pos);
    persist(&root, &store)
}

#[tauri::command]
pub fn clear_tasks(
    root: State<'_, AppRoot>,
    ts: State<'_, TaskState>,
    force: bool,
) -> Result<(), String> {
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    if !force && store.tasks.iter().any(|t| t.is_active_pipeline()) {
        return Err("存在执行中的任务，请确认后强制清除或先暂停".into());
    }
    if force {
        let tnow = now_ms();
        store.tasks.retain_mut(|t| {
            if t.is_active_pipeline() {
                t.cancel_requested = true;
                t.updated_at_ms = tnow;
                true
            } else {
                false
            }
        });
    } else {
        store.tasks.clear();
    }
    persist(&root, &store)
}

#[tauri::command]
pub fn start_task(root: State<'_, AppRoot>, ts: State<'_, TaskState>, id: String) -> Result<(), String> {
    let cfg = config_store::load_config(&root.0).map_err(|e| e.to_string())?;
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    let od_mode = store.output_dir_mode.clone();
    let od_custom = store.custom_output_dir.clone();
    let t = store
        .tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| "找不到任务".to_string())?;
    if !eligible_for_start(&t.status) {
        return Err("当前状态不可开始".into());
    }
    let tnow = now_ms();
    match t.status.as_str() {
        STATUS_PENDING => {
            apply_run_snapshot(t, &cfg, &od_mode, &od_custom);
            prepare_for_queue(t, tnow);
        }
        STATUS_PAUSED | STATUS_FAILED if has_snapshot(t) => {
            resume_with_existing_snapshot(t, tnow);
        }
        STATUS_PAUSED | STATUS_FAILED => {
            apply_run_snapshot(t, &cfg, &od_mode, &od_custom);
            prepare_for_queue(t, tnow);
        }
        _ => {}
    }
    persist(&root, &store)
}

#[tauri::command]
pub fn start_tasks(root: State<'_, AppRoot>, ts: State<'_, TaskState>) -> Result<u32, String> {
    let cfg = config_store::load_config(&root.0).map_err(|e| e.to_string())?;
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    let od_mode = store.output_dir_mode.clone();
    let od_custom = store.custom_output_dir.clone();
    let tnow = now_ms();
    let mut n = 0u32;
    for t in &mut store.tasks {
        if eligible_for_start(&t.status) {
            match t.status.as_str() {
                STATUS_PENDING => {
                    apply_run_snapshot(t, &cfg, &od_mode, &od_custom);
                    prepare_for_queue(t, tnow);
                }
                STATUS_PAUSED | STATUS_FAILED if has_snapshot(t) => {
                    resume_with_existing_snapshot(t, tnow);
                }
                STATUS_PAUSED | STATUS_FAILED => {
                    apply_run_snapshot(t, &cfg, &od_mode, &od_custom);
                    prepare_for_queue(t, tnow);
                }
                _ => {}
            }
            n += 1;
        }
    }
    persist(&root, &store)?;
    Ok(n)
}

#[tauri::command]
pub fn pause_task(root: State<'_, AppRoot>, ts: State<'_, TaskState>, id: String) -> Result<(), String> {
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    let t = store
        .tasks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or_else(|| "找不到任务".to_string())?;
    match t.status.as_str() {
        STATUS_QUEUED => {
            t.status = STATUS_PAUSED.into();
            t.updated_at_ms = now_ms();
        }
        STATUS_EXTRACTING | STATUS_TRANSCRIBING | STATUS_TRANSLATING => {
            t.status = STATUS_PAUSE_REQUESTED.into();
            t.updated_at_ms = now_ms();
        }
        _ => return Err("当前状态不可暂停".into()),
    }
    persist(&root, &store)
}

#[tauri::command]
pub fn pause_all_tasks(root: State<'_, AppRoot>, ts: State<'_, TaskState>) -> Result<(), String> {
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    let tnow = now_ms();
    for t in &mut store.tasks {
        match t.status.as_str() {
            STATUS_QUEUED => {
                t.status = STATUS_PAUSED.into();
                t.updated_at_ms = tnow;
            }
            STATUS_EXTRACTING | STATUS_TRANSCRIBING | STATUS_TRANSLATING => {
                t.status = STATUS_PAUSE_REQUESTED.into();
                t.updated_at_ms = tnow;
            }
            _ => {}
        }
    }
    persist(&root, &store)
}

#[tauri::command]
pub fn continue_all_tasks(root: State<'_, AppRoot>, ts: State<'_, TaskState>) -> Result<(), String> {
    let mut store = ts.0.lock().map_err(|e| e.to_string())?;
    let tnow = now_ms();
    for t in &mut store.tasks {
        if t.status == STATUS_PAUSED {
            resume_with_existing_snapshot(t, tnow);
        }
    }
    persist(&root, &store)
}

#[tauri::command]
pub async fn check_transcribe_deps(
    app: AppHandle,
    root: State<'_, AppRoot>,
) -> Result<crate::infra::transcribe_deps::TranscribeDepsCheck, String> {
    let cfg = config_store::load_config(&root.0).map_err(|e| e.to_string())?;
    let app_dir = root.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::infra::transcribe_deps::check_with_progress(&app_dir, &cfg, |progress| {
            let _ = app.emit("whisper-runtime-progress", &progress);
        })
    })
        .await
        .map_err(|e| format!("检测转写环境失败: {e}"))
}

#[tauri::command]
pub fn open_output_dir(
    app: AppHandle,
    _root: State<'_, AppRoot>,
    ts: State<'_, TaskState>,
) -> Result<(), String> {
    let store = ts.0.lock().map_err(|e| e.to_string())?;
    let first = store.tasks.first().map(|t| Path::new(t.video_path.as_str()));
    let path = resolve_open_path(&store, first)?;
    let path_str = path.to_string_lossy().to_string();
    app
        .opener()
        .open_path(path_str, Option::<&str>::None)
        .map_err(|e| e.to_string())
}
