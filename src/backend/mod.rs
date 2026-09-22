//! 存储后端实现模块

pub mod memory;

#[cfg(feature = "redis")]
pub mod redis;

pub use memory::LocalMemoryStore;

#[cfg(feature = "redis")]
pub use self::redis::RedisStore;
