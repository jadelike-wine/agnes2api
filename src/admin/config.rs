//! `/admin/api/config*` —— **配置的五条管理端点**（02 §9.18 ~ §9.22）。
//!
//! | 方法 | 路径 | 干什么 | 写存储吗 |
//! |---|---|---|---|
//! | GET | `/admin/api/config` | 逐字段四元组 + 凭据的 `{configured,hint}` | **否** |
//! | PUT | `/admin/api/config` | 校验 → 写 → `invalidate()` → **回读** | 是（成功时 1 次 put） |
//! | POST | `/admin/api/config/validate` | 干跑，只回错误 | **否，一次都不写** |
//! | POST | `/admin/api/config/secrets/clear` | 显式清空一把凭据 | 是 |
//! | POST | `/admin/api/config/reset` | 危险区：`config` 整把写回 `{}` | 是 |
//!
//! # 贯穿本文件的四条纪律，每一条都对应一次已经发生过的失效
//!
//! ① 🔴 **来源推导只有一份。** 「谁压过谁」全在
//!    [`crate::core::config::resolve_config`] 里。本文件**不做任何优先级判断**：
//!    四元组里的 `effective` 是**拿那个函数真跑一次**取回来的，不是照着
//!    「有 env 就显示 env，否则显示存储」再推一遍。
//!    那条推法在一个具体分支上会说假话 —— 存储值非法时网关用的是**默认值**，
//!    而推出来的那份会把存储里那个非法值显示成「生效值」。
//!    **两份实现之间的差，恰好落在最需要面板说实话的那一格上。**
//!
//! ② 🔴 **凭据只写不读。** `GET` 永不返回明文；`PUT` 时凭据字段**缺席或空串 = 不改**
//!    （不是清空）。把空串实现成清空的后果是：运维保存一次设置页就抹掉
//!    `gatewayToken`，而热实例因为持有者保留上一份快照**当场看不出任何异常**，
//!    直到下一次重启才整个停摆。清空只有 `secrets/clear` 一条路。
//!
//! ③ 🔴 **校验是纯函数、跑在写之前，失败一个字节都不写。** 先写后校验的话，
//!    一份非法配置已经落盘、而响应是 400 —— 运维照着 400 以为「没保存上」，
//!    下一次冷启动网关起不来。判据数的是 **put 次数**，只断言 400 抓不住这个形态。
//!
//! ④ 🔴 **回执从存储回读，不是把 handler 自己算出来的那份 `next` 换个形状交回去。**
//!    这是最容易写错、而且写错了响应体照样漂亮的一行：`fields_of(next)` 证明的只是
//!    「handler 说它写了什么」，要证明的是「存储里现在是什么」。
//!    ⚠️ 它与 `config.invalidate()` 是**两件事**：回读证明「落盘了」，
//!    `invalidate()` 保证「**同一个进程的下一个请求**看到的也是新值」——
//!    少了后者，`GET /admin/api/overview` 会在最多一个 `CONFIG_TTL_MS` 内继续报旧值，
//!    而运维刚刚才看到保存回执上是新值。
//!
//! # ⚠️ 本文件里三件「本该住在别处」的东西，如实登记
//!
//! 1. **字段表 / 校验 / 装载阻断判据**（[`FIELDS`]、[`validate_config_patch`]、
//!    [`config_load_blockers`]）本该住在 `core::admin::config_validate`（零 IO 的纯函数）。
//!    那个文件本轮不存在，而 `src/core/` 不归本面改 ⇒ 暂住在这里，
//!    **写法照 `core` 的规矩**（不读时钟、不碰存储、`env` 当数据收）。
//!    将来整体搬过去时，判据不必改：它们钉的是行为，不是谁实现的。
//! 2. **环境变量快照**（[`admin_env`]）。全仓唯一读 `std::env` 的地方本来是 `cli.rs`，
//!    而 `AppState` 里**没有** env 这一格（装配时它只进了 `ConfigHolder` 的私有字段）。
//!    ⇒ 这里留一个**装配期注入点** [`install_env_snapshot`]，
//!    没人注入时才退回读一次进程环境。**那是登记在案的偏离，不是没看见**，
//!    接线归装配那一面（见交付说明里的缺口条目）。
//! 3. `admin.storage_failed` 这条事件名沿用 [`crate::admin::keys`] 那一份
//!    （**同一个 grep 锚点，不另起第二个**）。
//!
//! # ⚠️ `propagation` 就是 `{configTtlMs, visibilityUpperBoundMs}`，**别再加第三项**
//!
//! 上一版这里还加了一个「KV 边缘缓存 60 秒」（30s + 60s = 90s），
//! 那一层随 Worker 形态一起没了 ⇒ 保存回执里那个数会比事实**多报 60 秒**。
//! 🔴 整层删干净了，别再把它加回来。

use crate::core::config::resolve_config;
use crate::core::config_errors::{ConfigError, ConfigErrorCode};
use crate::core::config_provenance::{CONFIG_KEY, EnvMap};
use crate::ports::logger::{LogEntry, LogLevel};
use crate::server::errors::{AdminError, GatewayError};
use crate::server::holders::config_holder::CONFIG_TTL_MS;
use crate::server::state::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;

// ── 1. 路径常量（🔴 真源，`app.rs` 与五语言文档都从这里取）────────────────────

/// 注册用的子树内路径。
pub const CONFIG_SUBPATH: &str = "/config";
/// 同上。
pub const CONFIG_VALIDATE_SUBPATH: &str = "/config/validate";
/// 同上。
pub const CONFIG_SECRETS_CLEAR_SUBPATH: &str = "/config/secrets/clear";
/// 同上。
pub const CONFIG_RESET_SUBPATH: &str = "/config/reset";

/// 危险区那条的**完整**对外路径。
///
/// 🔴 **它是单一真源**：五语言 DEPLOY.md 的危险区条目逐份写着它。
/// 与子树内路径的关系由 `the_reset_path_constant_is_the_single_source` 钉住。
pub const CONFIG_RESET_PATH: &str = "/admin/api/config/reset";

// ── 事件名（运维的 grep 锚点，一个字都不许改）────────────────────────────────

pub const EVENT_CONFIG_UPDATED: &str = "config.updated";
pub const EVENT_CONFIG_SECRET_CLEARED: &str = "config.secret_cleared";
pub const EVENT_CONFIG_RESET: &str = "config.reset";
/// 存储里那份配置读得出来、却构造不出一份合法配置，而逐字段判据说不出是哪一格。
pub const EVENT_CONFIG_UNLOADABLE: &str = "config.unloadable";

/// 网关口令的长度下限。**与管理口令那条是同一个数，理由逐字相同**：
/// 本网关**没有任何登录限速**，口令熵就是唯一的防线 —— 下限不是建议值。
pub const MIN_GATEWAY_TOKEN_LENGTH: usize = 24;

// ── 2. 环境变量快照 ─────────────────────────────────────────────────────────

static ENV_SNAPSHOT: OnceLock<EnvMap> = OnceLock::new();

/// 装配期注入环境变量快照。**第一次调用生效**，返回是否真的装上了。
///
/// 🔴 存在的理由见文件头第 2 条登记：`AppState` 里没有 env 这一格，
/// 而配置面的四元组里 `env` 与 `lockedBy` 两格全靠它 —— 缺了它们，
/// 面板会把一个被 `MAX_STRIKES=3` 锁死的字段显示成「可以改」，
/// 而那正是这套配置面存在的**最高频**理由。
pub fn install_env_snapshot(env: EnvMap) -> bool {
    ENV_SNAPSHOT.set(env).is_ok()
}

/// 当前的环境变量快照。
///
/// ⚠️ 没人注入时退回读一次进程环境 —— 那与装配时 `cli::env_snapshot()` 抓的是
/// **同一份**（进程生命周期内环境变量不变），所以生产路径上两者恒等。
#[must_use]
pub fn admin_env() -> &'static EnvMap {
    ENV_SNAPSHOT.get_or_init(env_fallback)
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn env_fallback() -> EnvMap {
    crate::cli::env_snapshot()
}

/// Workers 形态的 env 兜底：`cli` 不编进 wasm，环境变量只能走 Worker 的
/// `Env`（vars + secrets），而那一侧在装配时已经抓过一份 —— 没人注入时
/// 退回空表（与「未配置」同义，不炸面板）。
#[cfg(all(not(test), target_arch = "wasm32"))]
fn env_fallback() -> EnvMap {
    EnvMap::new()
}

/// ⚠️ 判据里**不许**让真实进程环境漏进来：那会让「本机恰好设了 `MAX_STRIKES`」
/// 变成一次随机失败。测试二进制里固定用同一份夹具 ⇒ 与执行顺序无关。
#[cfg(test)]
fn env_fallback() -> EnvMap {
    tests::fixture_env()
}

// ── 3. 字段表（🔴 唯一真源：曝光度 / 校验规则 / env 锁定，三件事同一张表）────

/// 一个字段的校验规则。
///
/// ⚠️ **`Text` / `Bool` / `ChannelOrNull` 三档本轮不在列**：它们今天唯一的用户是
/// 注册机子树，而那棵树**真的不存在**（见 `core::config` 文件头：
/// 「本轮不建一个半截的形状占位」）。造出没有字段用的档位，
/// 等于让 `not_a_boolean` / `not_a_channel` / `too_long` 三条码变成**永远发不出去**
/// 的死码 —— 而那正是「面板字典里有一条永远选不到的文案」那种漂移的起点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spec {
    /// 整数，且不小于 `min`。
    ///
    /// 🔴 `min` 不是「防负数」这种泛泛而谈：`UPSTREAM_TIMEOUT_MS = -1` 会让超时
    /// 立刻触发，**一个请求就能把整池打成 strike**；`MAX_STRIKES = 0` 更糟 ——
    /// `strikes >= max_strikes` 在第一次失败时即成立，等于跳过整个容错机制。
    /// **只有两个池子旋钮传 `min = 0`**：对它们 0 是「关闭」这个有意义的取值。
    Int { min: i64 },
    /// `http(s)://` 且解析得开。
    Url,
    /// 凭据：**缺席或空串 = 不改**，清空只能走 `secrets/clear`。
    Secret,
    /// 布尔。**只认真正的 `true` / `false`** ——
    /// 收 `"true"` 这种字符串会让「面板存了个字符串、装载器按字符串解析」
    /// 与「存了个布尔」两条路悄悄分岔，而两边都「能跑」。
    Bool,
    /// 自由文本，有上界。
    ///
    /// 🔴 `max` 不是排版洁癖：这些值会被拼进出站请求，
    /// 一个没有上界的字段就是一条把任意长度塞进上游的路。
    Text { max: usize },
    /// 通道名，或者 `null`（= 不指定）。
    ///
    /// ⚠️ **`null` 是一个合法取值，不是「没填」** ——
    /// 装载器对它有确定的处置（回落默认那条），面板也要能把它存回去。
    ChannelOrNull,
    /// 进四元组、但面板改不了。
    ///
    /// ⚠️ 两格各有各的理由，别合读：
    /// · `degraded` 是**装载的产物**不是旋钮 —— 做成可写会让面板能「把红色横幅关掉」，
    ///   而横幅要报告的那件事一点没变；
    /// · `usageStatsEnabled` **存储里就能改**（优先级 env > 存储 > 默认值），
    ///   不进这张表的话 `GET /config` 会对它返回一份自相矛盾的四元组
    ///   （`stored: true` / `env: null` / `effective: false`）—— 面板对它撒谎。
    ReadOnly,
}

/// 一个字段的完整声明。
pub struct FieldDef {
    /// 面板路径（点分）。
    pub path: &'static str,
    pub spec: Spec,
    /// **候选**环境变量名，顺序即优先级。空 = 这个字段没有环境变量。
    ///
    /// ⚠️ **是数组不是单串**：一个字段可以有正式名 + 兼容别名（注册机那条
    /// `REGISTRAR_CHANNEL` / `REGISTRAR_PRIMARY` 就是），存单串的那一版会让面板
    /// 把「是哪个名字锁的」答错，运维去 compose 里 grep 一个搜不到的字符串。
    pub env: &'static [&'static str],
}

/// 🔴 **全表。判据遍历它。**
///
/// 📜 这里曾登记「注册机那 15 格不在这里」，理由是
/// 「`GatewayConfig` 里没有注册机子树，于是它们的 `effective` 无从取值」。
/// **那条理由已经不成立**：注册机有自己的纯函数装载器
/// [`crate::registrar::runtime::load`]，四元组的 `effective`
/// **照旧是「拿那个函数真跑一次」取回来的**，不是在面板层另写一套来源推导。
/// ⇒ 15 格补齐。纪律 ① 一个字没破。
pub const FIELDS: [FieldDef; 28] = [
    FieldDef {
        path: "gatewayToken",
        spec: Spec::Secret,
        env: &["GATEWAY_TOKEN"],
    },
    FieldDef {
        path: "agnesBaseUrl",
        spec: Spec::Url,
        env: &["AGNES_BASE_URL"],
    },
    FieldDef {
        path: "upstreamTimeoutMs",
        spec: Spec::Int { min: 1 },
        env: &["UPSTREAM_TIMEOUT_MS"],
    },
    FieldDef {
        path: "upstreamSyncTimeoutMs",
        spec: Spec::Int { min: 1 },
        env: &["UPSTREAM_SYNC_TIMEOUT_MS"],
    },
    FieldDef {
        path: "maxStrikes",
        spec: Spec::Int { min: 1 },
        env: &["MAX_STRIKES"],
    },
    FieldDef {
        path: "cooldownRateLimitMs",
        spec: Spec::Int { min: 1 },
        env: &["COOLDOWN_RATE_LIMIT_MS"],
    },
    FieldDef {
        path: "cooldownPaymentMs",
        spec: Spec::Int { min: 1 },
        env: &["COOLDOWN_PAYMENT_MS"],
    },
    FieldDef {
        path: "cooldownStrikeMs",
        spec: Spec::Int { min: 1 },
        env: &["COOLDOWN_STRIKE_MS"],
    },
    // 🔴 这两个 `min = 0`：0 对它们是「关闭」这个有意义的取值，是用户的逃生口。
    FieldDef {
        path: "poolCacheTtlMs",
        spec: Spec::Int { min: 0 },
        env: &["POOL_CACHE_TTL_MS"],
    },
    FieldDef {
        path: "poolTouchIntervalMs",
        spec: Spec::Int { min: 0 },
        env: &["POOL_TOUCH_INTERVAL_MS"],
    },
    FieldDef {
        path: "degraded",
        spec: Spec::ReadOnly,
        // 它没有环境变量 ⇒ 四元组的 `env` / `lockedBy` 恒为 null，那是对的。
        env: &[],
    },
    FieldDef {
        path: "usageStatsEnabled",
        spec: Spec::ReadOnly,
        env: &["USAGE_STATS_ENABLED"],
    },
    // ── 注册机子树（15 格）─────────────────────────────────────────────
    FieldDef {
        path: "registrar.enabled",
        spec: Spec::Bool,
        env: &["REGISTRAR_ENABLED"],
    },
    // ⚠️ **只读**：它是装载的产物（选中的通道缺凭据 / 间隔下界大于上界…），
    // 不是旋钮。做成可写会让面板能「把红色横幅关掉」，而它报告的那件事一点没变。
    FieldDef {
        path: "registrar.blocked",
        spec: Spec::ReadOnly,
        // 它没有环境变量 ⇒ 四元组的 `env` / `lockedBy` 恒为 null，那是对的。
        env: &[],
    },
    FieldDef {
        path: "registrar.channel",
        spec: Spec::ChannelOrNull,
        // ⚠️ **旧名一起认**：存量部署里有 `REGISTRAR_PRIMARY`，
        // 漏掉它会让「面板上能改」与「其实被 env 钉死了」同时为真。
        env: &["REGISTRAR_CHANNEL", "REGISTRAR_PRIMARY"],
    },
    FieldDef {
        path: "registrar.targetKeys",
        spec: Spec::Int { min: 1 },
        env: &["REGISTRAR_TARGET_KEYS"],
    },
    FieldDef {
        path: "registrar.mintBatch",
        spec: Spec::Int { min: 1 },
        env: &["REGISTRAR_MINT_BATCH"],
    },
    FieldDef {
        path: "registrar.tendIntervalMs",
        spec: Spec::Int { min: 1 },
        env: &["REGISTRAR_TEND_INTERVAL_MS"],
    },
    FieldDef {
        path: "registrar.codeTimeoutMs",
        spec: Spec::Int { min: 1 },
        env: &["REGISTRAR_CODE_TIMEOUT_MS"],
    },
    FieldDef {
        path: "registrar.mintDelayMinMs",
        spec: Spec::Int { min: 1 },
        env: &["REGISTRAR_MINT_DELAY_MIN_MS"],
    },
    FieldDef {
        path: "registrar.mintDelayMaxMs",
        spec: Spec::Int { min: 1 },
        env: &["REGISTRAR_MINT_DELAY_MAX_MS"],
    },
    FieldDef {
        path: "registrar.maxDomainAttempts",
        spec: Spec::Int { min: 1 },
        env: &["REGISTRAR_MAX_DOMAIN_ATTEMPTS"],
    },
    FieldDef {
        path: "registrar.tokenName",
        spec: Spec::Text { max: 128 },
        env: &["REGISTRAR_TOKEN_NAME"],
    },
    FieldDef {
        path: "registrar.agnesPlatformUrl",
        spec: Spec::Url,
        env: &["AGNES_PLATFORM_URL"],
    },
    FieldDef {
        path: "registrar.yyds.baseUrl",
        spec: Spec::Url,
        env: &["YYDS_BASE_URL"],
    },
    FieldDef {
        path: "registrar.yyds.apiKey",
        spec: Spec::Secret,
        env: &["YYDS_API_KEY"],
    },
    FieldDef {
        path: "registrar.moemail.baseUrl",
        spec: Spec::Url,
        env: &["MOEMAIL_BASE_URL"],
    },
    FieldDef {
        path: "registrar.moemail.apiKey",
        spec: Spec::Secret,
        env: &["MOEMAIL_API_KEY"],
    },
];

/// 这个字段是不是凭据。**判据是 [`Spec`]，不是路径长得像不像密钥** ——
/// 后者是一张手写词表（`/token|key|secret/i` 那一类），本仓已登记为脆弱判据。
#[must_use]
pub fn is_secret(spec: Spec) -> bool {
    matches!(spec, Spec::Secret)
}

/// 面板能改的字段路径，**排好序**。前端不另写一份（写两份必漂）。
#[must_use]
pub fn editable_fields() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = FIELDS
        .iter()
        .filter(|f| f.spec != Spec::ReadOnly)
        .map(|f| f.path)
        .collect();
    v.sort_unstable();
    v
}

/// 哪几条路径是凭据。**从 [`FIELDS`] 派生**，不另写一份清单。
#[must_use]
pub fn secret_fields() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = FIELDS
        .iter()
        .filter(|f| is_secret(f.spec))
        .map(|f| f.path)
        .collect();
    v.sort_unstable();
    v
}

fn field_of(path: &str) -> Option<&'static FieldDef> {
    FIELDS.iter().find(|f| f.path == path)
}

/// 这个字段今天**该报哪个环境变量名**。
///
/// 判据是「env 里实际存在的那一个」，不是「正式名字是哪个」：运维要拿这个名字去
/// 自己的 compose / `.env` 里找那一行。都不存在时报第一个候选（正式名），
/// 那一档只出现在「没被锁」的场合，报什么都不会误导人。
#[must_use]
pub fn env_name_of(field: &str, env: &EnvMap) -> Option<&'static str> {
    let names = field_of(field)?.env;
    let first = names.first()?;
    Some(
        names
            .iter()
            .find(|n| env.contains_key(**n))
            .unwrap_or(first),
    )
}

/// 哪些字段被环境变量锁住了。
///
/// 🔴 **判据是「这个键在 env 里存在」，不是「值合法」**：`MAX_STRIKES=` 这种空串
/// 同样会走进装载器的 env 分支（然后拒绝启动），面板必须显示它是锁定的 ——
/// 否则用户会一直改存储、一直没效果。
#[must_use]
pub fn env_locked_fields(env: &EnvMap) -> Vec<String> {
    let mut v: Vec<String> = FIELDS
        .iter()
        .filter(|f| f.env.iter().any(|n| env.contains_key(*n)))
        .map(|f| f.path.to_string())
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

// ── 4. 纯逻辑：路径读写 ─────────────────────────────────────────────────────

fn as_object(v: Option<&Value>) -> Option<&Map<String, Value>> {
    v.and_then(Value::as_object)
}

fn get_at<'a>(root: &'a Map<String, Value>, path: &str) -> Option<&'a Value> {
    let mut cur: Option<&Value> = None;
    let mut here = root;
    for (i, seg) in path.split('.').enumerate() {
        if i > 0 {
            here = as_object(cur)?;
        }
        cur = here.get(seg);
        cur?;
    }
    cur
}

/// 深拷一层一层地写进去，**不改入参**（调用方手上那份存储原件必须原样保留）。
fn set_at(root: &Map<String, Value>, path: &str, value: Value) -> Map<String, Value> {
    let mut segs = path.splitn(2, '.');
    let head = segs.next().unwrap_or_default();
    let mut next = root.clone();
    match segs.next() {
        None => {
            next.insert(head.to_string(), value);
        }
        Some(rest) => {
            let child = next
                .get(head)
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            next.insert(head.to_string(), Value::Object(set_at(&child, rest, value)));
        }
    }
    next
}

fn delete_at(root: &Map<String, Value>, path: &str) -> Map<String, Value> {
    let mut segs = path.splitn(2, '.');
    let head = segs.next().unwrap_or_default();
    let mut next = root.clone();
    match segs.next() {
        None => {
            next.remove(head);
        }
        Some(rest) => {
            if let Some(child) = next.get(head).and_then(Value::as_object).cloned() {
                next.insert(head.to_string(), Value::Object(delete_at(&child, rest)));
            }
        }
    }
    next
}

fn err(field: &str, code: ConfigErrorCode) -> ConfigError {
    ConfigError {
        field: field.to_string(),
        code,
        params: BTreeMap::new(),
    }
}

fn err_with(field: &str, code: ConfigErrorCode, k: &str, v: impl ToString) -> ConfigError {
    let mut e = err(field, code);
    e.params.insert(k.to_string(), v.to_string());
    e
}

/// `http(s)://` 且解析得开。
///
/// ⚠️ **不放行别的协议**：这个字段会被拿去发请求，`javascript:` / `file:`
/// 在一个「转发目标」上没有任何正当含义。
fn is_http_url(v: &str) -> bool {
    (v.starts_with("http://") || v.starts_with("https://"))
        && v.len() > "https://".len()
        && !v[v.find("//").map_or(0, |i| i + 2)..].is_empty()
        && !v.contains(char::is_whitespace)
}

/// 可打印 ASCII（0x20–0x7E）。**与管理口令那条判据逐码位同源。**
///
/// ⚠️ 量词是「零个或多个」而不是「一个或多个」：这条规则说的是
/// 「不含送不出去的字符」，**非空不是它的职责**（空串在更早一步被当成「缺席」了）。
fn is_sendable(v: &str) -> bool {
    v.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

// ── 5. 纯逻辑：装载阻断判据 ─────────────────────────────────────────────────

/// 这份 `config` 装载得起来吗？装载不起来的每一条原因，逐字段列出来。
///
/// 它有三个消费者，这正是它存在的意义：[`validate_config_patch`] 的跨字段阶段、
/// `secrets/clear` 的**写前**预判、以及 `GET` / `PUT` 装载失败时的诊断视图。
///
/// ⚠️ **今天只说得出一条** —— 那是因为本轮的装载器（[`resolve_config`]）
/// 也**只剩这一条 fatal**：两边都没有网关口令。其余全是字段级降级（回落默认值 +
/// 一条 `config.invalid` 事件），它们既不让网关起不来、也不让任何子系统停跑 ⇒
/// 🔴 **不许拿「清单为空」当「装得起来」的判据**：它说得出的只是说得出的那些，
/// 说不出的那一档走 `config_unloadable`。
///
/// # 🔴 它同时是 `PUT` 那边**跨字段规则的唯一落点**
///
/// [`validate_config_patch`] 拿它在合并前后各跑一次、**只报新增的那几条**。
/// 于是「铸号间隔下界大于上界」「选中的通道没配凭据」这类跨字段规则
/// 不必在校验里另写一遍 —— 写两遍就会漂，而漂了没人会发现。
///
/// ⚠️ **差集这件事本身是承重的**：一个注册机只配了一半的存量部署，
/// 必须照样能在面板上改 `maxStrikes`。
/// 报全量的话，那份部署会被自己的历史**永久锁在设置页外面**。
///
/// # 🟡 与被移植的 TS 版的一处偏离，如实登记
///
/// TS 那边 `crossFieldErrors` 是**无条件推全量**的（`config-validate.ts:494`），
/// 于是 `REGISTRAR_ENABLED=true` 而没配 key 的部署
/// —— 一种**光靠 compose 就能进入、且面板上改不掉**的状态 ——
/// 会让**整个设置页一个字段都存不了**。
/// 这里改成差集：**「这次新引入的问题」照样当场拦下**（保护一个字没少），
/// 而「上一任留下的问题」不再牵连无关字段。
/// 🔴 别把它读成「放宽了校验」。
#[must_use]
pub fn config_load_blockers(stored: Option<&Value>, env: &EnvMap) -> Vec<ConfigError> {
    let mut out = Vec::new();
    if !has_gateway_token(stored, env) {
        out.push(err("gatewayToken", ConfigErrorCode::GatewayTokenRequired));
    }
    // 注册机那一族。**装载器零 panic**，所以这里无条件跑得动。
    let stored_registrar = stored
        .and_then(Value::as_object)
        .and_then(|o| o.get("registrar").cloned())
        .unwrap_or(Value::Null);
    for b in crate::registrar::runtime::load(env, &stored_registrar).blockers {
        out.push(ConfigError {
            field: b.field,
            code: crate::registrar::config::config_error_code_of(b.code),
            params: BTreeMap::new(),
        });
    }
    out
}

/// 与 [`resolve_config`] 里那一段**逐字同义**：env 一旦出现（哪怕空串）就赢，
/// 空串就是「缺」；env 缺席才看存储，存储侧只收非空字符串。
fn has_gateway_token(stored: Option<&Value>, env: &EnvMap) -> bool {
    match env.get("GATEWAY_TOKEN") {
        Some(v) => !v.is_empty(),
        None => stored
            .and_then(|v| v.get("gatewayToken"))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
    }
}

// ── 6. 纯逻辑：校验 ─────────────────────────────────────────────────────────

/// 校验通过时的产物。
#[derive(Debug)]
pub struct Verdict {
    /// 合并之后**应当整体写回存储的那份 `config`**。调用方照写，不再自己合并。
    pub next: Map<String, Value>,
    /// 这次真的改了哪些路径（值与合并前不同）。**凭据只报路径，不报值。**
    pub changed: Vec<String>,
}

/// 校验一份 patch，并给出合并后应当写回的整份 `config`。
///
/// `patch` 用**扁平点分路径**，不是嵌套对象：「缺席 = 不改」这条语义只有扁平形态
/// 表达得清楚 —— 嵌套形态下 `{a:{b:1}}` 到底是「只改 b」还是「把整个 a 换掉」
/// 取决于合并规则写在哪一层，而两种读法都有人会按。
///
/// 🔴 **纯函数**：不碰存储、不读时钟。调用方负责「校验通过之后才写」。
pub fn validate_config_patch(
    patch: &Value,
    stored: Option<&Value>,
    env: &EnvMap,
    admin_token: Option<&str>,
) -> Result<Verdict, Vec<ConfigError>> {
    let Some(body) = patch.as_object() else {
        return Err(vec![err("", ConfigErrorCode::UnknownField)]);
    };
    let stored_obj = stored
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut errors: Vec<ConfigError> = Vec::new();
    let mut next = stored_obj.clone();
    let mut changed: Vec<String> = Vec::new();

    for (field, value) in body {
        let Some(def) = field_of(field) else {
            // **拼错的字段名一律 400，不静默丢弃**：`{maxStrikess: 9}` 在宽松实现下
            // 是一次「保存成功、什么都没发生」，而面板会如实显示保存成功。
            errors.push(err(field, ConfigErrorCode::UnknownField));
            continue;
        };
        if def.spec == Spec::ReadOnly {
            // 它在四元组里看得见，但不是旋钮 —— 对面板而言与「不认识这个字段」同档。
            errors.push(err(field, ConfigErrorCode::UnknownField));
            continue;
        }

        // ⚠️ **被 env 锁定的字段一律拒绝，不是「写下去但不生效」。**
        // 写下去的后果是：面板显示保存成功、四元组里 `stored` 真的变了、
        // 而 `effective` 纹丝不动 —— 运维会以为是缓存没刷，去等那 30 秒，然后再等一次。
        if let Some(name) = env_name_of(field, env)
            && env.contains_key(name)
        {
            errors.push(err_with(field, ConfigErrorCode::LockedByEnv, "env", name));
            continue;
        }

        if def.spec == Spec::Secret {
            match check_secret(field, value, admin_token) {
                SecretVerdict::Unchanged => {}
                SecretVerdict::Rejected(e) => errors.push(e),
                SecretVerdict::Accept(s) => {
                    next = set_at(&next, field, Value::String(s));
                    changed.push(field.clone());
                }
            }
            continue;
        }

        if let Some(e) = check_leaf(field, def.spec, value) {
            errors.push(e);
            continue;
        }
        if get_at(&next, field) != Some(value) {
            changed.push(field.clone());
        }
        next = set_at(&next, field, value.clone());
    }

    // 逐字段校验没过时**不跑跨字段规则**：一个非法操作数会让跨字段比较报出
    // 第二条只会误导人的错误。
    if !errors.is_empty() {
        return Err(errors);
    }

    // ── 跨字段阶段：**只拒这次补丁新引入的 blocker** ────────────────────────
    //
    // 🔴 判据是「补丁前后两份清单的差」，不是「补丁之后还有没有」。
    // 后者的后果：那份配置**本来就**坏掉时，运维想改一个无关字段会被 400 拒 ——
    // 而他手上正拿着一份装不起来的配置，最需要的恰恰是一步一步修回来。
    // 它**不会把砖机场景放回来**：任何一条新引入的照旧当场 400。
    let before: Vec<String> = config_load_blockers(stored, env)
        .iter()
        .map(|b| format!("{}:{}", b.field, b.code))
        .collect();
    let after = Value::Object(next.clone());
    for b in config_load_blockers(Some(&after), env) {
        if !before.contains(&format!("{}:{}", b.field, b.code)) {
            errors.push(b);
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    changed.sort_unstable();
    Ok(Verdict { next, changed })
}

enum SecretVerdict {
    /// 缺席 / 空串 ⇒ **不改**。
    Unchanged,
    Accept(String),
    Rejected(ConfigError),
}

/// 凭据那四条硬规则，**顺序与管理口令那一份一样**：
/// 空白 → 字符集 → 长度 → 相同性。
fn check_secret(field: &str, value: &Value, admin_token: Option<&str>) -> SecretVerdict {
    let Some(raw) = value.as_str() else {
        return SecretVerdict::Rejected(err(field, ConfigErrorCode::NotAString));
    };
    // ⚠️⚠️ **判据是 `trim().is_empty()` 而不是 `is_empty()`**：一次「粘了几个空格」
    // 的误操作在后者下会被**收下并落盘**，而装载器对 `"   "` 认为「有口令」⇒
    // 一次都不 fail-fast，面板还显示 `configured: true`，
    // 而所有下游用户从此 401（实测逐字）。
    if raw.trim().is_empty() {
        return SecretVerdict::Unchanged;
    }
    if raw.trim() != raw {
        // HTTP 头值在传输层被 trim，而存储里不会 ⇒ 带空白的口令客户端**永远送不出来**，
        // 症状是「口令明明是对的却一直 401」。而首尾空格在面板上**根本渲染不出来**。
        return SecretVerdict::Rejected(err(field, ConfigErrorCode::WhitespacePadded));
    }
    if !is_sendable(raw) {
        return SecretVerdict::Rejected(err(field, ConfigErrorCode::NotSendable));
    }
    if field == "gatewayToken" {
        if raw.chars().count() < MIN_GATEWAY_TOKEN_LENGTH {
            return SecretVerdict::Rejected(err_with(
                field,
                ConfigErrorCode::TooShort,
                "min",
                MIN_GATEWAY_TOKEN_LENGTH,
            ));
        }
        // 🔴 写成与管理口令相等的后果：管理鉴权的每请求复查立刻把整个管理面判成 503，
        // **而改回去的那条 PUT 也是 503** ⇒ 面板把自己锁死（实测）。
        if admin_token == Some(raw) {
            return SecretVerdict::Rejected(err(field, ConfigErrorCode::SameAsAdminToken));
        }
    }
    SecretVerdict::Accept(raw.to_string())
}

fn check_leaf(field: &str, spec: Spec, value: &Value) -> Option<ConfigError> {
    match spec {
        Spec::Int { min } => {
            let n = value.as_i64().filter(|_| value.is_i64() || value.is_u64());
            let Some(n) = n else {
                return Some(err(field, ConfigErrorCode::NotAnInteger));
            };
            (n < min).then(|| err_with(field, ConfigErrorCode::BelowMin, "min", min))
        }
        Spec::Url => {
            let Some(s) = value.as_str() else {
                return Some(err(field, ConfigErrorCode::NotAString));
            };
            if s.is_empty() {
                return Some(err(field, ConfigErrorCode::Empty));
            }
            (!is_http_url(s)).then(|| err(field, ConfigErrorCode::NotAUrl))
        }
        Spec::Bool => (!value.is_boolean()).then(|| err(field, ConfigErrorCode::NotABoolean)),
        Spec::Text { max } => {
            let Some(s) = value.as_str() else {
                return Some(err(field, ConfigErrorCode::NotAString));
            };
            if s.is_empty() {
                return Some(err(field, ConfigErrorCode::Empty));
            }
            (s.chars().count() > max).then(|| err_with(field, ConfigErrorCode::TooLong, "max", max))
        }
        Spec::ChannelOrNull => {
            // 🔴 `null` 合法 —— 它是「不指定」，装载器对它有确定的处置。
            if value.is_null() {
                return None;
            }
            let Some(s) = value.as_str() else {
                return Some(err(field, ConfigErrorCode::NotAString));
            };
            (crate::registrar::config::Channel::parse_public(s).is_none())
                .then(|| err(field, ConfigErrorCode::NotAChannel))
        }
        // 结构上到不了：这两档在进来之前就分流了。
        Spec::Secret | Spec::ReadOnly => None,
    }
}

/// 显式清空一把凭据（`secrets/clear` 的执行体）。
///
/// ⚠️⚠️ **清掉 `gatewayToken` 而环境变量里也没有时，下一次冷启动会 fail-closed。**
/// 已经在跑的实例因为持有者保留上一份合法快照**不会当场停摆**，所以这件事在面板上
/// 是**看不见**的，直到下一次重启。**本函数不拦它**：这是一条显式动作
/// （专门的端点 + 面板二次确认 + 红色警告），而「把存储里那把泄漏的口令删掉」
/// 是一个正当且必须存在的能力。后果如实进响应体与事件。
#[must_use]
pub fn clear_secret(stored: Option<&Value>, field: &str) -> Option<Map<String, Value>> {
    if !secret_fields().contains(&field) {
        return None;
    }
    Some(delete_at(
        &stored
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default(),
        field,
    ))
}

// ── 7. 纯逻辑：四元组（**`effective` 来自真正的装载器**）────────────────────

/// 一次装载 + 切块的产物。
pub struct Snapshot {
    /// 公开字段的四元组。`None` = **诊断视图**（存储里那份读得出来但构造不出合法配置）。
    pub fields: Option<Map<String, Value>>,
    /// 凭据的 `{configured, hint, lockedBy}`。🔴 **永不含明文。**
    pub credentials: Option<Map<String, Value>>,
    pub config_degraded: bool,
    pub load_blocked: Vec<ConfigError>,
    /// **不拦人、但必须说出来的那几句话。**
    ///
    /// ⚠️ 今天恒为空：它唯一的一族（存量旧主备键的迁移提示）属于注册机子树。
    /// **字段保留**（它是对外契约的一格，面板据它渲染设置页顶部横幅），
    /// 缺口已登记 —— 🔴 别把它读成「这里没有东西要说」。
    pub load_notices: Vec<ConfigError>,
    /// 公开字段的生效值。`changed` 拿它做对照。
    effective: Option<BTreeMap<&'static str, Value>>,
}

/// 从一份**已经读到手**的存储原件切出快照。🔴 **纯函数，零 IO。**
///
/// # ⚠️ 与 TS 版的一处**刻意简化**，理由写全
///
/// TS 那边这一段是**三分**（读不出来 / 瞬时抖动 / 真的是配置问题），第二档存在的
/// 理由是「装载」与「读存储」在那一侧绑在同一个 async 函数里 ⇒ 一次装载失败分不清
/// 是存储抖了还是配置坏了。**Rust 侧这两件事在类型上就是分开的**：
/// 存储读在调用方、[`resolve_config`] 是一个对已读到手的 `Value` 求值的纯函数 ⇒
/// **「瞬时抖动」这一档在这里结构性地不存在**，三分塌成二分。
/// 🔴 这不是省事：多写那一档就是多一个**永远走不到**的分支。
#[must_use]
pub fn snapshot_of(stored: Option<&Value>, env: &EnvMap) -> (Snapshot, Option<ConfigError>) {
    // 逃生口 `RESET_CONFIG=1`：装载器**完全忽略**存储里那份 ⇒ 四元组的 `stored` 那一格
    // 也必须跟着显示「没有」，否则面板会把一个装载器根本没看的值说成「存储里配的是它」。
    let reset = env.get("RESET_CONFIG").map(String::as_str) == Some("1");
    let visible = if reset { None } else { stored };

    match resolve_config(env, stored, false) {
        Ok((cfg, flags)) => {
            let mut effective = effective_map(&cfg, flags.degraded);
            // 🔴 注册机那份**单独装载**，而且装载器零 panic：
            // 它缺凭据不该让整个配置面跟着进诊断视图。
            let stored_registrar = visible
                .and_then(Value::as_object)
                .and_then(|o| o.get("registrar").cloned())
                .unwrap_or(Value::Null);
            let rl = crate::registrar::runtime::load(env, &stored_registrar);
            effective.extend(registrar_effective(&rl));
            let (fields, credentials) = split(visible, env, &effective);
            (
                Snapshot {
                    fields: Some(fields),
                    credentials: Some(credentials),
                    config_degraded: flags.degraded,
                    // 📜 这两格曾经恒为空，登记的理由是「它们唯一的一族属于注册机子树」。
                    // 子树来了，两格跟着有了真实来源。
                    load_blocked: rl
                        .blockers
                        .iter()
                        .map(|b| ConfigError {
                            field: b.field.clone(),
                            code: crate::registrar::config::config_error_code_of(b.code),
                            params: BTreeMap::new(),
                        })
                        .collect(),
                    load_notices: rl
                        .notices
                        .iter()
                        .map(|n| ConfigError {
                            field: n.field.clone(),
                            code: crate::registrar::config::config_error_code_of(n.code),
                            params: BTreeMap::new(),
                        })
                        .collect(),
                    effective: Some(effective),
                },
                None,
            )
        }
        Err(_) => {
            // 🔴 **降级成诊断视图而不是 500** —— 那是运维唯一的出路。
            // 在它之前：存储里那份一旦装载不起来，`GET` 与 `PUT` 全是 500，
            // 于是「把口令重新填回去」这条自救路径**自己也走不通**。
            let mut blocked = config_load_blockers(stored, env);
            // 说不出是哪一格时兜一条，**不编一个具体字段出来**。
            let unloadable = blocked.is_empty();
            if unloadable {
                blocked.push(err("", ConfigErrorCode::ConfigUnloadable));
            }
            (
                Snapshot {
                    fields: None,
                    credentials: None,
                    config_degraded: true,
                    load_blocked: blocked,
                    load_notices: Vec::new(),
                    effective: None,
                },
                unloadable.then(|| err("", ConfigErrorCode::ConfigUnloadable)),
            )
        }
    }
}

/// 生效值逐格取出来。**这是整份四元组里唯一一处「生效值从哪来」的答案。**
fn effective_map(
    cfg: &crate::core::config::GatewayConfig,
    degraded: bool,
) -> BTreeMap<&'static str, Value> {
    let mut m = BTreeMap::new();
    m.insert("agnesBaseUrl", Value::from(cfg.agnes_base_url.clone()));
    m.insert("upstreamTimeoutMs", Value::from(cfg.upstream_timeout_ms));
    m.insert(
        "upstreamSyncTimeoutMs",
        Value::from(cfg.upstream_sync_timeout_ms),
    );
    m.insert("maxStrikes", Value::from(cfg.max_strikes));
    m.insert(
        "cooldownRateLimitMs",
        Value::from(cfg.cooldown_rate_limit_ms),
    );
    m.insert("cooldownPaymentMs", Value::from(cfg.cooldown_payment_ms));
    m.insert("cooldownStrikeMs", Value::from(cfg.cooldown_strike_ms));
    m.insert("poolCacheTtlMs", Value::from(cfg.pool_cache_ttl_ms));
    m.insert(
        "poolTouchIntervalMs",
        Value::from(cfg.pool_touch_interval_ms),
    );
    m.insert("degraded", Value::from(degraded));
    m.insert("usageStatsEnabled", Value::from(cfg.usage_stats_enabled));
    m
}

/// 注册机那 15 格的生效值。
///
/// 🔴 **同样是「拿真正的装载器跑一次」取回来的**，不是照着 env 与存储
/// 在这一层重新推一遍优先级 —— 那样面板上显示的「生效值」与注册机真正在用的那份
/// 会各自演化，而两边都「能跑」。
fn registrar_effective(
    load: &crate::registrar::config::RegistrarLoad,
) -> BTreeMap<&'static str, Value> {
    let c = &load.config;
    let mut m = BTreeMap::new();
    m.insert("registrar.enabled", Value::from(c.enabled));
    // 🔴 **`blocked` 为真时 `enabled` 一个字都不改** —— 真话是「已启用 · 本次没跑起来」。
    m.insert("registrar.blocked", Value::from(c.blocked));
    m.insert("registrar.channel", Value::from(c.channel.as_str()));
    m.insert("registrar.targetKeys", Value::from(c.target_keys));
    m.insert("registrar.mintBatch", Value::from(c.mint_batch));
    m.insert("registrar.tendIntervalMs", Value::from(c.tend_interval_ms));
    m.insert("registrar.codeTimeoutMs", Value::from(c.code_timeout_ms));
    m.insert("registrar.mintDelayMinMs", Value::from(c.mint_delay_min_ms));
    m.insert("registrar.mintDelayMaxMs", Value::from(c.mint_delay_max_ms));
    m.insert(
        "registrar.maxDomainAttempts",
        Value::from(c.max_domain_attempts),
    );
    m.insert("registrar.tokenName", Value::from(c.token_name.clone()));
    m.insert(
        "registrar.agnesPlatformUrl",
        Value::from(c.agnes_platform_url.clone()),
    );
    // ⚠️ 凭据那四格的「生效值」**永不进这张表** —— 它们走 `credentials` 那一侧，
    // 只报「配没配 + 末 4 位」。不过 `baseUrl` 两格不是凭据，照常给。
    m.insert(
        "registrar.yyds.baseUrl",
        c.yyds
            .as_ref()
            .map_or(Value::Null, |y| Value::from(y.base_url.clone())),
    );
    m.insert(
        "registrar.moemail.baseUrl",
        c.moemail
            .as_ref()
            .map_or(Value::Null, |y| Value::from(y.base_url.clone())),
    );
    m
}

/// 把字段表切成面板要的两块。
fn split(
    stored: Option<&Value>,
    env: &EnvMap,
    effective: &BTreeMap<&'static str, Value>,
) -> (Map<String, Value>, Map<String, Value>) {
    let stored_obj = stored
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut fields = Map::new();
    let mut credentials = Map::new();
    for def in &FIELDS {
        let name = env_name_of(def.path, env);
        let env_value = name.and_then(|n| env.get(n));
        let locked_by = env_value.map_or(Value::Null, |_| {
            Value::from(format!("env:{}", name.unwrap_or_default()))
        });
        if is_secret(def.spec) {
            // ⚠️ **`configured` 读的是「存储原件 + env」，不是生效模型**：
            // 照生效模型报的话，用户「先填凭据、后开开关」这个最自然的顺序下，
            // 面板会说「未配置」，逼他再填一遍。
            let raw: Option<&str> = env_value.map(String::as_str).or_else(|| {
                get_at(&stored_obj, def.path)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            });
            let raw = raw.filter(|s| !s.is_empty());
            let mut o = Map::new();
            o.insert("configured".into(), Value::from(raw.is_some()));
            o.insert(
                "hint".into(),
                raw.and_then(hint_of).map_or(Value::Null, Value::from),
            );
            o.insert("lockedBy".into(), locked_by);
            credentials.insert(def.path.to_string(), Value::Object(o));
            continue;
        }
        let mut o = Map::new();
        o.insert(
            "stored".into(),
            get_at(&stored_obj, def.path)
                .cloned()
                .unwrap_or(Value::Null),
        );
        o.insert(
            "env".into(),
            env_value.map_or(Value::Null, |v| Value::from(v.clone())),
        );
        o.insert(
            "effective".into(),
            effective.get(def.path).cloned().unwrap_or(Value::Null),
        );
        o.insert("lockedBy".into(), locked_by);
        fields.insert(def.path.to_string(), Value::Object(o));
    }
    (fields, credentials)
}

/// 一把凭据的末 4 位。
///
/// 🔴 **短于 5 位一律 `None`**：`hint` 存在的意义是让运维认出「我配的是哪一把」，
/// 而对一把 4 位的口令，末 4 位就是全部 —— 那不叫提示，那叫 reveal。
fn hint_of(secret: &str) -> Option<String> {
    let n = secret.chars().count();
    (n > 4).then(|| secret.chars().skip(n - 4).collect())
}

/// 真的变了的公开字段（**按回读出来的生效值比，不是按请求体比**），排好序。
///
/// 这个区别是有后果的：一个被 env 锁着的字段即便被写进去，生效值也纹丝不动 ——
/// **面板高亮的必须是真的变了的那些**。
/// ⚠️ 两侧任一装载不起来时一律空清单，而不是编一份差异出来：那时根本没有
/// 「生效值」这个东西可比。
fn changed_effective(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let (Some(b), Some(a)) = (before.effective.as_ref(), after.effective.as_ref()) else {
        return Vec::new();
    };
    let mut out: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(**k) != Some(*v))
        .map(|(k, _)| (*k).to_string())
        .collect();
    out.sort_unstable();
    out
}

// ── 8. 线格式 ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ConfigErrorWire {
    field: String,
    code: &'static str,
    /// 空时**不出现这一格**（空对象会让面板的「有没有参数」判断多出一种形态）。
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<BTreeMap<String, String>>,
}

impl From<&ConfigError> for ConfigErrorWire {
    fn from(e: &ConfigError) -> Self {
        Self {
            field: e.field.clone(),
            code: e.code.as_str(),
            params: (!e.params.is_empty()).then(|| e.params.clone()),
        }
    }
}

fn wire_errors(v: &[ConfigError]) -> Vec<ConfigErrorWire> {
    v.iter().map(ConfigErrorWire::from).collect()
}

/// 传播时间。🔴 **两格都等于 `CONFIG_TTL_MS`，中间不许再加任何一项。**
#[derive(Serialize)]
struct Propagation {
    #[serde(rename = "configTtlMs")]
    config_ttl_ms: i64,
    #[serde(rename = "visibilityUpperBoundMs")]
    visibility_upper_bound_ms: i64,
}

const PROPAGATION: Propagation = Propagation {
    config_ttl_ms: CONFIG_TTL_MS,
    visibility_upper_bound_ms: CONFIG_TTL_MS,
};

#[derive(Serialize)]
struct GetWire {
    fields: Option<Map<String, Value>>,
    credentials: Option<Map<String, Value>>,
    #[serde(rename = "configDegraded")]
    config_degraded: bool,
    #[serde(rename = "loadBlocked")]
    load_blocked: Vec<ConfigErrorWire>,
    #[serde(rename = "loadNotices")]
    load_notices: Vec<ConfigErrorWire>,
    editable: Vec<&'static str>,
    secrets: Vec<&'static str>,
    #[serde(rename = "resetBlocked")]
    reset_blocked: Vec<ConfigErrorWire>,
    propagation: Propagation,
}

#[derive(Serialize)]
struct PutWire {
    fields: Option<Map<String, Value>>,
    credentials: Option<Map<String, Value>>,
    #[serde(rename = "configDegraded")]
    config_degraded: bool,
    #[serde(rename = "loadBlocked")]
    load_blocked: Vec<ConfigErrorWire>,
    #[serde(rename = "loadNotices")]
    load_notices: Vec<ConfigErrorWire>,
    changed: Vec<String>,
    #[serde(rename = "credentialsChanged")]
    credentials_changed: Vec<String>,
    /// ⚠️ **不是「已生效」的承诺，就是服务器落盘的那一刻。**
    /// 面板不许拿它渲染「已保存并生效」—— 该看的是上面回读出来的 `fields`。
    #[serde(rename = "appliedAt")]
    applied_at: i64,
    propagation: Propagation,
}

#[derive(Serialize)]
struct ValidateWire {
    ok: bool,
    changed: Vec<String>,
}

#[derive(Serialize)]
struct ClearWire {
    cleared: String,
    #[serde(rename = "stillConfigured")]
    still_configured: bool,
    #[serde(rename = "gatewayTokenMissing")]
    gateway_token_missing: bool,
    #[serde(rename = "loadBlocked")]
    load_blocked: Vec<ConfigErrorWire>,
    fields: Option<Map<String, Value>>,
    credentials: Option<Map<String, Value>>,
    #[serde(rename = "configDegraded")]
    config_degraded: bool,
    #[serde(rename = "resetBlocked")]
    reset_blocked: Vec<ConfigErrorWire>,
    propagation: Propagation,
}

#[derive(Serialize)]
struct ResetWire {
    fields: Option<Map<String, Value>>,
    credentials: Option<Map<String, Value>>,
    #[serde(rename = "configDegraded")]
    config_degraded: bool,
    #[serde(rename = "loadBlocked")]
    load_blocked: Vec<ConfigErrorWire>,
    changed: Vec<String>,
    #[serde(rename = "credentialsChanged")]
    credentials_changed: Vec<String>,
    #[serde(rename = "resetBlocked")]
    reset_blocked: Vec<ConfigErrorWire>,
    #[serde(rename = "appliedAt")]
    applied_at: i64,
    propagation: Propagation,
}

/// 逐字段错误的统一信封。**`errors` 在顶层**：面板靠 `code` 选五语言文案，
/// 🔴 **不解析 `message`**。
#[derive(Serialize)]
struct InvalidWire {
    error: InvalidInner,
    errors: Vec<ConfigErrorWire>,
}

#[derive(Serialize)]
struct InvalidInner {
    r#type: &'static str,
    message: &'static str,
}

fn invalid(errors: &[ConfigError]) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(InvalidWire {
            error: InvalidInner {
                r#type: "invalid_request_error",
                message: "配置校验没通过，本次一个字节都没有写入",
            },
            errors: wire_errors(errors),
        }),
    )
        .into_response()
}

// ── 9. 请求体窄化 ───────────────────────────────────────────────────────────

/// 读一个 JSON **对象**请求体。
///
/// ⚠️ 畸形 JSON 那一档**带 `code`**（管理信封）：它在面板上是看得见的
/// （设置页保存时粘错东西就会走到）。而**其余几处刻意仍是不带码的网关信封** ——
/// 设置页从不渲染那几条的 `message`，它们够不着屏幕（02 §4.4 登记的刻意不一致）。
fn read_object(body: &Bytes) -> Result<Map<String, Value>, Response> {
    let v: Value =
        serde_json::from_slice(body).map_err(|_| AdminError::bad_json().into_response())?;
    match v {
        Value::Object(o) => Ok(o),
        _ => Err(bad_request("请求体必须是一个 JSON 对象")),
    }
}

fn bad_request(msg: impl Into<String>) -> Response {
    GatewayError::new(StatusCode::BAD_REQUEST, "invalid_request_error", msg).into_response()
}

/// 顶层只认给定的那几个键。
///
/// **拼错的顶层字段一律 400**：`{patchs: {...}}` 在宽松实现下是一次
/// 「保存成功、什么都没发生」，而面板会如实显示保存成功。
fn only_keys(o: &Map<String, Value>, allowed: &[&str]) -> Result<(), Response> {
    let extra: Vec<&str> = o
        .keys()
        .map(String::as_str)
        .filter(|k| !allowed.contains(k))
        .collect();
    if extra.is_empty() {
        Ok(())
    } else {
        Err(bad_request(format!("不认识的字段：{}", extra.join(", "))))
    }
}

// ── 10. handler 共用的取数 / 落盘 ───────────────────────────────────────────

async fn read_stored(st: &AppState) -> Result<Option<Value>, Response> {
    match st.storage.get(CONFIG_KEY).await {
        Ok(v) => Ok(v),
        Err(e) => {
            // 🔴 500 不许静默：实测过 14 次 500 之后事件流零记录，运维无从排障。
            st.logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    crate::admin::keys::EVENT_ADMIN_STORAGE_FAILED,
                    "管理树读存储失败",
                )
                .with("key", CONFIG_KEY)
                .with("err", e.to_string()),
            );
            Err(GatewayError::internal().into_response())
        }
    }
}

async fn read_all(st: &AppState) -> Result<Snapshot, Response> {
    let stored = read_stored(st).await?;
    let (snap, unloadable) = snapshot_of(stored.as_ref(), admin_env());
    if unloadable.is_some() {
        st.logger.log(LogEntry::new(
            LogLevel::Error,
            EVENT_CONFIG_UNLOADABLE,
            "存储里的配置读得出来、却构造不出一份合法配置，而逐字段判据说不出是哪一格。面板已降级成诊断视图。",
        ));
    }
    Ok(snap)
}

async fn put_config(st: &AppState, next: Value) -> Result<(), Response> {
    match st.storage.put(CONFIG_KEY, next, None).await {
        Ok(()) => Ok(()),
        Err(e) => {
            st.logger.log(
                LogEntry::new(
                    LogLevel::Error,
                    crate::admin::keys::EVENT_ADMIN_STORAGE_FAILED,
                    "管理树写存储失败",
                )
                .with("key", CONFIG_KEY)
                .with("err", e.to_string()),
            );
            Err(GatewayError::internal().into_response())
        }
    }
}

fn blocked_ids(v: &[ConfigError]) -> Value {
    if v.is_empty() {
        Value::Null
    } else {
        Value::from(
            v.iter()
                .map(|b| format!("{}:{}", b.field, b.code))
                .collect::<Vec<_>>()
                .join(","),
        )
    }
}

// ── 11. 五条 handler ────────────────────────────────────────────────────────

/// `GET /admin/api/config` —— 设置页的唯一取数端点。**一次存储写都不产生。**
pub async fn get(State(st): State<AppState>) -> Response {
    let snap = match read_all(&st).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let env = admin_env();
    Json(GetWire {
        fields: snap.fields,
        credentials: snap.credentials,
        config_degraded: snap.config_degraded,
        load_blocked: wire_errors(&snap.load_blocked),
        load_notices: wire_errors(&snap.load_notices),
        editable: editable_fields(),
        secrets: secret_fields(),
        // ⚠️ **必须在 `GET` 上就给，不能等重置回来再说**：面板要在**二次确认框里**
        // 就把后果说清，而那一刻还没有发过任何写请求。**零额外存储读**。
        reset_blocked: wire_errors(&config_load_blockers(Some(&Value::Object(Map::new())), env)),
        propagation: PROPAGATION,
    })
    .into_response()
}

/// 取出 `{ patch }`。`PUT` 与干跑**逐字共用**。
fn read_patch(body: &Bytes) -> Result<Value, Response> {
    let o = read_object(body)?;
    only_keys(&o, &["patch"])?;
    let patch = o.get("patch").cloned().unwrap_or(Value::Null);
    if patch.is_object() {
        Ok(patch)
    } else {
        Err(bad_request("patch 必须是一个 JSON 对象"))
    }
}

/// `PUT /admin/api/config` —— 校验 → 写 → `invalidate()` → **回读**。
///
/// 🔴 **顺序不是可以调的**，见文件头纪律 ③ 与 ④。
pub async fn put(State(st): State<AppState>, body: Bytes) -> Response {
    let patch = match read_patch(&body) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let env = admin_env();

    // **写之前先把当前状态读出来**：校验要拿它做合并（「缺席 = 不改」），
    // 回执里的 `changed` 也要拿它做对照。这一次读发生在任何写之前。
    let stored_before = match read_stored(&st).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let (before, _) = snapshot_of(stored_before.as_ref(), env);

    let verdict = match validate_config_patch(
        &patch,
        stored_before.as_ref(),
        env,
        st.admin_token.as_deref().map(String::as_str),
    ) {
        Ok(v) => v,
        Err(errors) => return invalid(&errors),
    };

    if let Err(r) = put_config(&st, Value::Object(verdict.next)).await {
        return r;
    }
    // **让同一个进程的下一个请求一定重载。** 少了这一行，`GET /admin/api/overview`
    // 会在最多一个 `CONFIG_TTL_MS`（默认 30 秒）内继续报旧值。
    st.config.invalidate();

    // ── 回读。**从存储读，不是把 `verdict.next` 换个形状交回去。** ──────────
    let after = match read_all(&st).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let changed = changed_effective(&before, &after);
    let secrets = secret_fields();
    let credentials_changed: Vec<String> = verdict
        .changed
        .iter()
        .filter(|p| secrets.contains(&p.as_str()))
        .cloned()
        .collect();

    if !verdict.changed.is_empty() {
        st.logger.log(
            LogEntry::new(LogLevel::Warn, EVENT_CONFIG_UPDATED, "面板改了网关配置")
                // 🔴 **只记路径，一个值都不记**：日志常被转发到第三方，而这批路径里有凭据。
                .with("fields", verdict.changed.join(","))
                .with("changedEffective", changed.join(","))
                .with("secrets", credentials_changed.len() as i64),
        );
    }

    Json(PutWire {
        fields: after.fields,
        credentials: after.credentials,
        config_degraded: after.config_degraded,
        load_blocked: wire_errors(&after.load_blocked),
        load_notices: wire_errors(&after.load_notices),
        changed,
        credentials_changed,
        applied_at: crate::adapters::now_ms(),
        propagation: PROPAGATION,
    })
    .into_response()
}

/// `POST /admin/api/config/validate` —— 干跑。**一次存储写都不产生。**
///
/// ⚠️⚠️ 它跑的是与 `PUT` **同一个** [`validate_config_patch`]，**上下文也逐字段一样**。
/// 另写一份「轻量版校验」、或者少传一个 `admin_token`，就等于让干跑与真跑给出不同
/// 答案 —— 那比没有干跑更坏（实测过的分叉方向是「干跑放行、真跑拒绝」，
/// 运维读到的是「面板刚说没问题」）。
pub async fn validate(State(st): State<AppState>, body: Bytes) -> Response {
    let patch = match read_patch(&body) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let stored = match read_stored(&st).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match validate_config_patch(
        &patch,
        stored.as_ref(),
        admin_env(),
        st.admin_token.as_deref().map(String::as_str),
    ) {
        Err(errors) => invalid(&errors),
        Ok(v) => Json(ValidateWire {
            ok: true,
            changed: v.changed,
        })
        .into_response(),
    }
}

/// `POST /admin/api/config/secrets/clear` —— 显式清空一把凭据。
///
/// 🔴 **这是清空凭据的唯一入口**（`PUT` 里的空串一律是「不改」）。
pub async fn clear(State(st): State<AppState>, body: Bytes) -> Response {
    let o = match read_object(&body) {
        Ok(o) => o,
        Err(r) => return r,
    };
    if let Err(r) = only_keys(&o, &["path"]) {
        return r;
    }
    let secrets = secret_fields();
    let path = match o.get("path").and_then(Value::as_str) {
        Some(p) if secrets.contains(&p) => p.to_string(),
        _ => {
            return bad_request(format!(
                "path 只能是这几条凭据之一：{}",
                secrets.join(" / ")
            ));
        }
    };

    let env = admin_env();
    let stored = match read_stored(&st).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let Some(next) = clear_secret(stored.as_ref(), &path) else {
        // 上面已经查过，这一支走不到；留着是因为吞掉返回值会让将来某次改动静默失败。
        return bad_request("这条路径不是凭据");
    };

    // ⚠️⚠️ **清空之后这份配置还装载得起来吗 —— 在写之前算好。**
    // 第一版把这一步放在写之后、并且让回读抛出去 ⇒ `put` 已经发生、响应却是 500：
    // **面板说「保存失败」，而那把凭据已经被删掉了。**
    let next = Value::Object(next);
    let blocked_after = config_load_blockers(Some(&next), env);

    if let Err(r) = put_config(&st, next).await {
        return r;
    }
    st.config.invalidate();

    // 清完之后这一格还有没有值 —— **只看环境变量**（存储里那份刚被删掉）。
    let still_configured = env_name_of(&path, env)
        .and_then(|n| env.get(n))
        .is_some_and(|v| !v.is_empty());
    // ⚠️ **两档必须分开说**：清掉网关口令是「下一次冷启动会失败」，
    // 而清掉一把子系统凭据只是那个子系统停摆 —— 拿砖机的措辞去说一件不砖机的事，
    // 会让运维在一次小事故上按大事故处置。
    let fatal_after = blocked_after
        .iter()
        .any(|b| b.code == ConfigErrorCode::GatewayTokenRequired);

    // ⚠️ **审计先落，再回读**：回读出问题的那一支正是最该留痕的那一支。
    st.logger.log(
        LogEntry::new(
            if blocked_after.is_empty() {
                LogLevel::Warn
            } else {
                LogLevel::Error
            },
            EVENT_CONFIG_SECRET_CLEARED,
            if fatal_after {
                "面板清空了一把凭据，清完之后这份配置已经装载不起来了——当前进程靠上一份快照还能跑，\
                 但下一次冷启动会失败。请立刻在设置页里补回来。"
            } else if !blocked_after.is_empty() {
                "面板清空了一把凭据，某个子系统因此装不起来了——网关转发不受影响。请在设置页里补回来。"
            } else if still_configured {
                "面板清空了存储里的一把凭据；环境变量里仍然有一份，生效值不变"
            } else {
                "面板清空了一把凭据；这份配置仍然装载得起来"
            },
        )
        // **只记路径与机器可读的原因码，值一个字都不记。**
        .with("path", path.clone())
        .with("stillConfigured", still_configured)
        .with("blocked", blocked_ids(&blocked_after)),
    );

    let after = match read_all(&st).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    Json(ClearWire {
        cleared: path,
        still_configured,
        gateway_token_missing: fatal_after,
        load_blocked: wire_errors(&blocked_after),
        fields: after.fields,
        credentials: after.credentials,
        config_degraded: after.config_degraded,
        reset_blocked: wire_errors(&config_load_blockers(Some(&Value::Object(Map::new())), env)),
        propagation: PROPAGATION,
    })
    .into_response()
}

/// `POST /admin/api/config/reset` —— 危险区。
///
/// **重置的对象只有 `config` 这一把键**，其余业务键一个字节都不动。
///
/// ⚠️ **写回的是 `{}` 而不是 `delete`**：与「清空凭据」走同一条写路径，形态一致，
/// 且避开删除墓碑那一族问题。
/// ⚠️ 它与 `RESET_CONFIG=1` **不是一回事**：那个逃生口**只忽略不删**，
/// 存储里那份原值还在；这条端点是真的把它抹掉。
///
/// 🔴 **`confirm: true` 是必填的，不是装饰**：枚举式鉴权矩阵会拿正确的管理口令把
/// 每一条路由真的打一遍，一条不带请求体就会重置整份配置的端点会在那一格里
/// **把夹具自己的配置抹掉**。
pub async fn reset(State(st): State<AppState>, body: Bytes) -> Response {
    let o = match read_object(&body) {
        Ok(o) => o,
        Err(r) => return r,
    };
    if let Err(r) = only_keys(&o, &["confirm"]) {
        return r;
    }
    if o.get("confirm") != Some(&Value::Bool(true)) {
        return bad_request("这一步不可撤销，必须显式带 confirm: true");
    }

    let env = admin_env();
    let before = match read_all(&st).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    // 重置之后这份配置还装不装得起来 —— **在写之前算好**，与清空凭据那条同源。
    let blocked_after = config_load_blockers(Some(&Value::Object(Map::new())), env);

    if let Err(r) = put_config(&st, Value::Object(Map::new())).await {
        return r;
    }
    st.config.invalidate();

    st.logger.log(
        LogEntry::new(
            if blocked_after.is_empty() {
                LogLevel::Warn
            } else {
                LogLevel::Error
            },
            EVENT_CONFIG_RESET,
            if blocked_after.is_empty() {
                "面板重置了存储里那份配置；生效值回落到环境变量与内置默认值"
            } else {
                "面板重置了存储里那份配置，重置之后它已经装载不起来了——当前进程靠上一份快照还能跑，\
                 但下一次冷启动会失败。请立刻在设置页里把缺的那几格填回来。"
            },
        )
        .with("blocked", blocked_ids(&blocked_after)),
    );

    let after = match read_all(&st).await {
        Ok(s) => s,
        Err(r) => return r,
    };
    let changed = changed_effective(&before, &after);
    // 这次真的从「已配置」变成了「未配置」的那几把凭据。**只有路径，没有值。**
    // ⚠️ 它是从**回读出来**的两份 `credentials` 差出来的，不是手列一份 `SECRET_FIELDS`：
    // 环境变量里也提供了的那几把，重置之后仍然是「已配置」⇒ 不该出现在这份清单里。
    let credentials_changed = credentials_lost(&before, &after);

    Json(ResetWire {
        fields: after.fields,
        credentials: after.credentials,
        config_degraded: after.config_degraded,
        load_blocked: wire_errors(&after.load_blocked),
        changed,
        credentials_changed,
        reset_blocked: wire_errors(&blocked_after),
        applied_at: crate::adapters::now_ms(),
        propagation: PROPAGATION,
    })
    .into_response()
}

fn credentials_lost(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let (Some(b), Some(a)) = (before.credentials.as_ref(), after.credentials.as_ref()) else {
        return Vec::new();
    };
    let configured = |m: &Map<String, Value>, k: &str| {
        m.get(k)
            .and_then(|v| v.get("configured"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let mut out: Vec<String> = b
        .keys()
        .filter(|k| configured(b, k) && !configured(a, k))
        .cloned()
        .collect();
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, Counts, MemoryStorage};
    use crate::ports::storage::Storage;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::body::to_bytes;
    use serde_json::json;
    use std::sync::Arc;

    /// 判据二进制里**唯一**的那份环境变量夹具。
    ///
    /// 🔴 它必须与执行顺序无关（见 [`env_fallback`] 上那段）：全部 handler 判据
    /// 共用这一份，于是「谁先跑」不影响任何一格的期望值。
    ///
    /// 夹具里**刻意锁住两个字段**（`GATEWAY_TOKEN` / `MAX_STRIKES`），
    /// 这样「被锁」与「没被锁」两条路径在同一份夹具里都测得到。
    pub(super) fn fixture_env() -> EnvMap {
        let mut e = EnvMap::new();
        e.insert("MAX_STRIKES".into(), "5".into());
        e
    }

    /// 夹具存储里那把网关口令。
    ///
    /// ⚠️ **刻意放在存储里而不是 env 里**：放 env 会把 `gatewayToken` 变成
    /// **env 锁定**字段 ⇒ 凭据那一整族（长度下限、不得等于管理口令、空串 = 不改）
    /// 在 handler 这一层**结构性地够不着** —— 每一条都在更早的 `locked_by_env` 那里
    /// 就返回了。实测过：那种夹具下「干跑漏传管理口令」这个变异一格都不红。
    const STORED_GATEWAY_TOKEN: &str = "stored-gateway-token-0123456789";

    fn env_of(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn with_token(mut e: EnvMap) -> EnvMap {
        e.entry("GATEWAY_TOKEN".into())
            .or_insert_with(|| "t-abcdefghijklmnopqrstuvwx".into());
        e
    }

    async fn raw_body(res: Response) -> (StatusCode, String) {
        let status = res.status();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(b.to_vec()).unwrap())
    }

    async fn body_of(res: Response) -> (StatusCode, Value) {
        let (status, raw) = raw_body(res).await;
        (status, serde_json::from_str(&raw).unwrap_or(Value::Null))
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
        assert_eq!(got, want, "顶层键集不对");

        let mut pos = 0;
        for k in expected {
            let pat = format!("\"{k}\":");
            let at = raw[pos..]
                .find(&pat)
                .unwrap_or_else(|| panic!("{k} 没有出现在它该在的位置之后：{raw}"));
            pos += at + pat.len();
        }
    }

    /// **只数写侧那两个桶**（put / delete）。
    ///
    /// 读次数不在这一族判据的射程里 —— 「一个字节都不写」说的是写。
    fn writes(c: &Counts) -> (u64, u64) {
        let (_, put, delete, _) = c.snapshot();
        (put, delete)
    }

    /// 一套夹具：状态 + 直接可读写的存储 + **写次数计数器**。
    ///
    /// 🔴 计数器不是可有可无：本组最要紧的两格判据（「400 那一次一个字节都不写」、
    /// 「干跑零写」）**只有数 put 次数才测得出来** —— 只断言状态码抓不住
    /// 「先写了再返回 400」。
    async fn app() -> (TestApp, Arc<MemoryStorage>, Arc<Counts>) {
        let mem = Arc::new(MemoryStorage::new());
        // 存储里预置一把网关口令，理由见 [`STORED_GATEWAY_TOKEN`]。
        // ⚠️ 经 `mem` 直接写 ⇒ **不计进 `counts`**，写次数那一族判据的基线才是干净的。
        mem.put(
            CONFIG_KEY,
            json!({ "gatewayToken": STORED_GATEWAY_TOKEN }),
            None,
        )
        .await
        .unwrap();
        let counting = Arc::new(CountingStorage::new(mem.clone()));
        let counts = counting.counts.clone();
        (state_with_storage(counting).await, mem, counts)
    }

    fn codes(v: &Value) -> Vec<String> {
        v["errors"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|e| e["code"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── 字段表 ──────────────────────────────────────────────────────────────

    /// 字段表逐条钉死：**28 格，25 格可编辑，3 把凭据**。期望值手写字面量。
    ///
    /// 📜 这里曾钉着 12 格，并写着「少了注册机那 15 格，因为 `GatewayConfig` 里
    /// 真的没有那棵子树」。**子树有了自己的纯函数装载器之后那条理由不再成立**，
    /// 15 格补齐 —— 而「加回来必须有人在这里表态」这条规矩正是靠这一格执行的。
    #[test]
    fn the_field_table_is_pinned() {
        assert_eq!(FIELDS.len(), 28);
        assert_eq!(
            editable_fields(),
            vec![
                "agnesBaseUrl",
                "cooldownPaymentMs",
                "cooldownRateLimitMs",
                "cooldownStrikeMs",
                "gatewayToken",
                "maxStrikes",
                "poolCacheTtlMs",
                "poolTouchIntervalMs",
                "registrar.agnesPlatformUrl",
                "registrar.channel",
                "registrar.codeTimeoutMs",
                "registrar.enabled",
                "registrar.maxDomainAttempts",
                "registrar.mintBatch",
                "registrar.mintDelayMaxMs",
                "registrar.mintDelayMinMs",
                "registrar.moemail.apiKey",
                "registrar.moemail.baseUrl",
                "registrar.targetKeys",
                "registrar.tendIntervalMs",
                "registrar.tokenName",
                "registrar.yyds.apiKey",
                "registrar.yyds.baseUrl",
                "upstreamSyncTimeoutMs",
                "upstreamTimeoutMs",
            ]
        );
        // 🔴 与 02 §9.18 那份 `secrets` 逐字相同。
        assert_eq!(
            secret_fields(),
            vec![
                "gatewayToken",
                "registrar.moemail.apiKey",
                "registrar.yyds.apiKey"
            ]
        );
        // 只读的几格进四元组、但不可编辑。
        for ro in ["degraded", "usageStatsEnabled", "registrar.blocked"] {
            assert!(FIELDS.iter().any(|f| f.path == ro));
            assert!(!editable_fields().contains(&ro), "{ro} 不该可编辑");
        }
    }

    /// 🔴 **两个池子旋钮的下界是 0，别的整数字段是 1。**
    ///
    /// 回归对象：把 `maxStrikes` 的下界写成 0 等于跳过整个容错机制
    /// （`strikes >= 0` 在第一次失败时即成立）；把池子旋钮的下界写成 1
    /// 等于拿掉用户关闭缓存的逃生口。
    #[test]
    fn only_the_two_pool_knobs_accept_zero() {
        for f in &FIELDS {
            if let Spec::Int { min } = f.spec {
                let expected =
                    i64::from(!matches!(f.path, "poolCacheTtlMs" | "poolTouchIntervalMs"));
                assert_eq!(min, expected, "{} 的下界不对", f.path);
            }
        }
    }

    /// 每个字段的 env 名两两不重（一个环境变量不许锁两个字段）。
    #[test]
    fn env_names_are_unique_across_fields() {
        let mut seen: Vec<&str> = FIELDS.iter().flat_map(|f| f.env.iter().copied()).collect();
        let n = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "有环境变量名被两个字段共用");
    }

    /// `env_locked_fields` 报的是**字段路径**，且判据是「键存在」不是「值合法」。
    #[test]
    fn env_lock_is_about_presence_not_validity() {
        // 空串同样算锁定：它会走进装载器的 env 分支然后拒绝启动，
        // 面板必须显示它是锁定的，否则用户会一直改存储、一直没效果。
        let e = env_of(&[("MAX_STRIKES", ""), ("POOL_CACHE_TTL_MS", "0")]);
        assert_eq!(
            env_locked_fields(&e),
            vec!["maxStrikes".to_string(), "poolCacheTtlMs".to_string()]
        );
        assert!(env_locked_fields(&EnvMap::new()).is_empty());
    }

    // ── 校验：逐字段 ────────────────────────────────────────────────────────

    /// 拼错的字段名一律 `unknown_field`，**不静默丢弃**。
    ///
    /// 回归对象：宽松实现下那是一次「保存成功、什么都没发生」，而面板会如实显示成功。
    #[test]
    fn a_misspelled_field_is_refused_not_dropped() {
        let e = validate_config_patch(
            &json!({ "maxStrikess": 9 }),
            None,
            &with_token(EnvMap::new()),
            None,
        )
        .unwrap_err();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].field, "maxStrikess");
        assert_eq!(e[0].code, ConfigErrorCode::UnknownField);
    }

    /// 只读的两格也走 `unknown_field` —— 面板**不许**把红色横幅关掉。
    #[test]
    fn read_only_fields_cannot_be_written() {
        for f in ["degraded", "usageStatsEnabled"] {
            let e =
                validate_config_patch(&json!({ f: true }), None, &with_token(EnvMap::new()), None)
                    .unwrap_err();
            assert_eq!(e[0].code, ConfigErrorCode::UnknownField, "{f}");
        }
    }

    /// 🔴 **被 env 锁定的字段拒绝写入，且报的是 env 里实际存在的那个名字。**
    ///
    /// 回归对象：静默收下的后果是「面板显示保存成功、`stored` 真的变了、
    /// 而生效值纹丝不动」—— 运维会以为是缓存没刷，去等那 30 秒，然后再等一次。
    #[test]
    fn an_env_locked_field_is_refused_with_the_variable_name() {
        let env = with_token(env_of(&[("MAX_STRIKES", "9")]));
        let e = validate_config_patch(&json!({ "maxStrikes": 3 }), None, &env, None).unwrap_err();
        assert_eq!(e[0].code, ConfigErrorCode::LockedByEnv);
        assert_eq!(
            e[0].params.get("env").map(String::as_str),
            Some("MAX_STRIKES"),
            "要报名字，否则运维不知道去 compose 里找哪一行"
        );
        // 没锁的时候同一个 patch 照常通过。
        let ok = validate_config_patch(
            &json!({ "maxStrikes": 3 }),
            None,
            &with_token(EnvMap::new()),
            None,
        )
        .unwrap();
        assert_eq!(ok.changed, vec!["maxStrikes".to_string()]);
    }

    /// 整数字段：非整数 / 越界各自一条码，越界那条要把下界说出来。
    #[test]
    fn integer_fields_report_their_own_shape() {
        let env = with_token(EnvMap::new());
        for (v, code) in [
            (json!("9"), ConfigErrorCode::NotAnInteger),
            (json!(1.5), ConfigErrorCode::NotAnInteger),
            (json!(true), ConfigErrorCode::NotAnInteger),
            (json!(null), ConfigErrorCode::NotAnInteger),
            (json!(0), ConfigErrorCode::BelowMin),
            (json!(-1), ConfigErrorCode::BelowMin),
        ] {
            let e =
                validate_config_patch(&json!({ "maxStrikes": v }), None, &env, None).unwrap_err();
            assert_eq!(e[0].code, code, "maxStrikes={v}");
        }
        let e = validate_config_patch(&json!({ "maxStrikes": 0 }), None, &env, None).unwrap_err();
        assert_eq!(e[0].params.get("min").map(String::as_str), Some("1"));

        // 池子旋钮的 0 是逃生口，不是越界值。
        assert!(validate_config_patch(&json!({ "poolCacheTtlMs": 0 }), None, &env, None).is_ok());
    }

    /// URL 字段：只认 `http(s)://`。
    ///
    /// 🔴 `javascript:` / `file:` 在一个「转发目标」上没有任何正当含义，
    /// 而这个字段会被真的拿去发请求。
    #[test]
    fn url_fields_only_accept_http_and_https() {
        let env = with_token(EnvMap::new());
        for (v, code) in [
            (json!(42), ConfigErrorCode::NotAString),
            (json!(""), ConfigErrorCode::Empty),
            (json!("javascript:alert(1)"), ConfigErrorCode::NotAUrl),
            (json!("file:///etc/passwd"), ConfigErrorCode::NotAUrl),
            (json!("ftp://example.invalid"), ConfigErrorCode::NotAUrl),
            (json!("https://"), ConfigErrorCode::NotAUrl),
            (json!("https://a b"), ConfigErrorCode::NotAUrl),
        ] {
            let e =
                validate_config_patch(&json!({ "agnesBaseUrl": v }), None, &env, None).unwrap_err();
            assert_eq!(e[0].code, code, "agnesBaseUrl={v}");
        }
        assert!(
            validate_config_patch(
                &json!({ "agnesBaseUrl": "https://example.invalid/v1" }),
                None,
                &env,
                None
            )
            .is_ok()
        );
    }

    // ── 校验：凭据 ──────────────────────────────────────────────────────────

    /// 🔴🔴 **凭据传空串是「不改」，不是清空。**
    ///
    /// 回归对象（实测）：把空串实现成清空之后，运维保存一次设置页就抹掉
    /// `gatewayToken`，而热实例因为持有者保留上一份快照当场看不出任何异常。
    ///
    /// ⚠️ 判据必须连**纯空白**一起覆盖：`"   "` 在「只判 `== \"\"`」的实现下会被
    /// 收下并落盘，而装载器认为它「有口令」⇒ 一次都不 fail-fast，
    /// 面板还显示 `configured: true`，所有下游用户从此 401。
    #[test]
    fn an_empty_secret_means_unchanged_not_cleared() {
        let stored = json!({ "gatewayToken": "kept-token-0123456789abcdef" });
        for raw in ["", "   ", "\t\n"] {
            let v = validate_config_patch(
                &json!({ "gatewayToken": raw }),
                Some(&stored),
                &EnvMap::new(),
                None,
            )
            .expect("空凭据是「不改」，不该报错");
            assert!(v.changed.is_empty(), "{raw:?} 不该算改过");
            assert_eq!(
                v.next.get("gatewayToken").and_then(Value::as_str),
                Some("kept-token-0123456789abcdef"),
                "{raw:?} 绝不许把原口令抹掉"
            );
        }
    }

    /// 凭据的四条硬规则各自一条码，**顺序就是这个顺序**。
    #[test]
    fn secret_rules_fire_in_order() {
        let long = "a".repeat(MIN_GATEWAY_TOKEN_LENGTH);
        for (raw, code) in [
            (format!(" {long} "), ConfigErrorCode::WhitespacePadded),
            (format!("{long}中"), ConfigErrorCode::NotSendable),
            ("short".to_string(), ConfigErrorCode::TooShort),
        ] {
            let e =
                validate_config_patch(&json!({ "gatewayToken": raw }), None, &EnvMap::new(), None)
                    .unwrap_err();
            assert_eq!(e[0].code, code, "gatewayToken={raw:?}");
        }
        let e = validate_config_patch(&json!({ "gatewayToken": 42 }), None, &EnvMap::new(), None)
            .unwrap_err();
        assert_eq!(e[0].code, ConfigErrorCode::NotAString);
    }

    /// 🔴 **网关口令不得等于管理口令 —— 面板会把自己锁死。**
    ///
    /// 实测：设成相等之后 `GET /config` 503、想改回去的 `PUT` 也 503。
    /// ⚠️ 而**不给 `admin_token` 时这一条静默跳过**，不是假装查过。
    #[test]
    fn the_gateway_token_may_not_equal_the_admin_token() {
        let token = "same-token-0123456789abcdef";
        let e = validate_config_patch(
            &json!({ "gatewayToken": token }),
            None,
            &EnvMap::new(),
            Some(token),
        )
        .unwrap_err();
        assert_eq!(e[0].code, ConfigErrorCode::SameAsAdminToken);

        assert!(
            validate_config_patch(
                &json!({ "gatewayToken": token }),
                None,
                &EnvMap::new(),
                None
            )
            .is_ok(),
            "不给管理口令时这一条跳过，不是假装查过"
        );
    }

    // ── 校验：跨字段 ────────────────────────────────────────────────────────

    /// 🔴 **只拒这次补丁新引入的 blocker，本来就有的不拦人。**
    ///
    /// 回归对象：拿「补丁之后还有没有 blocker」当判据的话，那份配置本来就坏掉时，
    /// 运维想改一个无关字段也会被 400 拒 —— 而他最需要的恰恰是一步一步修回来。
    #[test]
    fn only_newly_introduced_blockers_are_refused() {
        // 两边都没有口令 ⇒ 本来就坏着。改一个无关字段照样放行。
        let v = validate_config_patch(&json!({ "maxStrikes": 7 }), None, &EnvMap::new(), None)
            .expect("本来就坏着的配置必须还能一步一步修");
        assert_eq!(v.changed, vec!["maxStrikes".to_string()]);

        // 而把口令真的填回去也放行（那是修好的方向）。
        assert!(
            validate_config_patch(
                &json!({ "gatewayToken": "brand-new-token-0123456789" }),
                None,
                &EnvMap::new(),
                None
            )
            .is_ok()
        );
    }

    /// 一份**部分合法**的 patch：只报坏的那一格，好的那一格不进错误清单。
    ///
    /// 面板拿 `errors[].field` 去逐格加红框 —— 把没问题的那一格也报进来，
    /// 运维会去改一个本来就对的值。
    ///
    /// ⚠️⚠️ **它不覆盖「逐字段失败时不跑跨字段规则」那一行**（
    /// [`validate_config_patch`] 里的早退），如实登记：跨字段阶段本轮只产得出
    /// `gateway_token_required` 一条，而它只在「补丁**新引入**了它」时才追加，
    /// 补丁又没有能把口令删掉的路径 ⇒ 删掉那一行，任何输入的结果都一模一样
    /// （变异复检实测过：删掉早退，这一格仍绿）。
    /// 那一行的第一个真实用户是注册机那族跨字段规则（`channel_required` /
    /// `delay_min_gt_max` / …），**它们落地的同一轮必须补一格**
    /// 「`targetKeys: "abc"` 不许连带报出一条 `channel_required`」。
    #[test]
    fn only_the_offending_field_is_reported() {
        let e = validate_config_patch(
            &json!({ "maxStrikes": "x", "cooldownPaymentMs": 5 }),
            None,
            &EnvMap::new(),
            None,
        )
        .unwrap_err();
        assert_eq!(e.len(), 1, "好的那一格不该也进错误清单：{e:?}");
        assert_eq!(e[0].field, "maxStrikes");
        assert_eq!(e[0].code, ConfigErrorCode::NotAnInteger);
    }

    /// 本轮**发得出去的码是这 12 条**，期望值手写。
    ///
    /// ⚠️ 它钉的是「哪些码今天有真实出口」：`ConfigErrorCode` 全表 22 条，
    /// 剩下的 10 条属于注册机子树（缺口已登记）。加一条码而没有出口，
    /// 等于面板字典里多一条永远选不到的文案 —— 那必须有人在这里表态。
    #[test]
    fn the_reachable_code_set_is_pinned() {
        let reachable = [
            ConfigErrorCode::UnknownField,
            ConfigErrorCode::LockedByEnv,
            ConfigErrorCode::NotAnInteger,
            ConfigErrorCode::BelowMin,
            ConfigErrorCode::NotAString,
            ConfigErrorCode::Empty,
            ConfigErrorCode::NotAUrl,
            ConfigErrorCode::WhitespacePadded,
            ConfigErrorCode::NotSendable,
            ConfigErrorCode::TooShort,
            ConfigErrorCode::SameAsAdminToken,
            ConfigErrorCode::GatewayTokenRequired,
            ConfigErrorCode::ConfigUnloadable,
        ];
        assert_eq!(reachable.len(), 13);
        assert_eq!(
            ConfigErrorCode::ALL.len(),
            22,
            "全表没变的话，缺的就是注册机那族"
        );
    }

    // ── 四元组 ──────────────────────────────────────────────────────────────

    /// 🔴 **`effective` 来自真正的装载器 —— 存储值非法时它报的是默认值。**
    ///
    /// 回归对象：面板层照「有 env 就显示 env，否则显示存储」再推一遍的话，
    /// 这一格会把存储里那个**非法值**显示成「生效值」，而网关实际在用默认值。
    /// **两份实现之间的差，恰好落在最需要面板说实话的那一格上。**
    #[test]
    fn effective_comes_from_the_loader_not_from_a_second_derivation() {
        let stored = json!({ "maxStrikes": "九", "gatewayToken": "t-abcdefghijklmnopqrstuvwx" });
        let (snap, _) = snapshot_of(Some(&stored), &EnvMap::new());
        let f = snap.fields.unwrap();
        assert_eq!(f["maxStrikes"]["stored"], json!("九"), "原件如实报");
        assert_eq!(
            f["maxStrikes"]["effective"],
            json!(3),
            "生效值是默认值，不是存储里那个非法值"
        );
        assert_eq!(f["degraded"]["effective"], json!(true), "这次装载降级了");
    }

    /// 四元组四格齐全，`lockedBy` 报 `env:<名字>`。
    #[test]
    fn the_quad_has_all_four_slots() {
        let env = env_of(&[
            ("GATEWAY_TOKEN", "t-abcdefghijklmnopqrstuvwx"),
            ("MAX_STRIKES", "9"),
        ]);
        let stored = json!({ "maxStrikes": 4, "cooldownPaymentMs": 111_000 });
        let (snap, _) = snapshot_of(Some(&stored), &env);
        let f = snap.fields.unwrap();
        assert_eq!(
            f["maxStrikes"],
            json!({ "stored": 4, "env": "9", "effective": 9, "lockedBy": "env:MAX_STRIKES" })
        );
        assert_eq!(
            f["cooldownPaymentMs"],
            json!({ "stored": 111_000, "env": null, "effective": 111_000, "lockedBy": null })
        );
    }

    /// 🔴 **凭据只报 `{configured, hint, lockedBy}`，明文一个字节都不出现。**
    #[test]
    fn credentials_never_carry_the_plaintext() {
        let env = EnvMap::new();
        let stored = json!({ "gatewayToken": "supersecret-0123456789wxyz" });
        let (snap, _) = snapshot_of(Some(&stored), &env);
        let c = snap.credentials.unwrap();
        assert_eq!(
            c["gatewayToken"],
            json!({ "configured": true, "hint": "wxyz", "lockedBy": null })
        );
        let raw = serde_json::to_string(&c).unwrap();
        assert!(!raw.contains("supersecret"), "凭据明文漏进响应体了：{raw}");
        // 公开字段那一块也不许有它。
        let raw = serde_json::to_string(&snap.fields.unwrap()).unwrap();
        assert!(!raw.contains("supersecret"));
    }

    /// 🔴 **`hint` 在原值短于 5 位时是 `null`** —— 给了就是给全，那就不叫 hint 了。
    #[test]
    fn a_short_secret_gets_no_hint() {
        assert_eq!(hint_of("abcd"), None);
        assert_eq!(hint_of("abcde").as_deref(), Some("bcde"));
        assert!(hint_of("").is_none());
    }

    /// 装载不起来 ⇒ **诊断视图**（两块 `null` + 至少一条 blocker），**不是 500**。
    ///
    /// 🔴 这是运维唯一的出路：在它之前，`GET` 与 `PUT` 全是 500，
    /// 于是「把口令重新填回去」这条自救路径自己也走不通。
    #[test]
    fn an_unloadable_config_degrades_to_a_diagnostic_view() {
        let (snap, unloadable) = snapshot_of(None, &EnvMap::new());
        assert!(snap.fields.is_none() && snap.credentials.is_none());
        assert!(snap.config_degraded);
        assert_eq!(snap.load_blocked.len(), 1);
        assert_eq!(
            snap.load_blocked[0].code,
            ConfigErrorCode::GatewayTokenRequired
        );
        assert!(
            unloadable.is_none(),
            "说得出是哪一格时不该再兜一条 config_unloadable"
        );
    }

    /// ⚠️ `RESET_CONFIG=1` 时四元组的 `stored` 那一格也跟着显示「没有」。
    ///
    /// 装载器**完全忽略**存储里那份，照旧把它显示成「存储里配的是它」就是在撒谎。
    #[test]
    fn the_escape_hatch_also_hides_the_stored_slot() {
        let env = env_of(&[
            ("GATEWAY_TOKEN", "t-abcdefghijklmnopqrstuvwx"),
            ("RESET_CONFIG", "1"),
        ]);
        let stored = json!({ "maxStrikes": 9 });
        let (snap, _) = snapshot_of(Some(&stored), &env);
        let f = snap.fields.unwrap();
        assert_eq!(f["maxStrikes"]["stored"], json!(null));
        assert_eq!(f["maxStrikes"]["effective"], json!(3));
    }

    // ── 端点 ────────────────────────────────────────────────────────────────

    /// `GET` 的顶层九格逐格在。
    #[tokio::test]
    async fn get_returns_the_whole_contract() {
        let (TestApp { state, .. }, _, _) = app().await;
        let (status, raw) = raw_body(get(State(state)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_keys_in_order(
            &raw,
            &[
                "fields",
                "credentials",
                "configDegraded",
                "loadBlocked",
                "loadNotices",
                "editable",
                "secrets",
                "resetBlocked",
                "propagation",
            ],
        );
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            v["secrets"],
            json!([
                "gatewayToken",
                "registrar.moemail.apiKey",
                "registrar.yyds.apiKey"
            ])
        );
        assert_eq!(v["fields"]["maxStrikes"]["lockedBy"], "env:MAX_STRIKES");
    }

    /// 🔴 **`propagation` 就是 `{configTtlMs, visibilityUpperBoundMs}` 两格 30000。**
    ///
    /// 回归对象：上一版加过一个「KV 边缘缓存 60 秒」⇒ 保存回执里那个数比事实多 60 秒。
    #[tokio::test]
    async fn propagation_is_exactly_the_config_ttl() {
        let (TestApp { state, .. }, _, _) = app().await;
        let (_, v) = body_of(get(State(state)).await).await;
        assert_eq!(
            v["propagation"],
            json!({ "configTtlMs": 30_000, "visibilityUpperBoundMs": 30_000 })
        );
    }

    /// 🔴🔴 **校验失败的那一次，`put` 计数一动不动。**
    ///
    /// 只断言 400 抓不住「先写了再返回 400」——
    /// 而那个形态的后果是：一份非法配置已经落盘，运维照着 400 以为没保存上，
    /// 下一次冷启动网关起不来。
    #[tokio::test]
    async fn a_rejected_patch_writes_nothing() {
        let (TestApp { state, .. }, _mem, counts) = app().await;
        let before = writes(&counts);
        let (status, v) = body_of(
            put(
                State(state),
                Bytes::from(r#"{"patch":{"cooldownPaymentMs":0}}"#),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(codes(&v), vec!["below_min"]);
        assert_eq!(
            v["error"]["message"], "配置校验没通过，本次一个字节都没有写入",
            "这句话必须与事实一致"
        );
        assert_eq!(writes(&counts), before, "400 那一次一个字节都不许写");
    }

    /// 顶层拼错一个字段名 ⇒ 400，**不带 `code`**（网关信封），且零写。
    #[tokio::test]
    async fn an_unknown_top_level_field_is_refused_without_a_code() {
        let (TestApp { state, .. }, _mem, counts) = app().await;
        let before = writes(&counts);
        let (status, v) = body_of(
            put(
                State(state),
                Bytes::from(r#"{"patchs":{"cooldownPaymentMs":9}}"#),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(v["error"].get("code").is_none(), "这一档刻意不带码：{v}");
        assert!(
            v["error"]["message"].as_str().unwrap().contains("patchs"),
            "要说出是哪个字段：{v}"
        );
        assert_eq!(writes(&counts), before);
    }

    /// 畸形 JSON 那一档**带 `code`**：它在设置页上是看得见的。
    #[tokio::test]
    async fn malformed_json_carries_a_code() {
        let (TestApp { state, .. }, _, _) = app().await;
        let (status, v) = body_of(put(State(state), Bytes::from("{not json")).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["code"], "bad_json");
    }

    /// ⚠️ **保存成功之后同一个进程立刻看得见新值** —— 观测点走真持有者。
    ///
    /// 少了 `invalidate()`，概览页会在最多一个 `CONFIG_TTL_MS` 内继续报旧值，
    /// 而运维刚刚才看到保存回执上是新值。
    #[tokio::test]
    async fn a_successful_save_is_visible_to_the_next_request_in_this_process() {
        let (TestApp { state, .. }, mem, _) = app().await;
        // ⚠️ **`now` 必须落在 TTL 之内**：往回拨的时钟（比如传 0）会让
        //    `needs_reload` 无条件为真 ⇒ 这一格对「有没有 invalidate」完全看不见，
        //    是一格永远绿的假判据（实测过）。
        let at = crate::adapters::now_ms();
        assert_eq!(state.config.current().cooldown_payment_ms, 3_600_000);

        // ── 控制组：绕过面板直接改存储 ⇒ TTL 之内**本来就看不见** ──────────────
        mem.put(
            CONFIG_KEY,
            json!({ "gatewayToken": STORED_GATEWAY_TOKEN, "cooldownPaymentMs": 555_000 }),
            None,
        )
        .await
        .unwrap();
        state.config.ensure_fresh(at).await;
        assert_eq!(
            state.config.current().cooldown_payment_ms,
            3_600_000,
            "夹具前提：TTL 之内本来就看不见别人的写"
        );

        // ── 走一次面板保存 ⇒ 同一个进程的下一个请求必须立刻看见 ────────────────
        let (status, v) = body_of(
            put(
                State(state.clone()),
                Bytes::from(r#"{"patch":{"cooldownPaymentMs":111000}}"#),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["changed"], json!(["cooldownPaymentMs"]));
        assert_eq!(v["fields"]["cooldownPaymentMs"]["effective"], 111_000);

        state.config.ensure_fresh(at).await;
        assert_eq!(
            state.config.current().cooldown_payment_ms,
            111_000,
            "少了 invalidate() 这里还会是旧值"
        );
    }

    /// 🔴 **回执是从存储回读出来的，不是把 handler 手上那份 `next` 换个形状交回去。**
    ///
    /// 观察点：写完之后**把存储里那份换掉**，回执必须报出被换掉的那一份。
    #[tokio::test]
    async fn the_receipt_is_read_back_from_storage() {
        // ⚠️ 观察点必须**能把两者分开**：`verdict.next` = 存储原件 + patch，
        //    所以「回执里有这次没碰的那些字段」对两种实现都成立 —— 那样的判据是假的。
        //    唯一分得开的观察点是：**写完之后让存储里那份变成别的东西**，
        //    回执必须报出被换掉的那一份。
        let swap = Arc::new(SwapAfterPut::new(json!({
            // 口令留着，否则回读会降级成诊断视图（`fields` 为 null），这一格就测不到东西。
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "cooldownPaymentMs": 4242,
        })));
        let TestApp { state, .. } = state_with_storage(swap).await;
        let (status, v) = body_of(
            put(
                State(state),
                Bytes::from(r#"{"patch":{"cooldownRateLimitMs":7}}"#),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            v["fields"]["cooldownPaymentMs"]["stored"], 4242,
            "回执必须是**回读**出来的那一份，不是 handler 手上那份 next：{v}"
        );
        assert_eq!(
            v["fields"]["cooldownRateLimitMs"]["stored"],
            Value::Null,
            "被换掉的那份里没有它 —— 报 7 就说明回执是 handler 自己拼的：{v}"
        );
    }

    /// 写之后读回来的东西**变了**的存储。
    ///
    /// 存在的唯一理由见上一格：要把「从存储回读」与「把自己算出来那份换个形状交回去」
    /// 分开，观测量只能是「写完之后存储里那份被换掉了」。
    struct SwapAfterPut {
        inner: MemoryStorage,
        swapped: std::sync::atomic::AtomicBool,
        after: Value,
    }

    impl SwapAfterPut {
        fn new(after: Value) -> Self {
            Self {
                inner: MemoryStorage::new(),
                swapped: std::sync::atomic::AtomicBool::new(false),
                after,
            }
        }
    }

    #[async_trait::async_trait]
    impl Storage for SwapAfterPut {
        async fn get(&self, k: &str) -> crate::ports::storage::StorageResult<Option<Value>> {
            if k == CONFIG_KEY && self.swapped.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(Some(self.after.clone()));
            }
            self.inner.get(k).await
        }
        async fn put(
            &self,
            k: &str,
            v: Value,
            e: Option<i64>,
        ) -> crate::ports::storage::StorageResult<()> {
            if k == CONFIG_KEY {
                self.swapped
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.inner.put(k, v, e).await
        }
        async fn delete(&self, k: &str) -> crate::ports::storage::StorageResult<()> {
            self.inner.delete(k).await
        }
        async fn list(&self, p: &str) -> crate::ports::storage::StorageResult<Vec<String>> {
            self.inner.list(p).await
        }
    }

    /// 干跑与真跑对同一份输入给出**同一组错误码**，且干跑**零写**。
    ///
    /// ⚠️ 回归对象：干跑少传一个上下文（如管理口令）时，分叉方向是
    /// 「干跑放行、真跑拒绝」—— 运维读到的是「面板刚说没问题」。
    #[tokio::test]
    async fn the_dry_run_agrees_with_the_real_one() {
        let (TestApp { mut state, .. }, _mem, counts) = app().await;
        state.admin_token = Some(Arc::new("admin-token-0123456789abcdef".into()));

        for patch in [
            r#"{"patch":{"cooldownPaymentMs":0}}"#,
            r#"{"patch":{"gatewayToken":"admin-token-0123456789abcdef"}}"#,
            r#"{"patch":{"nope":1}}"#,
        ] {
            let before = writes(&counts);
            let (s1, v1) = body_of(validate(State(state.clone()), Bytes::from(patch)).await).await;
            assert_eq!(writes(&counts), before, "干跑一次写都不许有：{patch}");
            let (s2, v2) = body_of(put(State(state.clone()), Bytes::from(patch)).await).await;
            assert_eq!(s1, s2, "{patch}");
            assert_eq!(codes(&v1), codes(&v2), "{patch}");
        }
    }

    /// 干跑通过时回 `{ok:true, changed}`，**零写**。
    #[tokio::test]
    async fn a_passing_dry_run_reports_what_would_change() {
        let (TestApp { state, .. }, _mem, counts) = app().await;
        let before = writes(&counts);
        let (status, v) = body_of(
            validate(
                State(state),
                Bytes::from(r#"{"patch":{"cooldownStrikeMs":7,"cooldownPaymentMs":1}}"#),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            v,
            json!({ "ok": true, "changed": ["cooldownPaymentMs", "cooldownStrikeMs"] })
        );
        assert_eq!(writes(&counts), before);
    }

    /// `secrets/clear` 真的把存储里那把删掉，且回执如实报后果。
    ///
    /// ⚠️ 这一格的夹具里 env **有** `GATEWAY_TOKEN` ⇒ `stillConfigured` 为真、
    /// 清完之后照样装得起来 —— 那正是「环境变量提供口令的部署想清掉存储里那份多余
    /// 旧口令」这条正当路径。
    #[tokio::test]
    async fn clearing_a_secret_removes_it_and_reports_the_consequence() {
        let (TestApp { state, .. }, mem, _counts) = app().await;

        let (status, v) =
            body_of(clear(State(state), Bytes::from(r#"{"path":"gatewayToken"}"#)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["cleared"], "gatewayToken");

        // 🔴 **存储里那把必须真的没了** —— 不删而回 200 是「面板说做了、其实没做」。
        let left = mem.get(CONFIG_KEY).await.unwrap().unwrap();
        assert!(
            left.get("gatewayToken").is_none(),
            "存储里那把必须真的没了：{left}"
        );

        // ⚠️ **后果如实进响应体**：夹具的 env 里没有 `GATEWAY_TOKEN`，
        // 清完之后这份配置就装不起来了 —— 这是那条 `error` 级事件与面板红色警告
        // 要说的**砖机档**，🔴 不许与「只是某个子系统停摆」混成一句话。
        assert_eq!(v["stillConfigured"], false, "env 里也没有第二份");
        assert_eq!(v["gatewayTokenMissing"], true);
        assert_eq!(
            v["loadBlocked"],
            json!([{ "field": "gatewayToken", "code": "gateway_token_required" }]),
            "清完之后装不起来这件事必须如实报出来：{v}"
        );
        // 而且**照样清**：这是一条显式动作，不替运维做主。
        assert!(v["fields"].is_null(), "此刻已降级成诊断视图：{v}");
    }

    /// `path` 不是凭据 ⇒ 400，**不带 `code`**，且零写。
    #[tokio::test]
    async fn clearing_a_non_secret_path_is_refused() {
        let (TestApp { state, .. }, _mem, counts) = app().await;
        let before = writes(&counts);
        for body in [
            r#"{"path":"maxStrikes"}"#,
            r#"{"path":42}"#,
            r#"{"path":"gatewayToken","extra":1}"#,
        ] {
            let (status, v) = body_of(clear(State(state.clone()), Bytes::from(body)).await).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert!(v["error"].get("code").is_none(), "{body}");
        }
        assert_eq!(writes(&counts), before);
    }

    /// 🔴 **重置必须显式带 `confirm: true`**，否则 400 且零写。
    ///
    /// 回归对象：枚举式鉴权矩阵会拿正确的管理口令把每条路由真打一遍，
    /// 一条不带请求体就重置整份配置的端点会在那一格里**把夹具自己的配置抹掉**。
    #[tokio::test]
    async fn reset_demands_an_explicit_confirmation() {
        let (TestApp { state, .. }, mem, counts) = app().await;
        mem.put(CONFIG_KEY, json!({ "maxStrikes": 9 }), None)
            .await
            .unwrap();
        let before = writes(&counts);
        for body in ["{}", r#"{"confirm":false}"#, r#"{"confirm":"true"}"#, ""] {
            let (status, _) = body_of(reset(State(state.clone()), Bytes::from(body)).await).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
        }
        assert_eq!(writes(&counts), before, "没确认就一个字节都不许写");
        assert_eq!(mem.get(CONFIG_KEY).await.unwrap().unwrap()["maxStrikes"], 9);
    }

    /// 重置把 `config` 整把写回 `{}`，回执里的 `changed` 报的是**真的变了的生效值**。
    #[tokio::test]
    async fn reset_wipes_the_config_key_and_reports_the_real_diff() {
        let (TestApp { state, .. }, mem, _counts) = app().await;
        mem.put(
            CONFIG_KEY,
            json!({
                "gatewayToken": STORED_GATEWAY_TOKEN,
                "maxStrikes": 9,
                "cooldownPaymentMs": 111_000,
            }),
            None,
        )
        .await
        .unwrap();

        let (status, v) =
            body_of(reset(State(state), Bytes::from(r#"{"confirm":true}"#)).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            mem.get(CONFIG_KEY).await.unwrap().unwrap(),
            json!({}),
            "重置写回的是 {{}}，不是删键"
        );

        // ⚠️ **二次确认框里那句话的依据**：夹具的 env 里没有 `GATEWAY_TOKEN`，
        // 所以「重置之后这份配置装不起来」必须在**写之前**就算出来并如实报。
        assert_eq!(
            v["resetBlocked"],
            json!([{ "field": "gatewayToken", "code": "gateway_token_required" }]),
            "按下去会变成什么样，必须说清：{v}"
        );
        // ⚠️ 重置之后装不起来 ⇒ **没有「生效值」这个东西可比** ⇒ 空清单，
        // 而不是编一份差异出来。面板此时靠 `loadBlocked` 说话，不靠高亮。
        assert_eq!(v["changed"], json!([]));
        assert_eq!(
            v["credentialsChanged"],
            json!([]),
            "两侧任一是诊断视图时就没有可比的凭据状态"
        );
    }

    /// 🔴 **`changed` 按回读出来的生效值比，不是按请求体比。**
    ///
    /// 观察点挑的是唯一分得开两者的那一种输入：**写一个与当前生效值相同的值**。
    /// 存储里原本没有它（所以「按请求体比」会说改了），而生效值纹丝不动
    /// （所以「按生效值比」说没改）—— 后者才是真的，面板高亮的必须是真的变了的那些。
    #[tokio::test]
    async fn changed_is_computed_from_effective_values_not_from_the_patch() {
        let (TestApp { state, .. }, _, _) = app().await;
        let (status, v) = body_of(
            put(
                State(state),
                Bytes::from(r#"{"patch":{"cooldownPaymentMs":3600000}}"#),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["changed"], json!([]), "生效值纹丝不动，不许高亮：{v}");
        assert_eq!(
            v["fields"]["cooldownPaymentMs"]["stored"], 3_600_000,
            "但它确实落盘了 —— 「没变」说的是生效值，不是「没写」"
        );
    }

    /// 危险区那条路径的常量与子树内路径**是同一条**。
    ///
    /// 🔴 五语言 DEPLOY.md 逐份写着完整路径，手抄的那份迟早与注册路径漂开，
    /// 而漂开的现象是「文档里那条危险区路径 404」。
    #[test]
    fn the_reset_path_constant_is_the_single_source() {
        assert_eq!(
            CONFIG_RESET_PATH,
            format!(
                "{}{}",
                crate::admin::keys::ADMIN_API_PREFIX,
                CONFIG_RESET_SUBPATH
            )
        );
    }

    /// 存储读失败 ⇒ **500 + 一条事件**，🔴 **不静默**。
    #[tokio::test]
    async fn a_storage_failure_is_a_500_that_leaves_a_trace() {
        let (
            TestApp {
                state, recorded, ..
            },
            mem,
            _,
        ) = app().await;
        mem.fail_next("EIO");
        let (status, v) = body_of(get(State(state)).await).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(v["error"]["message"], "网关内部错误", "不回显内部细节");
        assert!(
            recorded
                .events()
                .contains(&crate::admin::keys::EVENT_ADMIN_STORAGE_FAILED),
            "500 不许静默：{:?}",
            recorded.events()
        );
    }

    /// 🔴 **`config.updated` 事件里只有路径，一个值都没有。**
    ///
    /// 日志常被转发到第三方，而这批路径里有凭据。
    #[tokio::test]
    async fn the_audit_event_records_paths_and_never_values() {
        let (
            TestApp {
                state, recorded, ..
            },
            _,
            _,
        ) = app().await;
        let (status, _) = body_of(
            put(
                State(state),
                // 夹具里 `MAX_STRIKES=5` 锁着这一格 ⇒ 这一次必然是 400。
                Bytes::from(r#"{"patch":{"maxStrikes":9}}"#),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            !recorded.events().contains(&EVENT_CONFIG_UPDATED),
            "被拒的那次不该留下「改过配置」的事件"
        );

        let (
            TestApp {
                state, recorded, ..
            },
            _,
            _,
        ) = app().await;
        let _ = put(
            State(state),
            Bytes::from(r#"{"patch":{"cooldownStrikeMs":123000}}"#),
        )
        .await;
        let e = recorded
            .entries()
            .into_iter()
            .find(|e| e.event == EVENT_CONFIG_UPDATED)
            .expect("改过配置必须留痕");
        assert_eq!(e.fields["fields"], json!("cooldownStrikeMs"));
        assert_eq!(e.fields["secrets"], json!(0));
        let raw = format!("{:?}", e.fields);
        assert!(!raw.contains("123000"), "事件里不许出现值：{raw}");
    }

    // ── 注册机那 15 格 ────────────────────────────────────────────────────
    //
    // 它们与别的字段**走同一套机器**：同一张 `FIELDS`、同一条 `check_leaf`、
    // 同一个「合并前后 blocker 差集」。这一组钉的是「接上去了、而且没有为它们
    // 另写一套」。

    /// 🔴 **`effective` 是拿真正的装载器跑出来的，不是在面板层另推一遍优先级。**
    ///
    /// 另推一遍的形态是：面板上显示的「生效值」与注册机真正在用的那份各自演化，
    /// **而两边都「能跑」**，直到有人对着面板调了一整天参数却毫无效果。
    #[test]
    fn the_registrar_effective_values_come_from_the_real_loader() {
        let mut env = with_token(EnvMap::new());
        // env 压过存储 —— 这条优先级只有装载器知道。
        env.insert("REGISTRAR_TARGET_KEYS".into(), "7".into());
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "targetKeys": "99", "mintBatch": "3" },
        });
        let (snap, _) = snapshot_of(Some(&stored), &env);
        let f = snap.fields.expect("装得起来");
        assert_eq!(
            f["registrar.targetKeys"]["effective"], 7,
            "🔴 显示的是存储里那个 99 —— 面板层自己推了一遍优先级",
        );
        assert_eq!(
            f["registrar.targetKeys"]["stored"], "99",
            "存储那一格照实说"
        );
        assert_eq!(
            f["registrar.targetKeys"]["lockedBy"], "env:REGISTRAR_TARGET_KEYS",
            "被 env 钉死了就要说出来，否则面板会让人改一个改不动的字段",
        );
        assert_eq!(
            f["registrar.mintBatch"]["effective"], 3,
            "没被 env 钉的照存储走"
        );
    }

    /// 🔴 **凭据那两把永不报值，只报「配没配 + 末 4 位」。**
    #[test]
    fn the_registrar_credentials_never_report_their_value() {
        let env = with_token(EnvMap::new());
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "yyds": { "apiKey": "sk-super-secret-1234" } },
        });
        let (snap, _) = snapshot_of(Some(&stored), &env);
        let raw = serde_json::to_string(&snap.credentials).unwrap();
        assert!(
            !raw.contains("sk-super-secret-1234"),
            "🔴 凭据明文进了配置面：{raw}",
        );
        let c = snap.credentials.unwrap();
        assert_eq!(c["registrar.yyds.apiKey"]["configured"], true);
        assert_eq!(c["registrar.yyds.apiKey"]["hint"], "1234", "只给末 4 位");
    }

    /// 布尔只认真正的布尔 —— 收字符串会让两条解析路径悄悄分岔。
    #[test]
    fn the_enabled_switch_only_accepts_a_real_boolean() {
        let env = with_token(EnvMap::new());
        let e = validate_config_patch(&json!({ "registrar.enabled": "true" }), None, &env, None)
            .unwrap_err();
        assert_eq!(e[0].code, ConfigErrorCode::NotABoolean, "{e:?}");

        // ⚠️ 凭据要先配好：**打开一个没凭据的注册机本来就该被拦**
        //（那是跨字段那条规则，与这一格测的「类型」是两件事）。
        // ⚠️ 通道也要显式选：**打开却没选通道本来就该被拦**
        //（两条通道完全平级，替他挑一条就是替他做决定）。
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "channel": "yyds", "yyds": { "apiKey": "yk" } },
        });
        let r = validate_config_patch(
            &json!({ "registrar.enabled": true }),
            Some(&stored),
            &env,
            None,
        );
        assert!(r.is_ok(), "{:?}", r.err());
    }

    /// 🔴 **打开了却没选通道 ⇒ 当场拒，不替他挑一条。**
    ///
    /// 两条通道**完全平级**。静默挑一条等于替运维做了一个他没做的决定，
    /// 而后果要到「凭据缺失」那条报错**指着一条他从没选过的通道**时才浮上来
    /// —— 那时他会去补那条通道的凭据，而他本来想用的是另一条。
    #[test]
    fn enabling_without_choosing_a_channel_is_refused_instead_of_picking_one() {
        let env = with_token(EnvMap::new());
        // 两条凭据都配好了 —— 所以拦下来的只可能是「没选」这件事本身。
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": {
                "yyds": { "apiKey": "yk" },
                "moemail": { "baseUrl": "https://m.test", "apiKey": "mk" },
            },
        });
        let e = validate_config_patch(
            &json!({ "registrar.enabled": true }),
            Some(&stored),
            &env,
            None,
        )
        .unwrap_err();
        assert!(
            e.iter().any(|x| x.code == ConfigErrorCode::ChannelRequired),
            "🔴 替运维挑了一条通道 —— 两条就不再是平级的了：{e:?}",
        );
    }

    /// ⚠️ **写错了与没选是两句不同的话，而且不同时说。**
    ///
    /// 同时说只会让运维以为有两处要改。
    #[test]
    fn a_typo_says_not_a_channel_and_never_also_says_channel_required() {
        let env = with_token(EnvMap::new());
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "enabled": true, "channel": "yydss", "yyds": { "apiKey": "yk" } },
        });
        let b = config_load_blockers(Some(&stored), &env);
        assert!(
            b.iter().any(|x| x.code == ConfigErrorCode::NotAChannel),
            "{b:?}"
        );
        assert!(
            !b.iter().any(|x| x.code == ConfigErrorCode::ChannelRequired),
            "🔴 两句话一起说了 —— 运维会以为有两处要改：{b:?}",
        );
    }

    /// 🔴 **打开一个没配凭据的注册机 ⇒ 当场拒。**
    ///
    /// 收下的话，运维会在面板上看到「已启用」，而注册机每一轮都被自己的配置拦住。
    #[test]
    fn enabling_the_registrar_without_credentials_is_refused_at_save_time() {
        let env = with_token(EnvMap::new());
        let e = validate_config_patch(&json!({ "registrar.enabled": true }), None, &env, None)
            .unwrap_err();
        assert!(
            e.iter()
                .any(|x| x.code == ConfigErrorCode::ChannelCredentialsMissing),
            "🔴 收下了一份打得开、却一把 key 都铸不出来的配置：{e:?}",
        );
    }

    /// 通道名：认得出的两条、或者 `null`；别的一律 `not_a_channel`。
    #[test]
    fn the_channel_accepts_the_two_names_or_null_and_nothing_else() {
        let env = with_token(EnvMap::new());
        for ok in [json!("yyds"), json!("moemail"), json!(null)] {
            assert!(
                validate_config_patch(&json!({ "registrar.channel": ok }), None, &env, None)
                    .is_ok(),
                "{ok} 该收下",
            );
        }
        let e = validate_config_patch(&json!({ "registrar.channel": "yydss" }), None, &env, None)
            .unwrap_err();
        assert_eq!(e[0].code, ConfigErrorCode::NotAChannel, "{e:?}");
    }

    /// 🔴 **把下界改到大于上界 ⇒ 当场拒，一个字节都不写。**
    ///
    /// 收下的话，注册机会在下一轮启动时被自己的配置拦住，
    /// 而运维在面板上看到的是「保存成功」。
    #[test]
    fn raising_the_delay_floor_above_the_ceiling_is_refused_at_save_time() {
        let env = with_token(EnvMap::new());
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "mintDelayMaxMs": "60000" },
        });
        let e = validate_config_patch(
            &json!({ "registrar.mintDelayMinMs": 90_000 }),
            Some(&stored),
            &env,
            None,
        )
        .unwrap_err();
        assert!(
            e.iter().any(|x| x.code == ConfigErrorCode::DelayMinGtMax),
            "🔴 收下了一份注册机起不来的配置，而面板说保存成功：{e:?}",
        );
    }

    /// 🔴 **选中一条没配凭据的通道 ⇒ 当场拒。**
    #[test]
    fn switching_to_a_channel_without_credentials_is_refused_at_save_time() {
        let env = with_token(EnvMap::new());
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "enabled": true, "yyds": { "apiKey": "yk" } },
        });
        let e = validate_config_patch(
            &json!({ "registrar.channel": "moemail" }),
            Some(&stored),
            &env,
            None,
        )
        .unwrap_err();
        assert!(
            e.iter()
                .any(|x| x.code == ConfigErrorCode::ChannelCredentialsMissing),
            "🔴 切到了一条没凭据的通道，而面板说保存成功：{e:?}",
        );
    }

    /// 🔴 **注册机只配了一半的存量部署，照样能改别的字段。**
    ///
    /// 报全量 blocker 的话，那份部署会被自己的历史**永久锁在设置页外面** ——
    /// 而它本来只想调一下 `maxStrikes`。
    #[test]
    fn a_half_configured_registrar_does_not_lock_the_operator_out_of_other_fields() {
        let env = with_token(EnvMap::new());
        // 存量：打开了、却没配凭据 ⇒ 本来就带着一条 blocker。
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "enabled": true },
        });
        assert!(
            !config_load_blockers(Some(&stored), &env).is_empty(),
            "前提：这份存量配置本来就带着 blocker",
        );
        assert!(
            validate_config_patch(
                &json!({ "cooldownStrikeMs": 1234 }),
                Some(&stored),
                &env,
                None
            )
            .is_ok(),
            "🔴 存量的注册机毛病把运维锁在了整个设置页外面",
        );
    }

    /// 旧主备键那一族**进 `loadNotices`，而不是拦人**。
    #[test]
    fn the_legacy_primary_key_surfaces_as_a_notice_not_a_blocker() {
        let env = with_token(EnvMap::new());
        let stored = json!({
            "gatewayToken": STORED_GATEWAY_TOKEN,
            "registrar": { "primary": "moemail", "moemail": { "baseUrl": "https://m.test", "apiKey": "mk" } },
        });
        let (snap, _) = snapshot_of(Some(&stored), &env);
        assert!(
            snap.load_notices
                .iter()
                .any(|n| n.code == ConfigErrorCode::LegacyChannelKey),
            "🔴 这一格曾经恒为空，登记的理由是「它唯一的一族属于注册机子树」\
             —— 子树来了，它必须真的有东西：{:?}",
            snap.load_notices,
        );
        assert!(
            snap.load_blocked.is_empty(),
            "它不拦人：{:?}",
            snap.load_blocked
        );
    }
}
