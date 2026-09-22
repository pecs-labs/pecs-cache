//! 核心特质 (Traits) 定义

use crate::context::CacheOpts;
use crate::error::CacheResult;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::Arc;

/// 通用缓存底层存储特质 (CacheStore)
///
/// 任何存储后端（LocalMemory, Redis, Memcached 等）只需实现该接口即可作为缓存驱动
#[async_trait]
pub trait CacheStore: Send + Sync {
    /// 获取原始字符串值
    async fn get(&self, key: &str) -> CacheResult<Option<String>>;

    /// 设置键值对并附带过期时间（秒，None 为永不过期）
    async fn set(&self, key: &str, value: &str, ttl_secs: Option<u64>) -> CacheResult<()>;

    /// 删除指定缓存键
    async fn del(&self, key: &str) -> CacheResult<()>;

    /// 判断键是否存在且未过期
    async fn exists(&self, key: &str) -> CacheResult<bool>;

    /// 删除指定前缀的所有缓存键（用于列表/分页缓存的批量淘汰），返回实际删除的数量
    async fn del_prefix(&self, prefix: &str) -> CacheResult<usize> {
        let _ = prefix;
        Ok(0)
    }
}

/// 兼容老代码的特质别名
pub trait CacheTrait: CacheStore {}
impl<T: ?Sized + CacheStore> CacheTrait for T {}

/// 泛型扩展特质，简化 JSON 序列化对象的直接读写
#[async_trait]
pub trait CacheExt: CacheStore {
    /// 获取并自动反序列化为泛型对象
    async fn get_json<T: DeserializeOwned>(&self, key: &str) -> CacheResult<Option<T>> {
        if let Some(s) = self.get(key).await? {
            if let Ok(val) = serde_json::from_str::<T>(&s) {
                return Ok(Some(val));
            }
        }
        Ok(None)
    }

    /// 序列化泛型对象并写入缓存
    async fn set_json<T: Serialize + Send + Sync>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: Option<u64>,
    ) -> CacheResult<()> {
        let s = serde_json::to_string(value)?;
        self.set(key, &s, ttl_secs).await
    }
}

impl<T: ?Sized + CacheStore> CacheExt for T {}

/// 请求/会话上下文特质 (CacheContext)
///
/// 用于向缓存引擎提供当前调用者的身份与访问凭证，以实现基于 Subject/Tenant/Origin 的多维作用域安全隔离。
/// 彻底与任何特定微服务或 Web 框架（如 ReqCtx、Session）解耦。
pub trait CacheContext: Send + Sync {
    /// 转换为统一的缓存选项小对象 (CacheOpts)
    ///
    /// 【极力推荐】：自定义上下文仅需实现这一个方法，构造并返回一个包含必要字段的轻量小对象即可！
    fn to_cache_opts(&self) -> CacheOpts;

    // 以下方法均提供默认实现，直接从 to_cache_opts() 读取，无需手动实现：
    fn subject(&self) -> Option<String> {
        self.to_cache_opts().subject
    }
    fn tenant(&self) -> Option<String> {
        self.to_cache_opts().tenant
    }
    fn origin(&self) -> Option<String> {
        self.to_cache_opts().origin
    }
    fn is_privileged(&self) -> bool {
        self.to_cache_opts().is_privileged
    }
    fn explicit_scope(&self) -> Option<String> {
        self.to_cache_opts().explicit_scope
    }
    fn target_subject(&self) -> Option<String> {
        self.to_cache_opts().target_subject
    }
    fn isolate_page(&self) -> bool {
        self.to_cache_opts().isolate_page
    }

    // 兼容别名方法
    fn owner(&self) -> Option<String> {
        self.target_subject()
    }

    // 兼容传统 Web/微服务语境的别名方法
    fn user_id(&self) -> Option<String> {
        self.subject()
    }
    fn client_ip(&self) -> Option<String> {
        self.origin()
    }
    fn is_admin(&self) -> bool {
        self.is_privileged()
    }
    fn is_manage_mode(&self) -> bool {
        self.is_privileged()
    }
    fn custom_scope(&self) -> Option<String> {
        self.explicit_scope()
    }
}

// 针对单元类型 () 的默认实现（脚本、定时任务或无需用户态时直接传 &() 即可）
impl CacheContext for () {
    fn to_cache_opts(&self) -> CacheOpts {
        CacheOpts::default()
    }
}

impl<T: CacheContext> CacheContext for &T {
    fn to_cache_opts(&self) -> CacheOpts {
        (**self).to_cache_opts()
    }
}

impl<T: CacheContext> CacheContext for Arc<T> {
    fn to_cache_opts(&self) -> CacheOpts {
        (**self).to_cache_opts()
    }
}

// 针对 Option<T> 的开箱即用支持
impl<T: CacheContext> CacheContext for Option<T> {
    fn to_cache_opts(&self) -> CacheOpts {
        self.as_ref().map(|v| v.to_cache_opts()).unwrap_or_default()
    }
}

// 针对常见主键/主体 ID 基础类型的开箱即用支持（直接传 uid/subject 即可作为上下文，零样板代码）
macro_rules! impl_cache_context_for_primitives {
    ($($t:ty),*) => {
        $(
            impl CacheContext for $t {
                fn to_cache_opts(&self) -> CacheOpts {
                    CacheOpts::subject(self)
                }
            }
        )*
    };
}

impl_cache_context_for_primitives!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);

impl CacheContext for String {
    fn to_cache_opts(&self) -> CacheOpts {
        CacheOpts::subject(self)
    }
}

impl CacheContext for &str {
    fn to_cache_opts(&self) -> CacheOpts {
        CacheOpts::subject(*self)
    }
}

/// 辅助 Trait：将各种类型（值、引用、Option）统一提取为 Option<String> 形式的主键 ID
pub trait ToCacheIdOpt {
    fn to_cache_id_opt(&self) -> Option<String>;
}

macro_rules! impl_to_cache_id_opt_for_primitives {
    ($($t:ty),*) => {
        $(
            impl ToCacheIdOpt for $t {
                fn to_cache_id_opt(&self) -> Option<String> {
                    Some(self.to_string())
                }
            }
        )*
    };
}

impl_to_cache_id_opt_for_primitives!(u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);

impl ToCacheIdOpt for &str {
    fn to_cache_id_opt(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl ToCacheIdOpt for String {
    fn to_cache_id_opt(&self) -> Option<String> {
        Some(self.clone())
    }
}

impl<T: ToCacheIdOpt> ToCacheIdOpt for Option<T> {
    fn to_cache_id_opt(&self) -> Option<String> {
        self.as_ref().and_then(|v| v.to_cache_id_opt())
    }
}

impl<T: ToCacheIdOpt + ?Sized> ToCacheIdOpt for &T {
    fn to_cache_id_opt(&self) -> Option<String> {
        (*self).to_cache_id_opt()
    }
}
