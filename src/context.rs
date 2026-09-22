//! 上下文通用选项与模型 (CacheOpts)
//!
//! 提供通用领域上下文小对象，彻底解耦微服务、Web、RPC、脚本或测试用例与缓存底层之间的联系。

use crate::traits::CacheContext;
use serde::{Deserialize, Serialize};

/// 通用缓存上下文配置选项 (CacheOpts)
///
/// 作为缓存引擎与业务系统之间的极简解耦介质。
/// 包含身份主体、租户、来源端点、特权标识与自定义隔离域。
///
/// 任何业务系统仅需将其会话、RPC 上下文或鉴权模型映射为此轻量小对象即可。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheOpts {
    /// 身份主体标识 (Subject / Principal)，如用户 ID、设备 ID、微服务 ID、玩家 ID 等
    pub subject: Option<String>,
    /// 多租户标识 (Tenant / Workspace / Organization)，支持 SaaS 场景下的物理隔离
    pub tenant: Option<String>,
    /// 接入来源端点 (Origin / Endpoint)，如客户端 IP、网关路由源、机房节点等
    pub origin: Option<String>,
    /// 是否具备特权模式 (Privileged / Supervisor / Admin)，特权模式下拥有全局跨隔离域数据视图
    pub is_privileged: bool,
    /// 显式自定义作用域 (Explicit Scope)，若设置将直接覆盖默认策略推导
    pub explicit_scope: Option<String>,
}

/// 兼容老代码的类型别名
pub type SimpleContext = CacheOpts;
pub type CacheContextOpt = CacheOpts;

impl CacheOpts {
    /// 构造默认空选项（代表共享/匿名上下文）
    pub fn new() -> Self {
        Self::default()
    }

    /// 快速构造指定主体 (Subject / User / Device) 的缓存配置
    pub fn subject<T: ToString + ?Sized>(sub: &T) -> Self {
        Self {
            subject: Some(sub.to_string()),
            ..Default::default()
        }
    }

    /// 快速构造指定租户 (Tenant / Workspace) 的缓存配置
    pub fn tenant<T: ToString + ?Sized>(tenant: &T) -> Self {
        Self {
            tenant: Some(tenant.to_string()),
            ..Default::default()
        }
    }

    /// 快速构造指定来源端点 (Origin / Client IP) 的缓存配置
    pub fn origin<T: ToString + ?Sized>(origin: &T) -> Self {
        Self {
            origin: Some(origin.to_string()),
            ..Default::default()
        }
    }

    /// 快速构造特权访问 (Privileged / Admin / Supervisor) 的缓存配置
    pub fn privileged() -> Self {
        Self {
            is_privileged: true,
            ..Default::default()
        }
    }

    /// 快速构造显式自定义隔离域 (Explicit Scope) 的缓存配置
    pub fn explicit<T: ToString + ?Sized>(scope: &T) -> Self {
        Self {
            explicit_scope: Some(scope.to_string()),
            ..Default::default()
        }
    }

    // --- Web / 微服务常见习惯友好别名 ---

    /// 别名：快速构造用户 ID 主体
    #[inline]
    pub fn uid<T: ToString + ?Sized>(uid: &T) -> Self {
        Self::subject(uid)
    }

    /// 别名：快速构造客户端 IP 来源
    #[inline]
    pub fn ip<T: ToString + ?Sized>(ip: &T) -> Self {
        Self::origin(ip)
    }

    /// 别名：快速构造管理员特权上下文
    #[inline]
    pub fn admin() -> Self {
        Self::privileged()
    }

    /// 别名：快速构造自定义作用域
    #[inline]
    pub fn scope<T: ToString + ?Sized>(scope: &T) -> Self {
        Self::explicit(scope)
    }

    // --- 流式链式构造方法 (Builder Pattern) ---

    pub fn with_subject(mut self, sub: impl Into<String>) -> Self {
        self.subject = Some(sub.into());
        self
    }

    pub fn with_tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = Some(tenant.into());
        self
    }

    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.origin = Some(origin.into());
        self
    }

    pub fn with_privileged(mut self, privileged: bool) -> Self {
        self.is_privileged = privileged;
        self
    }

    pub fn with_explicit_scope(mut self, scope: impl Into<String>) -> Self {
        self.explicit_scope = Some(scope.into());
        self
    }

    // 链式构造的友好别名
    #[inline]
    pub fn with_user_id(self, uid: impl Into<String>) -> Self {
        self.with_subject(uid)
    }

    #[inline]
    pub fn with_client_ip(self, ip: impl Into<String>) -> Self {
        self.with_origin(ip)
    }

    #[inline]
    pub fn with_admin(self, is_admin: bool) -> Self {
        self.with_privileged(is_admin)
    }

    #[inline]
    pub fn with_manage(self, is_manage: bool) -> Self {
        self.with_privileged(is_manage)
    }

    #[inline]
    pub fn with_custom_scope(self, scope: impl Into<String>) -> Self {
        self.with_explicit_scope(scope)
    }

    // --- 兼容旧字段读取的辅助方法 ---
    #[inline]
    pub fn user_id(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    #[inline]
    pub fn client_ip(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    #[inline]
    pub fn is_admin(&self) -> bool {
        self.is_privileged
    }

    #[inline]
    pub fn is_manage_mode(&self) -> bool {
        self.is_privileged
    }

    #[inline]
    pub fn custom_scope(&self) -> Option<&str> {
        self.explicit_scope.as_deref()
    }
}

/// CacheOpts 自带实现 CacheContext，方便直接传值使用
impl CacheContext for CacheOpts {
    fn to_cache_opts(&self) -> CacheOpts {
        self.clone()
    }
}
