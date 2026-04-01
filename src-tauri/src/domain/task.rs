use serde::{Deserialize, Serialize};

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_PAUSE_REQUESTED: &str = "pause_requested";
pub const STATUS_PAUSED: &str = "paused";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_FAILED: &str = "failed";

pub const ORIGINAL_STAGE_WAITING: &str = "waiting";
pub const ORIGINAL_STAGE_EXTRACTING_AUDIO: &str = "extracting_audio";
pub const ORIGINAL_STAGE_TRANSCRIBING: &str = "transcribing";
pub const ORIGINAL_STAGE_SEGMENTING: &str = "segmenting";
pub const ORIGINAL_STAGE_EXPORTING: &str = "exporting";
pub const ORIGINAL_STAGE_COMPLETED: &str = "completed";
pub const ORIGINAL_STAGE_FAILED: &str = "failed";

pub const TRANSLATION_STAGE_NOT_REQUIRED: &str = "not_required";
pub const TRANSLATION_STAGE_WAITING_ORIGINAL: &str = "waiting_original";
pub const TRANSLATION_STAGE_QUEUED: &str = "queued";
pub const TRANSLATION_STAGE_TRANSLATING: &str = "translating";
pub const TRANSLATION_STAGE_EXPORTING: &str = "exporting";
pub const TRANSLATION_STAGE_COMPLETED: &str = "completed";
pub const TRANSLATION_STAGE_FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TaskRecord {
    pub id: String,
    pub video_path: String,
    pub file_name: String,
    pub file_size: u64,
    #[serde(default)]
    pub duration_sec: Option<f64>,
    pub status: String,
    #[serde(default = "default_original_stage")]
    pub original_stage: String,
    #[serde(default = "default_translation_stage")]
    pub translation_stage: String,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub phase: String,
    pub will_translate: bool,
    pub translator_engine_snapshot: String,
    pub subtitle_mode_snapshot: String,
    pub translate_source_lang_snapshot: String,
    pub translate_target_lang_snapshot: String,
    #[serde(default)]
    pub segmentation_strategy_snapshot: String,
    #[serde(default)]
    pub segmentation_timing_mode_snapshot: String,
    #[serde(default)]
    pub snapshot_id: String,
    #[serde(default)]
    pub snapshot_summary: String,
    #[serde(default)]
    pub snapshot_whisper_model: String,
    #[serde(default)]
    pub snapshot_recognition_lang: String,
    #[serde(default)]
    pub snapshot_whisper_use_gpu: bool,
    #[serde(default)]
    pub snapshot_enable_vad: bool,
    #[serde(default)]
    pub snapshot_vad_threshold: f32,
    #[serde(default)]
    pub snapshot_vad_min_speech_ms: u32,
    #[serde(default)]
    pub snapshot_vad_min_silence_ms: u32,
    #[serde(default)]
    pub snapshot_vad_max_segment_ms: u32,
    #[serde(default)]
    pub snapshot_subtitle_overwrite: bool,
    #[serde(default)]
    pub snapshot_output_dir_mode: String,
    #[serde(default)]
    pub snapshot_custom_output_dir: String,
    #[serde(default)]
    pub snapshot_ffmpeg_path: String,
    #[serde(default)]
    pub snapshot_whisper_cli_path: String,
    #[serde(default)]
    pub snapshot_cpu_thread_limit: u32,
    #[serde(default)]
    pub snapshot_translate_style: String,
    #[serde(default)]
    pub snapshot_translate_max_segment_chars: u32,
    #[serde(default)]
    pub snapshot_segmentation_max_chars: u32,
    #[serde(default)]
    pub snapshot_segmentation_max_duration_ms: u32,
    #[serde(default)]
    pub snapshot_llm_base_url: String,
    #[serde(default)]
    pub snapshot_llm_model: String,
    #[serde(default)]
    pub snapshot_llm_timeout_sec: u32,
    #[serde(default)]
    pub snapshot_llm_max_retries: u32,
    #[serde(default)]
    pub snapshot_keep_proper_nouns: bool,
    #[serde(default)]
    pub snapshot_glossary_case_sensitive: bool,
    #[serde(default)]
    pub snapshot_translate_glossary_json: String,
    #[serde(default)]
    pub snapshot_translator_min_interval_ms: u32,
    #[serde(default)]
    pub snapshot_llm_translate_concurrency: u32,
    #[serde(default)]
    pub snapshot_translator_provider_url: String,
    #[serde(default)]
    pub snapshot_translator_use_proxy: bool,
    #[serde(default)]
    pub snapshot_task_auto_retry_max: u32,
    #[serde(default)]
    pub retry_attempts: u32,
    #[serde(default)]
    pub original_preview: Option<String>,
    #[serde(default)]
    pub translated_preview: Option<String>,
    #[serde(default)]
    pub segmentation_note: Option<String>,
    #[serde(default)]
    pub original_output_path: Option<String>,
    #[serde(default)]
    pub translated_output_path: Option<String>,
    #[serde(default)]
    pub bilingual_output_path: Option<String>,
    #[serde(default)]
    pub translate_note: Option<String>,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

fn default_original_stage() -> String {
    ORIGINAL_STAGE_WAITING.into()
}

fn default_translation_stage() -> String {
    TRANSLATION_STAGE_NOT_REQUIRED.into()
}

fn task_store_version() -> u32 {
    1
}

fn original_stage_terminal(stage: &str) -> bool {
    matches!(stage, ORIGINAL_STAGE_COMPLETED | ORIGINAL_STAGE_FAILED)
}

fn translation_stage_terminal(stage: &str) -> bool {
    matches!(
        stage,
        TRANSLATION_STAGE_NOT_REQUIRED | TRANSLATION_STAGE_COMPLETED | TRANSLATION_STAGE_FAILED
    )
}

fn translation_stage_active(stage: &str) -> bool {
    matches!(
        stage,
        TRANSLATION_STAGE_QUEUED | TRANSLATION_STAGE_TRANSLATING | TRANSLATION_STAGE_EXPORTING
    )
}

impl TaskRecord {
    pub fn normalize_state(&mut self) {
        if self.original_stage.is_empty() || self.translation_stage.is_empty() {
            self.apply_legacy_status_mapping();
        }
        if !self.will_translate && self.translation_stage != TRANSLATION_STAGE_FAILED {
            self.translation_stage = TRANSLATION_STAGE_NOT_REQUIRED.into();
        } else if self.will_translate && self.translation_stage == TRANSLATION_STAGE_NOT_REQUIRED {
            self.translation_stage = if self.status == STATUS_COMPLETED {
                TRANSLATION_STAGE_COMPLETED.into()
            } else {
                TRANSLATION_STAGE_WAITING_ORIGINAL.into()
            };
        }
        if self.status.is_empty() {
            self.status = STATUS_PENDING.into();
        }
    }

    fn apply_legacy_status_mapping(&mut self) {
        match self.status.as_str() {
            STATUS_PENDING => {
                self.original_stage = ORIGINAL_STAGE_WAITING.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            STATUS_QUEUED => {
                self.original_stage = ORIGINAL_STAGE_WAITING.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            "extracting_audio" => {
                self.status = STATUS_RUNNING.into();
                self.original_stage = ORIGINAL_STAGE_EXTRACTING_AUDIO.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            "transcribing" => {
                self.status = STATUS_RUNNING.into();
                self.original_stage = ORIGINAL_STAGE_TRANSCRIBING.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            "segmenting" => {
                self.status = STATUS_RUNNING.into();
                self.original_stage = ORIGINAL_STAGE_SEGMENTING.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            "translating" => {
                self.status = STATUS_RUNNING.into();
                self.original_stage = ORIGINAL_STAGE_COMPLETED.into();
                self.translation_stage = TRANSLATION_STAGE_TRANSLATING.into();
            }
            STATUS_PAUSE_REQUESTED => {
                self.original_stage = ORIGINAL_STAGE_WAITING.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            STATUS_PAUSED => {
                self.original_stage = ORIGINAL_STAGE_WAITING.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            STATUS_COMPLETED => {
                self.original_stage = ORIGINAL_STAGE_COMPLETED.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_COMPLETED.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
            STATUS_FAILED => {
                if self.will_translate {
                    self.original_stage = ORIGINAL_STAGE_COMPLETED.into();
                    self.translation_stage = TRANSLATION_STAGE_FAILED.into();
                } else {
                    self.original_stage = ORIGINAL_STAGE_FAILED.into();
                    self.translation_stage = TRANSLATION_STAGE_NOT_REQUIRED.into();
                }
            }
            _ => {
                self.original_stage = ORIGINAL_STAGE_WAITING.into();
                self.translation_stage = if self.will_translate {
                    TRANSLATION_STAGE_WAITING_ORIGINAL.into()
                } else {
                    TRANSLATION_STAGE_NOT_REQUIRED.into()
                };
            }
        }
    }

    pub fn is_active_pipeline(&self) -> bool {
        matches!(
            self.status.as_str(),
            STATUS_QUEUED | STATUS_RUNNING | STATUS_PAUSE_REQUESTED
        )
    }
    pub fn mark_failed(&mut self) {
        self.status = STATUS_FAILED.into();
        if !original_stage_terminal(&self.original_stage) {
            self.original_stage = ORIGINAL_STAGE_FAILED.into();
            if self.translation_stage != TRANSLATION_STAGE_NOT_REQUIRED {
                self.translation_stage = TRANSLATION_STAGE_WAITING_ORIGINAL.into();
            }
            return;
        }
        if self.will_translate && !translation_stage_terminal(&self.translation_stage) {
            self.translation_stage = TRANSLATION_STAGE_FAILED.into();
        }
    }

    pub fn original_status_label(&self) -> String {
        if self.cancel_requested && self.is_active_pipeline() {
            return "删除请求中".into();
        }
        if self.status == STATUS_PAUSE_REQUESTED && !original_stage_terminal(&self.original_stage) {
            return "暂停请求中".into();
        }
        if self.status == STATUS_PAUSED && !original_stage_terminal(&self.original_stage) {
            return "已暂停".into();
        }
        if self.status == STATUS_FAILED && self.original_stage == ORIGINAL_STAGE_FAILED {
            return "失败".into();
        }
        match self.original_stage.as_str() {
            ORIGINAL_STAGE_WAITING => "待开始".into(),
            ORIGINAL_STAGE_EXTRACTING_AUDIO => "提取音频中".into(),
            ORIGINAL_STAGE_TRANSCRIBING => "识别中".into(),
            ORIGINAL_STAGE_SEGMENTING => "原字幕分段中".into(),
            ORIGINAL_STAGE_EXPORTING => "导出中".into(),
            ORIGINAL_STAGE_COMPLETED => "已完成".into(),
            ORIGINAL_STAGE_FAILED => "失败".into(),
            _ => self.original_stage.clone(),
        }
    }

    pub fn translate_status_label(&self) -> String {
        if !self.will_translate {
            return "-".into();
        }
        if self.cancel_requested && self.is_active_pipeline() {
            return "删除请求中".into();
        }
        if self.status == STATUS_PAUSE_REQUESTED
            && translation_stage_active(&self.translation_stage)
        {
            return "暂停请求中".into();
        }
        if self.status == STATUS_PAUSED && translation_stage_active(&self.translation_stage) {
            return "已暂停".into();
        }
        if self.status == STATUS_FAILED && self.translation_stage == TRANSLATION_STAGE_FAILED {
            return "失败".into();
        }
        match self.translation_stage.as_str() {
            TRANSLATION_STAGE_NOT_REQUIRED => "-".into(),
            TRANSLATION_STAGE_WAITING_ORIGINAL => "等待原字幕".into(),
            TRANSLATION_STAGE_QUEUED => "待翻译".into(),
            TRANSLATION_STAGE_TRANSLATING => "翻译中".into(),
            TRANSLATION_STAGE_EXPORTING => "导出中".into(),
            TRANSLATION_STAGE_COMPLETED => {
                if let Some(n) = &self.translate_note {
                    format!("已完成（{n}）")
                } else {
                    "已完成".into()
                }
            }
            TRANSLATION_STAGE_FAILED => "失败".into(),
            _ => self.translation_stage.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskStoreFile {
    #[serde(default = "task_store_version")]
    pub version: u32,
    #[serde(default)]
    pub output_dir_mode: String,
    #[serde(default)]
    pub custom_output_dir: String,
    #[serde(default)]
    pub tasks: Vec<TaskRecord>,
}
