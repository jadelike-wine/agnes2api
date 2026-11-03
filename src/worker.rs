//! Cloudflare Workers 入口与运行时装配。
//!
//! # 形态边界
//!
//! 🔴 **本模块只在 `wasm32 + worker` 下编译**（见 `lib.rs`）。
//! 它与 `main.rs`（`#[tokio::main]` + `server::serve`）是同一套业务的两个入口：
//! `main.rs` 监听 TCP、跑 tokio 定时轮、落 `store.json`；
//! `worker.rs` 接 `#[event(fetch)]`、Cron Trigger 只做索引对账、落 Cloudflare D1。
//! 两种形态**共用同一棵 `axum::Router`** 与 `server::wire::build_app` ——
//! 这正是「不为 Workers 复制一套业务」的那条线。
//!
//! # 路由与复用
//!
//! workers-rs 的 `http` feature 让 `#[event(fetch)]` 直接吃
//! `http::Request<worker::Body>`、吐 `http::Response<axum::body::Body>`。
//! axum 的 `Router` 实现 `tower::Service<http::Request<axum::body::Body>>`，
//! 所以 `router.call(req)` 就能复用全部路由、中间件、提取器 ——
//! **不需要 `axum::serve`**（那是 hyper 的 TCP 服务器，Workers 上没有）。
//!
//! ⚠️ 类型对齐只有一处要人工做：`worker::Body` → `axum::body::Body`。
//! `axum::body::Body::new(stream)` 接受 `http_body::Body<Data = Bytes>`，
//! `worker::Body` 正是它（包 `web_sys::ReadableStream`）。

use std::sync::Arc;
use worker::{Context, Env, HttpRequest, event};

use crate::adapters::fetcher_cloudflare::CfFetcher;
use crate::adapters::logger_worker::WorkerLogger;
use crate::adapters::storage_d1::D1Storage;
use crate::core::config_provenance::EnvMap;
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::Storage;
use crate::server::wire::{BuildOptions, BuiltApp, build_app};

/// 每次请求的装配结果缓存。
///
/// Workers 的 `#[event(fetch)]` 是**逐请求**的，但 `build_app` 只做一次 —
/// 冷启动建 Router 是热的（把 AppState、存储、配置全读一遍），不能每个请求重来。
/// `OnceLock` 保证同一实例（同一 isolate）内只装一次；
/// 实例回收后下一次冷启动自然重建，这正是「实例回收后数据仍在」的边界。
///
/// `SendWrapper` 的 `new` 不是 const，静态初始化不能用 —— 用 `LazyLock` 延迟初始化。
static APP: std::sync::LazyLock<
    worker::send::SendWrapper<std::sync::OnceLock<Option<Arc<BuiltApp>>>>,
> = std::sync::LazyLock::new(|| worker::send::SendWrapper::new(std::sync::OnceLock::new()));

/// 逐键收集 env。返回是否成功装进表里（`GATEWAY_TOKEN` 缺失要如实失败）。
fn collect_env(env: &Env) -> EnvMap {
    // 🔴 **键名清单与 `core::config` 的字段表对齐**。新增配置项要在这里补一行 —
    //    这是 Workers 形态唯一能感知「这个键有人用」的地方。
    const KEYS: &[&str] = &[
        // ── 两道口令（secrets）────────────────────────────────────────────
        "GATEWAY_TOKEN",
        "ADMIN_TOKEN",
        // ── 池与超时旋钮 ─────────────────────────────────────────────────
        "UPSTREAM_TIMEOUT_MS",
        "UPSTREAM_SYNC_TIMEOUT_MS",
        "MAX_STRIKES",
        "COOLDOWN_STRIKE_MS",
        "LB_MODE",
        "POOL_CACHE_TTL_MS",
        // ── 上游 ─────────────────────────────────────────────────────────
        "AGNES_BASE_URL",
        // ── 行为开关 ─────────────────────────────────────────────────────
        "TRUST_PROXY",
        "USAGE_STATS_ENABLED",
        "USAGE_FLUSH_INTERVAL_MS",
    ];

    let mut map = EnvMap::new();
    for key in KEYS {
        // secrets 优先：它是密文，vars 是明文 —— 同名时密文才是凭据。
        // `Secret`/`Var` 都是 `StringBinding`（impl Display）→ `to_string()`。
        if let Ok(s) = env.secret(key) {
            let v = s.to_string();
            if !v.is_empty() {
                map.insert((*key).to_string(), v);
                continue;
            }
        }
        if let Ok(v) = env.var(key) {
            let s = v.to_string();
            if !s.is_empty() {
                map.insert((*key).to_string(), s);
            }
        }
    }
    map
}

/// 装配。冷启动（每实例一次）时由 `fetch` 调用。
///
/// `None` = 装配失败（fail-closed）。`build_app` 内部会打详细日志；
/// 这里只回一个「起不来」的信号，让 `fetch` 回 503 而不是 panic。
///
/// 🔴 **D1 绑定 `DB` 缺失 / `GATEWAY_TOKEN` 缺失** 都会走到 `Err`，
/// 但失败路径不在这里展开 —— `build_app` 的 `ConfigRefusal` 会把
/// 「缺哪一把钥匙」原样带进事件与错误消息。
async fn assemble(env: &Env) -> Option<Arc<BuiltApp>> {
    // Workers 的可见出口是 `console.*`（`wrangler tail` / 面板实时日志），
    // 不是 `stderr` —— 所以这里用 `WorkerLogger`，渲染规则与 Docker 侧同一份 logfmt。
    let logger: Arc<dyn Logger> = Arc::new(WorkerLogger);

    // D1 绑定。缺它就没有存储，网关起不来 —— 如实 fail。
    let storage = match D1Storage::from_env(env) {
        Ok(s) => s,
        Err(e) => {
            logger.log(LogEntry::new(
                LogLevel::Error,
                "worker.storage_missing",
                format!("D1 binding `DB` 不可用，Worker 无法启动: {e}"),
            ));
            return None;
        }
    };
    let storage: Arc<dyn Storage> = Arc::new(storage);

    // 环境快照。Workers 没有 `std::env`/`cli`，env 只能来自 bindings。
    let env_map = collect_env(env);
    // 管理面的配置来源与网关启动时一致；wasm 没有进程环境可供兜底读取。
    crate::admin::config::install_env_snapshot(env_map.clone());

    // `probe_writable` 在 Worker 上不适用：D1 不是文件系统，
    // 探针写的是 `probe:` 键，那会在 D1 里留一行垃圾 —— 关掉。
    // `fetcher` 显式给 CfFetcher（`worker::Fetch`），`build_registrar` 关：
    // 注册机依赖 tokio 定时轮，Workers 没有；而且它要自动铸 key，不是本形态需求。
    match build_app(
        &env_map,
        storage,
        BuildOptions {
            probe_storage: false,
            fetcher: Some(Arc::new(CfFetcher::data_plane())),
            build_registrar: false,
        },
        logger,
    )
    .await
    {
        Ok(b) => Some(Arc::new(b)),
        Err(e) => {
            // `build_app` 已经把原因打进 logger（fail-closed 的配置拒绝）。
            // 这里只多打一条「装配最终没成」的总结，方便 `wrangler tail` 第一眼看到。
            let _ = e;
            None
        }
    }
}

/// `#[event(fetch)]` —— Worker 的 HTTP 入口。
///
/// 返回 `http::Response<axum::body::Body>`：
/// axum 的 `Router` 吐 `axum::body::Body`，`worker::Response::from`（`axum` feature）
/// 会把它转成 `web_sys::Response` —— **流式体透传**，SSE 逐块走。
///
/// 🔴 **请求体**：`worker::Body` → `axum::body::Body`。
/// `axum::body::Body::new` 接受任何 `http_body::Body<Data = Bytes>`；
/// `worker::Body` 正是。上游侧，`CfFetcher` 再把 `axum::body::Body` 转回
/// `web_sys::RequestInit` 的 body。
#[event(fetch)]
pub async fn fetch(
    req: HttpRequest,
    env: Env,
    _ctx: Context,
) -> worker::Result<axum::http::Response<axum::body::Body>> {
    // 装配只跑一次。`OnceLock::get_or_try_init` 在 async 里不可用，
    // 所以先 `get`、没有再 init —— 两个并发首请求可能各装一次，
    // 但 `OnceLock` 只保留先完成的那个，后装的那份被 drop（它没注册任何副作用，安全）。
    let app = match APP.get() {
        Some(Some(a)) => a.clone(),
        Some(None) | None => {
            let built = assemble(&env).await;
            let _ = APP.set(built);
            match APP.get() {
                Some(Some(a)) => a.clone(),
                _ => {
                    return Ok(axum::http::Response::builder()
                        .status(503)
                        .header("content-type", "application/json")
                        .body(axum::body::Body::from(
                            r#"{"error":{"type":"service_unavailable","message":"worker 装配失败（缺 D1 binding 或 GATEWAY_TOKEN），请检查 wrangler tail 与 secrets"}}"#.to_string(),
                        ))
                        .expect("builder"),
                    );
                }
            }
        }
    };

    // `worker::Body` → `axum::body::Body`：把 http::Request 的 body 换掉。
    // `axum::body::Body::new` 收 `http_body::Body<Data = Bytes>` —— `worker::Body` 满足。
    let (parts, body) = req.into_parts();
    let req = axum::http::Request::from_parts(parts, axum::body::Body::new(body));

    // 调 axum Router。`Router` 实现 `Service<Request<Body>>`，
    // `call` 返回 `Response<Body>` —— 流式体原样透传。
    // `Service::call` 在 `tower::Service` 上，不在 `Router` 本身。
    let mut router = app.router.clone();
    let resp = tower::Service::call(&mut router, req)
        .await
        .map_err(|e| worker::Error::RustError(format!("axum router 调用失败: {e}")))?;

    Ok(resp)
}

/// `#[event(scheduled)]` —— Cron Trigger。
///
/// 🔴 **只做 key 池索引对账，绝不补池、绝不跑注册机。**
///
/// `spawn_scheduler` 里那条对账 + 补池一体的定时轮是 tokio 专属，
/// Workers 上没有 `tokio::spawn`；而「补池」这件事本身就不是本形态的需求
///（用户自备 Agnes key，手动导入）。索引对账则不同 —— `add`/`delete`
/// 的半程失败会留孤儿记录 / 幽灵索引，读路径虽能自愈，但 Cron 让它**定期**收敛。
///
/// 注册机区域在面板上会如实显示「不支持」（`state.registrar = None` ⇒ `503 not_wired`），
/// 不是假装它正常工作。
#[event(scheduled)]
pub async fn scheduled(_event: worker::ScheduledEvent, env: Env, _ctx: worker::ScheduleContext) {
    // 🔴 **Cron 可能比第一个 fetch 先到**（新部署后 30 分钟内没人访问也会触发）。
    // 所以这里也要能自己把装配立起来 —— 否则「对账」在最该跑的冷启动阶段永远不跑。
    let app = match APP.get() {
        Some(Some(a)) => a.clone(),
        _ => {
            let built = assemble(&env).await;
            let _ = APP.set(built);
            match APP.get() {
                Some(Some(a)) => a.clone(),
                // 装配失败（缺 D1 / GATEWAY_TOKEN）已经由 `build_app` 打了原因。
                // Cron 里没有客户端可回，如实返回即可 —— 下一次触发会再试。
                _ => return,
            }
        }
    };
    if let Err(e) = app.state.repo.reconcile().await {
        app.state.logger.log(
            LogEntry::new(
                LogLevel::Warn,
                "pool.index_reconcile_failed",
                "Cron 索引对账失败，本轮跳过",
            )
            .with("err", e.to_string()),
        );
    }
}
