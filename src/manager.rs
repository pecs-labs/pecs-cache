//! 便捷工厂与管理器构建器

use crate::backend::LocalMemoryStore;
#[cfg(feature = "redis")]
use crate::backend::RedisStore;
use crate::config::CacheConfig;
use crate::dynamic::DynamicCache;
use crate::error::{CacheError, CacheResult};
use crate::traits::CacheStore;
use std::sync::Arc;

/// 快速根据配置初始化 DynamicCache 实例
///
/// - 若 `backend == "redis"`：自动根据 `config.redis_url` 建立连接（需启用 `redis` feature）
/// - 若 `backend == "memory"` 或未指定：创建基于内存的 `LocalMemoryStore`
pub async fn create_dynamic_cache(config: CacheConfig) -> CacheResult<Arc<DynamicCache>> {
    let store: Arc<dyn CacheStore> = match config.backend.to_lowercase().as_str() {
        #[cfg(feature = "redis")]
        "redis" => {
            let url = config.redis_url.as_ref().ok_or_else(|| {
                CacheError::config("当 backend 为 redis 时，必须在 CacheConfig 中指定 redis_url")
            })?;
            let redis_store = RedisStore::connect(url).await?;
            Arc::new(redis_store)
        }
        #[cfg(not(feature = "redis"))]
        "redis" => {
            return Err(CacheError::config(
                "当前编译未开启 `redis` feature，无法使用 Redis 缓存后端",
            ));
        }
        _ => Arc::new(LocalMemoryStore::new()),
    };

    Ok(Arc::new(DynamicCache::new(store, config)))
}
