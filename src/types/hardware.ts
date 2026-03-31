export interface GpuInfoDto {
  name: string;
  memory_total_mb: number | null;
}

export interface HardwareInfoDto {
  cpu_brand: string;
  cpu_logical_cores: number;
  cpu_physical_cores: number;
  memory_total_mb: number;
  memory_available_mb: number;
  gpus: GpuInfoDto[];
  nvidia_nvml_available: boolean;
  whisper_recommended_models: string[];
  whisper_note: string;
}

export interface WhisperModelRowDto {
  id: string;
  file_name: string;
  size_bytes_estimate: number;
  downloaded: boolean;
  local_size_bytes: number | null;
}

export interface WhisperModelsListDto {
  models: WhisperModelRowDto[];
  models_dir: string;
  download_base_used: string;
}

export interface WhisperDownloadProgress {
  model_id: string;
  percent: number;
  bytes_received: number;
  bytes_total: number | null;
  phase: string;
  message: string;
}

export interface WhisperRuntimeProgress {
  phase: string;
  message: string;
  percent: number;
  bytes_received: number;
  bytes_total: number | null;
}
