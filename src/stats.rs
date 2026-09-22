//! 缓存可观测性与统计指标 (Observability & Metrics)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 缓存核心统计计数器
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    hits: Arc<AtomicU64>,
    misses: Arc<AtomicU64>,
    evictions: Arc<AtomicU64>,
    null_hits: Arc<AtomicU64>,
}

/// 缓存统计快照
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CacheStatsSummary {
    /// 缓存命中次数
    pub hits: u64,
    /// 缓存未命中次数（触发回源）
    pub misses: u64,
    /// 防穿透空值命中次数
    pub null_hits: u64,
    /// 缓存主动/被动淘汰次数
    pub evictions: u64,
    /// 缓存命中率 (0.0 ~ 1.0)
    pub hit_rate: f64,
}

impl CacheStats {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_null_hit(&self) {
        self.null_hits.fetch_add(1, Ordering::Relaxed);
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_eviction(&self, count: u64) {
        self.evictions.fetch_add(count, Ordering::Relaxed);
    }

    /// 获取当前统计数据快照
    pub fn summary(&self) -> CacheStatsSummary {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let null_hits = self.null_hits.load(Ordering::Relaxed);
        let evictions = self.evictions.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        };

        CacheStatsSummary {
            hits,
            misses,
            null_hits,
            evictions,
            hit_rate,
        }
    }

    /// 重置所有统计计数
    pub fn reset(&self) {
        self.hits.store(0, Ordering::Relaxed);
        self.misses.store(0, Ordering::Relaxed);
        self.null_hits.store(0, Ordering::Relaxed);
        self.evictions.store(0, Ordering::Relaxed);
    }
}
