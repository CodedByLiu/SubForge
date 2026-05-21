//! 任务级持久化缓存：按视频名 + 路径哈希组织子目录，
//! 用 stamp(hash) 校验各阶段产物，失败/中断后仍可断点续传。
//!
//! 目录布局：`{app_dir}/cache/{stem}_{hash8}/`
//!   - `manifest.json`           各阶段 stamp + 视频元数据
//!   - `audio.wav`               抽音产物
//!   - `w.srt` / `w.json`        Whisper 产物
//!   - `cues.json`               分句 + 优化后的 SubCue 列表
//!   - `translations.partial.json` 翻译增量（按原始 cue index 索引）
//!   - `translations.fallback.json` 上一轮翻译中回退到原文的 cue 索引列表（用于"重新翻译失败片段"）

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::srt::SubCue;
use super::subtitle_output::stable_path_hash;

const MANIFEST_FILE: &str = "manifest.json";
const AUDIO_FILE: &str = "audio.wav";
const W_SRT_FILE: &str = "w.srt";
const W_JSON_FILE: &str = "w.json";
const CUES_FILE: &str = "cues.json";
const PARTIAL_TRANSLATIONS_FILE: &str = "translations.partial.json";
const FALLBACK_INDICES_FILE: &str = "translations.fallback.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct StageStamp {
    pub hash: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct CacheManifest {
    pub video_path: String,
    pub video_size: u64,
    pub video_mtime_ms: i64,
    #[serde(default)]
    pub audio: StageStamp,
    #[serde(default)]
    pub whisper: StageStamp,
    #[serde(default)]
    pub segment: StageStamp,
    #[serde(default)]
    pub translate: StageStamp,
}

pub struct TaskCache {
    dir: PathBuf,
    manifest: CacheManifest,
}

pub fn cache_root(app_dir: &Path) -> PathBuf {
    app_dir.join("cache")
}

fn sanitize_stem(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let trimmed = cleaned.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "video".to_string()
    } else {
        trimmed
    }
}

pub fn derive_cache_key(video_path: &Path) -> String {
    let stem = video_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "video".to_string());
    let stem = sanitize_stem(&stem);
    let canonical = video_path.to_string_lossy().to_string();
    let hash = stable_path_hash(&canonical);
    format!("{stem}_{hash}")
}

fn read_video_meta(video_path: &Path) -> (u64, i64) {
    fs::metadata(video_path)
        .map(|m| {
            let size = m.len();
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            (size, mtime)
        })
        .unwrap_or((0, 0))
}

pub fn hash_parts(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update(b"\x1f");
    }
    let digest = h.finalize();
    let mut out = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

impl TaskCache {
    /// 打开（或新建）该视频对应的缓存目录。若视频文件大小/mtime 与 manifest 不一致，
    /// 视为视频被替换，丢弃所有 stamp 但保留目录结构。
    pub fn open(app_dir: &Path, video_path: &Path) -> Result<Self, String> {
        let root = cache_root(app_dir);
        fs::create_dir_all(&root).map_err(|e| format!("创建缓存根目录失败: {e}"))?;
        let dir = root.join(derive_cache_key(video_path));
        fs::create_dir_all(&dir).map_err(|e| format!("创建缓存目录失败: {e}"))?;

        let (size, mtime) = read_video_meta(video_path);
        let video_path_str = video_path.to_string_lossy().to_string();

        let manifest_path = dir.join(MANIFEST_FILE);
        let mut manifest: CacheManifest = if manifest_path.exists() {
            match fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|s| serde_json::from_str::<CacheManifest>(&s).ok())
            {
                Some(m) => m,
                None => CacheManifest::default(),
            }
        } else {
            CacheManifest::default()
        };

        let video_changed = manifest.video_path != video_path_str
            || manifest.video_size != size
            || manifest.video_mtime_ms != mtime;

        if video_changed {
            manifest = CacheManifest {
                video_path: video_path_str,
                video_size: size,
                video_mtime_ms: mtime,
                ..Default::default()
            };
            let _ = fs::remove_file(dir.join(AUDIO_FILE));
            let _ = fs::remove_file(dir.join(W_SRT_FILE));
            let _ = fs::remove_file(dir.join(W_JSON_FILE));
            let _ = fs::remove_file(dir.join(CUES_FILE));
            let _ = fs::remove_file(dir.join(PARTIAL_TRANSLATIONS_FILE));
            let _ = fs::remove_file(dir.join(FALLBACK_INDICES_FILE));
        }

        let cache = TaskCache { dir, manifest };
        cache.save_manifest()?;
        Ok(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn audio_path(&self) -> PathBuf {
        self.dir.join(AUDIO_FILE)
    }
    pub fn w_srt_path(&self) -> PathBuf {
        self.dir.join(W_SRT_FILE)
    }
    pub fn w_json_path(&self) -> PathBuf {
        self.dir.join(W_JSON_FILE)
    }
    pub fn w_prefix(&self) -> PathBuf {
        self.dir.join("w")
    }
    pub fn cues_path(&self) -> PathBuf {
        self.dir.join(CUES_FILE)
    }
    pub fn partial_translations_path(&self) -> PathBuf {
        self.dir.join(PARTIAL_TRANSLATIONS_FILE)
    }
    pub fn fallback_indices_path(&self) -> PathBuf {
        self.dir.join(FALLBACK_INDICES_FILE)
    }

    fn save_manifest(&self) -> Result<(), String> {
        let body =
            serde_json::to_string_pretty(&self.manifest).map_err(|e| e.to_string())?;
        fs::write(self.dir.join(MANIFEST_FILE), body)
            .map_err(|e| format!("写入 manifest 失败: {e}"))
    }

    pub fn audio_valid(&self, expected_hash: &str) -> bool {
        self.manifest.audio.done
            && self.manifest.audio.hash == expected_hash
            && self.audio_path().is_file()
    }

    pub fn whisper_valid(&self, expected_hash: &str) -> bool {
        self.manifest.whisper.done
            && self.manifest.whisper.hash == expected_hash
            && self.w_srt_path().is_file()
            && self.w_json_path().is_file()
    }

    pub fn segment_valid(&self, expected_hash: &str) -> bool {
        self.manifest.segment.done
            && self.manifest.segment.hash == expected_hash
            && self.cues_path().is_file()
    }

    fn invalidate_from_whisper(&mut self) {
        self.manifest.whisper = StageStamp::default();
        self.invalidate_from_segment();
        let _ = fs::remove_file(self.w_srt_path());
        let _ = fs::remove_file(self.w_json_path());
    }

    fn invalidate_from_segment(&mut self) {
        self.manifest.segment = StageStamp::default();
        self.invalidate_from_translate();
        let _ = fs::remove_file(self.cues_path());
    }

    fn invalidate_from_translate(&mut self) {
        self.manifest.translate = StageStamp::default();
        let _ = fs::remove_file(self.partial_translations_path());
        let _ = fs::remove_file(self.fallback_indices_path());
    }

    pub fn mark_audio_done(&mut self, hash: &str) -> Result<(), String> {
        if !self.manifest.audio.done || self.manifest.audio.hash != hash {
            self.invalidate_from_whisper();
        }
        self.manifest.audio = StageStamp {
            hash: hash.into(),
            done: true,
        };
        self.save_manifest()
    }

    pub fn mark_whisper_done(&mut self, hash: &str) -> Result<(), String> {
        if !self.manifest.whisper.done || self.manifest.whisper.hash != hash {
            self.invalidate_from_segment();
        }
        self.manifest.whisper = StageStamp {
            hash: hash.into(),
            done: true,
        };
        self.save_manifest()
    }

    pub fn mark_segment_done(&mut self, hash: &str) -> Result<(), String> {
        if !self.manifest.segment.done || self.manifest.segment.hash != hash {
            self.invalidate_from_translate();
        }
        self.manifest.segment = StageStamp {
            hash: hash.into(),
            done: true,
        };
        self.save_manifest()
    }

    /// 翻译前调用：若 hash 变化则清空 partial，否则保留以做增量。
    pub fn begin_translate(&mut self, hash: &str) -> Result<(), String> {
        if self.manifest.translate.hash != hash {
            self.invalidate_from_translate();
        }
        self.manifest.translate = StageStamp {
            hash: hash.into(),
            done: false,
        };
        self.save_manifest()
    }

    pub fn mark_translate_done(&mut self, hash: &str) -> Result<(), String> {
        self.manifest.translate = StageStamp {
            hash: hash.into(),
            done: true,
        };
        self.save_manifest()
    }

    pub fn write_cues(&self, cues: &[SubCue]) -> Result<(), String> {
        let body = serde_json::to_string(cues).map_err(|e| e.to_string())?;
        fs::write(self.cues_path(), body).map_err(|e| format!("写入 cues 缓存失败: {e}"))
    }

    pub fn load_cues(&self) -> Result<Vec<SubCue>, String> {
        let body = fs::read_to_string(self.cues_path())
            .map_err(|e| format!("读取 cues 缓存失败: {e}"))?;
        serde_json::from_str(&body).map_err(|e| format!("解析 cues 缓存失败: {e}"))
    }

    pub fn load_partial_translations(&self) -> HashMap<usize, String> {
        let path = self.partial_translations_path();
        if !path.is_file() {
            return HashMap::new();
        }
        let Ok(body) = fs::read_to_string(&path) else {
            return HashMap::new();
        };
        let raw: HashMap<String, String> = serde_json::from_str(&body).unwrap_or_default();
        raw.into_iter()
            .filter_map(|(k, v)| k.parse::<usize>().ok().map(|i| (i, v)))
            .collect()
    }

    pub fn save_partial_translations(
        &self,
        map: &HashMap<usize, String>,
    ) -> Result<(), String> {
        let serializable: HashMap<String, String> = map
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let body = serde_json::to_string(&serializable).map_err(|e| e.to_string())?;
        fs::write(self.partial_translations_path(), body)
            .map_err(|e| format!("写入 partial translations 失败: {e}"))
    }

    pub fn load_fallback_indices(&self) -> Vec<usize> {
        let path = self.fallback_indices_path();
        if !path.is_file() {
            return Vec::new();
        }
        let Ok(body) = fs::read_to_string(&path) else {
            return Vec::new();
        };
        let mut v: Vec<usize> = serde_json::from_str(&body).unwrap_or_default();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn save_fallback_indices(&self, indices: &[usize]) -> Result<(), String> {
        let body = serde_json::to_string(indices).map_err(|e| e.to_string())?;
        fs::write(self.fallback_indices_path(), body)
            .map_err(|e| format!("写入 fallback indices 失败: {e}"))
    }

    pub fn clear_fallback_indices(&self) {
        let _ = fs::remove_file(self.fallback_indices_path());
    }

    /// 成功完成任务后调用：删除整个缓存目录。
    pub fn cleanup(self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_stem_strips_unsafe_chars() {
        assert_eq!(sanitize_stem("hello world.mp4"), "hello_world_mp4");
        assert_eq!(sanitize_stem("中文电影 Final v2"), "Final_v2");
        assert_eq!(sanitize_stem(""), "video");
    }

    #[test]
    fn derive_key_stable_per_path() {
        let a = derive_cache_key(Path::new("/movies/Test.mp4"));
        let b = derive_cache_key(Path::new("/movies/Test.mp4"));
        assert_eq!(a, b);
        assert!(a.starts_with("Test_"));
    }

    #[test]
    fn hash_parts_changes_on_input_change() {
        let h1 = hash_parts(&["a", "b"]);
        let h2 = hash_parts(&["a", "c"]);
        assert_ne!(h1, h2);
    }
}
