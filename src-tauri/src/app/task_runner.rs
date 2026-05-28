use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter};

use crate::app::state::AppRoot;
use crate::domain::config::{AppConfig, GlossaryEntry};
use crate::domain::task::{
    TaskStoreFile, ORIGINAL_STAGE_COMPLETED, ORIGINAL_STAGE_EXPORTING,
    ORIGINAL_STAGE_EXTRACTING_AUDIO, ORIGINAL_STAGE_SEGMENTING, ORIGINAL_STAGE_TRANSCRIBING,
    STATUS_COMPLETED, STATUS_FAILED, STATUS_PAUSED, STATUS_PAUSE_REQUESTED, STATUS_QUEUED,
    STATUS_RUNNING, TRANSLATION_STAGE_COMPLETED, TRANSLATION_STAGE_EXPORTING,
    TRANSLATION_STAGE_NOT_REQUIRED, TRANSLATION_STAGE_QUEUED, TRANSLATION_STAGE_TRANSLATING,
    TRANSLATION_STAGE_WAITING_ORIGINAL,
};
use crate::infra::cache::{hash_parts, TaskCache};
use crate::infra::config_store;
use crate::infra::ffmpeg_tool::{extract_mono_16k_wav, resolve_ffmpeg};
use crate::infra::google_translate::{
    build_google_client, translate_all_cues_google, GoogleWebTranslateJob,
};
use crate::infra::llm_translate::{translate_all_cues, TranslateJob};
use crate::infra::runner_limits::{effective_max_parallel_tasks, LlmRequestSlots};
use crate::infra::secrets;
use crate::infra::srt::{
    apply_recognition_glossary, build_bilingual_cues_optimized, build_recognition_prompt,
    build_translated_cues, format_srt, optimize_source_cues, optimize_translated_cues, parse_srt,
    stitch_sentences, StitchOptions, SubCue,
};
use crate::infra::subtitle_output::{resolve_bilingual_srt_path, resolve_original_srt_path};
use crate::infra::subtitle_segmentation::{
    extract_strong_boundaries, segment_cues, SegmentationJob,
};
use crate::infra::task_store;
use crate::infra::whisper_models::model_file_path;
use crate::infra::whisper_runtime;
use crate::infra::whisper_tool::{
    boxed_progress, read_language_from_whisper_json, resolve_whisper_cli,
    run_whisper_srt_json, WhisperVadOptions,
};

#[derive(serde::Serialize, Clone)]
struct TaskProgressPayload {
    task_id: String,
    status: String,
    progress: u8,
    phase: String,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn emit_progress(app: &AppHandle, task_id: &str, status: &str, progress: u8, phase: &str) {
    let _ = app.emit(
        "task-progress",
        TaskProgressPayload {
            task_id: task_id.to_string(),
            status: status.to_string(),
            progress,
            phase: phase.to_string(),
        },
    );
}

fn occupies_runner_slot(status: &str) -> bool {
    matches!(status, STATUS_RUNNING | STATUS_PAUSE_REQUESTED)
}

fn lock_task_store<'a>(
    ts: &'a Arc<Mutex<TaskStoreFile>>,
    context: &str,
) -> MutexGuard<'a, TaskStoreFile> {
    match ts.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::error!(
                target: "subforge_task",
                "task_store_lock_poisoned context={context}; recovering poisoned mutex"
            );
            poisoned.into_inner()
        }
    }
}

fn update_task_runtime_state(
    ts: &Arc<Mutex<TaskStoreFile>>,
    root: &AppRoot,
    id: &str,
    task_status: Option<&str>,
    original_stage: Option<&str>,
    translation_stage: Option<&str>,
    progress: Option<u8>,
    phase: Option<&str>,
) {
    let mut g = lock_task_store(ts, "update_task_runtime_state");
    if let Some(t) = g.tasks.iter_mut().find(|t| t.id == id) {
        t.normalize_state();
        if let Some(status) = task_status {
            t.status = status.into();
        }
        if let Some(stage) = original_stage {
            t.original_stage = stage.into();
        }
        if let Some(stage) = translation_stage {
            t.translation_stage = stage.into();
        }
        if let Some(value) = progress {
            t.progress = value;
        }
        if let Some(value) = phase {
            t.phase = value.into();
        }
        t.updated_at_ms = now_ms();
        let _ = task_store::save_task_store_file(&root.0, &g);
    }
}

/// 把后端任务进度写入 task store 并广播 task-progress 事件。
fn push_progress(
    app: &AppHandle,
    ts: &Arc<Mutex<TaskStoreFile>>,
    root: &AppRoot,
    task_id: &str,
    progress: u8,
    phase: &str,
) {
    {
        let mut g = lock_task_store(ts, "push_progress");
        if let Some(t) = g.tasks.iter_mut().find(|t| t.id == task_id) {
            if t.progress != progress || t.phase != phase {
                t.progress = progress;
                if !phase.is_empty() {
                    t.phase = phase.into();
                }
                t.updated_at_ms = now_ms();
                let _ = task_store::save_task_store_file(&root.0, &g);
            }
        }
    }
    emit_progress(app, task_id, STATUS_RUNNING, progress, phase);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MidRun {
    Continue,
    Pause,
    Cancel,
}

fn mid_run_poll(ts: &Arc<Mutex<TaskStoreFile>>, id: &str) -> MidRun {
    let g = lock_task_store(ts, "mid_run_poll");
    let Some(t) = g.tasks.iter().find(|x| x.id == id) else {
        return MidRun::Cancel;
    };
    if t.cancel_requested {
        MidRun::Cancel
    } else if t.status == STATUS_PAUSE_REQUESTED {
        MidRun::Pause
    } else {
        MidRun::Continue
    }
}

fn translation_should_abort(ts: &Arc<Mutex<TaskStoreFile>>, id: &str) -> bool {
    matches!(mid_run_poll(ts, id), MidRun::Pause | MidRun::Cancel)
}

fn remove_task_if_present(
    app: &AppHandle,
    ts: &Arc<Mutex<TaskStoreFile>>,
    root: &AppRoot,
    id: &str,
) {
    let mut g = lock_task_store(ts, "remove_task_if_present");
    let before = g.tasks.len();
    g.tasks.retain(|t| t.id != id);
    if g.tasks.len() < before {
        log::info!(target: "subforge_task", "task_removed id={id}");
        let _ = task_store::save_task_store_file(&root.0, &g);
        emit_progress(app, id, "cancelled", 0, "");
    }
}

fn apply_paused(ts: &Arc<Mutex<TaskStoreFile>>, root: &AppRoot, id: &str, app: &AppHandle) {
    let mut g = lock_task_store(ts, "apply_paused");
    if let Some(t) = g.tasks.iter_mut().find(|t| t.id == id) {
        t.normalize_state();
        if t.status == STATUS_PAUSE_REQUESTED {
            t.status = STATUS_PAUSED.into();
            t.phase.clear();
            t.updated_at_ms = now_ms();
            let prog = t.progress;
            let st = t.status.clone();
            let _ = task_store::save_task_store_file(&root.0, &g);
            emit_progress(app, id, &st, prog, "");
        }
    }
}

fn preview_text(lines: &[String]) -> Option<String> {
    let joined = lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        })
        .take(2)
        .collect::<Vec<_>>()
        .join(" / ");
    if joined.is_empty() {
        None
    } else {
        let mut preview = String::new();
        for ch in joined.chars().take(120) {
            preview.push(ch);
        }
        if joined.chars().count() > 120 {
            preview.push('…');
        }
        Some(preview)
    }
}

fn is_retryable_task_error(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "network",
        "connection",
        "connect",
        "http ",
        "429",
        "500",
        "502",
        "503",
        "504",
        "json",
        "rate limit",
        "dns",
        "tls",
        "socket",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn fail_task(app: &AppHandle, ts: &Arc<Mutex<TaskStoreFile>>, root: &AppRoot, id: &str, msg: &str) {
    let mut g = lock_task_store(ts, "fail_task");
    if !g.tasks.iter().any(|t| t.id == id) {
        return;
    }
    if let Some(t) = g.tasks.iter_mut().find(|t| t.id == id) {
        t.normalize_state();
        let log_msg: String = msg.chars().take(512).collect();
        let retry_limit = t.snapshot_task_auto_retry_max;
        let can_retry = !t.cancel_requested
            && retry_limit > 0
            && t.retry_attempts < retry_limit
            && is_retryable_task_error(msg);
        if can_retry {
            t.retry_attempts += 1;
            t.status = STATUS_QUEUED.into();
            t.original_stage = crate::domain::task::ORIGINAL_STAGE_WAITING.into();
            t.translation_stage = if t.will_translate {
                TRANSLATION_STAGE_WAITING_ORIGINAL.into()
            } else {
                TRANSLATION_STAGE_NOT_REQUIRED.into()
            };
            t.progress = 0;
            t.phase = format!("retry_{}", t.retry_attempts);
            t.cancel_requested = false;
            t.translate_note = Some(format!("自动重试 {}/{}", t.retry_attempts, retry_limit));
            t.segmentation_note = None;
            t.error_message = Some(msg.chars().take(4000).collect());
            t.original_preview = None;
            t.translated_preview = None;
            t.original_output_path = None;
            t.translated_output_path = None;
            t.bilingual_output_path = None;
            t.updated_at_ms = now_ms();
            log::warn!(
                target: "subforge_task",
                "task_retrying id={id} attempt={}/{} {log_msg}",
                t.retry_attempts,
                retry_limit
            );
            let _ = task_store::save_task_store_file(&root.0, &g);
            emit_progress(app, id, STATUS_QUEUED, 0, "retry");
            return;
        }

        log::warn!(target: "subforge_task", "task_failed id={id} {log_msg}");
        t.mark_failed();
        t.cancel_requested = false;
        t.translate_note = None;
        t.segmentation_note = None;
        t.error_message = Some(msg.chars().take(4000).collect());
        t.phase.clear();
        t.updated_at_ms = now_ms();
        let prog = t.progress;
        let _ = task_store::save_task_store_file(&root.0, &g);
        emit_progress(app, id, STATUS_FAILED, prog, "");
    }
}

struct JobSnapshot {
    video_path: String,
    whisper_model: String,
    recognition_lang: String,
    whisper_use_gpu: bool,
    whisper_enable_vad: bool,
    vad_threshold: f32,
    vad_min_speech_ms: u32,
    vad_min_silence_ms: u32,
    vad_max_segment_ms: u32,
    subtitle_overwrite: bool,
    output_dir_mode: String,
    custom_output_dir: String,
    ffmpeg_path: String,
    whisper_cli_path: String,
    cpu_thread_limit: u32,
    will_translate: bool,
    translator_engine: String,
    subtitle_mode: String,
    translate_source_lang: String,
    translate_target_lang: String,
    segmentation_strategy: String,
    segmentation_timing_mode: String,
    segmentation_max_chars: u32,
    segmentation_max_duration_ms: u32,
    segmentation_sentence_gap_ms: u32,
    translator_provider_url: String,
    translator_use_proxy: bool,
    llm_base_url: String,
    llm_model: String,
    llm_timeout_sec: u64,
    llm_max_retries: u32,
    translator_min_interval_ms: u64,
    translate_style: String,
    translate_max_segment_chars: u32,
    keep_proper_nouns: bool,
    glossary_case_sensitive: bool,
    glossary: Vec<GlossaryEntry>,
    translate_concurrency: u32,
    recognition_glossary: Vec<GlossaryEntry>,
    recognition_glossary_case_sensitive: bool,
}

fn load_job(ts: &Arc<Mutex<TaskStoreFile>>, task_id: &str, cfg: &AppConfig) -> Option<JobSnapshot> {
    let g = lock_task_store(ts, "load_job");
    let t = g.tasks.iter().find(|x| x.id == task_id)?;
    let snap = !t.snapshot_whisper_model.trim().is_empty();
    let (output_dir_mode, custom_output_dir) = if snap && !t.snapshot_output_dir_mode.is_empty() {
        (
            t.snapshot_output_dir_mode.clone(),
            t.snapshot_custom_output_dir.clone(),
        )
    } else {
        (g.output_dir_mode.clone(), g.custom_output_dir.clone())
    };
    let glossary: Vec<GlossaryEntry> =
        if snap && !t.snapshot_translate_glossary_json.trim().is_empty() {
            serde_json::from_str(&t.snapshot_translate_glossary_json).unwrap_or_default()
        } else {
            cfg.translate.glossary.clone()
        };
    Some(JobSnapshot {
        video_path: t.video_path.clone(),
        whisper_model: if snap {
            t.snapshot_whisper_model.clone()
        } else {
            cfg.whisper.model.clone()
        },
        recognition_lang: if snap {
            t.snapshot_recognition_lang.clone()
        } else {
            cfg.whisper.recognition_lang.clone()
        },
        whisper_use_gpu: if snap {
            t.snapshot_whisper_use_gpu
        } else {
            cfg.whisper.use_gpu
        },
        whisper_enable_vad: if snap {
            t.snapshot_enable_vad
        } else {
            cfg.whisper.enable_vad
        },
        vad_threshold: if snap && t.snapshot_vad_threshold > 0.0 {
            t.snapshot_vad_threshold
        } else {
            cfg.whisper.vad_threshold
        },
        vad_min_speech_ms: if snap && t.snapshot_vad_min_speech_ms > 0 {
            t.snapshot_vad_min_speech_ms
        } else {
            cfg.whisper.vad_min_speech_ms
        },
        vad_min_silence_ms: if snap && t.snapshot_vad_min_silence_ms > 0 {
            t.snapshot_vad_min_silence_ms
        } else {
            cfg.whisper.vad_min_silence_ms
        },
        vad_max_segment_ms: if snap && t.snapshot_vad_max_segment_ms > 0 {
            t.snapshot_vad_max_segment_ms
        } else {
            cfg.whisper.vad_max_segment_ms
        },
        subtitle_overwrite: if snap {
            t.snapshot_subtitle_overwrite
        } else {
            cfg.subtitle.overwrite
        },
        output_dir_mode,
        custom_output_dir,
        ffmpeg_path: if snap && !t.snapshot_ffmpeg_path.trim().is_empty() {
            t.snapshot_ffmpeg_path.clone()
        } else {
            cfg.whisper.ffmpeg_path.clone()
        },
        whisper_cli_path: if snap && !t.snapshot_whisper_cli_path.trim().is_empty() {
            t.snapshot_whisper_cli_path.clone()
        } else {
            cfg.whisper.whisper_cli_path.clone()
        },
        cpu_thread_limit: if snap && t.snapshot_cpu_thread_limit > 0 {
            t.snapshot_cpu_thread_limit
        } else {
            cfg.runtime.cpu_thread_limit
        },
        will_translate: t.will_translate,
        translator_engine: t.translator_engine_snapshot.clone(),
        subtitle_mode: t.subtitle_mode_snapshot.clone(),
        translate_source_lang: t.translate_source_lang_snapshot.clone(),
        translate_target_lang: t.translate_target_lang_snapshot.clone(),
        segmentation_strategy: if snap && !t.segmentation_strategy_snapshot.is_empty() {
            t.segmentation_strategy_snapshot.clone()
        } else {
            cfg.segmentation.strategy.clone()
        },
        segmentation_timing_mode: if snap && !t.segmentation_timing_mode_snapshot.is_empty() {
            t.segmentation_timing_mode_snapshot.clone()
        } else {
            cfg.segmentation.timing_mode.clone()
        },
        segmentation_max_chars: if snap && t.snapshot_segmentation_max_chars > 0 {
            t.snapshot_segmentation_max_chars
        } else {
            cfg.segmentation.max_chars_per_segment
        },
        segmentation_max_duration_ms: if snap && t.snapshot_segmentation_max_duration_ms > 0 {
            t.snapshot_segmentation_max_duration_ms
        } else {
            (cfg.segmentation.max_duration_seconds.max(0.0) * 1000.0).round() as u32
        },
        segmentation_sentence_gap_ms: cfg.segmentation.sentence_gap_ms,
        translator_provider_url: if snap && !t.snapshot_translator_provider_url.trim().is_empty() {
            t.snapshot_translator_provider_url.clone()
        } else {
            cfg.translator.provider_url.clone()
        },
        translator_use_proxy: if snap {
            t.snapshot_translator_use_proxy
        } else {
            cfg.translator.use_proxy
        },
        llm_base_url: if snap && !t.snapshot_llm_base_url.trim().is_empty() {
            t.snapshot_llm_base_url.clone()
        } else {
            cfg.llm.base_url.clone()
        },
        llm_model: if snap && !t.snapshot_llm_model.trim().is_empty() {
            t.snapshot_llm_model.clone()
        } else {
            cfg.llm.model.clone()
        },
        llm_timeout_sec: if snap && t.snapshot_llm_timeout_sec > 0 {
            t.snapshot_llm_timeout_sec as u64
        } else {
            cfg.llm.timeout_sec as u64
        },
        llm_max_retries: if snap && t.snapshot_llm_max_retries > 0 {
            t.snapshot_llm_max_retries
        } else {
            cfg.llm.max_retries
        },
        translator_min_interval_ms: if snap && t.snapshot_translator_min_interval_ms > 0 {
            t.snapshot_translator_min_interval_ms as u64
        } else {
            cfg.translator.min_request_interval_ms as u64
        },
        translate_style: if snap && !t.snapshot_translate_style.is_empty() {
            t.snapshot_translate_style.clone()
        } else {
            cfg.translate.style.clone()
        },
        translate_max_segment_chars: if snap && t.snapshot_translate_max_segment_chars > 0 {
            t.snapshot_translate_max_segment_chars
        } else {
            cfg.translate.max_segment_chars
        },
        keep_proper_nouns: if snap {
            t.snapshot_keep_proper_nouns
        } else {
            cfg.translate.keep_proper_nouns_in_source
        },
        glossary_case_sensitive: if snap {
            t.snapshot_glossary_case_sensitive
        } else {
            cfg.translate.glossary_case_sensitive
        },
        glossary,
        translate_concurrency: if snap && t.snapshot_llm_translate_concurrency > 0 {
            t.snapshot_llm_translate_concurrency
        } else {
            cfg.llm.translate_concurrency
        },
        recognition_glossary: cfg.whisper.recognition_glossary.clone(),
        recognition_glossary_case_sensitive: cfg.whisper.recognition_glossary_case_sensitive,
    })
}

fn succeed_task(
    app: &AppHandle,
    ts: &Arc<Mutex<TaskStoreFile>>,
    root: &AppRoot,
    id: &str,
    note: Option<String>,
    fallback_count: u32,
) {
    let mut g = lock_task_store(ts, "succeed_task");
    if !g.tasks.iter().any(|t| t.id == id) {
        return;
    }
    if let Some(t) = g.tasks.iter_mut().find(|t| t.id == id) {
        log::info!(target: "subforge_task", "task_completed id={id}");
        t.normalize_state();
        t.status = STATUS_COMPLETED.into();
        t.original_stage = ORIGINAL_STAGE_COMPLETED.into();
        if t.will_translate {
            t.translation_stage = TRANSLATION_STAGE_COMPLETED.into();
        } else {
            t.translation_stage = TRANSLATION_STAGE_NOT_REQUIRED.into();
        }
        t.progress = 100;
        t.phase.clear();
        t.error_message = None;
        t.translate_note = note;
        t.translate_fallback_count = fallback_count;
        t.retry_translation_only = false;
        t.updated_at_ms = now_ms();
        let _ = task_store::save_task_store_file(&root.0, &g);
        emit_progress(app, id, STATUS_COMPLETED, 100, "");
    }
}

fn cache_task_outputs(
    ts: &Arc<Mutex<TaskStoreFile>>,
    root: &AppRoot,
    id: &str,
    original_preview: Option<String>,
    translated_preview: Option<String>,
    original_output_path: Option<&Path>,
    translated_output_path: Option<&Path>,
    bilingual_output_path: Option<&Path>,
) {
    let mut g = lock_task_store(ts, "cache_task_outputs");
    if let Some(t) = g.tasks.iter_mut().find(|t| t.id == id) {
        t.original_preview = original_preview;
        t.translated_preview = translated_preview;
        t.original_output_path = original_output_path.map(|p| p.to_string_lossy().to_string());
        t.translated_output_path = translated_output_path.map(|p| p.to_string_lossy().to_string());
        t.bilingual_output_path = bilingual_output_path.map(|p| p.to_string_lossy().to_string());
        t.updated_at_ms = now_ms();
        let _ = task_store::save_task_store_file(&root.0, &g);
    }
}

fn cache_original_output_path(
    ts: &Arc<Mutex<TaskStoreFile>>,
    root: &AppRoot,
    id: &str,
    path: &Path,
) {
    let mut g = lock_task_store(ts, "cache_original_output_path");
    if let Some(t) = g.tasks.iter_mut().find(|t| t.id == id) {
        t.original_output_path = Some(path.to_string_lossy().to_string());
        t.updated_at_ms = now_ms();
        let _ = task_store::save_task_store_file(&root.0, &g);
    }
}

fn cache_segmentation_note(
    ts: &Arc<Mutex<TaskStoreFile>>,
    root: &AppRoot,
    id: &str,
    note: Option<String>,
) {
    let mut g = lock_task_store(ts, "cache_segmentation_note");
    if let Some(t) = g.tasks.iter_mut().find(|t| t.id == id) {
        t.segmentation_note = note;
        t.updated_at_ms = now_ms();
        let _ = task_store::save_task_store_file(&root.0, &g);
    }
}

fn effective_threads(snap_lim: u32, cfg_lim: u32) -> u32 {
    let lim = (if snap_lim > 0 { snap_lim } else { cfg_lim }).max(1);
    let sys = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(8)
        .max(1);
    lim.min(sys).max(1)
}

pub async fn run_forever(
    app: AppHandle,
    root: AppRoot,
    ts: Arc<Mutex<TaskStoreFile>>,
    llm_slots: LlmRequestSlots,
) {
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let cfg = match config_store::load_config(&root.0) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let cap = effective_max_parallel_tasks(&cfg);
        let task_id = {
            let mut g = lock_task_store(&ts, "run_forever");
            let mut changed = false;
            for task in &mut g.tasks {
                let old_status = task.status.clone();
                let old_original = task.original_stage.clone();
                let old_translation = task.translation_stage.clone();
                task.normalize_state();
                if task.status != old_status
                    || task.original_stage != old_original
                    || task.translation_stage != old_translation
                {
                    changed = true;
                }
            }
            if changed {
                let _ = task_store::save_task_store_file(&root.0, &g);
            }
            let active = g
                .tasks
                .iter()
                .filter(|t| occupies_runner_slot(&t.status))
                .count();
            if active >= cap as usize {
                continue;
            }
            let Some(idx) = g.tasks.iter().position(|t| t.status == STATUS_QUEUED) else {
                continue;
            };
            let id = g.tasks[idx].id.clone();
            g.tasks[idx].normalize_state();
            g.tasks[idx].status = STATUS_RUNNING.into();
            g.tasks[idx].original_stage = ORIGINAL_STAGE_EXTRACTING_AUDIO.into();
            g.tasks[idx].translation_stage = if g.tasks[idx].will_translate {
                TRANSLATION_STAGE_WAITING_ORIGINAL.into()
            } else {
                TRANSLATION_STAGE_NOT_REQUIRED.into()
            };
            g.tasks[idx].progress = 5;
            g.tasks[idx].phase = "extract_audio".into();
            g.tasks[idx].updated_at_ms = now_ms();
            let _ = task_store::save_task_store_file(&root.0, &g);
            id
        };
        let ts2 = ts.clone();
        let root2 = root.clone();
        let app2 = app.clone();
        let tid = task_id.clone();
        let slots = llm_slots.clone();
        tokio::spawn(async move {
            let join = tokio::task::spawn_blocking(move || {
                run_one_task(&app2, &root2, &ts2, &tid, &slots)
            })
            .await;
            if let Err(e) = join {
                log::error!("任务执行 join 失败: {e}");
            }
        });
    }
}

fn run_one_task(
    app: &AppHandle,
    root: &AppRoot,
    ts: &Arc<Mutex<TaskStoreFile>>,
    task_id: &str,
    llm_slots: &LlmRequestSlots,
) {
    let cfg = match config_store::load_config(&root.0) {
        Ok(c) => c,
        Err(e) => {
            fail_task(app, ts, root, task_id, &e.to_string());
            return;
        }
    };
    let Some(job) = load_job(ts, task_id, &cfg) else {
        return;
    };

    let video = Path::new(&job.video_path);
    if !video.exists() {
        fail_task(
            app,
            ts,
            root,
            task_id,
            &format!("视频文件不存在: {}", video.display()),
        );
        return;
    }

    let mut cache = match TaskCache::open(&root.0, video) {
        Ok(c) => c,
        Err(e) => {
            fail_task(app, ts, root, task_id, &e);
            return;
        }
    };

    macro_rules! check_pause_cancel {
        () => {
            match mid_run_poll(ts, task_id) {
                MidRun::Continue => {}
                MidRun::Pause => {
                    apply_paused(ts, root, task_id, app);
                    return;
                }
                MidRun::Cancel => {
                    cache.cleanup();
                    remove_task_if_present(app, ts, root, task_id);
                    return;
                }
            }
        };
    }

    // 读取"仅重试翻译"标记与已导出的原字幕路径——开局快照一次，
    // 后续在每个阶段决定是否跳过实际工作 / 是否能从外部 SRT 复用 cues。
    let (retry_translation_only, original_output_path_snapshot) = {
        let g = lock_task_store(ts, "read_retry_flag");
        g.tasks
            .iter()
            .find(|t| t.id == task_id)
            .map(|t| (t.retry_translation_only, t.original_output_path.clone()))
            .unwrap_or((false, None))
    };

    check_pause_cancel!();

    // ---------- 阶段 1：抽取音频 ----------
    let audio_hash = hash_parts(&["audio_16k_mono_v1"]);
    if retry_translation_only || cache.audio_valid(&audio_hash) {
        push_progress(app, ts, root, task_id, 10, "extract_audio");
    } else {
        let ffmpeg = match resolve_ffmpeg(&job.ffmpeg_path) {
            Ok(p) => p,
            Err(e) => {
                fail_task(app, ts, root, task_id, &e);
                return;
            }
        };
        push_progress(app, ts, root, task_id, 5, "extract_audio");
        if let Err(e) = extract_mono_16k_wav(&ffmpeg, video, &cache.audio_path()) {
            fail_task(app, ts, root, task_id, &e);
            return;
        }
        if let Err(e) = cache.mark_audio_done(&audio_hash) {
            fail_task(app, ts, root, task_id, &e);
            return;
        }
        push_progress(app, ts, root, task_id, 10, "extract_audio");
    }

    check_pause_cancel!();

    // ---------- 阶段 2：Whisper 识别 ----------
    let p_tr_start = if job.will_translate { 12u8 } else { 15u8 };
    let recognition_peak = if job.will_translate {
        44u8
    } else if job.segmentation_strategy != "disabled" {
        69u8
    } else {
        89u8
    };

    let threads = effective_threads(job.cpu_thread_limit, cfg.runtime.cpu_thread_limit);
    let rec_lang = job.recognition_lang.clone();
    let force_cpu = !job.whisper_use_gpu;
    let model_id = job.whisper_model.trim();

    let vad_signature = if job.whisper_enable_vad {
        format!(
            "vad:{:.2}:{}:{}:{}",
            job.vad_threshold,
            job.vad_min_speech_ms,
            job.vad_min_silence_ms,
            job.vad_max_segment_ms
        )
    } else {
        "vad:off".to_string()
    };
    let whisper_hash = hash_parts(&[
        &audio_hash,
        model_id,
        rec_lang.trim(),
        &threads.to_string(),
        if force_cpu { "cpu" } else { "gpu" },
        &vad_signature,
    ]);

    {
        let mut g = lock_task_store(ts, "set_transcribing_state");
        if let Some(t) = g.tasks.iter_mut().find(|t| t.id == task_id) {
            t.normalize_state();
            t.status = STATUS_RUNNING.into();
            t.original_stage = ORIGINAL_STAGE_TRANSCRIBING.into();
            t.progress = p_tr_start;
            t.phase = if job.whisper_enable_vad {
                "vad_detect".into()
            } else {
                "transcribe".into()
            };
            t.updated_at_ms = now_ms();
            let _ = task_store::save_task_store_file(&root.0, &g);
        }
    }
    emit_progress(
        app,
        task_id,
        STATUS_RUNNING,
        p_tr_start,
        if job.whisper_enable_vad {
            "vad_detect"
        } else {
            "transcribe"
        },
    );

    check_pause_cancel!();

    if retry_translation_only || cache.whisper_valid(&whisper_hash) {
        push_progress(app, ts, root, task_id, recognition_peak, "transcribe");
    } else {
        let model_path = match model_file_path(&root.0, model_id) {
            Ok(p) => p,
            Err(e) => {
                fail_task(app, ts, root, task_id, &e.to_string());
                return;
            }
        };
        if !model_path.exists() {
            fail_task(
                app,
                ts,
                root,
                task_id,
                &format!("Whisper 模型未下载: {model_id}（{}）", model_path.display()),
            );
            return;
        }

        if job.whisper_cli_path.trim().is_empty() {
            if let Err(e) = whisper_runtime::ensure_managed_whisper_cli(&root.0, |_| {}) {
                fail_task(
                    app,
                    ts,
                    root,
                    task_id,
                    &format!("自动安装 Whisper CLI 失败: {e}"),
                );
                return;
            }
        }

        let whisper_cli = match resolve_whisper_cli(&root.0, &job.whisper_cli_path) {
            Ok(p) => p,
            Err(e) => {
                fail_task(app, ts, root, task_id, &e);
                return;
            }
        };

        let vad_model_path = if job.whisper_enable_vad {
            match whisper_runtime::ensure_managed_whisper_vad_model(
                &root.0,
                &cfg.whisper.mirror_url,
                cfg.whisper.prefer_mirror,
                &cfg.whisper.download_url,
                |_| {},
            ) {
                Ok(p) => Some(p),
                Err(e) => {
                    fail_task(
                        app,
                        ts,
                        root,
                        task_id,
                        &format!("准备 Whisper VAD 模型失败: {e}"),
                    );
                    return;
                }
            }
        } else {
            None
        };
        let whisper_vad = vad_model_path.as_ref().map(|model_path| WhisperVadOptions {
            model_path: model_path.as_path(),
            threshold: job.vad_threshold,
            min_speech_ms: job.vad_min_speech_ms,
            min_silence_ms: job.vad_min_silence_ms,
            max_segment_ms: job.vad_max_segment_ms,
        });

        // 真实进度回调：whisper 内部 0..=100 → 任务进度 p_tr_start..=recognition_peak
        let app_cb = app.clone();
        let ts_cb = ts.clone();
        let root_cb = root.clone();
        let task_id_cb = task_id.to_string();
        let span = recognition_peak.saturating_sub(p_tr_start);
        let progress_cb = boxed_progress(move |whisper_pct: u8| {
            let mapped = p_tr_start
                .saturating_add(((whisper_pct as u32 * span as u32) / 100u32) as u8)
                .min(recognition_peak);
            push_progress(&app_cb, &ts_cb, &root_cb, &task_id_cb, mapped, "transcribe");
        });

        let recognition_prompt = build_recognition_prompt(&job.recognition_glossary);
        if let Err(e) = run_whisper_srt_json(
            &whisper_cli,
            &model_path,
            &cache.audio_path(),
            &rec_lang,
            threads,
            force_cpu,
            &cache.w_prefix(),
            whisper_vad.as_ref(),
            recognition_prompt.as_deref(),
            Some(progress_cb),
        ) {
            fail_task(app, ts, root, task_id, &e);
            return;
        }

        if !cache.w_srt_path().exists() {
            fail_task(
                app,
                ts,
                root,
                task_id,
                "Whisper 未生成 SRT 文件（请确认 whisper.cpp 版本支持 -osrt）",
            );
            return;
        }

        if let Err(e) = cache.mark_whisper_done(&whisper_hash) {
            fail_task(app, ts, root, task_id, &e);
            return;
        }
        push_progress(app, ts, root, task_id, recognition_peak, "transcribe");
    }

    let json_path = cache.w_json_path();
    let lang_for_name =
        if rec_lang.trim().is_empty() || rec_lang.trim().eq_ignore_ascii_case("auto") {
            read_language_from_whisper_json(&json_path, "und")
        } else {
            rec_lang.trim().to_string()
        };

    check_pause_cancel!();

    // ---------- 阶段 3：分句 + 优化（cues 缓存） ----------
    let llm_api_key_storage = if job.translator_engine == "llm"
        || (matches!(job.segmentation_strategy.as_str(), "auto" | "llm_preferred")
            && !job.llm_base_url.trim().is_empty()
            && !job.llm_model.trim().is_empty())
    {
        match secrets::load_secrets(&root.0) {
            Ok(s) => s.llm_api_key.trim().to_string(),
            Err(e) => {
                fail_task(app, ts, root, task_id, &e.to_string());
                return;
            }
        }
    } else {
        String::new()
    };

    let segment_hash = hash_parts(&[
        &whisper_hash,
        job.segmentation_strategy.as_str(),
        job.segmentation_timing_mode.as_str(),
        &job.segmentation_max_chars.to_string(),
        &job.segmentation_max_duration_ms.to_string(),
        &job.segmentation_sentence_gap_ms.to_string(),
        if job.segmentation_strategy != "disabled" {
            job.llm_model.as_str()
        } else {
            ""
        },
    ]);

    let cues: Vec<SubCue> = if cache.segment_valid(&segment_hash) {
        match cache.load_cues() {
            Ok(c) => c,
            Err(e) => {
                fail_task(app, ts, root, task_id, &e);
                return;
            }
        }
    } else if retry_translation_only {
        // 重试翻译模式：先看 cache 的 cues.json（哪怕 stamp 不一致也认），
        // 否则尝试从视频旁已导出的原字幕 SRT 文件解析回 cues。
        if cache.cues_path().is_file() {
            match cache.load_cues() {
                Ok(c) => c,
                Err(e) => {
                    fail_task(app, ts, root, task_id, &e);
                    return;
                }
            }
        } else if let Some(srt_path) = original_output_path_snapshot
            .as_deref()
            .filter(|p| Path::new(p).exists())
        {
            let raw = match fs::read_to_string(srt_path) {
                Ok(s) => s,
                Err(e) => {
                    fail_task(
                        app,
                        ts,
                        root,
                        task_id,
                        &format!("读取原字幕文件失败: {e}（{srt_path}）"),
                    );
                    return;
                }
            };
            let parsed = match parse_srt(&raw) {
                Ok(c) => c,
                Err(e) => {
                    fail_task(app, ts, root, task_id, &e);
                    return;
                }
            };
            if let Err(e) = cache.write_cues(&parsed) {
                fail_task(app, ts, root, task_id, &e);
                return;
            }
            parsed
        } else {
            fail_task(
                app,
                ts,
                root,
                task_id,
                "无法重新翻译：原字幕缓存与已导出文件都已丢失，请用「重试」从头执行",
            );
            return;
        }
    } else {
        let srt_raw = match fs::read_to_string(cache.w_srt_path()) {
            Ok(s) => s,
            Err(e) => {
                fail_task(app, ts, root, task_id, &format!("读取临时 SRT 失败: {e}"));
                return;
            }
        };
        let parsed = match parse_srt(&srt_raw) {
            Ok(c) => c,
            Err(e) => {
                fail_task(app, ts, root, task_id, &e);
                return;
            }
        };

        let mut working = parsed;
        if job.segmentation_strategy != "disabled" {
            {
                let mut g = lock_task_store(ts, "set_segmenting_state");
                if let Some(t) = g.tasks.iter_mut().find(|t| t.id == task_id) {
                    t.normalize_state();
                    t.status = STATUS_RUNNING.into();
                    t.original_stage = ORIGINAL_STAGE_SEGMENTING.into();
                    t.progress = if job.will_translate { 45 } else { 70 };
                    t.phase = "segment_subtitles".into();
                    t.updated_at_ms = now_ms();
                    let _ = task_store::save_task_store_file(&root.0, &g);
                }
            }
            emit_progress(
                app,
                task_id,
                STATUS_RUNNING,
                if job.will_translate { 45 } else { 70 },
                "segment_subtitles",
            );

            let seg_client = match reqwest::blocking::Client::builder()
                .use_rustls_tls()
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    fail_task(
                        app,
                        ts,
                        root,
                        task_id,
                        &format!("HTTP 客户端初始化失败: {e}"),
                    );
                    return;
                }
            };
            let seg_job = SegmentationJob {
                strategy: job.segmentation_strategy.as_str(),
                timing_mode: job.segmentation_timing_mode.as_str(),
                max_chars_per_segment: job.segmentation_max_chars,
                max_duration_ms: job.segmentation_max_duration_ms,
                llm_base_url: job.llm_base_url.as_str(),
                llm_model: job.llm_model.as_str(),
                llm_api_key: llm_api_key_storage.as_str(),
                llm_timeout_sec: job.llm_timeout_sec,
            };

            // 先把 VAD 声学碎片贪心重组成完整句子，再交给 segment_cues 按长度拆回。
            // 词级时间戳可用时（word_timestamps_first）据"强停顿"细化句边界，否则退化为标点+上下限驱动。
            let strong_boundaries = if job.segmentation_timing_mode == "word_timestamps_first" {
                extract_strong_boundaries(&json_path, job.segmentation_sentence_gap_ms as i64)
            } else {
                Vec::new()
            };
            let stitch_opts = StitchOptions {
                strong_boundaries_ms: strong_boundaries,
                ..StitchOptions::default()
            };
            working = stitch_sentences(&working, &stitch_opts);

            match segment_cues(&seg_client, &seg_job, &working, Some(&json_path)) {
                Ok(res) => {
                    working = res.cues;
                    cache_segmentation_note(ts, root, task_id, res.note);
                }
                Err(e) => {
                    fail_task(app, ts, root, task_id, &e);
                    return;
                }
            }

            check_pause_cancel!();
        }

        working = optimize_source_cues(&working);
        apply_recognition_glossary(
            &mut working,
            &job.recognition_glossary,
            job.recognition_glossary_case_sensitive,
        );

        if let Err(e) = cache.write_cues(&working) {
            fail_task(app, ts, root, task_id, &e);
            return;
        }
        if let Err(e) = cache.mark_segment_done(&segment_hash) {
            fail_task(app, ts, root, task_id, &e);
            return;
        }
        working
    };

    let p_tr_done = if job.will_translate { 60u8 } else { 90u8 };
    push_progress(app, ts, root, task_id, p_tr_done, "transcribe");

    // ---------- 阶段 4a：仅原文字幕 — 直接导出 ----------
    if !job.will_translate {
        let final_path = match resolve_original_srt_path(
            video,
            &job.video_path,
            &job.output_dir_mode,
            &job.custom_output_dir,
            &lang_for_name,
        ) {
            Ok(p) => p,
            Err(e) => {
                fail_task(app, ts, root, task_id, &e);
                return;
            }
        };
        if !job.subtitle_overwrite && final_path.exists() {
            fail_task(
                app,
                ts,
                root,
                task_id,
                &format!("目标字幕已存在且配置为不覆盖: {}", final_path.display()),
            );
            return;
        }
        {
            let mut g = lock_task_store(ts, "set_export_original_state");
            if let Some(t) = g.tasks.iter_mut().find(|t| t.id == task_id) {
                t.normalize_state();
                t.status = STATUS_RUNNING.into();
                t.original_stage = ORIGINAL_STAGE_EXPORTING.into();
                t.progress = 95;
                t.phase = "export_srt".into();
                t.updated_at_ms = now_ms();
                let _ = task_store::save_task_store_file(&root.0, &g);
            }
        }
        emit_progress(app, task_id, STATUS_RUNNING, 95, "export_srt");
        if let Some(parent) = final_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                fail_task(app, ts, root, task_id, &format!("创建输出目录失败: {e}"));
                return;
            }
        }
        let body = format_srt(&cues);
        if let Err(e) = fs::write(&final_path, body) {
            fail_task(
                app,
                ts,
                root,
                task_id,
                &format!("写入字幕失败: {e}（{}）", final_path.display()),
            );
            return;
        }
        let original_preview = preview_text(
            &cues
                .iter()
                .take(2)
                .map(|cue| cue.text.clone())
                .collect::<Vec<_>>(),
        );
        cache_task_outputs(
            ts,
            root,
            task_id,
            original_preview,
            None,
            Some(&final_path),
            None,
            None,
        );
        update_task_runtime_state(
            ts,
            root,
            task_id,
            Some(STATUS_COMPLETED),
            Some(ORIGINAL_STAGE_COMPLETED),
            Some(TRANSLATION_STAGE_NOT_REQUIRED),
            Some(100),
            Some(""),
        );
        cache.cleanup();
        succeed_task(app, ts, root, task_id, None, 0);
        return;
    }

    // ---------- 阶段 4b：翻译 + 导出 ----------
    match job.translator_engine.as_str() {
        "google_web" | "llm" => {}
        _ => {
            fail_task(
                app,
                ts,
                root,
                task_id,
                &format!("不支持的翻译引擎: {}", job.translator_engine),
            );
            return;
        }
    }

    if job.subtitle_mode == "original_only" {
        fail_task(
            app,
            ts,
            root,
            task_id,
            "内部错误：仅原文字幕任务不应进入翻译",
        );
        return;
    }

    let src_file_lang = if job.translate_source_lang.trim().is_empty()
        || job
            .translate_source_lang
            .trim()
            .eq_ignore_ascii_case("auto")
    {
        lang_for_name.clone()
    } else {
        job.translate_source_lang.trim().to_string()
    };
    let tgt_file_lang = job.translate_target_lang.trim().to_string();
    if tgt_file_lang.is_empty() {
        fail_task(app, ts, root, task_id, "翻译目标语言为空，请在配置中设置");
        return;
    }

    let (dual_orig, dual_tgt, bio_path): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) =
        match job.subtitle_mode.as_str() {
            "dual_files" => {
                // 重试翻译模式下，原字幕路径优先复用任务记录的实际文件，避免因 lang_for_name
                // 在缺失 whisper json 时退化为 "und" 而生成多余的 .und.srt。
                let a = if let Some(existing) = original_output_path_snapshot
                    .as_deref()
                    .map(PathBuf::from)
                    .filter(|p| retry_translation_only && p.exists())
                {
                    existing
                } else {
                    match resolve_original_srt_path(
                        video,
                        &job.video_path,
                        &job.output_dir_mode,
                        &job.custom_output_dir,
                        &src_file_lang,
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            fail_task(app, ts, root, task_id, &e);
                            return;
                        }
                    }
                };
                let b = match resolve_original_srt_path(
                    video,
                    &job.video_path,
                    &job.output_dir_mode,
                    &job.custom_output_dir,
                    &tgt_file_lang,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        fail_task(app, ts, root, task_id, &e);
                        return;
                    }
                };
                (Some(a), Some(b), None)
            }
            "bilingual_single" => {
                let p = match resolve_bilingual_srt_path(
                    video,
                    &job.video_path,
                    &job.output_dir_mode,
                    &job.custom_output_dir,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        fail_task(app, ts, root, task_id, &e);
                        return;
                    }
                };
                (None, None, Some(p))
            }
            _ => {
                fail_task(
                    app,
                    ts,
                    root,
                    task_id,
                    &format!("未知字幕模式: {}", job.subtitle_mode),
                );
                return;
            }
        };

    // 重试翻译模式天然就是要覆盖上一轮的输出，跳过 overwrite=false 的存在性检查。
    if !job.subtitle_overwrite && !retry_translation_only {
        if let Some(p) = &dual_orig {
            if p.exists() {
                fail_task(
                    app,
                    ts,
                    root,
                    task_id,
                    &format!("目标字幕已存在且配置为不覆盖: {}", p.display()),
                );
                return;
            }
        }
        if let Some(p) = &dual_tgt {
            if p.exists() {
                fail_task(
                    app,
                    ts,
                    root,
                    task_id,
                    &format!("目标字幕已存在且配置为不覆盖: {}", p.display()),
                );
                return;
            }
        }
        if let Some(p) = &bio_path {
            if p.exists() {
                fail_task(
                    app,
                    ts,
                    root,
                    task_id,
                    &format!("目标字幕已存在且配置为不覆盖: {}", p.display()),
                );
                return;
            }
        }
    }

    if job.subtitle_mode == "dual_files" {
        let Some(po) = dual_orig.as_ref() else {
            fail_task(
                app,
                ts,
                root,
                task_id,
                "Internal error: missing original subtitle output path for dual_files mode",
            );
            return;
        };
        // 重试翻译模式下原字幕文件就是 cues 的来源（或与之等价），不重写，避免改动 mtime。
        if !(retry_translation_only && po.exists()) {
            update_task_runtime_state(
                ts,
                root,
                task_id,
                Some(STATUS_RUNNING),
                Some(ORIGINAL_STAGE_EXPORTING),
                Some(TRANSLATION_STAGE_WAITING_ORIGINAL),
                None,
                Some("export_srt"),
            );
            if let Some(parent) = po.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    fail_task(app, ts, root, task_id, &format!("创建输出目录失败: {e}"));
                    return;
                }
            }
            if let Err(e) = fs::write(po, format_srt(&cues)) {
                fail_task(app, ts, root, task_id, &format!("写入原文字幕失败: {e}"));
                return;
            }
        }
        // 立即持久化原字幕路径——即便后续翻译失败，下次点"继续/重试"也能识别到原字幕已就绪。
        cache_original_output_path(ts, root, task_id, po);
    }

    check_pause_cancel!();

    update_task_runtime_state(
        ts,
        root,
        task_id,
        Some(STATUS_RUNNING),
        Some(ORIGINAL_STAGE_COMPLETED),
        Some(TRANSLATION_STAGE_QUEUED),
        Some(60),
        Some(""),
    );

    if job.translator_engine == "llm" && llm_api_key_storage.trim().is_empty() {
        fail_task(
            app,
            ts,
            root,
            task_id,
            "未配置 LLM API Key：请在配置页保存密钥",
        );
        return;
    }

    // 计算 translate stamp 并加载已有 partial
    let translate_hash = hash_parts(&[
        &segment_hash,
        job.translator_engine.as_str(),
        job.translate_source_lang.trim(),
        job.translate_target_lang.trim(),
        if job.translator_engine == "llm" {
            job.llm_model.as_str()
        } else {
            ""
        },
        job.translate_style.as_str(),
    ]);
    if let Err(e) = cache.begin_translate(&translate_hash) {
        fail_task(app, ts, root, task_id, &e);
        return;
    }
    let mut partial = cache.load_partial_translations();

    // 若任务被标记为"仅重试翻译"模式，把上一轮回退到原文的索引从 partial 中移除，
    // 让它们重新进入 remaining_cues 走完整翻译流程。
    {
        let mut g = lock_task_store(ts, "consume_retry_translation_only");
        if let Some(t) = g.tasks.iter_mut().find(|x| x.id == task_id) {
            if t.retry_translation_only {
                let fb = cache.load_fallback_indices();
                if !fb.is_empty() {
                    for idx in &fb {
                        partial.remove(idx);
                    }
                    let _ = cache.save_partial_translations(&partial);
                }
                t.retry_translation_only = false;
                t.updated_at_ms = now_ms();
                let _ = task_store::save_task_store_file(&root.0, &g);
            }
        }
    }

    let mut remaining_cues: Vec<SubCue> = Vec::with_capacity(cues.len());
    let mut remaining_to_orig: Vec<usize> = Vec::with_capacity(cues.len());
    for (orig_idx, cue) in cues.iter().enumerate() {
        if !partial.contains_key(&orig_idx) {
            remaining_cues.push(cue.clone());
            remaining_to_orig.push(orig_idx);
        }
    }

    let initial_progress = compute_translate_progress(partial.len(), cues.len());
    {
        let mut g = lock_task_store(ts, "set_translating_state");
        if let Some(t) = g.tasks.iter_mut().find(|t| t.id == task_id) {
            t.normalize_state();
            t.status = STATUS_RUNNING.into();
            t.original_stage = ORIGINAL_STAGE_COMPLETED.into();
            t.translation_stage = TRANSLATION_STAGE_TRANSLATING.into();
            t.progress = initial_progress;
            t.phase = if job.translator_engine == "google_web" {
                "translate_google".into()
            } else {
                "translate_llm".into()
            };
            t.updated_at_ms = now_ms();
            let _ = task_store::save_task_store_file(&root.0, &g);
        }
    }
    emit_progress(
        app,
        task_id,
        STATUS_RUNNING,
        initial_progress,
        if job.translator_engine == "google_web" {
            "translate_google"
        } else {
            "translate_llm"
        },
    );

    let translate_res: Result<(Vec<String>, Vec<usize>), String> = if remaining_cues.is_empty() {
        Ok((Vec::new(), Vec::new()))
    } else {
        let client = match reqwest::blocking::Client::builder()
            .use_rustls_tls()
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = cache.save_partial_translations(&partial);
                fail_task(
                    app,
                    ts,
                    root,
                    task_id,
                    &format!("HTTP 客户端初始化失败: {e}"),
                );
                return;
            }
        };

        let total_overall = cues.len();
        let already_done = partial.len();
        let phase_label = if job.translator_engine == "google_web" {
            "translate_google"
        } else {
            "translate_llm"
        }
        .to_string();

        let app_prog = app.clone();
        let ts_prog = ts.clone();
        let root_prog = root.clone();
        let tid_prog = task_id.to_string();
        let phase_prog = phase_label.clone();
        let mut on_progress = move |done_remaining: usize, _total_remaining: usize| {
            let total_done = already_done + done_remaining;
            let p = compute_translate_progress(total_done, total_overall);
            push_progress(&app_prog, &ts_prog, &root_prog, &tid_prog, p, &phase_prog);
        };

        let cache_dir_clone = cache.dir().to_path_buf();
        let partial_ref: Arc<Mutex<HashMap<usize, String>>> =
            Arc::new(Mutex::new(std::mem::take(&mut partial)));
        let map_to_orig = remaining_to_orig.clone();
        let partial_for_cb = partial_ref.clone();
        let mut on_item_done = move |rel_idx: usize, text: &str| {
            let Some(&orig_idx) = map_to_orig.get(rel_idx) else {
                return;
            };
            let snapshot: HashMap<String, String> = {
                let mut g = partial_for_cb.lock().unwrap_or_else(|e| e.into_inner());
                g.insert(orig_idx, text.to_string());
                g.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
            };
            if let Ok(body) = serde_json::to_string(&snapshot) {
                let _ = fs::write(cache_dir_clone.join("translations.partial.json"), body);
            }
        };

        let ts_p = ts.clone();
        let tid_p = task_id.to_string();
        let result = if job.translator_engine == "llm" {
            let tjob = TranslateJob {
                base_url: &job.llm_base_url,
                model: &job.llm_model,
                api_key: &llm_api_key_storage,
                timeout_sec: job.llm_timeout_sec.max(5),
                max_retries_per_batch: job.llm_max_retries,
                min_interval_ms: job.translator_min_interval_ms,
                source_lang: job.translate_source_lang.as_str(),
                target_lang: job.translate_target_lang.as_str(),
                style: job.translate_style.as_str(),
                keep_proper_nouns: job.keep_proper_nouns,
                glossary: &job.glossary,
                glossary_case_sensitive: job.glossary_case_sensitive,
            };
            translate_all_cues(
                &client,
                &tjob,
                &remaining_cues,
                job.translate_max_segment_chars,
                move || translation_should_abort(&ts_p, &tid_p),
                &mut on_progress,
                &mut on_item_done,
                Some(llm_slots),
                job.translate_concurrency,
            )
        } else {
            let google_client = match build_google_client(
                job.translator_use_proxy,
                job.llm_timeout_sec.max(5),
            ) {
                Ok(c) => c,
                Err(e) => {
                    partial = match partial_ref.lock() {
                        Ok(g) => g.clone(),
                        Err(p) => p.into_inner().clone(),
                    };
                    let _ = cache.save_partial_translations(&partial);
                    fail_task(app, ts, root, task_id, &e);
                    return;
                }
            };
            let gjob = GoogleWebTranslateJob {
                provider_url: job.translator_provider_url.as_str(),
                use_proxy: job.translator_use_proxy,
                min_interval_ms: job.translator_min_interval_ms,
                source_lang: job.translate_source_lang.as_str(),
                target_lang: job.translate_target_lang.as_str(),
            };
            translate_all_cues_google(
                &google_client,
                &gjob,
                &remaining_cues,
                move || translation_should_abort(&ts_p, &tid_p),
                &mut on_progress,
                &mut on_item_done,
            )
            .map(|t| (t, Vec::new()))
        };

        partial = match partial_ref.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        };
        result
    };

    match translate_res {
        Ok((_remaining_translations, fallback_rel_indices)) => {
            // 将翻译器返回的相对索引（基于 remaining_cues）映射回 cues 的绝对索引
            let fallback_orig: Vec<usize> = fallback_rel_indices
                .iter()
                .filter_map(|&rel| remaining_to_orig.get(rel).copied())
                .collect();
            // 回退片段虽然未保存进 partial，但导出时也要走原文，所以这里把它们写入 translated
            let translated: Vec<String> = (0..cues.len())
                .map(|i| {
                    partial
                        .get(&i)
                        .cloned()
                        .unwrap_or_else(|| cues[i].text.clone())
                })
                .collect();

            {
                let mut g = lock_task_store(ts, "set_translation_export_state");
                if let Some(t) = g.tasks.iter_mut().find(|t| t.id == task_id) {
                    t.normalize_state();
                    t.status = STATUS_RUNNING.into();
                    t.translation_stage = TRANSLATION_STAGE_EXPORTING.into();
                    t.progress = 90;
                    t.phase = "export_srt".into();
                    t.updated_at_ms = now_ms();
                    let _ = task_store::save_task_store_file(&root.0, &g);
                }
            }
            emit_progress(app, task_id, STATUS_RUNNING, 90, "export_srt");

            let r_export = (|| -> Result<(), String> {
                match job.subtitle_mode.as_str() {
                    "dual_files" => {
                        let pb = dual_tgt.as_ref().ok_or_else(|| {
                            "Internal error: missing translated subtitle output path for dual_files mode"
                                .to_string()
                        })?;
                        if let Some(parent) = pb.parent() {
                            fs::create_dir_all(parent)
                                .map_err(|e| format!("创建输出目录失败: {e}"))?;
                        }
                        let trans_cues =
                            optimize_translated_cues(&build_translated_cues(&cues, &translated)?);
                        fs::write(pb, format_srt(&trans_cues))
                            .map_err(|e| format!("写入译文字幕失败: {e}"))?;
                    }
                    "bilingual_single" => {
                        let pb = bio_path.as_ref().ok_or_else(|| {
                            "Internal error: missing bilingual subtitle output path for bilingual_single mode"
                                .to_string()
                        })?;
                        if let Some(parent) = pb.parent() {
                            fs::create_dir_all(parent)
                                .map_err(|e| format!("创建输出目录失败: {e}"))?;
                        }
                        let bio = build_bilingual_cues_optimized(&cues, &translated)?;
                        fs::write(pb, format_srt(&bio))
                            .map_err(|e| format!("写入双语字幕失败: {e}"))?;
                    }
                    _ => {}
                }
                Ok(())
            })();

            match r_export {
                Ok(()) => {
                    let _ = cache.mark_translate_done(&translate_hash);
                    let original_preview = preview_text(
                        &cues
                            .iter()
                            .take(2)
                            .map(|cue| cue.text.clone())
                            .collect::<Vec<_>>(),
                    );
                    let translated_preview =
                        preview_text(&translated.iter().take(2).cloned().collect::<Vec<_>>());
                    cache_task_outputs(
                        ts,
                        root,
                        task_id,
                        original_preview,
                        translated_preview,
                        dual_orig.as_deref(),
                        dual_tgt.as_deref(),
                        bio_path.as_deref(),
                    );
                    let (note, fb_count) = if fallback_orig.is_empty() {
                        cache.clear_fallback_indices();
                        cache.cleanup();
                        (None, 0u32)
                    } else {
                        if let Err(e) = cache.save_fallback_indices(&fallback_orig) {
                            log::warn!(target: "subforge_task", "save_fallback_indices failed: {e}");
                        }
                        (
                            Some("部分片段翻译失败，已回退原文".into()),
                            fallback_orig.len() as u32,
                        )
                    };
                    succeed_task(app, ts, root, task_id, note, fb_count);
                }
                Err(e) => {
                    let _ = cache.save_partial_translations(&partial);
                    fail_task(app, ts, root, task_id, &e);
                }
            }
        }
        Err(e) if e == "__pause__" => {
            let _ = cache.save_partial_translations(&partial);
            match mid_run_poll(ts, task_id) {
                MidRun::Pause => apply_paused(ts, root, task_id, app),
                _ => {
                    cache.cleanup();
                    remove_task_if_present(app, ts, root, task_id);
                }
            }
        }
        Err(e) => {
            let _ = cache.save_partial_translations(&partial);
            fail_task(app, ts, root, task_id, &e);
        }
    }
}

fn compute_translate_progress(done: usize, total: usize) -> u8 {
    if total == 0 {
        return 62;
    }
    let p = 62u32 + ((done as u32).saturating_mul(26) / total as u32).min(26);
    p.min(88) as u8
}

#[cfg(test)]
mod tests {
    use super::occupies_runner_slot;
    use crate::domain::task::{
        STATUS_PAUSE_REQUESTED, STATUS_PENDING, STATUS_QUEUED, STATUS_RUNNING,
    };

    #[test]
    fn queued_tasks_do_not_consume_runner_slots() {
        assert!(!occupies_runner_slot(STATUS_PENDING));
        assert!(!occupies_runner_slot(STATUS_QUEUED));
        assert!(occupies_runner_slot(STATUS_RUNNING));
        assert!(occupies_runner_slot(STATUS_PAUSE_REQUESTED));
    }
}
