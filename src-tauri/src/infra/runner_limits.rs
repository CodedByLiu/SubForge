//! Phase 6：任务并发上限与全局 LLM 请求槽位

use std::sync::{Arc, Condvar, Mutex};

use crate::domain::config::AppConfig;

/// 根据配置与硬件估算可同时跑几条管线（Whisper 偏重，自动模式偏保守）
pub fn effective_max_parallel_tasks(cfg: &AppConfig) -> u32 {
    let cap = cfg.runtime.max_parallel_tasks.max(1).min(16);
    if !cfg.runtime.auto_detect_hardware {
        return cap;
    }
    let cores = std::thread::available_parallelism()
        .map(|p| p.get() as u32)
        .unwrap_or(4)
        .max(1);
    let suggested = (cores / 2).max(1).min(8);
    cap.min(suggested)
}

struct LlmSlotsInner {
    active: Mutex<u32>,
    cvar: Condvar,
}

/// 跨任务共享：限制同时在飞的 LLM 翻译 HTTP 请求数（见 `llm.translate_concurrency`）
#[derive(Clone)]
pub struct LlmRequestSlots {
    inner: Arc<LlmSlotsInner>,
}

impl LlmRequestSlots {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(LlmSlotsInner {
                active: Mutex::new(0),
                cvar: Condvar::new(),
            }),
        }
    }

    pub fn acquire(&self, max_parallel: u32) -> LlmRequestPermit {
        let max = max_parallel.max(1).min(64);
        let mut n = self.inner.active.lock().unwrap();
        while *n >= max {
            n = self.inner.cvar.wait(n).unwrap();
        }
        *n += 1;
        drop(n);
        LlmRequestPermit {
            inner: self.inner.clone(),
        }
    }
}

pub struct LlmRequestPermit {
    inner: Arc<LlmSlotsInner>,
}

impl Drop for LlmRequestPermit {
    fn drop(&mut self) {
        let mut n = self.inner.active.lock().unwrap();
        *n = n.saturating_sub(1);
        self.inner.cvar.notify_one();
    }
}
