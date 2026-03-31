use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::domain::task::TaskStoreFile;
use crate::infra::task_store;

#[derive(Clone)]
pub struct AppRoot(pub PathBuf);

pub struct TaskState(pub Arc<Mutex<TaskStoreFile>>);

impl TaskState {
    pub fn load(app_dir: &PathBuf) -> Result<Self, String> {
        let data =
            task_store::load_task_store_file(app_dir.as_path()).map_err(|e| e.to_string())?;
        Ok(Self(Arc::new(Mutex::new(data))))
    }
}

/// 同一时间只允许一个 Whisper 模型下载
pub struct WhisperDownloadLock(pub tokio::sync::Mutex<()>);

impl Default for WhisperDownloadLock {
    fn default() -> Self {
        Self(tokio::sync::Mutex::new(()))
    }
}
