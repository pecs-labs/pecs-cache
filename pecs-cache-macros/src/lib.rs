//! # pecs-cache-macros: 声明式缓存切面过程宏 (Declarative Caching Procedural Macros)
//!
//! 提供 `#[cacheable]`、`#[cache_evict]` 与 `#[cache_page]` 零侵入切面属性宏。
//! 支持智能参数推导（id/query/context/cache 均可全自动推导）、Option 包装返回值与空值防穿透。

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    parse_macro_input, Expr, FnArg, Ident, ImplItemFn, ItemFn, LitBool, Pat, Path, ReturnType,
    Token,
};

/// 检查函数返回值是否包装了 Option（如 Result<Option<T>, E>）
fn is_return_type_option(output: &ReturnType) -> bool {
    match output {
        ReturnType::Type(_, ty) => {
            let ty_str = quote!(#ty).to_string();
            ty_str.contains("Option <") || ty_str.contains("Option<")
        }
        ReturnType::Default => false,
    }
}

/// 智能推导业务主键 ID 表达式
fn deduce_id(inputs: &Punctuated<FnArg, Token![,]>, explicit_id: Option<Expr>) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(id) = explicit_id {
        return Ok(quote! { #id });
    }
    for arg in inputs {
        if let FnArg::Typed(pat_type) = arg {
            if let Pat::Ident(pat_ident) = &*pat_type.pat {
                let name = pat_ident.ident.to_string();
                if name == "id" {
                    return Ok(quote! { id });
                }
                if name == "param" || name == "req" || name == "request" {
                    return Ok(quote! { #pat_ident.payload.id });
                }
                if name == "bo" || name == "entity" {
                    return Ok(quote! { #pat_ident.id });
                }
            }
        }
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "无法自动推导 id 表达式，请显式指定，例如 `#[cacheable(UserBo, id = param.payload.id)]`",
    ))
}

/// 智能推导查询 Query 表达式
fn deduce_query(inputs: &Punctuated<FnArg, Token![,]>, explicit_query: Option<Expr>) -> proc_macro2::TokenStream {
    if let Some(q) = explicit_query {
        return quote! { &#q };
    }
    for arg in inputs {
        if let FnArg::Typed(pat_type) = arg {
            if let Pat::Ident(pat_ident) = &*pat_type.pat {
                let name = pat_ident.ident.to_string();
                if name == "query" || name == "q" {
                    return quote! { &#pat_ident };
                }
                if name == "param" || name == "req" || name == "request" {
                    return quote! { &#pat_ident.payload };
                }
            }
        }
    }
    for arg in inputs {
        if let FnArg::Typed(pat_type) = arg {
            if let Pat::Ident(pat_ident) = &*pat_type.pat {
                return quote! { &#pat_ident };
            }
        }
    }
    quote! { &() }
}

/// 解析 `#[cache_evict]` 属性宏入参
struct CacheEvictArgs {
    target: Path,
    id: Option<Expr>,
    page: bool,
    all: bool,
    ctx: Option<Expr>,
    sub: Option<Expr>,
    tenant: Option<Expr>,
    owner: Option<Expr>,
    opt: Option<Expr>,
    cache: Option<Expr>,
}

impl Parse for CacheEvictArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut target: Option<Path> = None;
        let mut id: Option<Expr> = None;
        let mut page = false;
        let mut all = false;
        let mut ctx: Option<Expr> = None;
        let mut sub: Option<Expr> = None;
        let mut tenant: Option<Expr> = None;
        let mut owner: Option<Expr> = None;
        let mut opt: Option<Expr> = None;
        let mut cache: Option<Expr> = None;

        let mut is_first = true;

        while !input.is_empty() {
            if is_first && !input.peek2(Token![=]) {
                target = Some(input.parse::<Path>()?);
                is_first = false;
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }
            is_first = false;

            let ident = input.parse::<Ident>()?;
            let key = ident.to_string();

            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                match key.as_str() {
                    "target" => {
                        target = Some(input.parse::<Path>()?);
                    }
                    "id" => {
                        id = Some(input.parse::<Expr>()?);
                    }
                    "page" => {
                        let lit = input.parse::<LitBool>()?;
                        page = lit.value();
                    }
                    "all" => {
                        let lit = input.parse::<LitBool>()?;
                        all = lit.value();
                    }
                    "ctx" => {
                        ctx = Some(input.parse::<Expr>()?);
                    }
                    "sub" | "subject" | "uid" | "user_id" => {
                        sub = Some(input.parse::<Expr>()?);
                    }
                    "tenant" => {
                        tenant = Some(input.parse::<Expr>()?);
                    }
                    "owner" | "target_sub" | "target_subject" => {
                        owner = Some(input.parse::<Expr>()?);
                    }
                    "opt" | "opts" => {
                        opt = Some(input.parse::<Expr>()?);
                    }
                    "cache" => {
                        cache = Some(input.parse::<Expr>()?);
                    }
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("未知参数 `{key}`，支持 target, id, page, all, ctx, sub/uid, tenant, owner/target_sub, opt, cache"),
                        ));
                    }
                }
            } else {
                match key.as_str() {
                    "page" => {
                        page = true;
                    }
                    "all" => {
                        all = true;
                    }
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("未知标志 `{key}`，单独布尔标志仅支持 `page` 或 `all`"),
                        ));
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let target = target.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                "必须指定 target 类型，例如 `#[cache_evict(UserBo, ...)]`",
            )
        })?;

        Ok(Self {
            target,
            id,
            page,
            all,
            ctx,
            sub,
            tenant,
            owner,
            opt,
            cache,
        })
    }
}

/// 解析 `#[cacheable]` 属性宏入参
struct CacheableArgs {
    target: Path,
    id: Option<Expr>,
    ctx: Option<Expr>,
    sub: Option<Expr>,
    tenant: Option<Expr>,
    opt: Option<Expr>,
    cache: Option<Expr>,
    optional: bool,
}

impl Parse for CacheableArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut target: Option<Path> = None;
        let mut id: Option<Expr> = None;
        let mut ctx: Option<Expr> = None;
        let mut sub: Option<Expr> = None;
        let mut tenant: Option<Expr> = None;
        let mut opt: Option<Expr> = None;
        let mut cache: Option<Expr> = None;
        let mut optional = false;

        let mut is_first = true;

        while !input.is_empty() {
            if is_first && !input.peek2(Token![=]) {
                target = Some(input.parse::<Path>()?);
                is_first = false;
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }
            is_first = false;

            let ident = input.parse::<Ident>()?;
            let key = ident.to_string();

            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                match key.as_str() {
                    "target" => {
                        target = Some(input.parse::<Path>()?);
                    }
                    "id" => {
                        id = Some(input.parse::<Expr>()?);
                    }
                    "ctx" => {
                        ctx = Some(input.parse::<Expr>()?);
                    }
                    "sub" | "subject" | "uid" | "user_id" => {
                        sub = Some(input.parse::<Expr>()?);
                    }
                    "tenant" => {
                        tenant = Some(input.parse::<Expr>()?);
                    }
                    "opt" | "opts" => {
                        opt = Some(input.parse::<Expr>()?);
                    }
                    "cache" => {
                        cache = Some(input.parse::<Expr>()?);
                    }
                    "optional" => {
                        let lit = input.parse::<LitBool>()?;
                        optional = lit.value();
                    }
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("未知参数 `{key}`，支持 target, id, ctx, sub/uid, tenant, opt, cache, optional"),
                        ));
                    }
                }
            } else {
                match key.as_str() {
                    "optional" => {
                        optional = true;
                    }
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("未知标志 `{key}`，可选标志仅支持 `optional`"),
                        ));
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let target = target.ok_or_else(|| {
            syn::Error::new(input.span(), "必须指定 target 类型，例如 `#[cacheable(UserBo)]`")
        })?;

        Ok(Self {
            target,
            id,
            ctx,
            sub,
            tenant,
            opt,
            cache,
            optional,
        })
    }
}

/// 解析 `#[cache_page]` 属性宏入参
struct CachePageArgs {
    target: Path,
    query: Option<Expr>,
    ctx: Option<Expr>,
    sub: Option<Expr>,
    tenant: Option<Expr>,
    opt: Option<Expr>,
    cache: Option<Expr>,
}

impl Parse for CachePageArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut target: Option<Path> = None;
        let mut query: Option<Expr> = None;
        let mut ctx: Option<Expr> = None;
        let mut sub: Option<Expr> = None;
        let mut tenant: Option<Expr> = None;
        let mut opt: Option<Expr> = None;
        let mut cache: Option<Expr> = None;

        let mut is_first = true;

        while !input.is_empty() {
            if is_first && !input.peek2(Token![=]) {
                target = Some(input.parse::<Path>()?);
                is_first = false;
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                continue;
            }
            is_first = false;

            let ident = input.parse::<Ident>()?;
            let key = ident.to_string();

            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                match key.as_str() {
                    "target" => {
                        target = Some(input.parse::<Path>()?);
                    }
                    "query" | "q" => {
                        query = Some(input.parse::<Expr>()?);
                    }
                    "ctx" => {
                        ctx = Some(input.parse::<Expr>()?);
                    }
                    "sub" | "subject" | "uid" | "user_id" => {
                        sub = Some(input.parse::<Expr>()?);
                    }
                    "tenant" => {
                        tenant = Some(input.parse::<Expr>()?);
                    }
                    "opt" | "opts" => {
                        opt = Some(input.parse::<Expr>()?);
                    }
                    "cache" => {
                        cache = Some(input.parse::<Expr>()?);
                    }
                    _ => {
                        return Err(syn::Error::new(
                            ident.span(),
                            format!("未知参数 `{key}`，支持 target, query, ctx, sub, tenant, opt, cache"),
                        ));
                    }
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        let target = target.ok_or_else(|| {
            syn::Error::new(input.span(), "必须指定 target 类型，例如 `#[cache_page(UserBo)]`")
        })?;

        Ok(Self {
            target,
            query,
            ctx,
            sub,
            tenant,
            opt,
            cache,
        })
    }
}

/// 智能推导当前方法调用的上下文 `CacheContext` 引用
fn deduce_ctx(
    inputs: &Punctuated<FnArg, Token![,]>,
    explicit_ctx: Option<Expr>,
    explicit_sub: Option<Expr>,
    explicit_tenant: Option<Expr>,
    explicit_owner: Option<Expr>,
    explicit_opt: Option<Expr>,
) -> proc_macro2::TokenStream {
    if let Some(opt) = explicit_opt {
        return quote! { &#opt };
    }
    if let Some(owner) = explicit_owner {
        return quote! { &::pecs_cache::CacheOpts::default().with_owner((#owner).to_string()).with_privileged(true) };
    }
    if let Some(tenant) = explicit_tenant {
        return quote! { &::pecs_cache::CacheOpts::tenant(&(#tenant)) };
    }
    if let Some(sub) = explicit_sub {
        return quote! { &::pecs_cache::CacheOpts::subject(&(#sub)) };
    }
    if let Some(ctx) = explicit_ctx {
        return quote! { #ctx };
    }
    for arg in inputs {
        if let FnArg::Typed(pat_type) = arg {
            if let Pat::Ident(pat_ident) = &*pat_type.pat {
                let name = pat_ident.ident.to_string();
                if name == "param" || name == "req" || name == "request" {
                    return quote! { &#pat_ident.context };
                }
                if name == "ctx" || name == "context" || name == "req_ctx" {
                    return quote! { &#pat_ident };
                }
                if name == "opt" || name == "opts" {
                    return quote! { &#pat_ident };
                }
                if name == "sub" || name == "subject" || name == "uid" || name == "user_id" {
                    return quote! { &::pecs_cache::CacheOpts::subject(&#pat_ident) };
                }
                if name == "tenant" {
                    return quote! { &::pecs_cache::CacheOpts::tenant(&#pat_ident) };
                }
            }
        }
    }
    // 默认空上下文，由 CacheContext for () 兜底
    quote! { &() }
}

/// 智能推导缓存管理器引用（默认约定为当前结构体中的 `self.cache`）
fn deduce_cache(explicit_cache: Option<Expr>) -> proc_macro2::TokenStream {
    if let Some(cache) = explicit_cache {
        quote! { #cache }
    } else {
        quote! { self.cache }
    }
}

/// 声明式写操作缓存失效属性宏
#[proc_macro_attribute]
pub fn cache_evict(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as CacheEvictArgs);

    // 场景 A: 修饰 `impl ...` 块中的成员方法
    if let Ok(mut impl_fn) = syn::parse::<ImplItemFn>(item.clone()) {
        let target = &args.target;
        let ctx = deduce_ctx(&impl_fn.sig.inputs, args.ctx, args.sub, args.tenant, args.owner, args.opt);
        let cache = deduce_cache(args.cache);

        let id_expr_opt = args.id.clone().or_else(|| deduce_id(&impl_fn.sig.inputs, None).ok().map(|ts| syn::parse2(ts).unwrap()));

        let evict_stmt = if args.all {
            if let Some(ref id_expr) = id_expr_opt {
                quote! {
                    let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                    if let Err(__err) = #cache.evict_all_smart::<#target>(__id_opt, #ctx).await {
                        ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict all in #[cache_evict]");
                    }
                }
            } else {
                quote! {
                    if let Err(__err) = #cache.evict_page::<#target>(#ctx).await {
                        ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict page in #[cache_evict]");
                    }
                }
            }
        } else if args.page {
            quote! {
                if let Err(__err) = #cache.evict_page::<#target>(#ctx).await {
                    ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict page in #[cache_evict]");
                }
            }
        } else if let Some(ref id_expr) = id_expr_opt {
            quote! {
                let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                if let Some(__id_str) = __id_opt {
                    if let Err(__err) = #cache.evict_with_ctx::<#target>(__id_str, #ctx).await {
                        ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict item in #[cache_evict]");
                    }
                }
            }
        } else {
            quote! {}
        };

        let orig_stmts = impl_fn.block.stmts;
        let new_block = syn::parse_quote!({
            let __res = async move {
                #(#orig_stmts)*
            }.await;

            if __res.is_ok() {
                #evict_stmt
            }

            __res
        });

        impl_fn.block = new_block;
        return TokenStream::from(quote! { #impl_fn });
    }

    // 场景 B: 修饰独立顶级函数 `item_fn`
    if let Ok(mut item_fn) = syn::parse::<ItemFn>(item) {
        let target = &args.target;
        let ctx = deduce_ctx(&item_fn.sig.inputs, args.ctx, args.sub, args.tenant, args.owner, args.opt);
        let cache = deduce_cache(args.cache);

        let id_expr_opt = args.id.clone().or_else(|| deduce_id(&item_fn.sig.inputs, None).ok().map(|ts| syn::parse2(ts).unwrap()));

        let evict_stmt = if args.all {
            if let Some(ref id_expr) = id_expr_opt {
                quote! {
                    let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                    if let Err(__err) = #cache.evict_all_smart::<#target>(__id_opt, #ctx).await {
                        ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict all in #[cache_evict]");
                    }
                }
            } else {
                quote! {
                    if let Err(__err) = #cache.evict_page::<#target>(#ctx).await {
                        ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict page in #[cache_evict]");
                    }
                }
            }
        } else if args.page {
            quote! {
                if let Err(__err) = #cache.evict_page::<#target>(#ctx).await {
                    ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict page in #[cache_evict]");
                }
            }
        } else if let Some(ref id_expr) = id_expr_opt {
            quote! {
                let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                if let Some(__id_str) = __id_opt {
                    if let Err(__err) = #cache.evict_with_ctx::<#target>(__id_str, #ctx).await {
                        ::tracing::warn!(target: "pecs_cache", error = %__err, "Failed to evict item in #[cache_evict]");
                    }
                }
            }
        } else {
            quote! {}
        };

        let orig_stmts = item_fn.block.stmts;
        let new_block = syn::parse_quote!({
            let __res = async move {
                #(#orig_stmts)*
            }.await;

            if __res.is_ok() {
                #evict_stmt
            }

            __res
        });

        item_fn.block = Box::new(new_block);
        return TokenStream::from(quote! { #item_fn });
    }

    syn::Error::new(proc_macro2::Span::call_site(), "#[cache_evict] 仅支持修饰 async 函数或方法")
        .to_compile_error()
        .into()
}

/// 声明式读操作透明穿透与自动回填属性宏
#[proc_macro_attribute]
pub fn cacheable(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as CacheableArgs);

    // 场景 A: 修饰 `impl ...` 块中的成员方法
    if let Ok(mut impl_fn) = syn::parse::<ImplItemFn>(item.clone()) {
        let target = &args.target;
        let id_expr = match deduce_id(&impl_fn.sig.inputs, args.id) {
            Ok(expr) => expr,
            Err(err) => return err.to_compile_error().into(),
        };
        let ctx = deduce_ctx(&impl_fn.sig.inputs, args.ctx, args.sub, args.tenant, None, args.opt);
        let cache = deduce_cache(args.cache);
        let is_option = args.optional || is_return_type_option(&impl_fn.sig.output);

        let orig_stmts = impl_fn.block.stmts;
        let fallback_stmts = orig_stmts.clone();

        let new_block = if is_option {
            syn::parse_quote!({
                let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                if let Some(__id) = __id_opt {
                    match #cache.lookup_with_ctx::<#target>(&__id, #ctx).await {
                        ::pecs_cache::CacheLookup::Hit(__cached) => return Ok(Some(__cached)),
                        ::pecs_cache::CacheLookup::NullHit => return Ok(None),
                        ::pecs_cache::CacheLookup::Miss => {}
                    }

                    let __res = async move {
                        #(#orig_stmts)*
                    }.await;

                    if let Ok(Some(ref __val)) = __res {
                        let _ = #cache.set_with_ctx::<#target>(&__id, __val, #ctx).await;
                    } else if let Ok(None) = __res {
                        let _ = #cache.set_null_with_ctx::<#target>(&__id, #ctx).await;
                    }

                    return __res;
                }

                async move {
                    #(#fallback_stmts)*
                }.await
            })
        } else {
            syn::parse_quote!({
                let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                if let Some(__id) = __id_opt {
                    if let Some(__cached) = #cache.get_with_ctx::<#target>(&__id, #ctx).await {
                        return Ok(__cached);
                    }

                    let __res = async move {
                        #(#orig_stmts)*
                    }.await;

                    if let Ok(ref __val) = __res {
                        let _ = #cache.set_with_ctx::<#target>(&__id, __val, #ctx).await;
                    }

                    return __res;
                }

                async move {
                    #(#fallback_stmts)*
                }.await
            })
        };

        impl_fn.block = new_block;
        return TokenStream::from(quote! { #impl_fn });
    }

    // 场景 B: 修饰独立顶级函数 `item_fn`
    if let Ok(mut item_fn) = syn::parse::<ItemFn>(item) {
        let target = &args.target;
        let id_expr = match deduce_id(&item_fn.sig.inputs, args.id) {
            Ok(expr) => expr,
            Err(err) => return err.to_compile_error().into(),
        };
        let ctx = deduce_ctx(&item_fn.sig.inputs, args.ctx, args.sub, args.tenant, None, args.opt);
        let cache = deduce_cache(args.cache);
        let is_option = args.optional || is_return_type_option(&item_fn.sig.output);

        let orig_stmts = item_fn.block.stmts;
        let fallback_stmts = orig_stmts.clone();

        let new_block = if is_option {
            syn::parse_quote!({
                let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                if let Some(__id) = __id_opt {
                    match #cache.lookup_with_ctx::<#target>(&__id, #ctx).await {
                        ::pecs_cache::CacheLookup::Hit(__cached) => return Ok(Some(__cached)),
                        ::pecs_cache::CacheLookup::NullHit => return Ok(None),
                        ::pecs_cache::CacheLookup::Miss => {}
                    }

                    let __res = async move {
                        #(#orig_stmts)*
                    }.await;

                    if let Ok(Some(ref __val)) = __res {
                        let _ = #cache.set_with_ctx::<#target>(&__id, __val, #ctx).await;
                    } else if let Ok(None) = __res {
                        let _ = #cache.set_null_with_ctx::<#target>(&__id, #ctx).await;
                    }

                    return __res;
                }

                async move {
                    #(#fallback_stmts)*
                }.await
            })
        } else {
            syn::parse_quote!({
                let __id_opt = ::pecs_cache::ToCacheIdOpt::to_cache_id_opt(&(#id_expr));
                if let Some(__id) = __id_opt {
                    if let Some(__cached) = #cache.get_with_ctx::<#target>(&__id, #ctx).await {
                        return Ok(__cached);
                    }

                    let __res = async move {
                        #(#orig_stmts)*
                    }.await;

                    if let Ok(ref __val) = __res {
                        let _ = #cache.set_with_ctx::<#target>(&__id, __val, #ctx).await;
                    }

                    return __res;
                }

                async move {
                    #(#fallback_stmts)*
                }.await
            })
        };

        item_fn.block = Box::new(new_block);
        return TokenStream::from(quote! { #item_fn });
    }

    syn::Error::new(proc_macro2::Span::call_site(), "#[cacheable] 仅支持修饰 async 函数或方法")
        .to_compile_error()
        .into()
}

/// 声明式分页查询透明穿透与自动回填属性宏 (`#[cache_page]`)
///
/// 彻底免去在 Service/Biz 层手写 `self.cache.page::<T>().load(...)` 样板代码，
/// 自动推导 query 入参、上下文与缓存引用，实现真正的零侵入。
#[proc_macro_attribute]
pub fn cache_page(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as CachePageArgs);

    // 场景 A: 修饰 `impl ...` 块中的成员方法
    if let Ok(mut impl_fn) = syn::parse::<ImplItemFn>(item.clone()) {
        let target = &args.target;
        let query_expr = deduce_query(&impl_fn.sig.inputs, args.query);
        let ctx = deduce_ctx(&impl_fn.sig.inputs, args.ctx, args.sub, args.tenant, None, args.opt);
        let cache = deduce_cache(args.cache);

        let orig_stmts = impl_fn.block.stmts;

        let new_block = syn::parse_quote!({
            let __query = #query_expr;
            #cache.page::<#target>().load(__query, #ctx, || async move {
                #(#orig_stmts)*
            }).await
        });

        impl_fn.block = new_block;
        return TokenStream::from(quote! { #impl_fn });
    }

    // 场景 B: 修饰独立顶级函数 `item_fn`
    if let Ok(mut item_fn) = syn::parse::<ItemFn>(item) {
        let target = &args.target;
        let query_expr = deduce_query(&item_fn.sig.inputs, args.query);
        let ctx = deduce_ctx(&item_fn.sig.inputs, args.ctx, args.sub, args.tenant, None, args.opt);
        let cache = deduce_cache(args.cache);

        let orig_stmts = item_fn.block.stmts;

        let new_block = syn::parse_quote!({
            let __query = #query_expr;
            #cache.page::<#target>().load(__query, #ctx, || async move {
                #(#orig_stmts)*
            }).await
        });

        item_fn.block = Box::new(new_block);
        return TokenStream::from(quote! { #item_fn });
    }

    syn::Error::new(proc_macro2::Span::call_site(), "#[cache_page] 仅支持修饰 async 函数或方法")
        .to_compile_error()
        .into()
}
