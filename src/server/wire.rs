//! 装配：环境变量 + 存储 → [`AppState`] → `Router`。**顺序有意义。**

#[cfg(not(target_arch = "wasm32"))]
use crate::adapters::fetcher_reqwest::ReqwestFetcher;
use crate::adapters::logger_multi::MultiLogger;
use crate::adapters::logger_store::StoreLogger;
use crate::adapters::now_ms;
use crate::adapters::storage_watch::{WatchedStorage, probe_writable};
use crate::core::config::resolve_config;
use crate::core::config_errors::ConfigRefusal;
use crate::core::config_provenance::{
    CONFIG_KEY, EVENT_CONFIG_INVALID, EVENT_CONFIG_STORAGE_UNREADABLE, EnvMap, LoadFlags,
};
use crate::core::keypool::LbMode;
use crate::core::storage_health::StorageHealth;
use crate::pool::repo::KeyPoolRepo;
use crate::ports::fetcher::Fetcher;
use crate::ports::logger::{LogEntry, LogLevel, Logger};
use crate::ports::storage::Storage;
use crate::server::app::build_router;
use crate::server::holders::apikey_holder::{APIKEY_CACHE_TTL_MS, ApiKeyHolder};
use crate::server::holders::config_holder::ConfigHolder;
use crate::server::middleware::admin_auth::validate_admin_token;
use crate::server::state::AppState;
use axum::Router;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

/// 装配选项。
pub struct BuildOptions {
    /// 要不要在启动时真的探一次「数据目录写不写得进」。
    ///
    /// **只有真实启动路径开**：测试里开它等于给每一格判据加一次写。
    pub probe_storage: bool,

    /// 出站客户端。**`None` = 按目标平台取默认**。
    ///
    /// 原生（Docker）默认 `ReqwestFetcher::data_plane()`；
    /// wasm（Workers）默认 `CfFetcher`（`worker::Fetch`）。
    /// Worker 装配显式传一份，是为了让**本函数在两种形态下共用一个签名** —
    /// 否则就得在 `wire.rs` 里用 cfg 藏两条不同的装配路，而那两条路迟早会漂。
    pub fetcher: Option<Arc<dyn Fetcher>>,

    /// 要不要构造注册机持有者。**Worker 形态传 `false`**：
    /// 注册机依赖 tokio 定时轮与 reqwest，Workers 上没有这两样；
    /// 而且注册机的存在前提（自动铸 key）本就不是 Workers 形态的需求。
    /// `false` ⇒ `state.registrar = None`，三条端点如实回 `503 not_wired`，
    /// 面板显示「不支持」，而不是假装它正常工作。
    pub build_registrar: bool,
}

impl BuildOptions {
    /// 测试与简单装配用的最小选项集：**默认按 Docker 形态**（注册机开、探针关）。
    ///
    /// `Default` 不派生是因为 `build_registrar` 的语义默认（`true` = 原生行为）
    /// 与「全 `false`」相反，写一个显式构造函数把这个选择摆在明面上。
    pub fn minimal() -> Self {
        Self {
            probe_storage: false,
            fetcher: None,
            build_registrar: true,
        }
    }
}

/// 装配的产物。
///
/// 交出 `state` **不是为了测试方便**，是一条结构性约束：
/// 面板写完 key 之后要让**这一个**实例失效，另建一个等于空操作。
pub struct BuiltApp {
    pub router: Router,
    pub state: AppState,
}

/// 装配。
///
/// # 顺序（编号与 TS 版的 12 步对齐）
///
/// | # | 步骤 | 要点 |
/// |---|---|---|
/// | 1 | 统一时钟 | 见下方长注释 |
/// | 2 | 存储健康 + 包一层观察写操作 | 包了之后**所有写的成败自动反映到 `/health`** |
/// | 3 | `shard_id`（每进程一次、终生不变） | 事件 sink 与用量 sink **共用同一个** |
/// | 4 | 事件落库 sink | 🔴 它的 `on_error` 走**裸 console**，不走扇出后的 logger |
/// | 5 | 扇出 logger | |
/// | 6 | `probe_writable`（仅真实启动路径） | 失败**不阻止启动** |
/// | 7 | 装载配置 | 🔴 **失败必须抛**（缺 `GATEWAY_TOKEN` 拒绝服务是不变量） |
/// | 8 | `KeyPoolRepo` | ⚠️ 两个旋钮**建 app 时读一次**，改了要重启 |
/// | 9 | 注册机 + 定时轮 | ⚠️ **关着也构造持有者、也起定时轮**（面板要读得出、也要开得起来） |
/// | 10 | 用量 sink | ⚠️ **关 = `None`**，不是「建好再用 `if` 拦住写」 |
/// | 11 | 对外密钥持有者 | **无条件建**，与第 10 步刻意不同 |
/// | 12 | 建 Router | |
///
/// ## 第 1 步：关于「统一时钟」
///
/// TS 版在这里造一个 `now` 闭包并让本次装配的**所有**组件共用它，理由是实测踩过：
/// 存储 TTL 与 sink 的 `expiresAt` 用不同时钟会造出「生下来就过期」的键。
///
/// Rust 侧的做法是**全体经由 [`crate::adapters::now_ms`] 这一个函数**读墙钟，
/// `core` 一律收 `now: i64` 参数。这在「只有一个时钟基准」这件事上等价，
/// 但**弱在一点上**：它不是一个可注入的东西。
/// ⚠️ 事件 sink 那一个确实需要可注入时钟（判据要推着它走最小间隔闸），
/// 所以它走的是 `with_clock` —— 生产路径传的仍然是 `now_ms`。
pub async fn build_app(
    env: &EnvMap,
    storage: Arc<dyn Storage>,
    opts: BuildOptions,
    console: Arc<dyn Logger>,
) -> Result<BuiltApp, ConfigRefusal> {
    // 2. 包一层，让每一次**真实的写**都反映到 /health 上。
    let health = Arc::new(StorageHealth::new());
    let storage: Arc<dyn Storage> = Arc::new(WatchedStorage::new(storage, health.clone()));

    // 3. shard_id：**每进程一次，终生不变**。
    let shard_id = random_shard_id();

    // 4. 事件落库 sink。
    //    🔴 `fallback` 是**裸 console**，不是下面那个扇出 logger ——
    //       sink 自己坏了还往自己里写没有任何意义。
    let event_sink = Arc::new(StoreLogger::new(
        storage.clone(),
        shard_id.clone(),
        console.clone(),
    ));

    // 5. 扇出。**一个 sink 坏掉不影响别的 sink，更不影响调用方。**
    let logger: Arc<dyn Logger> =
        Arc::new(MultiLogger::new(vec![console.clone(), event_sink.clone()]));

    // 6. 数据目录是绑定挂载，属主不匹配就整个网关不可用（写不进 store.json），
    //    必须在启动那一刻探出来并让 /health 如实报告，**不能等到第一个请求失败才发现**。
    //    🔴 探测失败**不阻止启动**：让容器起来并报 unhealthy，比崩溃重启循环更容易排障。
    if opts.probe_storage
        && let Some(err) = probe_writable(storage.as_ref(), health.as_ref()).await
    {
        logger.log(
            LogEntry::new(
                LogLevel::Error,
                "storage.probe_failed",
                "数据目录写不进去，key 池无法持久化；网关照常启动但 /health 会报 degraded",
            )
            .with("error", err.to_string()),
        );
    }

    // 7. 装载配置。
    let (stored, storage_unreadable) = read_stored_config(storage.as_ref(), logger.as_ref()).await;
    let (config, flags) = resolve_config(env, stored.as_ref(), storage_unreadable)?;
    emit_notices(logger.as_ref(), &flags);

    // 8. key 池。⚠️ 两个旋钮**建 app 时读一次**（它们绑定的是部署形态，
    //    不是逐次可调的策略）。**面板文案不许写「立即生效」。**
    let repo = Arc::new(KeyPoolRepo::new(
        storage.clone(),
        logger.clone(),
        config.pool_cache_ttl_ms,
        config.pool_touch_interval_ms,
    ));

    // 10. 用量 sink。
    //
    //     🔴 **关着时是 `None`，而不是「建好再用 `if` 拦住写」** ——
    //     那条形态差别是可观测的：关着时**零存储调用、零累加器**，
    //     连那条 flush 中间件都不在链上。
    //     ⚠️ **落盘间隔只从环境变量读，一个字都不从存储读** ——
    //     它因此不进 `GatewayConfig`、不进面板的四元组。那个取舍与它的代价
    //     照搬 TS 侧的裁定：这个值改一次要重启，而面板上能改的东西必须是热生效的。
    let usage_flush_interval_ms = env
        .get("USAGE_FLUSH_INTERVAL_MS")
        .map(String::as_str)
        .filter(|s| !s.trim().is_empty())
        .map_or(
            Ok(crate::server::usage_sink::USAGE_FLUSH_DEFAULT_MS),
            |raw| {
                raw.parse::<i64>()
                    .ok()
                    .filter(|n| *n >= 1)
                    // 🔴 **非法值一律拒绝启动，不降级**：这是部署时错误，运维必须立刻看得见，
                    // 而且它不可能是面板写坏的（面板永远碰不到环境变量）。
                    .ok_or_else(|| {
                        crate::core::config_errors::ConfigRefusal::new(format!(
                            "环境变量 USAGE_FLUSH_INTERVAL_MS 必须是不小于 1 的整数: {raw}"
                        ))
                    })
            },
        )?;
    let usage_sink = if config.usage_stats_enabled {
        Some(Arc::new(crate::server::usage_sink::UsageSink::new(
            storage.clone(),
            logger.clone(),
            &shard_id,
            usage_flush_interval_ms,
            now_ms(),
        )))
    } else {
        None
    };

    // 11. 对外密钥持有者：**无条件建**。
    //     与第 10 步刻意不同 —— 它在「没签发过子密钥」的部署上一次存储调用都不产生，
    //     因为那条路径上根本没有调用点（不是因为某个 `if`）。
    let api_keys = Some(Arc::new(ApiKeyHolder::new(
        storage.clone(),
        logger.clone(),
        apikey_cache_ttl_ms(env)?,
    )));

    let lb_mode = lb_mode(env, logger.as_ref());
    let admin_token = admin_token(env, logger.as_ref());
    let trust_proxy = truthy(env.get("TRUST_PROXY"));

    let config = Arc::new(ConfigHolder::prime(
        env,
        storage.clone(),
        logger.clone(),
        config,
        now_ms(),
    ));

    // **数据面**那一个：转发热路径。控制面（探测 / 模型测试）由管理面自己建 ——
    // 两面的连接池与超时口径不同，共用一个会让一次慢探测占住转发的连接。
    let fetcher: Arc<dyn Fetcher> = match opts.fetcher {
        Some(f) => f,
        None => default_fetcher(),
    };

    // ── 注册机（可选子系统）──────────────────────────────────────────────
    //
    // 🔴 **装不起来就压根不构造它**，转发路径一个字都感知不到。
    // 装载器零 panic：它只产出 blocker，启不启动由这里 gate。
    let registrar = if opts.build_registrar {
        // 🔴 **复用装配期已经读出来的那一份，绝不再读一次 `config` 键。**
        //
        // 我第一版在这里又 `get("config")` 了一次 —— `assembly_without_probe_writes_nothing`
        // 那格当场红（它钉着「只该读一次」）。装配期每多一次读都是真实的 IO，
        // 而这一份**就在手边**。
        let stored_registrar = stored
            .as_ref()
            .and_then(|v| v.get("registrar").cloned())
            .unwrap_or(serde_json::Value::Null);
        let load = crate::registrar::runtime::load(env, &stored_registrar);
        // 装载期的 blocker / notice / warning **都要说出去**：
        // blocker 说「本次没跑起来」，warning 说「跑得起来但这么配有代价」。
        for b in &load.blockers {
            logger.log(crate::ports::logger::LogEntry::new(
                crate::ports::logger::LogLevel::Warn,
                "registrar.blocked",
                b.msg.clone(),
            ));
        }
        for w in &load.warnings {
            logger.log(crate::ports::logger::LogEntry::new(
                crate::ports::logger::LogLevel::Warn,
                w.event,
                w.msg.clone(),
            ));
        }
        // ⚠️ **注册机整个关着时也构造这个持有者**：面板要能读出「它是关着的」——
        // 返回 `None` 会让那三条端点回 `503 not_wired`，
        // 而 `not_wired` 的含义是「这个部署没装注册机」，与「装了但关着」是两回事。
        Some(Arc::new(crate::registrar::runtime::RegistrarRuntime {
            config: load.config,
            blockers: load.blockers,
            notices: load.notices,
            warnings: load.warnings,
            storage: Arc::clone(&storage),
            logger: Arc::clone(&logger),
            fetcher: Arc::clone(&fetcher),
            // 每一轮重读配置要用它。**只快照 env，存储那一份每轮现读。**
            env: env.clone(),
        }))
    } else {
        None
    };

    let state = AppState {
        storage,
        health,
        logger,
        event_sink,
        config,
        api_keys,
        repo,
        fetcher,
        key_cursor: Arc::new(AtomicUsize::new(0)),
        sticky_key: Arc::new(Mutex::new(None)),
        lb_mode,
        admin_token,
        trust_proxy,
        usage_sink,
        registrar,
        tend_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    // 🔴 **定时轮起在这里，而且不看注册机开没开。**
    //
    // 从前这里还有一道 `&& rt.runnable()`，它同时踩了两个坑，见 `spawn_scheduler`
    // 头上那两条：① 关着的部署**永远不对账**（而索引残留不挑注册机开没开）；
    // ② 启动时关着就再也打不开、启动时开着就再也关不掉 —— 面板是配置的编辑器，
    // 而编辑器写完没有任何人再读。
    //
    // ⚠️ 它绝不许把进程带走 —— `config-degrade` 那条事故判据（真机 74% 请求 500）
    // 钉的正是**注册机出问题时网关照常活着**。
    //
    // 🔴 **Workers 形态没有 tokio 定时轮**：`spawn_scheduler` 只在非 wasm 上存在。
    //    Workers 上的索引对账由 `#[event(scheduled)]` 的 Cron Trigger 单独承担，
    //    且那条 handler **只做对账、不补池**（见 `worker::scheduled`）。
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(rt) = state.registrar.clone() {
        crate::registrar::runtime::spawn_scheduler(rt, Arc::clone(&state.repo));
    }

    Ok(BuiltApp {
        router: build_router(state.clone()),
        state,
    })
}

/// 数据面默认出站客户端（装配不传 `fetcher` 时）。
///
/// 两种形态各自填自己的那一个：原生 `reqwest`，Workers `worker::Fetch`。
/// 之所以抽成一个函数而不是在 `build_app` 里写 cfg ——
/// `build_app` 的装配顺序注释已经按行号对齐 TS 版 12 步，塞一条 cfg 会打乱那个对齐。
#[cfg(not(target_arch = "wasm32"))]
fn default_fetcher() -> Arc<dyn Fetcher> {
    Arc::new(ReqwestFetcher::data_plane())
}

/// Workers 形态默认出站客户端：`worker::Fetch`（CF 原生 fetch API）。
#[cfg(all(target_arch = "wasm32", feature = "worker"))]
fn default_fetcher() -> Arc<dyn Fetcher> {
    Arc::new(crate::adapters::fetcher_cloudflare::CfFetcher::data_plane())
}

/// 8 位十六进制的进程标识。🔴 **CSPRNG**：两个副本撞上同一个分片槽会互相覆写事件。
fn random_shard_id() -> String {
    let mut buf = [0u8; 4];
    // 取不到熵时**不 panic**：shard id 只影响事件分片，退化成一个固定值的后果
    // 是「多副本的事件挤在同一个槽里」，远比起不来轻。
    if getrandom::getrandom(&mut buf).is_err() {
        return "00000000".to_string();
    }
    hex::encode(buf)
}

/// `APIKEY_CACHE_TTL_MS`：**空串 == 没设**；`0` 合法（关缓存，逃生口）；
/// 非法值**装配期抛**（而不是悄悄回落默认值 —— 那会让运维以为自己调过了）。
fn apikey_cache_ttl_ms(env: &EnvMap) -> Result<i64, ConfigRefusal> {
    let Some(raw) = env.get("APIKEY_CACHE_TTL_MS") else {
        return Ok(APIKEY_CACHE_TTL_MS);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(APIKEY_CACHE_TTL_MS);
    }
    match raw.parse::<i64>() {
        Ok(v) if v >= 0 => Ok(v),
        _ => Err(ConfigRefusal::new(format!(
            "APIKEY_CACHE_TTL_MS 必须是一个不小于 0 的整数（毫秒），收到的是：{raw}"
        ))),
    }
}

/// `LOAD_BALANCING_MODE`。**默认 `priority`。**
///
/// ⚠️ 切默认值（TS 版是游标轮询）是一次**行为变更**，要写进 CHANGELOG 与五语言文档。
/// 依据：粘滞选号解决的是「几百个身份在同一个 IP 上交替刷屏」这个形状，
/// 而 agnes 的 key 全是注册机铸出来的新号 —— **这个形状在这里比在 kiro 侧更成立**。
///
/// 值不认得时**回落默认并留一条事件**（不抛）：它不是安全相关的开关，
/// 让网关因为一个拼错的模式名起不来是不划算的。
fn lb_mode(env: &EnvMap, logger: &dyn Logger) -> LbMode {
    let Some(raw) = env.get("LOAD_BALANCING_MODE") else {
        return LbMode::Priority;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return LbMode::Priority;
    }
    LbMode::parse(raw).unwrap_or_else(|| {
        logger.log(
            LogEntry::new(
                LogLevel::Warn,
                "config.invalid",
                "LOAD_BALANCING_MODE 不认得，本次回落到默认值",
            )
            .with("field", "loadBalancingMode")
            .with("raw", raw.to_string())
            .with("fallback", LbMode::Priority.as_str()),
        );
        LbMode::Priority
    })
}

/// `ADMIN_TOKEN` 的装配期三条规则。
///
/// 🔴 **空串必须落进「未配置」分支** —— 历史上出过一次实际的鉴权绕过，
/// 成因就是空串在 `??` 下不下坠。
///
/// 🔴 不合规 ⇒ **整棵 `/admin` 树不注册（404）**，而不是「起不来」：
/// 一个配错了后台口令的部署，它的**网关功能**仍然完全可用。
fn admin_token(env: &EnvMap, logger: &dyn Logger) -> Option<Arc<String>> {
    let raw = env.get("ADMIN_TOKEN")?;
    if raw.is_empty() {
        return None;
    }
    match validate_admin_token(raw) {
        Ok(()) => Some(Arc::new(raw.clone())),
        Err(p) => {
            logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    "config.admin_token_rejected",
                    "ADMIN_TOKEN 不合规，本次启动整个管理接口不注册（网关转发不受影响）",
                )
                // 🔴 只记原因，**绝不记口令本身**。
                .with("reason", p.reason()),
            );
            None
        }
    }
}

/// 「真值」的口径。⚠️ **空串不算设**。
fn truthy(v: Option<&String>) -> bool {
    matches!(
        v.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// 读存储里那份面板配置。
///
/// 🔴 **冷启动这条路径「读不出来 ⇒ 降级」，不是「起不来」。**
/// 与「存储非法值字段级降级」同一条原则：存储**读不出来**比存储值非法更轻，
/// 不该得到更重的处置。而且这条路径上的口令通常就在环境变量里，
/// 存储那份只是覆盖层 —— 读不出来不该让整个网关起不来。
///
/// ⚠️ **热路径（[`ConfigHolder::ensure_fresh`]）要的是相反的行为。**
/// 那条路径上有**严格更好**的兜底 —— 失败时原样保留上一份合法快照。
/// ⇒ **不许复用这个函数**，它是冷启动专用的。
async fn read_stored_config(
    storage: &dyn Storage,
    logger: &dyn Logger,
) -> (Option<serde_json::Value>, bool) {
    match storage.get(CONFIG_KEY).await {
        Ok(v) => (v, false),
        Err(e) => {
            logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    EVENT_CONFIG_STORAGE_UNREADABLE,
                    "读取存储中的 config 键失败，本次装载只用环境变量与内置默认值（面板里保存的配置本次不生效）",
                )
                .with("err", e.to_string()),
            );
            (None, true)
        }
    }
}

/// 把装载期攒下的通知真的打出去。
///
/// 🔴 **这一步漏掉 = 面板报了降级而日志里查不到原因。**
fn emit_notices(logger: &dyn Logger, flags: &LoadFlags) {
    for n in &flags.notices {
        let mut e = LogEntry::new(
            LogLevel::Warn,
            n.event,
            match n.event {
                EVENT_CONFIG_INVALID => "存储中的配置值非法，本字段回落到默认值",
                _ => "配置装载降级",
            },
        );
        if let Some(f) = &n.field {
            e = e.with("field", f.clone());
        }
        if let Some(s) = n.source {
            e = e.with("source", s);
        }
        if let Some(r) = &n.raw {
            e = e.with("raw", r.clone());
        }
        if let Some(fb) = &n.fallback {
            e = e.with("fallback", fb.clone());
        }
        logger.log(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::ports::logger::NullLogger;
    use parking_lot::Mutex as PlMutex;
    use serde_json::json;

    #[derive(Default)]
    struct RecordingLogger(PlMutex<Vec<LogEntry>>);

    impl Logger for RecordingLogger {
        fn log(&self, entry: LogEntry) {
            self.0.lock().push(entry);
        }
    }

    impl RecordingLogger {
        fn events(&self) -> Vec<&'static str> {
            self.0.lock().iter().map(|e| e.event).collect()
        }
    }

    fn env(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn token_env() -> EnvMap {
        env(&[("GATEWAY_TOKEN", "t-abcdefghijklmnop")])
    }

    async fn build(
        e: &EnvMap,
        storage: Arc<dyn Storage>,
        logger: Arc<dyn Logger>,
    ) -> Result<BuiltApp, ConfigRefusal> {
        build_app(e, storage, BuildOptions::minimal(), logger).await
    }

    /// 🔴 **缺 `GATEWAY_TOKEN` ⇒ 装配失败**（fail-closed 是不变量）。
    #[tokio::test]
    async fn assembly_refuses_without_a_gateway_token() {
        let err = build(
            &EnvMap::new(),
            Arc::new(MemoryStorage::new()),
            Arc::new(NullLogger),
        )
        .await
        .err()
        .expect("缺 GATEWAY_TOKEN 必须装不起来");
        assert_eq!(err.to_string(), "缺少 GATEWAY_TOKEN，网关无法启动");
    }

    /// 存储里保存过的配置在装配时被读进来。
    #[tokio::test]
    async fn stored_config_is_read_at_assembly() {
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(CONFIG_KEY, json!({ "maxStrikes": 9 }), None)
            .await
            .unwrap();
        let app = build(&token_env(), storage, Arc::new(NullLogger))
            .await
            .unwrap();
        assert_eq!(app.state.config.current().max_strikes, 9);
    }

    /// 🔴 存储**读不出来**只降级，网关照常起来，并且**留下一条事件**。
    #[tokio::test]
    async fn unreadable_storage_degrades_and_leaves_a_trail() {
        let storage = Arc::new(MemoryStorage::new());
        storage.fail_next("EACCES");
        let logger = Arc::new(RecordingLogger::default());
        let app = build(&token_env(), storage, logger.clone())
            .await
            .expect("读不出来不该让网关起不来");
        assert_eq!(app.state.config.current().max_strikes, 3);
        assert!(
            logger.events().contains(&"config.storage_unreadable"),
            "降级了却没留下任何线索，运维无从排障"
        );
    }

    /// 🔴 存储里的字段非法 ⇒ 降级 + **必须打出 `config.invalid`**。
    #[tokio::test]
    async fn field_level_degrade_emits_config_invalid() {
        let storage = Arc::new(MemoryStorage::new());
        storage
            .put(CONFIG_KEY, json!({ "maxStrikes": "九" }), None)
            .await
            .unwrap();
        let logger = Arc::new(RecordingLogger::default());
        let app = build(&token_env(), storage, logger.clone()).await.unwrap();
        assert_eq!(app.state.config.current().max_strikes, 3);

        let recorded = logger.0.lock();
        let e = recorded
            .iter()
            .find(|e| e.event == "config.invalid")
            .expect("字段级降级必须打一条 config.invalid");
        assert_eq!(e.fields["field"], json!("maxStrikes"));
        assert_eq!(e.fields["fallback"], json!("3"));
    }

    /// 🔴 **`probe_storage: false` 时装配路径一次写都没有。**
    ///
    /// 四个桶全数：只数 `get` 的版本看不见探测那一次 `put`。
    #[tokio::test]
    async fn assembly_without_probe_writes_nothing() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        build(&token_env(), counting, Arc::new(NullLogger))
            .await
            .unwrap();
        let (get, put, delete, list) = counts.snapshot();
        assert_eq!(
            (put, delete, list),
            (0, 0, 0),
            "不探测就不该有任何写或 list"
        );
        assert_eq!(get, 1, "只该读一次 config 键");
    }

    /// 开了探测就**恰好**一次 put + 一次 delete（写进去、不留痕迹）。
    #[tokio::test]
    async fn probing_writes_exactly_one_probe_key_and_cleans_it_up() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        build_app(
            &token_env(),
            counting,
            BuildOptions {
                probe_storage: true,
                ..BuildOptions::minimal()
            },
            Arc::new(NullLogger),
        )
        .await
        .unwrap();
        let (_, put, delete, _) = counts.snapshot();
        assert_eq!((put, delete), (1, 1));
    }

    /// 探测失败 ⇒ **网关照常起来**，`/health` 转 degraded，并留下一条事件。
    #[tokio::test]
    async fn a_failed_probe_never_blocks_startup() {
        let storage = Arc::new(MemoryStorage::new());
        storage.fail_next("EACCES: permission denied");
        let logger = Arc::new(RecordingLogger::default());
        let app = build_app(
            &token_env(),
            storage,
            BuildOptions {
                probe_storage: true,
                ..BuildOptions::minimal()
            },
            logger.clone(),
        )
        .await
        .expect("写不进也要起得来，否则只能看见重启循环");
        assert!(!app.state.health.status().writable);
        assert!(logger.events().contains(&"storage.probe_failed"));
    }

    /// 🔴🔴 **注册机没接线，网关照常活着。**
    ///
    /// 回归对象：真机上那次 74% 请求 500 的事故。**这必须是一条断言，不是一句注释。**
    #[tokio::test]
    async fn the_gateway_lives_even_though_the_registrar_is_not_wired() {
        let app = build(
            &token_env(),
            Arc::new(MemoryStorage::new()),
            Arc::new(NullLogger),
        )
        .await
        .unwrap();
        // 🔴 **这一格换了钉的对象，因为行为真的变了。**
        //
        // P1/P2 时它钉的是「注册机恒没接线」。P3 接线之后，
        // 一份**没打开注册机**的环境里它**照样构造持有者** ——
        // 面板要能读出「它是关着的」，而回 `None` 会让那三条端点
        // 报 `not_wired`（含义是「这个部署没装注册机」），与「装了但关着」是两回事。
        //
        // ⇒ 今天该钉的是：**持有者在、但跑不起来**，而且网关照常活着。
        assert!(
            app.state.registrar_wired(),
            "没打开注册机时也该有持有者 —— 面板要能读出「它是关着的」",
        );
        let rt = app.state.registrar.clone().unwrap();
        assert!(!rt.config.enabled, "没设 REGISTRAR_ENABLED 却认成打开了");
        assert!(
            !rt.runnable(),
            "🔴 关着的注册机不许被判成跑得起来 —— 定时轮会据此起来",
        );
        // 路由树照常装得起来，`/health` 照常是 200。
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;
        let res = app
            .router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    /// `ADMIN_TOKEN` 不合规 ⇒ 管理树不注册，**但网关本身照常起来**，并留下原因
    /// —— 🔴 而原因里**没有口令本身**。
    #[tokio::test]
    async fn a_bad_admin_token_disables_the_admin_tree_without_killing_the_gateway() {
        let mut e = token_env();
        // 口令本身取一个**不会出现在任何 reason 串里**的形状，
        // 这样「日志里有没有口令」才是可判的（`too_short` 里就含 `short`）。
        e.insert("ADMIN_TOKEN".into(), "zq7".into());
        let logger = Arc::new(RecordingLogger::default());
        let app = build(&e, Arc::new(MemoryStorage::new()), logger.clone())
            .await
            .unwrap();
        assert!(app.state.admin_token.is_none());
        let recorded = logger.0.lock();
        let entry = recorded
            .iter()
            .find(|x| x.event == "config.admin_token_rejected")
            .expect("必须留下原因");
        assert_eq!(entry.fields["reason"], json!("too_short"));
        assert!(
            !format!("{:?}", entry.fields).contains("zq7"),
            "🔴 口令本身绝不许进日志"
        );
    }

    /// 🔴 **空串 `ADMIN_TOKEN` 落进「未配置」分支。**
    ///
    /// 回归对象：历史上出过一次实际的鉴权绕过，成因就是空串不下坠。
    #[tokio::test]
    async fn an_empty_admin_token_counts_as_unset() {
        let mut e = token_env();
        e.insert("ADMIN_TOKEN".into(), String::new());
        let app = build(&e, Arc::new(MemoryStorage::new()), Arc::new(NullLogger))
            .await
            .unwrap();
        assert!(app.state.admin_token.is_none());
    }

    /// 合规的 `ADMIN_TOKEN` 被收下。
    #[tokio::test]
    async fn a_valid_admin_token_is_accepted() {
        let mut e = token_env();
        e.insert("ADMIN_TOKEN".into(), "admin-token-0123456789abcdef".into());
        let app = build(&e, Arc::new(MemoryStorage::new()), Arc::new(NullLogger))
            .await
            .unwrap();
        assert_eq!(
            app.state.admin_token.as_deref().map(String::as_str),
            Some("admin-token-0123456789abcdef")
        );
    }

    /// 默认负载均衡模式是 **`priority`**（⚠️ 相对 TS 版是一次行为变更）。
    #[tokio::test]
    async fn the_default_load_balancing_mode_is_priority() {
        let app = build(
            &token_env(),
            Arc::new(MemoryStorage::new()),
            Arc::new(NullLogger),
        )
        .await
        .unwrap();
        assert_eq!(app.state.lb_mode, LbMode::Priority);
    }

    /// 拼错的模式名 ⇒ 回落默认 + 一条事件，**不让网关起不来**。
    #[tokio::test]
    async fn a_misspelled_lb_mode_falls_back_loudly() {
        let mut e = token_env();
        e.insert("LOAD_BALANCING_MODE".into(), "roundrobin".into());
        let logger = Arc::new(RecordingLogger::default());
        let app = build(&e, Arc::new(MemoryStorage::new()), logger.clone())
            .await
            .unwrap();
        assert_eq!(app.state.lb_mode, LbMode::Priority);
        assert!(logger.events().contains(&"config.invalid"));
    }

    /// `APIKEY_CACHE_TTL_MS`：**空串 == 没设**，`0` 合法，非法值**装配期抛**。
    #[test]
    fn the_apikey_cache_ttl_env_has_three_documented_behaviours() {
        assert_eq!(apikey_cache_ttl_ms(&EnvMap::new()), Ok(APIKEY_CACHE_TTL_MS));
        assert_eq!(
            apikey_cache_ttl_ms(&env(&[("APIKEY_CACHE_TTL_MS", "  ")])),
            Ok(APIKEY_CACHE_TTL_MS)
        );
        assert_eq!(
            apikey_cache_ttl_ms(&env(&[("APIKEY_CACHE_TTL_MS", "0")])),
            Ok(0)
        );
        assert_eq!(
            apikey_cache_ttl_ms(&env(&[("APIKEY_CACHE_TTL_MS", "1000")])),
            Ok(1000)
        );
        // 非法值**不许悄悄回落默认** —— 那会让运维以为自己调过了。
        assert!(apikey_cache_ttl_ms(&env(&[("APIKEY_CACHE_TTL_MS", "abc")])).is_err());
        assert!(apikey_cache_ttl_ms(&env(&[("APIKEY_CACHE_TTL_MS", "-1")])).is_err());
    }

    /// `TRUST_PROXY` 的口径：**空串不算设**。
    #[test]
    fn trust_proxy_is_off_unless_explicitly_truthy() {
        assert!(!truthy(None));
        assert!(!truthy(Some(&String::new())));
        assert!(!truthy(Some(&"0".to_string())));
        assert!(!truthy(Some(&"no".to_string())));
        assert!(truthy(Some(&"1".to_string())));
        assert!(truthy(Some(&"true".to_string())));
        assert!(truthy(Some(&"TRUE".to_string())));
    }

    /// 🔴 `shard_id` 是 CSPRNG 出来的 8 位十六进制，两次不同。
    ///
    /// 回归对象：两个副本撞上同一个分片槽会互相覆写事件。
    #[test]
    fn the_shard_id_is_random_hex() {
        let a = random_shard_id();
        assert_eq!(a.len(), 8);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, random_shard_id());
    }

    /// 对外密钥持有者**无条件建**（与用量 sink 刻意不同）。
    #[tokio::test]
    async fn the_api_key_holder_is_always_built_while_the_usage_sink_is_not() {
        let app = build(
            &token_env(),
            Arc::new(MemoryStorage::new()),
            Arc::new(NullLogger),
        )
        .await
        .unwrap();
        assert!(app.state.api_keys.is_some(), "子密钥持有者必须无条件建");
        assert!(app.state.usage_sink.is_none(), "P1 的用量 sink 恒为 None");
    }
}
