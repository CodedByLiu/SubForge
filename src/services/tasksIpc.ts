import { invoke } from "@tauri-apps/api/core";
import type {
  ImportVideosResult,
  TaskListPanel,
  TranscribeDepsCheck,
} from "@/types/tasks";

export async function listTasks(): Promise<TaskListPanel> {
  return invoke("list_tasks");
}

export async function setPanelOutput(payload: {
  output_dir_mode: string;
  custom_output_dir: string;
}): Promise<void> {
  return invoke("set_panel_output", { req: payload });
}

export async function importVideos(paths: string[]): Promise<ImportVideosResult> {
  return invoke("import_videos", { paths });
}

export async function deleteTask(id: string): Promise<void> {
  return invoke("delete_task", { id });
}

export async function clearTasks(force: boolean): Promise<void> {
  return invoke("clear_tasks", { force });
}

export async function startTask(id: string): Promise<void> {
  return invoke("start_task", { id });
}

export async function startTasks(): Promise<number> {
  return invoke("start_tasks");
}

export async function pauseTask(id: string): Promise<void> {
  return invoke("pause_task", { id });
}

export async function pauseAllTasks(): Promise<void> {
  return invoke("pause_all_tasks");
}

export async function continueAllTasks(): Promise<void> {
  return invoke("continue_all_tasks");
}

export async function openOutputDir(): Promise<void> {
  return invoke("open_output_dir");
}

export async function checkTranscribeDeps(): Promise<TranscribeDepsCheck> {
  return invoke("check_transcribe_deps");
}
