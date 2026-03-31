import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  AppConfigView,
  AppInfo,
  LlmTestResult,
} from "@/types/config";

export async function getAppInfo(): Promise<AppInfo> {
  return invoke("get_app_info");
}

export async function getConfig(): Promise<AppConfigView> {
  return invoke("get_config");
}

export interface SaveConfigPayload extends AppConfig {
  llm_api_key?: string | null;
  clear_llm_api_key?: boolean;
}

export async function saveConfig(payload: SaveConfigPayload): Promise<AppConfigView> {
  return invoke("save_config", { req: payload });
}

export interface TestLlmPayload {
  base_url: string;
  model: string;
  api_key?: string | null;
  timeout_sec: number;
}

export async function testLlmConnection(
  payload: TestLlmPayload,
): Promise<LlmTestResult> {
  return invoke("test_llm_connection", { req: payload });
}

export async function getHardwareInfo(
  useWhisperGpu: boolean,
): Promise<import("@/types/hardware").HardwareInfoDto> {
  return invoke("get_hardware_info", { useWhisperGpu });
}

export async function listWhisperModels(): Promise<
  import("@/types/hardware").WhisperModelsListDto
> {
  return invoke("list_whisper_models");
}

export async function deleteWhisperModel(modelId: string): Promise<void> {
  return invoke("delete_whisper_model", { modelId });
}

export async function downloadWhisperModel(modelId: string): Promise<void> {
  return invoke("download_whisper_model", { modelId });
}
