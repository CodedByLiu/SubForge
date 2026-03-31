use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::domain::task::TaskStoreFile;

use super::paths::tasks_cache_path;

pub fn load_task_store_file(app_dir: &Path) -> Result<TaskStoreFile> {
    let path = tasks_cache_path(app_dir);
    if !path.exists() {
        return Ok(TaskStoreFile {
            version: 1,
            output_dir_mode: "video_dir".into(),
            custom_output_dir: String::new(),
            tasks: Vec::new(),
        });
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("读取 {}", path.display()))?;
    let mut f: TaskStoreFile = serde_json::from_str(&raw).context("解析 tasks-cache.json")?;
    if f.output_dir_mode.is_empty() {
        f.output_dir_mode = "video_dir".into();
    }
    Ok(f)
}

pub fn save_task_store_file(app_dir: &Path, data: &TaskStoreFile) -> Result<()> {
    let path = tasks_cache_path(app_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data).context("序列化任务缓存")?;
    fs::write(&path, json).with_context(|| format!("写入 {}", path.display()))?;
    Ok(())
}

pub fn video_extension_ok(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "mp4" | "mkv" | "mov" | "avi" | "webm"
            )
        })
        .unwrap_or(false)
}

pub fn normalize_existing_path(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("无效路径: {}", path.display()))
}
