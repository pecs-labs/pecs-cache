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

/// 业务维度细粒度动态覆盖配置
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CacheEntryConfig {
    /// 是否开启当前业务的缓存（为 false 时瞬间降级为纯查库）
    #[serde(default = "default_true")]
    pub enable: bool,

    /// 动态覆盖策略（如将 shared 改为 user）
    #[serde(default)]
    pub strategy: Option<CacheStrategy>,

    /// 动态覆盖过期时间（秒）
    #[serde(default)]
    pub ttl: Option<u64>,
}

/// 统一缓存全局配置
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheConfig {
    /// 全局总开关
    #[serde(default = "default_true")]
    pub enable: bool,

    /// Key 全局前缀，用于隔离不同微服务或环境（如 "user_svc_"）
    #[serde(default = "default_prefix")]
    pub prefix: String,

    /// 存储驱动类型："memory" 或 "redis"
    #[serde(default = "default_backend")]
    pub backend: String,

    /// 全局默认过期时间（秒）
    #[serde(default = "default_ttl")]
    pub ttl_seconds: u64,

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
            enable: true,
            prefix: prefix.into(),
            backend: "memory".to_string(),
            ttl_seconds: 300,
            redis_url: None,
            entries: HashMap::new(),
        }
    }

    /// 快速构建以 redis 为后端的配置
    pub fn redis(prefix: impl Into<String>, redis_url: impl Into<String>) -> Self {
        Self {
            enable: true,
            prefix: prefix.into(),
            backend: "redis".to_string(),
            ttl_seconds: 300,
            redis_url: Some(redis_url.into()),
            entries: HashMap::new(),
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
            let ttl = entry.ttl.unwrap_or(default_ttl);
            return (strategy != CacheStrategy::None, strategy, ttl);
        }

        (
            default_strategy != CacheStrategy::None,
            default_strategy,
            default_ttl,
        )
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
