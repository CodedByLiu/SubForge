use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::State;

use crate::app::state::AppRoot;
use crate::infra::cache::cache_root;

#[derive(Serialize)]
pub struct CacheStatsDto {
    pub entry_count: u32,
    pub total_bytes: u64,
}

#[derive(Serialize)]
pub struct CacheClearResultDto {
    pub deleted_count: u32,
    pub freed_bytes: u64,
}

fn dir_size_recursive(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = fs::read_dir(path) else {
        return 0;
    };
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total = total.saturating_add(dir_size_recursive(&entry.path()));
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

fn entry_age(path: &Path) -> Option<Duration> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

#[tauri::command]
pub fn get_cache_stats(root: State<'_, AppRoot>) -> Result<CacheStatsDto, String> {
    let dir = cache_root(&root.0);
    if !dir.is_dir() {
        return Ok(CacheStatsDto {
            entry_count: 0,
            total_bytes: 0,
        });
    }
    let mut count: u32 = 0;
    let mut total: u64 = 0;
    let rd = fs::read_dir(&dir).map_err(|e| format!("读取缓存目录失败: {e}"))?;
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        count = count.saturating_add(1);
        total = total.saturating_add(dir_size_recursive(&entry.path()));
    }
    Ok(CacheStatsDto {
        entry_count: count,
        total_bytes: total,
    })
}

/// 清理缓存目录。
/// - `older_than_days = 0` 清理全部
/// - `older_than_days > 0` 仅清理 mtime 早于该天数的子目录
#[tauri::command]
pub fn clear_cache(
    root: State<'_, AppRoot>,
    older_than_days: u32,
) -> Result<CacheClearResultDto, String> {
    let dir = cache_root(&root.0);
    if !dir.is_dir() {
        return Ok(CacheClearResultDto {
            deleted_count: 0,
            freed_bytes: 0,
        });
    }
    let threshold = if older_than_days == 0 {
        None
    } else {
        Some(Duration::from_secs(older_than_days as u64 * 24 * 60 * 60))
    };

    let mut deleted: u32 = 0;
    let mut freed: u64 = 0;

    let rd = fs::read_dir(&dir).map_err(|e| format!("读取缓存目录失败: {e}"))?;
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        if let Some(min_age) = threshold {
            match entry_age(&path) {
                Some(age) if age >= min_age => {}
                _ => continue,
            }
        }
        let size = dir_size_recursive(&path);
        match fs::remove_dir_all(&path) {
            Ok(()) => {
                deleted = deleted.saturating_add(1);
                freed = freed.saturating_add(size);
            }
            Err(e) => {
                log::warn!(
                    "清理缓存目录失败 path={} err={e}",
                    path.display()
                );
            }
        }
    }
    Ok(CacheClearResultDto {
        deleted_count: deleted,
        freed_bytes: freed,
    })
}
