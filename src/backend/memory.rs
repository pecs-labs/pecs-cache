//! 本地并发安全内存缓存后端实现 (LocalMemoryStore)
//!
//! 纯 Rust 原生实现，零外部网络与存储依赖，提供毫秒/微秒级低延迟响应。

use crate::error::CacheResult;
use crate::traits::CacheStore;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

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

/// 本地高性能并发安全内存缓存存储 (LocalMemoryStore)
///
/// 具备能力：
/// 1. 纯 Rust 原生实现，无需 Redis，开箱即用；
/// 2. 线程安全、支持 Tokio 异步、可廉价 Clone (Arc + RwLock)；
/// 3. 天然实现 `CacheStore`，支持 TTL、前缀批量删除 (`del_prefix`) 与计数器；
/// 4. 适合单元测试、单机应用、开发环境以及作为 Redis 的离线降级方案。
#[derive(Clone, Default)]
pub struct LocalMemoryStore {
    store: Arc<RwLock<HashMap<String, MemoryEntry>>>,
}

pub type LocalMemoryManager = LocalMemoryStore;

impl LocalMemoryStore {
    /// 创建一个新的本地内存缓存实例
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 获取原始字符串值
    pub async fn get(&self, key: &str) -> CacheResult<Option<String>> {
        let store = self.store.read().await;
        if let Some(entry) = store.get(key) {
            if !entry.is_expired() {
                return Ok(Some(entry.value.clone()));
            }
        }
        Ok(None)
    }

    /// 设置键值并指定过期时间（秒，None 为永不过期）
    pub async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> CacheResult<()> {
        let expires_at = ttl_secs.map(|ttl| Instant::now() + std::time::Duration::from_secs(ttl));
        let mut store = self.store.write().await;
        store.insert(
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
        let mut store = self.store.write().await;
        Ok(store.remove(key).is_some())
    }

    /// 判断键是否存在且未过期
    pub async fn exists(&self, key: &str) -> CacheResult<bool> {
        let store = self.store.read().await;
        if let Some(entry) = store.get(key) {
            return Ok(!entry.is_expired());
        }
        Ok(false)
    }

    /// 查询剩余 TTL 秒数
    pub async fn ttl(&self, key: &str) -> CacheResult<Option<i64>> {
        let store = self.store.read().await;
        if let Some(entry) = store.get(key) {
            return Ok(entry.remaining_ttl_secs());
        }
        Ok(None)
    }

    /// 删除指定前缀的所有键
    pub async fn del_prefix(&self, prefix: &str) -> CacheResult<usize> {
        let mut store = self.store.write().await;
        let keys_to_remove: Vec<String> = store
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();

        let count = keys_to_remove.len();
        for k in keys_to_remove {
            store.remove(&k);
        }
        Ok(count)
    }

    /// 原子自增 (INCRBY)
    pub async fn incr_by(&self, key: &str, delta: i64) -> CacheResult<i64> {
        let mut store = self.store.write().await;
        let current_val = if let Some(entry) = store.get(key) {
            if entry.is_expired() {
                0
            } else {
                entry.value.parse::<i64>().unwrap_or(0)
            }
        } else {
            0
        };

        let new_val = current_val + delta;
        let expires_at = store.get(key).and_then(|e| e.expires_at);
        store.insert(
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
        let mut store = self.store.write().await;
        store.clear();
    }

    /// 主动清理所有已过期条目
    pub async fn purge_expired(&self) -> usize {
        let mut store = self.store.write().await;
        let now = Instant::now();
        let before_len = store.len();
        store.retain(|_, entry| match entry.expires_at {
            Some(exp) => exp > now,
            None => true,
        });
        before_len - store.len()
    }

    /// 当前有效数据量
    pub async fn len(&self) -> usize {
        let store = self.store.read().await;
        let now = Instant::now();
        store
            .values()
            .filter(|e| match e.expires_at {
                Some(exp) => exp > now,
                None => true,
            })
            .count()
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
