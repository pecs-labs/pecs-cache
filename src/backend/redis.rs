//! Redis 异步存储后端实现 (RedisStore)
//!
//! 基于 `redis-rs` 官方提供的 `ConnectionManager`，具备自动重连、克隆开销低、完全非阻塞等特性。

use crate::error::{CacheError, CacheResult};
use crate::traits::CacheStore;
use async_trait::async_trait;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};

/// 高性能 Redis 异步缓存存储后端
#[derive(Clone)]
pub struct RedisStore {
    manager: ConnectionManager,
}

impl RedisStore {
    /// 基于外部微服务已存在的 `ConnectionManager` 创建存储适配器
    ///
    /// 极其推荐在微服务中使用此方式：微服务自身管理连接池，与分布式锁、队列等共享同一连接。
    pub fn from_manager(manager: ConnectionManager) -> Self {
        Self { manager }
    }

    /// 通过 Redis URL 直接建立异步连接（新项目极简开箱即用）
    ///
    /// 支持 URL 格式：`redis://:password@127.0.0.1:6379/0`
    pub async fn connect(url: &str) -> CacheResult<Self> {
        let client = Client::open(url)
            .map_err(|e| CacheError::backend(format!("无法解析 Redis URL: {e}")))?;
        let manager = client
            .get_connection_manager()
            .await
            .map_err(|e| CacheError::backend(format!("建立 Redis 连接管理器失败: {e}")))?;

        Ok(Self { manager })
    }

    /// 获取底层的 `ConnectionManager` 引用副本，供外部微服务复用执行其他原生 Redis 命令
    pub fn connection_manager(&self) -> ConnectionManager {
        self.manager.clone()
    }
}

#[async_trait]
impl CacheStore for RedisStore {
    async fn get(&self, key: &str) -> CacheResult<Option<String>> {
        let mut conn = self.manager.clone();
        Ok(conn.get(key).await?)
    }

    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> CacheResult<()> {
        let mut conn = self.manager.clone();
        if let Some(ttl) = ttl_secs {
            let _: () = conn.set_ex(key, value, ttl).await?;
        } else {
            let _: () = conn.set(key, value).await?;
        }
        Ok(())
    }

    async fn del(&self, key: &str) -> CacheResult<()> {
        let mut conn = self.manager.clone();
        let _: () = conn.del(key).await?;
        Ok(())
    }

    async fn exists(&self, key: &str) -> CacheResult<bool> {
        let mut conn = self.manager.clone();
        Ok(conn.exists(key).await?)
    }

    /// 生产安全的前缀扫描批量淘汰（使用 SCAN 替代阻塞性的 KEYS）
    async fn del_prefix(&self, prefix: &str) -> CacheResult<usize> {
        let mut conn = self.manager.clone();
        let pattern = format!("{prefix}*");
        let mut cursor: u64 = 0;
        let mut total_deleted: usize = 0;

        loop {
            // SCAN cursor MATCH pattern COUNT 100
            let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await
                .map_err(|e| CacheError::backend(format!("Redis SCAN 失败: {e}")))?;

            if !keys.is_empty() {
                let deleted: usize = conn
                    .del(&keys)
                    .await
                    .map_err(|e| CacheError::backend(format!("Redis 批量 DEL 失败: {e}")))?;
                total_deleted += deleted;
            }

            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(total_deleted)
    }
}
