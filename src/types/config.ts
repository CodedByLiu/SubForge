export interface LlmPublicConfig {
  base_url: string;
  model: string;
  timeout_sec: number;
  max_retries: number;
  translate_concurrency: number;
}

export interface TranslatorConfig {
  engine: string;
  provider_url: string;
  use_proxy: boolean;
  min_request_interval_ms: number;
}

export interface WhisperConfig {
  model: string;
  use_gpu: boolean;
  recognition_lang: string;
  enable_vad: boolean;
  vad_threshold: number;
  vad_min_speech_ms: number;
  vad_min_silence_ms: number;
  vad_max_segment_ms: number;
  ffmpeg_path: string;
  whisper_cli_path: string;
  download_url: string;
  mirror_url: string;
  prefer_mirror: boolean;
}

export interface GlossaryEntry {
  source: string;
  target: string;
  note: string;
}

export interface TranslateConfig {
  source_lang: string;
  target_lang: string;
  style: string;
  max_segment_chars: number;
  keep_proper_nouns_in_source: boolean;
  glossary_case_sensitive: boolean;
  glossary: GlossaryEntry[];
}

export interface SegmentationConfig {
  strategy: string;
  max_chars_per_segment: number;
  max_duration_seconds: number;
  timing_mode: string;
}

export interface SubtitleConfig {
  mode: string;
  format: string;
  output_dir_mode: string;
  custom_output_dir: string;
  overwrite: boolean;
}

export interface RuntimeConfig {
  auto_detect_hardware: boolean;
  max_parallel_tasks: number;
  cpu_thread_limit: number;
  task_auto_retry_max: number;
}

export interface AppConfig {
  llm: LlmPublicConfig;
  translator: TranslatorConfig;
  whisper: WhisperConfig;
  translate: TranslateConfig;
  segmentation: SegmentationConfig;
  subtitle: SubtitleConfig;
  runtime: RuntimeConfig;
}

export interface AppConfigView extends AppConfig {
  api_key_configured: boolean;
}

export interface LlmTestResult {
  ok: boolean;
  code: string;
  message: string;
  detail?: string;
}

export interface AppInfo {
  app_dir: string;
  version: string;
}

export function defaultAppConfig(): AppConfig {
  return {
    llm: {
      base_url: "",
      model: "",
      timeout_sec: 60,
      max_retries: 3,
      translate_concurrency: 2,
    },
    translator: {
      engine: "none",
      provider_url: "",
      use_proxy: false,
      min_request_interval_ms: 1000,
    },
    whisper: {
      model: "base",
      use_gpu: true,
      recognition_lang: "auto",
      enable_vad: true,
      vad_threshold: 0.5,
      vad_min_speech_ms: 500,
      vad_min_silence_ms: 300,
      vad_max_segment_ms: 30_000,
      ffmpeg_path: "",
      whisper_cli_path: "",
      download_url: "",
      mirror_url: "",
      prefer_mirror: true,
    },
    translate: {
      source_lang: "auto",
      target_lang: "zh",
      style: "term_first",
      max_segment_chars: 800,
      keep_proper_nouns_in_source: true,
      glossary_case_sensitive: false,
      glossary: [],
    },
    segmentation: {
      strategy: "auto",
      max_chars_per_segment: 42,
      max_duration_seconds: 6,
      timing_mode: "word_timestamps_first",
    },
    subtitle: {
      mode: "bilingual_single",
      format: "srt",
      output_dir_mode: "video_dir",
      custom_output_dir: "",
      overwrite: true,
    },
    runtime: {
      auto_detect_hardware: true,
      max_parallel_tasks: 2,
      cpu_thread_limit: 8,
      task_auto_retry_max: 0,
    },
  };
}
