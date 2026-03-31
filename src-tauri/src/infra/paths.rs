use std::path::{Path, PathBuf};

pub fn portable_app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

pub fn config_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("config")
}

pub fn bin_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("bin")
}

pub fn data_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("data")
}

pub fn models_whisper_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("models").join("whisper")
}

pub fn logs_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("logs")
}

pub fn temp_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("temp")
}

pub fn app_config_path(app_dir: &Path) -> PathBuf {
    config_dir(app_dir).join("app-config.json")
}

pub fn secrets_path(app_dir: &Path) -> PathBuf {
    config_dir(app_dir).join("secrets.enc.json")
}

pub fn vault_key_path(app_dir: &Path) -> PathBuf {
    config_dir(app_dir).join("vault.key")
}

pub fn tasks_cache_path(app_dir: &Path) -> PathBuf {
    data_dir(app_dir).join("tasks-cache.json")
}
