//! `GET /admin/api/overview` —— 概览页的唯一取数端点（02 §9.2）。
//!
//! # 🔴 **逐块降级：某块失败就该块返回 `null`，绝不伪造 0**
//!
//! 这是产品不变式，不是风格偏好。一个「读不出来」被画成 `0` 之后，
//! 面板上「池子里有 0 把 key」与「这次没读到池子」长得一模一样，
//! 而两者的处置方向正好相反（去补 key vs 去查存储）。前端见 `null` 渲染 `—`。
//!
//! 包降级的恰好是**三处**：`repo.all()`、进程指标、以及从记录派生的 `poolStats`。
//!
//! # ⚠️ **`config` 块刻意不包降级**（但它的 `degraded` 那一格是另一回事）
//!
//! [`crate::server::holders::config_holder::ConfigHolder::current`] 的契约逐字是
//! 「同步读、**永不抛**」—— 它只读内存里最近一次成功装载的快照，装载失败的处理
//! 发生在持有者内部（保留上一份合法快照 + 打一条 error）。包一层降级会让
//! `config` 看起来像是在参与逐块降级、有一个「读失败 ⇒ null」的分支，
//! 而那个分支**永远走不到** —— 一格永远绿的假判据比没有判据更坏。
//!
//! 🔴 **`config.degraded` 不来自那份快照**：降级是**装载的产物**（`LoadFlags`），
//! 而持有者把它丢了 ⇒ 这条端点自己再装一次，见 [`config_degraded`]。
//! 「块本身永不为 `null`」与「块里那一格如实报降级」是两件事，别读成一件。
//!
//! # ⚠️ 与五语言 API.md 那份响应样例的**一处出入，以实现为准**
//!
//! 样例的 `config` 块只有 5 格，实现有 6 格 —— 多的是 **`registrarBlocked`**
//! （「开着、但这份配置本次没装起来」）。面板**必须 `registrarEnabled &&
//! registrarBlocked` 一起判**：关着的注册机该说「未启用」，不是「没跑起来」。
//! 不给这一格的话，概览卡片会照旧声称有一个在工作的注册机，而补池其实一轮都没跑。
//!
//! # 🟡 一条已登记的文案偏差（归面板那一面）
//!
//! `process` 为 `null` 今天只剩**一个**成因：取进程指标出错。面板仍把它渲染成
//! 「Serverless · 无常驻进程」，**那句文案现在偏了**（Worker 形态没了）。

use crate::core::keypool::pool_health;
use crate::core::stats::sum_stats;
use crate::server::holders::config_holder::CONFIG_TTL_MS;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// 注册用的子树内路径。🔴 `app.rs` 的 `.route()` 从这里取，不许手抄字面量。
pub const OVERVIEW_SUBPATH: &str = "/overview";

// ── 进程指标 ────────────────────────────────────────────────────────────────

/// 常驻进程的 PID / 常驻内存 / 已运行时长。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProcessMetrics {
    pub pid: u32,
    #[serde(rename = "rssBytes")]
    pub rss_bytes: u64,
    #[serde(rename = "uptimeMs")]
    pub uptime_ms: i64,
}

/// 取一次进程指标。**取不到就是 `None`**（调用方把它渲染成 `null`）。
///
/// ⚠️ **本该住在一个 `ports::runtime` 端口后面**（全仓的分层约定是「全局与 IO 只许
/// 出现在 `adapters`」），而那个端口本轮不存在、也不归本面新建。⇒ 暂住在这里，
/// 与 [`crate::admin::keys`] 文件头那三件「本该住在别处」的东西同一条登记方式。
///
/// ⚠️ **`/proc` 只有 Linux 有。** 读不到一律 `None` —— 那正是「取进程指标出错」
/// 这一档该有的表达，🔴 **不许编一个 0 出来**（0 字节常驻内存是一句当场可证伪的假话）。
///
/// 🔴 **`wasm32`（Cloudflare Workers）恒 `None`**：Workers 没有进程、没有 PID、
/// 没有 `/proc`，而 `std::process::id()` 在那里**会 panic**
///（`sys/process/unsupported.rs: no pids on this platform`，本地 Miniflare 实测
/// 直接把 `/admin/api/overview` 打成 500 并终结 isolate）。所以整支按目标裁掉，
/// 面板那一格如实显示 `null` —— 与本函数「取不到就是 null」的既定语义一致。
#[must_use]
#[cfg(not(target_arch = "wasm32"))]
pub fn process_metrics() -> Option<ProcessMetrics> {
    let pid = std::process::id();
    let rss_bytes = rss_bytes()?;
    let uptime_ms = uptime_ms()?;
    Some(ProcessMetrics {
        pid,
        rss_bytes,
        uptime_ms,
    })
}

/// Workers 形态：没有进程这回事，**如实返回 `None`**（见上一个同名函数的说明）。
#[must_use]
#[cfg(target_arch = "wasm32")]
pub fn process_metrics() -> Option<ProcessMetrics> {
    None
}

/// 常驻内存。`/proc/self/statm` 的第二个数是**页数**，要乘页大小。
#[cfg(not(target_arch = "wasm32"))]
fn rss_bytes() -> Option<u64> {
    let raw = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = raw.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages.saturating_mul(page_size()))
}

/// 页大小。
///
/// ⚠️ **不写死 4096**：aarch64 上 16 KiB 页是常态，写死会让报出去的常驻内存
/// 差四倍，而那个数字面板是直接画出来的。
///
/// `std` 没有暴露 `sysconf`，而 `libc` 不是本仓的依赖（为一个常数加一条依赖不划算）。
/// ⇒ 走内核自己导出的那份：`/proc/self/auxv` 里的 `AT_PAGESZ`。
/// 读 `/proc` 这一路本来就限定 Linux，与上面 `rss_bytes` 同一个前提，不新增假设。
///
/// ⚠️ 拿不到就用 4096：**宁可报小，也不报一个编出来的大数**。
#[cfg(not(target_arch = "wasm32"))]
fn page_size() -> u64 {
    std::fs::read("/proc/self/auxv")
        .ok()
        .and_then(|buf| parse_auxv_pagesize(&buf))
        .and_then(|n| u64::try_from(n).ok())
        .filter(|n| *n > 0)
        .unwrap_or(4096)
}

/// 从 `/proc/self/auxv` 里取 `AT_PAGESZ`（键 6）。
///
/// 结构是一串 `(usize, usize)` 对，以 `(0, 0)` 收尾。
#[cfg(not(target_arch = "wasm32"))]
fn parse_auxv_pagesize(buf: &[u8]) -> Option<i64> {
    const AT_PAGESZ: u64 = 6;
    let w = std::mem::size_of::<usize>();
    let mut i = 0;
    while i + 2 * w <= buf.len() {
        let key = read_usize(&buf[i..i + w])?;
        let val = read_usize(&buf[i + w..i + 2 * w])?;
        if key == 0 {
            return None;
        }
        if key == AT_PAGESZ {
            return i64::try_from(val).ok();
        }
        i += 2 * w;
    }
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn read_usize(b: &[u8]) -> Option<u64> {
    match b.len() {
        8 => Some(u64::from_ne_bytes(b.try_into().ok()?)),
        4 => Some(u64::from(u32::from_ne_bytes(b.try_into().ok()?))),
        _ => None,
    }
}

/// 本进程已经跑了多久。
///
/// `/proc/self/stat` 的第 22 个字段是进程启动时刻（单位：时钟滴答，自开机起算），
/// `/proc/uptime` 的第一个数是开机至今的秒数 ⇒ 两者相减就是进程存活时长。
///
/// ⚠️ **不用「第一次调用时记一个 `Instant`」那种写法**：那个数会随「谁先打开概览页」
/// 变化，于是面板上的「已运行」在第一次刷新时恒等于 0 —— 一个看起来合理的假值。
#[cfg(not(target_arch = "wasm32"))]
fn uptime_ms() -> Option<i64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // 可执行文件名可能含空格与右括号，所以从**最后一个** `)` 之后开始切。
    let rest = &stat[stat.rfind(')')? + 1..];
    // 切完之后第 1 个字段是 `state`（原编号 3）⇒ 启动时刻（原编号 22）是第 20 个。
    let ticks: f64 = rest.split_whitespace().nth(19)?.parse().ok()?;
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    let boot_secs: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    // 用户态时钟滴答恒为 100 Hz（`USER_HZ`，内核 ABI 的一部分，与 `CONFIG_HZ` 无关）。
    let started_secs = ticks / 100.0;
    let alive_ms = (boot_secs - started_secs) * 1000.0;
    if alive_ms.is_finite() && alive_ms >= 0.0 {
        Some(alive_ms as i64)
    } else {
        None
    }
}

// ── 线格式 ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Wire {
    version: &'static str,
    #[serde(rename = "serverTime")]
    server_time: i64,
    runtime: RuntimeBlock,
    /// 🔴 取不到就是 `null`，**不伪造 0**。
    process: Option<ProcessMetrics>,
    storage: StorageBlock,
    /// 🔴 读不出来就是 `null`，**不伪造一个空池**。
    pool: Option<crate::core::keypool::PoolHealth>,
    #[serde(rename = "poolStats")]
    pool_stats: Option<PoolStatsBlock>,
    freshness: FreshnessBlock,
    config: ConfigBlock,
}

#[derive(Serialize)]
struct RuntimeBlock {
    name: &'static str,
}

#[derive(Serialize)]
struct StorageBlock {
    backend: &'static str,
    writable: bool,
    #[serde(rename = "checkedAt")]
    checked_at: Option<i64>,
}

/// Tier-1 池级聚合。
///
/// ⚠️ **只挑用得上的四个计数字段，不整段展开 `sum_stats()`** —— 那还带着
/// `last_error_at` / `last_error_kind`，概览不消费它们（错误面正经的归宿是事件板块），
/// 挂两个没人看的字段在响应里迟早会漂。
#[derive(Serialize)]
struct PoolStatsBlock {
    requests: u64,
    success: u64,
    failed: u64,
    #[serde(rename = "clientErrors")]
    client_errors: u64,
    /// **近似值**，面板必须打 `≈`。这一格是那个 `≈` 的唯一驱动源，不许在前端写死。
    approximate: bool,
}

/// 两个 TTL **都要给**：它们回答的不是同一个问题
/// （「别的副本判的冷却/剔除多久能看到」 vs 「面板上刚改完配置多久能看见」）。
///
/// ⚠️ **两条上界就等于各自的 TTL，中间不许再加任何一项。** 上一版各加过一个
/// 「KV 边缘缓存 60 秒」，那一层随 Worker 形态没了 —— 加回来就是对运维多报 60 秒。
#[derive(Serialize)]
struct FreshnessBlock {
    #[serde(rename = "poolCacheTtlMs")]
    pool_cache_ttl_ms: i64,
    #[serde(rename = "poolVisibilityUpperBoundMs")]
    pool_visibility_upper_bound_ms: i64,
    #[serde(rename = "poolTouchIntervalMs")]
    pool_touch_interval_ms: i64,
    #[serde(rename = "configTtlMs")]
    config_ttl_ms: i64,
    #[serde(rename = "configVisibilityUpperBoundMs")]
    config_visibility_upper_bound_ms: i64,
}

#[derive(Serialize)]
struct ConfigBlock {
    #[serde(rename = "registrarEnabled")]
    registrar_enabled: bool,
    /// 「开着、但这份配置本次没装起来」。面板必须与上一格**一起判**。
    #[serde(rename = "registrarBlocked")]
    registrar_blocked: bool,
    channel: Option<&'static str>,
    #[serde(rename = "targetKeys")]
    target_keys: i64,
    #[serde(rename = "envLocked")]
    env_locked: Vec<String>,
    /// 本次装载有没有降级。红色横幅的依据。
    degraded: bool,
}

// ── 注册机那四格的 P1 取值（🔴 别读成「注册机已接线」）───────────────────────

/// 注册机关着时的目标池大小。
///
/// ⚠️ 注册机子树（补池）是后面那一轮的对象，`GatewayConfig` 里**真的没有**这棵树
/// （`core::config` 文件头逐字写着「本轮不建一个半截的形状占位」）。
/// ⇒ 这四格如实报「没开、没被挡、没选通道、目标值是内置默认值」，
/// 而 `registrar_wired` 为假时 `registrar_enabled` 恒为假 ——
/// **面板于是画「未启用」，不画一个在工作的注册机**。
const REGISTRAR_TARGET_KEYS_DEFAULT: i64 = 20;

/// 红色横幅的那一格：**这份配置现在装起来有没有打折扣**。
///
/// 🔴 **必须自己再装一次，不能读 `st.config`**：`ConfigHolder` 把装载产物
/// [`crate::core::config_provenance::LoadFlags`] 整个丢掉了
/// （`ensure_fresh` 里那句 `Ok((cfg, _flags))`），⇒ 进程里没有任何一处能告诉这条
/// 端点「本次装载降级了没有」。在它之前这一格是个 `false` 字面量 ——
/// 手改 `store.json` 打错一个字，网关已经悄悄跑在内置默认值上，而横幅永远点不亮。
///
/// ⚠️ **刻意与 [`crate::admin::config::snapshot_of`] 同一个函数、同一份 env**：
/// 那是 `GET /admin/api/config` 的 `configDegraded` 的来源 ⇒ 两条端点结构上
/// 不可能再互相矛盾（各写一份推导的话，改了一处而没改另一处时不会有任何信号）。
///
/// ⚠️ 代价老实写：面板每刷一次概览就**多一次存储读**（`FileStorage` 上是一次整份
/// 文件读 + 解析）。换来的是那条横幅真的有依据 —— 而设置页那条端点付的是同一笔账。
/// 本该由持有者把 flags 留下来省掉这一次读，`server::holders` 不归本面改，缺口登记在此。
///
/// ⚠️ **读不出来一律算降级**：与启动那一次的 `storage_unreadable` 同一套语义
/// （见 `server::wire` 把它原样喂给 [`crate::core::config::resolve_config`]）。
/// 🔴 这里**不打事件**：概览是被面板轮询的，打了就是按轮询频率往事件环里灌同一条。
async fn config_degraded(st: &AppState) -> bool {
    let Ok(stored) = st
        .storage
        .get(crate::core::config_provenance::CONFIG_KEY)
        .await
    else {
        return true;
    };
    let (snap, _) =
        crate::admin::config::snapshot_of(stored.as_ref(), crate::admin::config::admin_env());
    snap.config_degraded
}

/// `GET /admin/api/overview`。
pub async fn overview(State(st): State<AppState>) -> Response {
    let at = crate::adapters::now_ms();

    // ① 池子。读失败 ⇒ 整块 `null`（连带 `poolStats`）。
    let records = st.repo.all(at).await.ok();

    // ② 配置。**同步读、永不抛** ⇒ 刻意不参与逐块降级。
    let cfg = st.config.current();
    // ②' 降级标记是**装载的产物**，快照里没有它 ⇒ 单独装一次，见 [`config_degraded`]。
    let degraded = config_degraded(&st).await;

    // ③ 存储可写性只读一次：`writable` 与 `checkedAt` 必须来自同一份快照。
    let storage_status = st.health.status();

    Json(Wire {
        version: crate::VERSION,
        server_time: at,
        runtime: RuntimeBlock {
            name: crate::admin::capabilities::RUNTIME_NAME,
        },
        process: process_metrics(),
        storage: StorageBlock {
            backend: crate::admin::capabilities::STORAGE_BACKEND,
            writable: storage_status.writable,
            checked_at: storage_status.checked_at,
        },
        pool: records.as_ref().map(|r| pool_health(r, at)),
        pool_stats: records.as_ref().map(|r| {
            let s = sum_stats(r.iter().map(|r| r.stats.as_ref()));
            PoolStatsBlock {
                requests: s.requests,
                success: s.success,
                failed: s.failed,
                client_errors: s.client_errors,
                approximate: true,
            }
        }),
        freshness: FreshnessBlock {
            pool_cache_ttl_ms: cfg.pool_cache_ttl_ms,
            pool_visibility_upper_bound_ms: cfg.pool_cache_ttl_ms,
            pool_touch_interval_ms: cfg.pool_touch_interval_ms,
            config_ttl_ms: CONFIG_TTL_MS,
            config_visibility_upper_bound_ms: CONFIG_TTL_MS,
        },
        config: ConfigBlock {
            registrar_enabled: st.registrar_wired(),
            registrar_blocked: false,
            channel: None,
            target_keys: REGISTRAR_TARGET_KEYS_DEFAULT,
            env_locked: crate::admin::config::env_locked_fields(crate::admin::config::admin_env()),
            degraded,
        },
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::ports::storage::Storage;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::body::to_bytes;
    use serde_json::Value;
    use std::sync::Arc;

    async fn raw_of(st: AppState) -> String {
        let res = overview(State(st)).await;
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    async fn call(st: AppState) -> Value {
        serde_json::from_str(&raw_of(st).await).unwrap()
    }

    /// 🔴 **本组六条端点真的挂在路由树上，且真的在管理闸后面。**
    ///
    /// 存在的理由：上面每一格判据都是**直接调 handler 函数**的 —— 它们对
    /// 「`app.rs` 里那几行 `.route()` 漏了一条 / 挂错了方法 / 挂在闸外面」
    /// **结构性地看不见**。那正是已登记的假阳性形态第 ① 条
    /// （判据对着一条没有消费者的代码路径写）。
    ///
    /// ⚠️ 观察点是**两条**：带对口令拿到的不是 404（路由在），
    /// 不带口令拿到的是 401（闸在它之前）。少任何一条都测不出这两件事。
    #[tokio::test]
    async fn all_six_endpoints_are_mounted_behind_the_admin_gate() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        const ADMIN: &str = "admin-token-0123456789abcdef";
        let TestApp { mut state, .. } = fresh().await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = crate::server::app::build_router(state);

        // 🔴 路径从常量现算，**不手抄字面量** —— 手抄的那份与注册路径迟早会漂。
        let prefix = crate::admin::keys::ADMIN_API_PREFIX;
        let cap = crate::admin::capabilities::CAPABILITIES_SUBPATH;
        let cfg = crate::admin::config::CONFIG_SUBPATH;
        let val = crate::admin::config::CONFIG_VALIDATE_SUBPATH;
        let clr = crate::admin::config::CONFIG_SECRETS_CLEAR_SUBPATH;
        let rst = crate::admin::config::CONFIG_RESET_SUBPATH;

        for (method, sub, body) in [
            (Method::GET, cap, ""),
            (Method::GET, OVERVIEW_SUBPATH, ""),
            (Method::GET, cfg, ""),
            (Method::PUT, cfg, r#"{"patch":{}}"#),
            (Method::POST, val, r#"{"patch":{}}"#),
            (Method::POST, clr, r#"{"path":"gatewayToken"}"#),
            (Method::POST, rst, r#"{"confirm":true}"#),
        ] {
            let uri = format!("{prefix}{sub}");
            let req = |key: Option<&str>| {
                let mut b = Request::builder()
                    .method(method.clone())
                    .uri(&uri)
                    .header("content-type", "application/json");
                if let Some(k) = key {
                    b = b.header("x-admin-key", k);
                }
                b.body(Body::from(body.to_string())).unwrap()
            };

            let res = app.clone().oneshot(req(Some(ADMIN))).await.unwrap();
            assert_ne!(
                res.status(),
                StatusCode::NOT_FOUND,
                "{method} {uri} 没挂上去（或方法挂错了）"
            );

            let res = app.clone().oneshot(req(None)).await.unwrap();
            assert_eq!(
                res.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} 必须在管理闸后面"
            );
        }
    }

    async fn fresh() -> TestApp {
        state_with_storage(Arc::new(MemoryStorage::new())).await
    }

    /// 顶层键**恰好是这些、而且就是这个顺序**。
    ///
    /// ⚠️ 顺序必须比**原始字节**：`serde_json::Value` 是 `BTreeMap`，
    /// 反序列化回来就把顺序信息抹掉了 —— 对着它比是一格永远绿的假判据。
    fn assert_keys_in_order(raw: &str, expected: &[&str]) {
        let v: Value = serde_json::from_str(raw).unwrap();
        let mut got: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let mut want: Vec<&str> = expected.to_vec();
        got.sort_unstable();
        want.sort_unstable();
        assert_eq!(got, want, "键集不对");

        let mut pos = 0;
        for k in expected {
            let pat = format!("\"{k}\":");
            let at = raw[pos..]
                .find(&pat)
                .unwrap_or_else(|| panic!("{k} 没有出现在它该在的位置之后：{raw}"));
            pos += at + pat.len();
        }
    }

    /// 每一次操作都真抛的存储。
    ///
    /// 🔴 **不能拿 `MemoryStorage::fail_next` 当它用**：key 池的索引读**故意**把
    /// 读错误当成「索引缺失」（那是一条自愈路径），于是只失败一次的替身会让
    /// `repo.all()` 掉进 `list()` 兜底并**成功**返回一个空池 ——
    /// 而这一族判据要区分的恰恰是「空池」与「读不出来」。
    struct BrokenStorage;

    #[async_trait::async_trait]
    impl Storage for BrokenStorage {
        async fn get(&self, _k: &str) -> crate::ports::storage::StorageResult<Option<Value>> {
            Err(crate::ports::storage::StorageError::new("EIO"))
        }
        async fn put(
            &self,
            _k: &str,
            _v: Value,
            _e: Option<i64>,
        ) -> crate::ports::storage::StorageResult<()> {
            Err(crate::ports::storage::StorageError::new("EIO"))
        }
        async fn delete(&self, _k: &str) -> crate::ports::storage::StorageResult<()> {
            Err(crate::ports::storage::StorageError::new("EIO"))
        }
        async fn list(&self, _p: &str) -> crate::ports::storage::StorageResult<Vec<String>> {
            Err(crate::ports::storage::StorageError::new("EIO"))
        }
    }

    /// 顶层九块逐块在，`config` 块的**六格**逐格在。
    ///
    /// ⚠️ 五语言 API.md 的样例里 `config` 只有 5 格 —— 少的那一格是
    /// `registrarBlocked`，而面板必须 `registrarEnabled && registrarBlocked` 一起判。
    #[tokio::test]
    async fn the_config_block_carries_registrar_blocked_which_the_docs_sample_omits() {
        let TestApp { state, .. } = fresh().await;
        let raw = raw_of(state).await;
        assert_keys_in_order(
            &raw,
            &[
                "version",
                "serverTime",
                "runtime",
                "process",
                "storage",
                "pool",
                "poolStats",
                "freshness",
                "config",
            ],
        );
        let v: Value = serde_json::from_str(&raw).unwrap();
        let mut cfg: Vec<&str> = v["config"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        cfg.sort_unstable();
        assert_eq!(
            cfg,
            vec![
                "channel",
                "degraded",
                "envLocked",
                "registrarBlocked",
                "registrarEnabled",
                "targetKeys",
            ],
            "面板必须 registrarEnabled && registrarBlocked 一起判，少一格它就判不了"
        );
    }

    /// 🔴 **池子读不出来 ⇒ `pool` 与 `poolStats` 都是 `null`，绝不伪造 0。**
    ///
    /// 回归对象：把读失败画成 `0` 之后，「池子里有 0 把 key」与「这次没读到池子」
    /// 在面板上长得一模一样，而两者的处置方向正好相反。
    ///
    /// ⚠️ 观察点必须是 `is_null()`，不是 `!= 0` —— 后者会把一个真的空池算过。
    #[tokio::test]
    async fn a_failed_pool_read_becomes_null_not_a_fabricated_zero() {
        // 先确认健康那一支真的给的是对象（否则这一格测不出差别）。
        let TestApp { state, .. } = fresh().await;
        let ok = call(state).await;
        assert!(ok["pool"].is_object(), "健康时 pool 必须是对象：{ok}");
        assert!(ok["poolStats"].is_object());

        let TestApp { state, .. } = state_with_storage(Arc::new(BrokenStorage)).await;
        let v = call(state).await;
        assert!(v["pool"].is_null(), "读失败必须是 null，实际 {}", v["pool"]);
        assert!(v["poolStats"].is_null(), "poolStats 必须跟着 null");
    }

    /// ⚠️ **`config` 块永远不是 `null`，哪怕存储整个坏掉。**
    ///
    /// 它钉的是「`config` 刻意不参与逐块降级」这个决定本身：`ConfigHolder.current()`
    /// 的契约是同步读、永不抛，给它包一层降级会造出一个**永远走不到**的分支。
    ///
    /// ⚠️ 末尾那句原本是 `degraded == false` —— 它**钉错了对象**：那一格当时是个
    /// `false` 字面量，于是这句话钉住的不是「块没塌成 null」，而是那个字面量本身
    /// （存储整个读不出来恰恰是降级最硬的一档，却被要求报「没降级」）。
    /// 🔴 现在钉真实行为：块还在（不是 `null`），而那一格如实报「这次装载降级了」。
    #[tokio::test]
    async fn the_config_block_never_degrades_to_null() {
        let TestApp { state, .. } = state_with_storage(Arc::new(BrokenStorage)).await;
        let v = call(state).await;
        assert!(v["pool"].is_null(), "夹具前提：这一次存储确实坏了");
        assert!(v["config"].is_object(), "config 块不许 null：{v}");
        assert_eq!(
            v["config"]["degraded"], true,
            "读不出来是降级最硬的一档，块还在不等于这一格该说没事"
        );
    }

    /// 🔴 **`config.degraded` 与 `GET /admin/api/config` 的 `configDegraded`
    /// 是同一件事，两条端点不许互相矛盾。**
    ///
    /// 回归对象：这一格曾经是一个 `false` **字面量** ⇒ 进程里没有任何东西能让它变成真。
    /// 实际后果：手改 `store.json` 打错一个字，网关已经悄悄跑在内置默认值上
    /// （超时 / strike 阈值回落），而面板上那条红色横幅**永远点不亮**。
    ///
    /// ⚠️ 观测量是**两条端点各自吐出去的那一格**，不是内部的 `LoadFlags` ——
    /// 比内部标记的话，「handler 把它接反了 / 干脆没接」这一族全看不见。
    /// ⚠️ 两组夹具缺一不可：只有降级那一组时，把这一格写死成 `true` 也照样绿。
    #[tokio::test]
    async fn the_degraded_flag_never_contradicts_the_config_endpoint() {
        // ① 装得起来的一份 ⇒ 两边都说「没降级」。
        let (ov, cfg) = degraded_pair(serde_json::json!({
            "gatewayToken": "stored-gateway-token-0123456789",
        }))
        .await;
        assert!(!cfg, "夹具前提：这一份必须是装得起来的");
        assert_eq!(ov, cfg, "两条端点对同一次装载的判断必须一致");

        // ② 存储里一格非法值 ⇒ 字段级降级（回落默认值），两边都必须说「降级了」。
        //    ⚠️ 挑 `upstreamTimeoutMs` 而不是 `maxStrikes`：判据夹具的 env 里有
        //    `MAX_STRIKES`，env 侧一旦出现就赢 ⇒ 存储里那格坏值根本走不到降级那一支。
        let (ov, cfg) = degraded_pair(serde_json::json!({
            "gatewayToken": "stored-gateway-token-0123456789",
            "upstreamTimeoutMs": "x9",
        }))
        .await;
        assert!(cfg, "夹具前提：这一份必须真的降级了");
        assert_eq!(ov, cfg, "🔴 同一部署的两条端点当场自相矛盾");
    }

    /// 拿同一份存储装一套，取**两条端点**各自报的那一格降级标记。
    async fn degraded_pair(stored: Value) -> (bool, bool) {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(crate::core::config_provenance::CONFIG_KEY, stored, None)
            .await
            .unwrap();
        let TestApp { state, .. } = state_with_storage(mem).await;

        let overview = call(state.clone()).await;
        let res = crate::admin::config::get(State(state)).await;
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let config: Value = serde_json::from_slice(&b).unwrap();
        (
            overview["config"]["degraded"]
                .as_bool()
                .unwrap_or_else(|| panic!("overview 的 config.degraded 必须是布尔：{overview}")),
            config["configDegraded"]
                .as_bool()
                .unwrap_or_else(|| panic!("GET /admin/api/config 没给 configDegraded：{config}")),
        )
    }

    /// 🔴 **两条可见性上界就等于各自的 TTL，中间不许再加任何一项。**
    ///
    /// 回归对象：上一版各加了一个「KV 边缘缓存 60 秒」，而那一层随 Worker 形态没了 ——
    /// 加回来就是面板对运维**多报 60 秒**。期望值手写字面量。
    #[tokio::test]
    async fn the_two_visibility_bounds_equal_their_ttls_with_nothing_added() {
        let TestApp { state, .. } = fresh().await;
        let v = call(state).await;
        let f = &v["freshness"];
        assert_eq!(f["configTtlMs"], 30_000);
        assert_eq!(
            f["configVisibilityUpperBoundMs"], 30_000,
            "上界就是 TTL，不许再加一项"
        );
        assert_eq!(f["poolCacheTtlMs"], 60_000);
        assert_eq!(
            f["poolVisibilityUpperBoundMs"], 60_000,
            "上界就是 TTL，不许再加一项"
        );
        assert_eq!(f["poolTouchIntervalMs"], 21_600_000);
    }

    /// `freshness` 里的池子那两个数**跟着生效配置走**，不是写死的常量。
    #[tokio::test]
    async fn the_pool_bounds_follow_the_effective_config() {
        let mem = Arc::new(MemoryStorage::new());
        mem.put(
            crate::core::config_provenance::CONFIG_KEY,
            serde_json::json!({ "poolCacheTtlMs": 7_000, "poolTouchIntervalMs": 0 }),
            None,
        )
        .await
        .unwrap();
        let TestApp { state, .. } = state_with_storage(mem).await;
        state.config.invalidate();
        state.config.ensure_fresh(i64::MAX).await;

        let v = call(state).await;
        assert_eq!(v["freshness"]["poolCacheTtlMs"], 7_000);
        assert_eq!(v["freshness"]["poolVisibilityUpperBoundMs"], 7_000);
        assert_eq!(v["freshness"]["poolTouchIntervalMs"], 0);
    }

    /// ⚠️ **`poolStats` 只挑那四个计数字段**，`lastErrorAt` / `lastErrorKind` 不许挂上来。
    ///
    /// 没有消费者的响应字段迟早会漂，而概览页的错误面正经的归宿是事件板块。
    #[tokio::test]
    async fn pool_stats_carries_the_four_counters_and_nothing_else() {
        let TestApp { state, .. } = fresh().await;
        state
            .repo
            .add("sk-overview-0123456789abcdef", crate::adapters::now_ms())
            .await
            .unwrap();
        let raw = raw_of(state).await;
        let v: Value = serde_json::from_str(&raw).unwrap();
        let mut keys: Vec<&str> = v["poolStats"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "approximate",
                "clientErrors",
                "failed",
                "requests",
                "success"
            ],
            "lastErrorAt / lastErrorKind 不许挂上来"
        );
        assert_eq!(v["poolStats"]["approximate"], true, "≈ 由这一格驱动");
        assert_eq!(v["pool"]["total"], 1);
        assert_eq!(v["pool"]["fresh"], 1);
    }

    /// 进程指标：Linux 上取得到，且**每一格都不是编出来的 0**。
    ///
    /// ⚠️ 只断言「存在」是不够的 —— 一个恒 0 的 `rssBytes` 会照样通过。
    #[test]
    fn process_metrics_are_real_numbers_not_placeholders() {
        let Some(m) = process_metrics() else {
            // 非 Linux（或没挂 /proc）上取不到 —— 那时它必须是 `None` 而不是一份假数据。
            return;
        };
        assert_eq!(m.pid, std::process::id());
        assert!(m.rss_bytes > 0, "常驻内存不可能是 0 字节");
        assert!(m.uptime_ms >= 0);
        assert!(
            m.rss_bytes < 64 * 1024 * 1024 * 1024,
            "取到的不该是一个离谱的数：{}",
            m.rss_bytes
        );
    }

    /// `auxv` 解析：认得 `AT_PAGESZ`，读到结尾对 `(0,0)` 就收手。
    #[test]
    fn auxv_parsing_finds_the_page_size_and_stops_at_the_terminator() {
        let w = std::mem::size_of::<usize>();
        let mut buf = Vec::new();
        let push = |buf: &mut Vec<u8>, v: u64| {
            if w == 8 {
                buf.extend_from_slice(&v.to_ne_bytes());
            } else {
                buf.extend_from_slice(&(v as u32).to_ne_bytes());
            }
        };
        push(&mut buf, 3); // AT_PHDR，跳过
        push(&mut buf, 0xdead);
        push(&mut buf, 6); // AT_PAGESZ
        push(&mut buf, 16_384);
        assert_eq!(parse_auxv_pagesize(&buf), Some(16_384));

        // 终止对在前 ⇒ 看不到后面那条。
        let mut ended = Vec::new();
        push(&mut ended, 0);
        push(&mut ended, 0);
        push(&mut ended, 6);
        push(&mut ended, 4_096);
        assert_eq!(parse_auxv_pagesize(&ended), None);
    }

    /// `envLocked` 报的是**被环境变量锁住的字段路径**，不是环境变量名。
    ///
    /// 面板拿它给那几格加锁标记；报成 env 名的话面板要么全画错、要么得自己再写一张表。
    #[tokio::test]
    async fn env_locked_lists_field_paths_not_variable_names() {
        let TestApp { state, .. } = fresh().await;
        let v = call(state).await;
        let locked: Vec<String> = serde_json::from_value(v["config"]["envLocked"].clone()).unwrap();
        assert_eq!(
            locked,
            vec!["maxStrikes".to_string()],
            "判据夹具的 env 里有 MAX_STRIKES，它锁的字段是 maxStrikes：{locked:?}"
        );
        assert!(
            !locked.iter().any(|s| s == "MAX_STRIKES"),
            "报的必须是字段路径而不是环境变量名：{locked:?}"
        );
    }
}
