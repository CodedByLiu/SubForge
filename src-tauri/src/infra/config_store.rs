use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::domain::config::AppConfig;

use super::paths::app_config_path;

pub fn load_config(app_dir: &Path) -> Result<AppConfig> {
    let path = app_config_path(app_dir);
    if !path.exists() {
        return Ok(AppConfig::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("读取 {}", path.display()))?;
    let mut cfg: AppConfig = serde_json::from_str(&raw).context("解析 app-config.json")?;
    merge_config_defaults(&mut cfg);
    Ok(cfg)
}

pub fn save_config(app_dir: &Path, cfg: &AppConfig) -> Result<()> {
    let path = app_config_path(app_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg).context("序列化配置")?;
    fs::write(&path, json).with_context(|| format!("写入 {}", path.display()))?;
    Ok(())
}

fn merge_config_defaults(cfg: &mut AppConfig) {
    let d = AppConfig::default();
    if cfg.llm.timeout_sec == 0 {
        cfg.llm.timeout_sec = d.llm.timeout_sec;
    }
    if cfg.llm.max_retries == 0 && cfg.llm.base_url.is_empty() {
        cfg.llm.max_retries = d.llm.max_retries;
    }
    if cfg.llm.translate_concurrency == 0 {
        cfg.llm.translate_concurrency = d.llm.translate_concurrency;
    }
    if cfg.translator.engine.is_empty() {
        cfg.translator.engine = d.translator.engine;
    }
    if cfg.whisper.model.is_empty() {
        cfg.whisper.model = d.whisper.model;
    }
    if cfg.whisper.recognition_lang.is_empty() {
        cfg.whisper.recognition_lang = d.whisper.recognition_lang;
    }
    if cfg.translate.source_lang.is_empty() {
        cfg.translate.source_lang = d.translate.source_lang;
    }
    if cfg.translate.target_lang.is_empty() {
        cfg.translate.target_lang = d.translate.target_lang;
    }
    if cfg.translate.style.is_empty() {
        cfg.translate.style = d.translate.style;
    }
    if cfg.translate.max_segment_chars == 0 {
        cfg.translate.max_segment_chars = d.translate.max_segment_chars;
    }
    if cfg.segmentation.strategy.is_empty() {
        cfg.segmentation.strategy = d.segmentation.strategy;
    }
    if cfg.segmentation.max_chars_per_segment == 0 {
        cfg.segmentation.max_chars_per_segment = d.segmentation.max_chars_per_segment;
    }
    if cfg.segmentation.max_duration_seconds <= 0.0 {
        cfg.segmentation.max_duration_seconds = d.segmentation.max_duration_seconds;
    }
    if cfg.segmentation.timing_mode.is_empty() {
        cfg.segmentation.timing_mode = d.segmentation.timing_mode;
    }
    if cfg.subtitle.mode.is_empty() {
        cfg.subtitle.mode = d.subtitle.mode;
    }
    if cfg.subtitle.format.is_empty() {
        cfg.subtitle.format = d.subtitle.format;
    }
    if cfg.subtitle.output_dir_mode.is_empty() {
        cfg.subtitle.output_dir_mode = d.subtitle.output_dir_mode;
    }
    if cfg.runtime.max_parallel_tasks == 0 {
        cfg.runtime.max_parallel_tasks = d.runtime.max_parallel_tasks;
    }
    if cfg.runtime.cpu_thread_limit == 0 {
        cfg.runtime.cpu_thread_limit = d.runtime.cpu_thread_limit;
    }
}
