use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::paths::{
    bin_dir, config_dir, data_dir, logs_dir, models_whisper_dir, portable_app_dir, temp_dir,
};

pub fn ensure_app_layout() -> Result<std::path::PathBuf> {
    let app_dir = portable_app_dir();
    for d in [
        config_dir(&app_dir),
        bin_dir(&app_dir),
        data_dir(&app_dir),
        models_whisper_dir(&app_dir),
        logs_dir(&app_dir),
        temp_dir(&app_dir),
    ] {
        fs::create_dir_all(&d).with_context(|| format!("创建目录失败: {}", d.display()))?;
    }
    check_writable(&app_dir)?;
    Ok(app_dir)
}

fn check_writable(app_dir: &Path) -> Result<()> {
    let probe = app_dir.join(".write_probe");
    fs::write(&probe, b"ok").with_context(|| {
        format!(
            "软件目录不可写: {}。请将程序放到有写入权限的文件夹。",
            app_dir.display()
        )
    })?;
    let _ = fs::remove_file(&probe);
    Ok(())
}
