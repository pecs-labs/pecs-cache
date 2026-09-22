//! 缓存策略与声明式特质

use serde::{Deserialize, Serialize};

/// 缓存隔离范围与路由策略
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CacheStrategy {
    /// 关闭缓存（默认安全模式：直接穿透底层数据库，不产生缓存，杜绝数据泄漏）
    #[default]
    None,
    /// 全局共享缓存：所有主体共享相同 Key（如公共配置、字典、公开文章）
    Shared,
    /// 主体隔离缓存：基于身份主体严格隔离 Key（如用户 ID、设备 ID，未认证主体安全穿透，不污染共享域）
    #[serde(alias = "user")]
    Subject,
    /// 多租户隔离缓存：基于租户/空间 ID 严格隔离 Key（多租户 SaaS 系统）
    Tenant,
    /// 来源端点隔离缓存：基于客户端 IP/接入来源隔离 Key（适合频控或访客防护）
    #[serde(alias = "ip")]
    Origin,
    /// 私有级联缓存：优先 Subject，未认证降级为 Origin，无凭证安全穿透
    #[serde(alias = "private")]
    Cascading,
}

impl CacheStrategy {
    // 兼容传统 Web / 微服务语境的别名常量
    #[allow(non_upper_case_globals)]
    pub const User: CacheStrategy = CacheStrategy::Subject;
    #[allow(non_upper_case_globals)]
    pub const Ip: CacheStrategy = CacheStrategy::Origin;
    #[allow(non_upper_case_globals)]
    pub const Private: CacheStrategy = CacheStrategy::Cascading;

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Shared => "shared",
            Self::Subject => "subject",
            Self::Tenant => "tenant",
            Self::Origin => "origin",
            Self::Cascading => "cascading",
        }
    }
}

/// 领域实体 / BO / VO 的声明式缓存策略特质
///
/// 任何需要缓存的对象实现该 Trait（推荐直接使用 `cache_policy!` 宏）：
pub trait CachePolicy: Send + Sync {
    /// 业务唯一命名空间标识（例如 "user", "post", "order"）
    const BIZ: &'static str;

    /// 编译期默认策略（默认为 None，保证默认安全，可被配置文件动态覆盖）
    const DEFAULT_STRATEGY: CacheStrategy = CacheStrategy::None;

    /// 编译期默认 TTL（秒，可被配置文件动态覆盖）
    const DEFAULT_TTL: u64 = 300;
}

/// 声明式缓存策略绑定宏
///
/// 供业务层极简声明实体所绑定的缓存策略与生命周期：
/// ```rust,ignore
/// use pecs_cache::{cache_policy, CacheStrategy};
///
/// // 声明 UserBo 使用 Subject 隔离策略（或兼容别名 User），默认缓存 600 秒
/// cache_policy!(UserBo, biz = "user", strategy = Subject, ttl = 600);
/// ```
#[macro_export]
macro_rules! cache_policy {
    ($target:ident, biz = $biz:expr, strategy = $strategy:ident, ttl = $ttl:expr) => {
        impl $crate::CachePolicy for $target {
            const BIZ: &'static str = $biz;
            const DEFAULT_STRATEGY: $crate::CacheStrategy = $crate::CacheStrategy::$strategy;
            const DEFAULT_TTL: u64 = $ttl;
        }
    };
    ($target:ident, biz = $biz:expr, strategy = $strategy:ident) => {
        $crate::cache_policy!($target, biz = $biz, strategy = $strategy, ttl = 300);
    };
    ($target:ident, biz = $biz:expr) => {
        $crate::cache_policy!($target, biz = $biz, strategy = None, ttl = 300);
    };
}
