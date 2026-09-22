//! 错误定义模块 (Error Definitions)

use thiserror::Error;

/// 统一缓存错误枚举
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CacheError {
    #[error("key not found: {0}")]
    NotFound(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[cfg(feature = "redis")]
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),

    #[error("backend store error: {0}")]
    Backend(String),

    #[error("type mismatch: {0}")]
    TypeMismatch(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("other error: {0}")]
    Other(String),
}

impl CacheError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn backend(msg: impl Into<String>) -> Self {
        Self::Backend(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }
}

/// 缓存统一 Result 类型
pub type CacheResult<T> = Result<T, CacheError>;
