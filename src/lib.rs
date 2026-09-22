//! # pecs-cache: 通用声明式多级策略缓存框架
//!
//! 专为 Rust 高性能后端服务打造的开箱即用通用缓存框架：
//!
//! ## 核心特性
//! 1. **声明式策略路由 (`CachePolicy` / `CacheStrategy`)**：通过 `Shared`, `User`, `Ip`, `Private`, `None` 严格划分数据安全边界，默认安全，杜绝跨租户串线；
//! 2. **AOP 切面属性宏 (`#[cacheable]` / `#[cache_evict]`)**：透明实现 Cache-Aside 自动回填与数据更新主动失效，业务代码零样板；
//! 3. **多后端支持**：
//!    - **LocalMemoryStore**：纯 Rust 并发安全内存缓存，零外部依赖，纳秒/微秒级低延迟，单元测试极速启动；
//!    - **RedisStore**：基于 `redis-rs` 官方 `ConnectionManager`，支持外部微服务共享已有连接，避免连接池重复开销；
//! 4. **防数据泄漏与管理员代客操作机制**：精准推导操作人 (Operator) 与数据属主 (Owner) 的缓存淘汰，支持确定性哈希分页指纹与前缀批量清理；
//! 5. **动态配置插拔 (`CacheConfig`)**：支持按业务名称在 YAML/JSON 运行期动态覆盖策略与 TTL。

pub mod backend;
pub mod config;
pub mod context;
pub mod dynamic;
pub mod error;
pub mod manager;
pub mod policy;
pub mod traits;

// 常用类型顶级扁平导出
pub use backend::LocalMemoryStore;
#[cfg(feature = "redis")]
pub use backend::RedisStore;

pub use config::{CacheConfig, CacheEntryConfig};
pub use context::{CacheContextOpt, CacheOpts, SimpleContext};
pub use dynamic::{query_fingerprint, DynamicCache, DynamicCacheItem, DynamicCachePage};
pub use error::{CacheError, CacheResult};
pub use manager::create_dynamic_cache;
pub use policy::{CachePolicy, CacheStrategy};
pub use traits::{CacheContext, CacheExt, CacheStore, CacheTrait, ToCacheIdOpt};

// 过程宏导出
#[cfg(feature = "macros")]
pub use pecs_cache_macros::{cache_evict, cacheable};
