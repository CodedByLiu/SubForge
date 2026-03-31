mod app;
mod domain;
mod infra;

use app::config_commands;
use app::hardware_commands;
use app::state::{AppRoot, TaskState, WhisperDownloadLock};
use app::task_commands;
use app::task_runner;
use infra::layout;
use infra::runner_limits::LlmRequestSlots;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(if cfg!(debug_assertions) {
                        log::LevelFilter::Debug
                    } else {
                        log::LevelFilter::Info
                    })
                    .build(),
            )?;
            let app_dir = match layout::ensure_app_layout() {
                Ok(d) => d,
                Err(e) => {
                    log::error!("初始化目录失败: {e:#}");
                    return Err(format!("{e:#}").into());
                }
            };
            let task_state = match TaskState::load(&app_dir) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("加载任务缓存失败: {e}");
                    return Err(format!("加载任务缓存失败: {e}").into());
                }
            };
            let runner_tasks = task_state.0.clone();
            let runner_handle = app.handle().clone();
            let runner_dir = app_dir.clone();
            let llm_slots = LlmRequestSlots::new();
            tauri::async_runtime::spawn(async move {
                task_runner::run_forever(
                    runner_handle,
                    AppRoot(runner_dir),
                    runner_tasks,
                    llm_slots,
                )
                .await;
            });
            app.manage(AppRoot(app_dir));
            app.manage(task_state);
            app.manage(WhisperDownloadLock::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            config_commands::get_app_info,
            config_commands::get_config,
            config_commands::save_config,
            config_commands::test_llm_connection,
            task_commands::list_tasks,
            task_commands::set_panel_output,
            task_commands::import_videos,
            task_commands::delete_task,
            task_commands::clear_tasks,
            task_commands::start_task,
            task_commands::start_tasks,
            task_commands::pause_task,
            task_commands::pause_all_tasks,
            task_commands::continue_all_tasks,
            task_commands::open_output_dir,
            task_commands::check_transcribe_deps,
            hardware_commands::get_hardware_info,
            hardware_commands::list_whisper_models,
            hardware_commands::delete_whisper_model,
            hardware_commands::download_whisper_model,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
