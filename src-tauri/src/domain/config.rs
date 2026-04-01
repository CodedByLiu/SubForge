use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AppConfig {
    pub llm: LlmPublicConfig,
    pub translator: TranslatorConfig,
    pub whisper: WhisperConfig,
    pub translate: TranslateConfig,
    #[serde(default)]
    pub segmentation: SegmentationConfig,
    pub subtitle: SubtitleConfig,
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmPublicConfig {
    pub base_url: String,
    pub model: String,
    pub timeout_sec: u32,
    pub max_retries: u32,
    pub translate_concurrency: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatorConfig {
    pub engine: String,
    pub provider_url: String,
    #[serde(default)]
    pub use_proxy: bool,
    #[serde(default)]
    pub min_request_interval_ms: u32,
    #[serde(default)]
    pub experimental_acknowledged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperConfig {
    pub model: String,
    #[serde(default = "default_true")]
    pub use_gpu: bool,
    pub recognition_lang: String,
    #[serde(default = "default_true")]
    pub enable_vad: bool,
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,
    #[serde(default = "default_vad_min_speech_ms")]
    pub vad_min_speech_ms: u32,
    #[serde(default = "default_vad_min_silence_ms")]
    pub vad_min_silence_ms: u32,
    #[serde(default = "default_vad_max_segment_ms")]
    pub vad_max_segment_ms: u32,
    /// 留空则从 PATH 查找 `ffmpeg` / `ffmpeg.exe`
    #[serde(default)]
    pub ffmpeg_path: String,
    /// whisper.cpp 可执行文件；留空则尝试 `whisper-cli` / `main` 等
    #[serde(default)]
    pub whisper_cli_path: String,
    pub download_url: String,
    pub mirror_url: String,
    #[serde(default = "default_true")]
    pub prefer_mirror: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub source: String,
    pub target: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslateConfig {
    pub source_lang: String,
    pub target_lang: String,
    pub style: String,
    pub max_segment_chars: u32,
    #[serde(default)]
    pub keep_proper_nouns_in_source: bool,
    #[serde(default)]
    pub glossary_case_sensitive: bool,
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentationConfig {
    pub strategy: String,
    pub max_chars_per_segment: u32,
    pub max_duration_seconds: f64,
    pub timing_mode: String,
}

impl Default for SegmentationConfig {
    fn default() -> Self {
        Self {
            strategy: "auto".into(),
            max_chars_per_segment: 42,
            max_duration_seconds: 6.0,
            timing_mode: "word_timestamps_first".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleConfig {
    pub mode: String,
    #[serde(default = "default_srt")]
    pub format: String,
    pub output_dir_mode: String,
    pub custom_output_dir: String,
    #[serde(default = "default_true")]
    pub overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    #[serde(default = "default_true")]
    pub auto_detect_hardware: bool,
    pub max_parallel_tasks: u32,
    pub cpu_thread_limit: u32,
    pub task_auto_retry_max: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfigView {
    #[serde(flatten)]
    pub config: AppConfig,
    pub api_key_configured: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SaveConfigRequest {
    #[serde(flatten)]
    pub config: AppConfig,
    #[serde(default)]
    pub llm_api_key: Option<String>,
    #[serde(default)]
    pub clear_llm_api_key: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestLlmRequest {
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub timeout_sec: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmTestResult {
    pub ok: bool,
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_srt() -> String {
    "srt".into()
}

fn default_vad_threshold() -> f32 {
    0.5
}

fn default_vad_min_speech_ms() -> u32 {
    500
}

fn default_vad_min_silence_ms() -> u32 {
    300
}

fn default_vad_max_segment_ms() -> u32 {
    30_000
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            llm: LlmPublicConfig {
                base_url: String::new(),
                model: String::new(),
                timeout_sec: 60,
                max_retries: 3,
                translate_concurrency: 2,
            },
            translator: TranslatorConfig {
                engine: "none".into(),
                provider_url: String::new(),
                use_proxy: false,
                min_request_interval_ms: 1000,
                experimental_acknowledged: false,
            },
            whisper: WhisperConfig {
                model: "base".into(),
                use_gpu: true,
                recognition_lang: "auto".into(),
                enable_vad: true,
                vad_threshold: default_vad_threshold(),
                vad_min_speech_ms: default_vad_min_speech_ms(),
                vad_min_silence_ms: default_vad_min_silence_ms(),
                vad_max_segment_ms: default_vad_max_segment_ms(),
                ffmpeg_path: String::new(),
                whisper_cli_path: String::new(),
                download_url: String::new(),
                mirror_url: String::new(),
                prefer_mirror: true,
            },
            translate: TranslateConfig {
                source_lang: "auto".into(),
                target_lang: "zh".into(),
                style: "term_first".into(),
                max_segment_chars: 800,
                keep_proper_nouns_in_source: true,
                glossary_case_sensitive: false,
                glossary: Vec::new(),
            },
            segmentation: SegmentationConfig::default(),
            subtitle: SubtitleConfig {
                mode: "bilingual_single".into(),
                format: "srt".into(),
                output_dir_mode: "video_dir".into(),
                custom_output_dir: String::new(),
                overwrite: true,
            },
            runtime: RuntimeConfig {
                auto_detect_hardware: true,
                max_parallel_tasks: 2,
                cpu_thread_limit: 8,
                task_auto_retry_max: 0,
            },
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.llm.timeout_sec == 0 {
            return Err("LLM 超时时间必须大于 0".into());
        }
        if self.llm.max_retries > 20 {
            return Err("最大重试次数过大".into());
        }
        if self.llm.translate_concurrency == 0 || self.llm.translate_concurrency > 64 {
            return Err("翻译并发数需在 1–64 之间".into());
        }
        if self.translate.max_segment_chars < 64 || self.translate.max_segment_chars > 32000 {
            return Err("每段最大字符数需在合理范围内".into());
        }
        if !(0.1..=0.9).contains(&self.whisper.vad_threshold) {
            return Err("VAD 阈值需在 0.1–0.9 之间".into());
        }
        if self.whisper.vad_min_speech_ms < 100 || self.whisper.vad_min_speech_ms > 5000 {
            return Err("VAD 最小语音时长需在 100–5000 毫秒之间".into());
        }
        if self.whisper.vad_min_silence_ms < 50 || self.whisper.vad_min_silence_ms > 3000 {
            return Err("VAD 最小静音时长需在 50–3000 毫秒之间".into());
        }
        if self.whisper.vad_max_segment_ms < 3000 || self.whisper.vad_max_segment_ms > 30000 {
            return Err("VAD 单段最大语音时长需在 3000–30000 毫秒之间".into());
        }
        if self.whisper.vad_max_segment_ms <= self.whisper.vad_min_speech_ms {
            return Err("VAD 单段最大语音时长需大于最小语音时长".into());
        }
        if self.segmentation.max_chars_per_segment < 8
            || self.segmentation.max_chars_per_segment > 500
        {
            return Err("原字幕单条最大字符数需在 8–500 之间".into());
        }
        if self.segmentation.max_duration_seconds <= 0.5
            || self.segmentation.max_duration_seconds > 60.0
        {
            return Err("原字幕单条最大持续时长需在 0.5–60 秒之间".into());
        }
        if self.runtime.max_parallel_tasks == 0 || self.runtime.max_parallel_tasks > 16 {
            return Err("最大并发任务数需在 1–16 之间".into());
        }
        if self.runtime.cpu_thread_limit == 0 || self.runtime.cpu_thread_limit > 256 {
            return Err("CPU 线程上限无效".into());
        }
        if self.runtime.task_auto_retry_max > 10 {
            return Err("任务自动重试次数过大".into());
        }
        let engine = self.translator.engine.as_str();
        if !matches!(engine, "none" | "llm" | "google_web") {
            return Err("翻译引擎类型无效".into());
        }
        let seg = self.segmentation.strategy.as_str();
        if !matches!(seg, "disabled" | "auto" | "rules_only" | "llm_preferred") {
            return Err("原字幕分段策略无效".into());
        }
        let timing = self.segmentation.timing_mode.as_str();
        if !matches!(timing, "word_timestamps_first" | "approximate_reflow") {
            return Err("分段时间策略无效".into());
        }
        let mode = self.subtitle.mode.as_str();
        if !matches!(mode, "original_only" | "dual_files" | "bilingual_single") {
            return Err("字幕生成模式无效".into());
        }
        if self.subtitle.output_dir_mode != "video_dir" && self.subtitle.output_dir_mode != "custom"
        {
            return Err("输出目录模式无效".into());
        }
        if self.subtitle.output_dir_mode == "custom"
            && self.subtitle.custom_output_dir.trim().is_empty()
        {
            return Err("自定义输出目录不能为空".into());
        }
        Ok(())
    }

    /// §6.2：是否执行翻译（字幕模式非「仅原文」且引擎非关闭）
    pub fn will_run_translation(&self) -> bool {
        self.subtitle.mode != "original_only" && self.translator.engine != "none"
    }

    pub fn has_llm_endpoint_config(&self) -> bool {
        !self.llm.base_url.trim().is_empty() && !self.llm.model.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn translation_is_disabled_for_original_only_mode() {
        let mut cfg = AppConfig::default();
        cfg.translator.engine = "llm".into();
        cfg.subtitle.mode = "original_only".into();
        assert!(!cfg.will_run_translation());
    }

    #[test]
    fn translation_is_disabled_when_engine_is_none() {
        let mut cfg = AppConfig::default();
        cfg.translator.engine = "none".into();
        cfg.subtitle.mode = "dual_files".into();
        assert!(!cfg.will_run_translation());
    }

    #[test]
    fn translation_runs_when_mode_and_engine_are_enabled() {
        let mut cfg = AppConfig::default();
        cfg.translator.engine = "llm".into();
        cfg.subtitle.mode = "bilingual_single".into();
        assert!(cfg.will_run_translation());
    }

    #[test]
    fn validate_rejects_invalid_parallel_limit() {
        let mut cfg = AppConfig::default();
        cfg.runtime.max_parallel_tasks = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_custom_output_without_path() {
        let mut cfg = AppConfig::default();
        cfg.subtitle.output_dir_mode = "custom".into();
        cfg.subtitle.custom_output_dir.clear();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_vad_threshold() {
        let mut cfg = AppConfig::default();
        cfg.whisper.vad_threshold = 0.95;
        assert!(cfg.validate().is_err());
    }
}
