//! 配置模型定义

use crate::policy::CacheStrategy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_true() -> bool {
    true
}

fn default_prefix() -> String {
    "app_cache_".to_string()
}

fn default_backend() -> String {
    "memory".to_string()
}

fn default_ttl() -> u64 {
    300
}

fn default_null_ttl() -> u64 {
    30
}

fn default_jitter_ratio() -> f64 {
    0.05
}

/// 业务维度细粒度动态覆盖配置
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct CacheEntryConfig {
    /// 是否开启当前业务的缓存（为 false 时瞬间降级为纯查库）
    #[serde(default = "default_true")]
    pub enable: bool,

    /// 动态覆盖策略（如将 shared 改为 user/subject）
    #[serde(default)]
    pub strategy: Option<CacheStrategy>,

    /// 动态覆盖过期时间（秒）
    #[serde(default)]
    pub ttl: Option<u64>,

    /// 是否开启空值防穿透缓存（覆盖全局配置）
    #[serde(default)]
    pub cache_null: Option<bool>,

    /// 空值缓存的 TTL（秒，覆盖全局配置）
    #[serde(default)]
    pub null_ttl: Option<u64>,

    /// 是否对该业务启用 Single-flight 并发合并（覆盖全局配置）
    #[serde(default)]
    pub singleflight: Option<bool>,

    /// 该业务的分页是否默认按主体隔离（防止带用户过滤时污染共享缓存）
    #[serde(default)]
    pub isolate_page: Option<bool>,
}

/// 统一缓存全局配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheConfig {
    /// 全局总开关
    #[serde(default = "default_true")]
    pub enable: bool,

    /// Key 全局前缀，用于隔离不同服务或环境（如 "shop_svc_"）
    #[serde(default = "default_prefix")]
    pub prefix: String,

    /// 存储驱动类型："memory" 或 "redis"
    #[serde(default = "default_backend")]
    pub backend: String,

    /// 全局默认过期时间（秒）
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u64,

    /// 是否开启防穿透空值占位缓存（对 DB 中不存在的 None 记录缓存短时间）
    #[serde(default = "default_true")]
    pub cache_null: bool,

    /// 空值防穿透缓存默认 TTL（秒，默认 30 秒）
    #[serde(default = "default_null_ttl")]
    pub null_ttl_seconds: u64,

    /// TTL 随机抖动比例（0.0 ~ 0.5），用于消除缓存雪崩，默认 0.05 (±5%)
    #[serde(default = "default_jitter_ratio")]
    pub ttl_jitter_ratio: f64,

    /// 是否启用 Single-flight 并发合并（防缓存击穿/狗桩效应），默认 true
    #[serde(default = "default_true")]
    pub singleflight: bool,

    /// Redis 连接 URL（当 backend 为 "redis" 且需直接根据配置连 Redis 时使用）
    #[serde(default)]
    pub redis_url: Option<String>,

    /// 各业务细粒度覆盖项
    #[serde(default)]
    pub entries: HashMap<String, CacheEntryConfig>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enable: true,
            prefix: default_prefix(),
            backend: default_backend(),
            ttl_seconds: default_ttl(),
            cache_null: true,
            null_ttl_seconds: default_null_ttl(),
            ttl_jitter_ratio: default_jitter_ratio(),
            singleflight: true,
            redis_url: None,
            entries: HashMap::new(),
        }
    }
}

impl CacheConfig {
    /// 从 YAML 字符串反序列化
    #[cfg(feature = "serde_yaml")]
    pub fn from_yaml_str(yaml_str: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml_str)
    }

    /// 从 JSON 字符串反序列化
    pub fn from_json_str(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    /// 快速构建以 memory 为后端的配置
    pub fn memory(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            backend: "memory".to_string(),
            ..Self::default()
        }
    }

    /// 快速构建以 redis 为后端的配置
    pub fn redis(prefix: impl Into<String>, redis_url: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            backend: "redis".to_string(),
            redis_url: Some(redis_url.into()),
            ..Self::default()
        }
    }

    /// 检查指定业务是否启用了缓存，若启用则返回适用的有效 TTL 秒数
    pub fn get_effective_ttl(&self, biz: &str) -> Option<u64> {
        if !self.enable {
            return None;
        }

        if let Some(entry) = self.entries.get(biz) {
            if !entry.enable {
                return None;
            }
            return Some(entry.ttl.unwrap_or(self.ttl_seconds));
        }

        // 默认按全局策略启用
        Some(self.ttl_seconds)
    }

    /// 为 TTL 应用随机抖动，防止雪崩
    pub fn apply_ttl_jitter(&self, ttl: u64) -> u64 {
        if ttl == 0 || self.ttl_jitter_ratio <= 0.0 {
            return ttl;
        }
        // 使用高效伪随机哈希计算抖动，无需额外重型依赖
        let now_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(42);
        let factor = (now_nanos % 200) as f64 / 100.0 - 1.0; // -1.0 .. +1.0
        let delta = (ttl as f64 * self.ttl_jitter_ratio * factor).round() as i64;
        let final_ttl = (ttl as i64 + delta).max(1) as u64;
        final_ttl
    }

    /// 检查指定实体类型的实际缓存启用状态、生效策略与 TTL
    ///
    /// 优先级：配置 entries 动态覆盖 > 实体代码标记默认值
    pub fn resolve<T: crate::policy::CachePolicy>(&self) -> (bool, CacheStrategy, u64) {
        if !self.enable {
            return (false, CacheStrategy::None, 0);
        }

        let default_strategy = T::DEFAULT_STRATEGY;
        let default_ttl = if T::DEFAULT_TTL > 0 {
            T::DEFAULT_TTL
        } else {
            self.ttl_seconds
        };

        if let Some(entry) = self.entries.get(T::BIZ) {
            if !entry.enable {
                return (false, CacheStrategy::None, 0);
            }
            let strategy = entry.strategy.unwrap_or(default_strategy);
            let raw_ttl = entry.ttl.unwrap_or(default_ttl);
            return (strategy != CacheStrategy::None, strategy, self.apply_ttl_jitter(raw_ttl));
        }

        (
            default_strategy != CacheStrategy::None,
            default_strategy,
            self.apply_ttl_jitter(default_ttl),
        )
    }

    /// 检查指定实体是否启用了空值缓存以及空值 TTL
    pub fn resolve_null_cache<T: crate::policy::CachePolicy>(&self) -> (bool, u64) {
        if let Some(entry) = self.entries.get(T::BIZ) {
            let enabled = entry.cache_null.unwrap_or(self.cache_null);
            let ttl = entry.null_ttl.unwrap_or(self.null_ttl_seconds);
            (enabled, ttl)
        } else {
            (self.cache_null, self.null_ttl_seconds)
        }
    }

    /// 检查指定实体是否启用了 Single-flight
    pub fn resolve_singleflight<T: crate::policy::CachePolicy>(&self) -> bool {
        if let Some(entry) = self.entries.get(T::BIZ) {
            entry.singleflight.unwrap_or(self.singleflight)
        } else {
            self.singleflight
        }
    }
}

impl From<&CacheConfig> for CacheConfig {
    fn from(cfg: &CacheConfig) -> Self {
        cfg.clone()
    }
}

impl From<std::sync::Arc<CacheConfig>> for CacheConfig {
    fn from(arc: std::sync::Arc<CacheConfig>) -> Self {
        (*arc).clone()
    }
}

impl From<&std::sync::Arc<CacheConfig>> for CacheConfig {
    fn from(arc: &std::sync::Arc<CacheConfig>) -> Self {
        (**arc).clone()
    }
}
