use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use super::paths::models_whisper_dir;

pub const DEFAULT_DOWNLOAD_BASE: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

#[derive(Debug, Clone, Copy)]
pub struct CatalogEntry {
    pub id: &'static str,
    pub file_name: &'static str,
    pub size_bytes_estimate: u64,
}

pub const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        id: "tiny",
        file_name: "ggml-tiny.bin",
        size_bytes_estimate: 78 * 1024 * 1024,
    },
    CatalogEntry {
        id: "base",
        file_name: "ggml-base.bin",
        size_bytes_estimate: 148 * 1024 * 1024,
    },
    CatalogEntry {
        id: "small",
        file_name: "ggml-small.bin",
        size_bytes_estimate: 488 * 1024 * 1024,
    },
    CatalogEntry {
        id: "medium",
        file_name: "ggml-medium.bin",
        size_bytes_estimate: 1570 * 1024 * 1024,
    },
    CatalogEntry {
        id: "large-v3",
        file_name: "ggml-large-v3.bin",
        size_bytes_estimate: 3200 * 1024 * 1024,
    },
];

pub fn entry_for_id(id: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|e| e.id == id)
}

/// 与配置一致：优先镜像、备用 download_url、默认 HF
pub fn resolve_download_base(mirror_url: &str, prefer_mirror: bool, download_url: &str) -> String {
    let m = mirror_url.trim();
    let d = download_url.trim();
    let base = if prefer_mirror && !m.is_empty() {
        m
    } else if !d.is_empty() {
        d
    } else if !m.is_empty() {
        m
    } else {
        DEFAULT_DOWNLOAD_BASE
    };
    base.trim_end_matches('/').to_string()
}

pub fn build_file_url(base: &str, file_name: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), file_name)
}

#[derive(Debug, Serialize)]
pub struct WhisperModelRowDto {
    pub id: String,
    pub file_name: String,
    pub size_bytes_estimate: u64,
    pub downloaded: bool,
    pub local_size_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct WhisperModelsListDto {
    pub models: Vec<WhisperModelRowDto>,
    pub models_dir: String,
    pub download_base_used: String,
}

pub fn list_installed_and_catalog(
    app_dir: &Path,
    mirror_url: &str,
    prefer_mirror: bool,
    download_url: &str,
) -> Result<WhisperModelsListDto> {
    let dir = models_whisper_dir(app_dir);
    fs::create_dir_all(&dir).ok();

    let base = resolve_download_base(mirror_url, prefer_mirror, download_url);

    let mut models = Vec::new();
    for e in CATALOG {
        let path = dir.join(e.file_name);
        let (downloaded, local_size) = if path.exists() {
            let len = fs::metadata(&path).map(|m| m.len()).ok();
            (true, len)
        } else {
            (false, None)
        };
        models.push(WhisperModelRowDto {
            id: e.id.into(),
            file_name: e.file_name.into(),
            size_bytes_estimate: e.size_bytes_estimate,
            downloaded,
            local_size_bytes: local_size,
        });
    }

    Ok(WhisperModelsListDto {
        models,
        models_dir: dir.to_string_lossy().to_string(),
        download_base_used: base,
    })
}

pub fn model_file_path(app_dir: &Path, id: &str) -> Result<PathBuf> {
    let ent = entry_for_id(id).ok_or_else(|| anyhow::anyhow!("未知模型: {id}"))?;
    let dir = models_whisper_dir(app_dir);
    Ok(dir.join(ent.file_name))
}

pub fn delete_model_file(app_dir: &Path, id: &str) -> Result<()> {
    let p = model_file_path(app_dir, id)?;
    if p.exists() {
        fs::remove_file(&p).with_context(|| format!("删除 {}", p.display()))?;
    }
    Ok(())
}
