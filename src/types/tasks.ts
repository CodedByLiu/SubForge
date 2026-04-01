export interface TaskRowDto {
  id: string;
  video_path: string;
  file_name: string;
  file_size: number;
  duration_sec: number | null;
  status: string;
  progress: number;
  phase: string;
  will_translate: boolean;
  retry_attempts: number;
  cancel_requested: boolean;
  snapshot_summary: string;
  original_status_display: string;
  translate_status_display: string;
  original_preview: string | null;
  translated_preview: string | null;
  error_message: string | null;
}

export interface TaskListPanel {
  output_dir_mode: string;
  custom_output_dir: string;
  show_translate_column: boolean;
  has_active_pipeline: boolean;
  needs_progress_refresh: boolean;
  tasks: TaskRowDto[];
}

export interface TranscribeDepsCheck {
  ffmpeg_resolved: string | null;
  ffmpeg_ok: boolean;
  ffmpeg_detail: string;
  whisper_resolved: string | null;
  whisper_ok: boolean;
  whisper_detail: string;
  vad_enabled: boolean;
  vad_model_path: string | null;
  vad_ok: boolean;
  vad_detail: string;
  model_path: string | null;
  model_ok: boolean;
  model_detail: string;
}

export interface ImportVideosResult {
  added: number;
  skipped_duplicates: number;
  skipped_invalid: number;
}
