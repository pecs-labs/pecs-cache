//! 本地并发安全内存缓存后端实现 (LocalMemoryStore)
//!
//! 纯 Rust 原生实现，零外部网络与存储依赖，提供毫秒/微秒级低延迟响应。
//! 具备分段读写锁 (Sharded Locks) 降低并发竞争、容量上限与淘汰保护、统计指标可观测性。

use crate::error::CacheResult;
use crate::stats::{CacheStats, CacheStatsSummary};
use crate::traits::CacheStore;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

const DEFAULT_SHARD_COUNT: usize = 16;
const DEFAULT_CAPACITY: usize = 100_000;

/// 内部存储条目实体
#[derive(Clone, Debug)]
struct MemoryEntry {
    value: String,
    expires_at: Option<Instant>,
}

impl MemoryEntry {
    #[inline]
    fn is_expired(&self) -> bool {
        if let Some(exp) = self.expires_at {
            Instant::now() >= exp
        } else {
            false
        }
    }

    #[inline]
    fn remaining_ttl_secs(&self) -> Option<i64> {
        match self.expires_at {
            Some(exp) => {
                let now = Instant::now();
                if now >= exp {
                    None
                } else {
                    Some((exp - now).as_secs() as i64)
                }
            }
            None => Some(-1),
        }
    }
}

/// 快速计算分片索引 (FNV-1a 64-bit)
#[inline]
fn shard_idx(key: &str, shard_count: usize) -> usize {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in key.as_bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    (hash as usize) % shard_count
}

/// 本地高性能并发安全分段内存缓存存储 (LocalMemoryStore)
///
/// 具备能力：
/// 1. 纯 Rust 原生实现，无需 Redis，开箱即用；
/// 2. 16 分段锁 (Sharded Locks)，将并发锁竞争降低 16 倍；
/// 3. 容量上限与防 OOM 保护：默认最大 100,000 条目，支持自定义容量；
/// 4. 内置指标统计：自动记录命中、未命中、失效淘汰次数；
/// 5. 天然实现 `CacheStore`，支持 TTL、前缀批量删除 (`del_prefix`) 与计数器。
#[derive(Clone)]
pub struct LocalMemoryStore {
    shards: Arc<Vec<RwLock<HashMap<String, MemoryEntry>>>>,
    max_capacity_per_shard: usize,
    stats: CacheStats,
}

pub type LocalMemoryManager = LocalMemoryStore;

impl Default for LocalMemoryStore {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl LocalMemoryStore {
    /// 创建一个新的本地内存缓存实例（默认容量 100,000）
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建具有指定最大条目容量的内存缓存实例
    pub fn with_capacity(max_capacity: usize) -> Self {
        let max_per_shard = (max_capacity / DEFAULT_SHARD_COUNT).max(1);
        let mut shards = Vec::with_capacity(DEFAULT_SHARD_COUNT);
        for _ in 0..DEFAULT_SHARD_COUNT {
            shards.push(RwLock::new(HashMap::new()));
        }

        Self {
            shards: Arc::new(shards),
            max_capacity_per_shard: max_per_shard,
            stats: CacheStats::new(),
        }
    }

    /// 获取缓存统计快照
    pub fn stats(&self) -> CacheStatsSummary {
        self.stats.summary()
    }

    /// 重置缓存统计计数
    pub fn reset_stats(&self) {
        self.stats.reset();
    }

    #[inline]
    fn get_shard(&self, key: &str) -> &RwLock<HashMap<String, MemoryEntry>> {
        let idx = shard_idx(key, self.shards.len());
        &self.shards[idx]
    }

    /// 获取原始字符串值
    pub async fn get(&self, key: &str) -> CacheResult<Option<String>> {
        let shard = self.get_shard(key);
        let map = shard.read().await;
        if let Some(entry) = map.get(key) {
            if !entry.is_expired() {
                self.stats.record_hit();
                return Ok(Some(entry.value.clone()));
            }
        }
        self.stats.record_miss();
        Ok(None)
    }

    /// 设置键值并指定过期时间（秒，None 为永不过期）
    pub async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> CacheResult<()> {
        let expires_at = ttl_secs.map(|ttl| Instant::now() + std::time::Duration::from_secs(ttl));
        let shard = self.get_shard(key);
        let mut map = shard.write().await;

        // 容量淘汰保护：若该分片超限，先淘汰过期 Key，若仍满淘汰最接近过期的 Key
        if map.len() >= self.max_capacity_per_shard && !map.contains_key(key) {
            let now = Instant::now();
            let before = map.len();
            map.retain(|_, v| match v.expires_at {
                Some(exp) => exp > now,
                None => true,
            });
            let purged = before - map.len();
            if purged > 0 {
                self.stats.record_eviction(purged as u64);
            }

            // 若仍超过容量，剔除一个最早过期的条目
            if map.len() >= self.max_capacity_per_shard {
                let candidate = map
                    .iter()
                    .filter_map(|(k, v)| v.expires_at.map(|exp| (k.clone(), exp)))
                    .min_by_key(|(_, exp)| *exp)
                    .map(|(k, _)| k);

                if let Some(k) = candidate {
                    map.remove(&k);
                    self.stats.record_eviction(1);
                } else if let Some(k) = map.keys().next().cloned() {
                    map.remove(&k);
                    self.stats.record_eviction(1);
                }
            }
        }

        map.insert(
            key.to_string(),
            MemoryEntry {
                value: value.to_string(),
                expires_at,
            },
        );
        Ok(())
    }

    /// 删除指定键
    pub async fn del(&self, key: &str) -> CacheResult<bool> {
        let shard = self.get_shard(key);
        let mut map = shard.write().await;
        let removed = map.remove(key).is_some();
        if removed {
            self.stats.record_eviction(1);
        }
        Ok(removed)
    }

    /// 判断键是否存在且未过期
    pub async fn exists(&self, key: &str) -> CacheResult<bool> {
        let shard = self.get_shard(key);
        let map = shard.read().await;
        if let Some(entry) = map.get(key) {
            return Ok(!entry.is_expired());
        }
        Ok(false)
    }

    /// 查询剩余 TTL 秒数
    pub async fn ttl(&self, key: &str) -> CacheResult<Option<i64>> {
        let shard = self.get_shard(key);
        let map = shard.read().await;
        if let Some(entry) = map.get(key) {
            return Ok(entry.remaining_ttl_secs());
        }
        Ok(None)
    }

    /// 删除指定前缀的所有键
    pub async fn del_prefix(&self, prefix: &str) -> CacheResult<usize> {
        let mut total_deleted = 0;
        for shard in self.shards.iter() {
            let mut map = shard.write().await;
            let keys_to_remove: Vec<String> = map
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect();

            let count = keys_to_remove.len();
            for k in keys_to_remove {
                map.remove(&k);
            }
            total_deleted += count;
        }
        if total_deleted > 0 {
            self.stats.record_eviction(total_deleted as u64);
        }
        Ok(total_deleted)
    }

    /// 原子自增 (INCRBY)
    pub async fn incr_by(&self, key: &str, delta: i64) -> CacheResult<i64> {
        let shard = self.get_shard(key);
        let mut map = shard.write().await;
        let current_val = if let Some(entry) = map.get(key) {
            if entry.is_expired() {
                0
            } else {
                entry.value.parse::<i64>().unwrap_or(0)
            }
        } else {
            0
        };

        let new_val = current_val + delta;
        let expires_at = map.get(key).and_then(|e| e.expires_at);
        map.insert(
            key.to_string(),
            MemoryEntry {
                value: new_val.to_string(),
                expires_at,
            },
        );
        Ok(new_val)
    }

    /// 清空所有数据
    pub async fn clear(&self) {
        let mut total_cleared = 0;
        for shard in self.shards.iter() {
            let mut map = shard.write().await;
            total_cleared += map.len();
            map.clear();
        }
        if total_cleared > 0 {
            self.stats.record_eviction(total_cleared as u64);
        }
    }

    /// 主动清理所有已过期条目
    pub async fn purge_expired(&self) -> usize {
        let now = Instant::now();
        let mut total_purged = 0;
        for shard in self.shards.iter() {
            let mut map = shard.write().await;
            let before = map.len();
            map.retain(|_, entry| match entry.expires_at {
                Some(exp) => exp > now,
                None => true,
            });
            total_purged += before - map.len();
        }
        if total_purged > 0 {
            self.stats.record_eviction(total_purged as u64);
        }
        total_purged
    }

    /// 当前有效数据量
    pub async fn len(&self) -> usize {
        let now = Instant::now();
        let mut count = 0;
        for shard in self.shards.iter() {
            let map = shard.read().await;
            count += map
                .values()
                .filter(|e| match e.expires_at {
                    Some(exp) => exp > now,
                    None => true,
                })
                .count();
        }
        count
    }

    /// 当前是否为空
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }
}

#[async_trait]
impl CacheStore for LocalMemoryStore {
    async fn get(&self, key: &str) -> CacheResult<Option<String>> {
        self.get(key).await
    }

    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> CacheResult<()> {
        self.set(key, value, ttl_secs).await
    }

    async fn del(&self, key: &str) -> CacheResult<()> {
        let _ = self.del(key).await?;
        Ok(())
    }

    async fn exists(&self, key: &str) -> CacheResult<bool> {
        self.exists(key).await
    }

    async fn del_prefix(&self, prefix: &str) -> CacheResult<usize> {
        self.del_prefix(prefix).await
    }
}
