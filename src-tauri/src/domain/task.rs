use serde::{Deserialize, Serialize};

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_QUEUED: &str = "queued";
pub const STATUS_PAUSE_REQUESTED: &str = "pause_requested";
pub const STATUS_EXTRACTING: &str = "extracting_audio";
pub const STATUS_TRANSCRIBING: &str = "transcribing";
pub const STATUS_SEGMENTING: &str = "segmenting";
pub const STATUS_TRANSLATING: &str = "translating";
pub const STATUS_PAUSED: &str = "paused";
pub const STATUS_COMPLETED: &str = "completed";
pub const STATUS_FAILED: &str = "failed";

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
    /// 点击「开始」时冻结的运行参数（空则 runner 回退读当前配置）
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
    /// 成功完成时的非错误提示（如部分片段回退原文）
    #[serde(default)]
    pub translate_note: Option<String>,
    /// 用户删除执行中任务时置位，runner 边界处移除任务
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub error_message: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl TaskRecord {
    pub fn is_active_pipeline(&self) -> bool {
        matches!(
            self.status.as_str(),
            STATUS_EXTRACTING
                | STATUS_TRANSCRIBING
                | STATUS_SEGMENTING
                | STATUS_TRANSLATING
                | STATUS_PAUSE_REQUESTED
        )
    }

    pub fn original_status_label(&self) -> String {
        if self.cancel_requested && self.is_active_pipeline() {
            return "删除请求中".into();
        }
        match self.status.as_str() {
            STATUS_PENDING => "待开始".into(),
            STATUS_QUEUED => "排队中".into(),
            STATUS_PAUSE_REQUESTED => "暂停请求中".into(),
            STATUS_EXTRACTING => "提取音频中".into(),
            STATUS_TRANSCRIBING => "识别中".into(),
            STATUS_SEGMENTING => "原字幕分段中".into(),
            STATUS_TRANSLATING => "翻译中".into(),
            STATUS_PAUSED => "已暂停".into(),
            STATUS_COMPLETED => "已完成".into(),
            STATUS_FAILED => "失败".into(),
            _ => self.status.clone(),
        }
    }

    pub fn translate_status_label(&self) -> String {
        if !self.will_translate {
            return "-".to_string();
        }
        if self.cancel_requested && self.is_active_pipeline() {
            return "删除请求中".into();
        }
        match self.status.as_str() {
            STATUS_PENDING | STATUS_QUEUED => "待开始".into(),
            STATUS_EXTRACTING | STATUS_TRANSCRIBING | STATUS_SEGMENTING => "-".into(),
            STATUS_TRANSLATING => "翻译中".into(),
            STATUS_PAUSE_REQUESTED => "暂停请求中".into(),
            STATUS_PAUSED => "已暂停".into(),
            STATUS_COMPLETED => {
                if let Some(n) = &self.translate_note {
                    format!("已完成（{n}）")
                } else {
                    "已完成".into()
                }
            }
            STATUS_FAILED => "—".into(),
            _ => "-".into(),
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

fn task_store_version() -> u32 {
    1
}
