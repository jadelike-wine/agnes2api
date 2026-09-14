//! `GET /admin/api/usage` 与 `GET /admin/api/usage/{date}`（02 §9.23、§9.24）—— **Tier-2 读侧**。
//!
//! # 🔴 关着的时候回 **200 + `tier:"off"`**，不是 404、不是 503
//!
//! 503 会让面板把「**运维没打开统计**」渲染成「**后端坏了**」。
//! 这两件事的处置完全不同（去改一个环境变量 vs 去看容器日志），
//! 而一个状态码分不开它们。
//!
//! ⚠️ 连带：这两条端点**照常注册、照常鉴权**，路由表绝不许随运行时配置变化 ——
//! 枚举式鉴权矩阵会因为夹具恰好关着某功能而让端点从矩阵里**静默消失**。
//!
//! # ⚠️⚠️ 最贵的是**读扇出**，这条代价必须明写
//!
//! 30 天那一档一次请求发 `30 × USAGE_SLOTS` = **60 次 `get`**，
//! 而 `FileStorage` 上**每一次 `get` 都是整份 `store.json` 的读 + 解析** ——
//! 60 次就是 60 遍。
//! （这条代价原先的立论是 Cloudflare Worker 的子请求上限，那个上限随 Worker 形态
//! 在 v0.4.0 一起没了；**结论一个字没变，防的那件事换了**。）
//!
//! 🔴 **那一档必须「失败得诚实」**：扇出里**任何一次**抛错都让整块失败
//! （`days` / `total` / `byApiKey` / `shards` / `malformed` **同生同死**地变 `null`），
//! **绝不把读到的那部分当成全份交出去** —— 一段不完整的 30 天被渲染成完整的，
//! 比一句「读不出来」糟得多。
//!
//! # ⚠️ 本文件里住着两件「本该在别处」的东西，如实登记
//!
//! 1. [`UsageBucket`] / [`merge_day_shards`] / [`usage_candidate_keys`] 那一族纯算术，
//!    按模块布局归 `core/admin/usage_stats.rs`。那个文件本轮还不存在而 `src/core/`
//!    不归本面改 ⇒ 先住在这里，体例同 [`crate::admin::keys`] 文件头登记的三件事。
//!    将来整体搬走，**判据不必改**（它们钉的是算术与端点行为）。
//! 2. **写侧（`UsageSink`）整个不在这里，也不在本轮**：`AppState::usage_sink`
//!    🔴 P1 恒为 `None`，而「关 = sink 根本不存在」**不是**「建好再用 `if` 拦住写」。
//!    本文件因此只有读侧。

use crate::ports::storage::Storage;
use crate::server::errors::GatewayError;
use crate::server::state::AppState;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

// ── 1. 存储形态与键空间 ─────────────────────────────────────────────────────

/// 一天。
pub const USAGE_DAY_MS: i64 = 86_400_000;

/// 槽位数。**取 2，与事件环的 `EVENT_SLOTS` 相同**：它是「同一天里最多几个副本
/// 能各写各的而不互相覆盖」。
///
/// ⚠️ **这不是「不会丢计数」的保证**：超过 2 个副本时同槽位仍是 last-write-wins。
/// 这正是 Tier-2 全程要打 `≈`（`approximate: true`）的原因之一。
pub const USAGE_SLOTS: usize = 2;

/// 保留多少天。**读路径的键数上界 = `USAGE_DAY_RETAIN × USAGE_SLOTS` = 60。**
pub const USAGE_DAY_RETAIN: i64 = 30;

/// TTL 余量。与事件环的 `EVENT_TTL_MARGIN_MS` 同一条理由（时钟偏差）。
pub const USAGE_TTL_MARGIN_MS: i64 = USAGE_DAY_MS;

/// 用量键的前缀。
///
/// **提成常量是为了让它可被扫到**：全仓对存储键有一份「重置到底重置了什么」的
/// 封闭登记，而钉住那份登记的办法之一是**按名字扫源码**。
/// 键名藏在格式串里的时候那一格扫不到它，于是「新增一个存储键而不进登记」就没人拦。
pub const USAGE_KEY_PREFIX: &str = "usage:";

/// UTC 日序号。
///
/// ⚠️ 用 `div_euclid` 而不是 `/`：Rust 的 `/` 对负数向零截断，
/// 1970 年之前的时钟会算出**错一格**的日号（与事件环那边同一条）。
#[must_use]
pub fn usage_day_index(at: i64) -> i64 {
    at.div_euclid(USAGE_DAY_MS)
}

/// `usage:<日>:<槽位>`。
#[must_use]
pub fn usage_day_key(day: i64, slot: usize) -> String {
    format!("{USAGE_KEY_PREFIX}{day}:{slot}")
}

/// 这把键该在什么时刻过期。
///
/// **有界性是存储自己的性质（TTL），不依赖落盘节奏、槽位随机性、进程会不会被结束。**
/// 上一版把有界性绑在「每次成功落盘后顺手删一把旧键」上 —— 那个方案被逐场景实测
/// 推翻：稀疏落盘时清理率直接归零。
#[must_use]
pub fn usage_expires_at(day: i64) -> i64 {
    (day + USAGE_DAY_RETAIN) * USAGE_DAY_MS + USAGE_TTL_MARGIN_MS
}

/// 读某个时间区间要取哪些键。
///
/// 🔴 **上界是这个函数自身的不变量，不是一句注释**：返回的键数**对任意入参**
/// 都 `<= USAGE_DAY_RETAIN × USAGE_SLOTS`（= 60）。
///
/// ⚠️ **夹逼的理由就是上面那条上界，不是「反正已经过期」** —— 后者被
/// [`USAGE_TTL_MARGIN_MS`] 直接证伪：TTL 多留了一整天余量，
/// **`今天 − 30` 那一天的键在任何时刻都还活着，却永远不会被这个函数取到**。
/// 那份余量随一天的推移线性缩小（落在 `(0, 86_400_000]` 内，日初满格、日末趋近 0），
/// 但恒 `> 0`。**别照「反正过期了」去把那天加回来 —— 那会直接打破 60 这条上界。**
///
/// ⚠️ **TS 侧那两道护栏（`Number.isFinite(nowMs)` + 结构性计数闸）在 Rust 侧的
/// 处境不同，如实写清楚**：`i64` 没有 `NaN` / `±Infinity`，也没有「`d + 1 == d`」
/// 这种精度打转（那是 `f64` 超过 `2**53` 之后的形态）⇒ **第一道护栏结构性不需要**。
/// 第二道（计数闸）**照样留着**：它让「恒 ≤ 60」这条不变量**不依赖任何关于
/// `from` / `to` 取值的推理**，而失效形态是「不返回」而不是「数字变大」——
/// 评审当年的第一次探针就是把 v8 跑成了 OOM。
///
/// ⚠️⚠️ **「夹逼」与「计数闸」对 ≤60 这条上界是**互为冗余**的，变异复检实测过，
/// 别把这一段读成「两道闸各守一半」**：
/// · 只删夹逼（`oldest` 不再 `max(oldest_allowed)`）⇒ 计数闸把格数压在 30
///   ⇒ [`tests::candidate_keys_are_hard_bounded_for_any_input`] **不红**（实测 ESCAPED）；
/// · 只删计数闸 ⇒ 夹逼把 `oldest` 顶到 `今天 − 29` ⇒ 同样不红；
/// · **两道一起删才红**。
/// ⇒ 夹逼**自己**的、不与计数闸重合的那份价值在别处：它决定**哪一天取得到**，
/// 由 [`tests::the_day_excluded_by_clamping_has_not_actually_expired_yet`] 单独钉着
/// （那一格只删夹逼就红）。**两格缺一不可。**
#[must_use]
pub fn usage_candidate_keys(now_ms: i64, from_ms: i64, to_ms: i64) -> Vec<String> {
    let newest = usage_day_index(to_ms.min(now_ms));
    let oldest_allowed = usage_day_index(now_ms) - USAGE_DAY_RETAIN + 1;
    let oldest = usage_day_index(from_ms).max(oldest_allowed);
    let mut keys = Vec::new();
    let mut d = oldest;
    let mut n = 0i64;
    while d <= newest && n < USAGE_DAY_RETAIN {
        for s in 0..USAGE_SLOTS {
            keys.push(usage_day_key(d, s));
        }
        d += 1;
        n += 1;
    }
    keys
}

// ── 2. 桶与窄化 ────────────────────────────────────────────────────────────

/// 一个桶。**没有原始延迟数组**：只存和与次数，均值现算（明确放弃 P95）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    pub requests: i64,
    pub success: i64,
    pub errors: i64,
    /// ⚠️ **只有非流式、且协议路由解析过响应体的那几条**才有 token。
    pub tokens_in: i64,
    pub tokens_out: i64,
    /// 流式请求单列一栏，**让 token 的缺口可见**。
    pub streaming_requests: i64,
    /// ⚠️ **只累计成功请求的延迟。**
    ///
    /// 上一版的注释写「请求真的完成了才有延迟可言」而代码对失败请求同样 `+1` ——
    /// 后果不是措辞问题：一个 8 秒超时和一个 30 毫秒的 401 都会进平均延迟，
    /// 于是面板上那个「平均延迟」**不是任何真实的东西**。
    /// 失败请求的耗时由「错误率」那张卡负责，不掺进延迟。
    pub latency_sum: i64,
    pub latency_count: i64,
}

impl UsageBucket {
    /// 两个桶相加。**纯函数，不改入参。**
    #[must_use]
    pub fn plus(self, o: Self) -> Self {
        Self {
            requests: self.requests + o.requests,
            success: self.success + o.success,
            errors: self.errors + o.errors,
            tokens_in: self.tokens_in + o.tokens_in,
            tokens_out: self.tokens_out + o.tokens_out,
            streaming_requests: self.streaming_requests + o.streaming_requests,
            latency_sum: self.latency_sum + o.latency_sum,
            latency_count: self.latency_count + o.latency_count,
        }
    }
}

/// 存储里的一个数 ⇒ 非负整数。坏的一律按 0 记。
fn num(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_f64)
        .filter(|f| f.is_finite() && *f >= 0.0)
        .map_or(0, |f| f.trunc() as i64)
}

/// 一个桶的窄化。**认不出就 `None`**（调用方据它决定这是不是一个畸形分片）。
///
/// ⚠️ **数组要单独拒掉**：`{ "total": [] }` 若被判成合法分片，
/// 一个畸形分片就伪装成了合法的，`shards` 那个数也跟着变假 ——
/// 那比被丢掉更糟。
fn narrow_bucket(v: Option<&Value>) -> Option<UsageBucket> {
    let Some(Value::Object(r)) = v else {
        return None;
    };
    Some(UsageBucket {
        requests: num(r.get("requests")),
        success: num(r.get("success")),
        errors: num(r.get("errors")),
        tokens_in: num(r.get("tokensIn")),
        tokens_out: num(r.get("tokensOut")),
        streaming_requests: num(r.get("streamingRequests")),
        latency_sum: num(r.get("latencySum")),
        latency_count: num(r.get("latencyCount")),
    })
}

/// `Record<string, UsageBucket>` 的窄化。**认不出那一格就让那个键消失**
/// （分片本身仍算合法，`malformed` 不加）。
///
/// ⚠️ **方向无害但确实是一次静默丢弃，明写**：`malformed` 今天只数**分片级**的畸形，
/// **不数桶级的**。要把桶级也数进去得改返回形状，那属于显示口径，不在这一层。
fn narrow_record(v: Option<&Value>) -> BTreeMap<String, UsageBucket> {
    let Some(Value::Object(m)) = v else {
        return BTreeMap::new();
    };
    m.iter()
        .filter_map(|(k, val)| narrow_bucket(Some(val)).map(|b| (k.clone(), b)))
        .collect()
}

/// 一个副本在某一个 UTC 日里攒下的全部计数。
#[derive(Debug, Clone)]
struct UsageDayShard {
    /// UTC 日序号，**不是日期串**：整数比对不会有时区歧义。
    day: i64,
    total: UsageBucket,
    hours: BTreeMap<String, UsageBucket>,
    by_model: BTreeMap<String, UsageBucket>,
    by_protocol: BTreeMap<String, UsageBucket>,
    /// ⚠️ **存量分片里没有这一格**：那些天的按密钥分解因此是空的 ——
    /// **这是实情，不是缺陷**。零迁移。
    by_api_key: BTreeMap<String, UsageBucket>,
}

fn narrow_shard(v: Option<&Value>) -> Option<UsageDayShard> {
    let Some(Value::Object(r)) = v else {
        return None;
    };
    let total = narrow_bucket(r.get("total"))?;
    let day = r
        .get("day")
        .and_then(Value::as_f64)
        .filter(|f| f.is_finite())? as i64;
    Some(UsageDayShard {
        day,
        total,
        hours: narrow_record(r.get("hours")),
        by_model: narrow_record(r.get("byModel")),
        by_protocol: narrow_record(r.get("byProtocol")),
        by_api_key: narrow_record(r.get("byApiKey")),
    })
}

/// 若干个日分片合并成一份。
///
/// ⚠️⚠️ **`hours` 是跨天求和的，调用方必须传单日区间 —— 这是契约不是实现细节**：
/// 本函数把每个分片的 `hours` 无差别地加在一起，所以多日区间下 `hours["13"]` 是
/// **「区间里所有天的 13 点之和」**（一条 hour-of-day 剖面），**不是某一天的 13 点**。
/// 拿多日区间的返回值去画单日小时图是错的。（`by_day` 不受影响，它按分片自报的
/// `day` 分了键。）
///
/// ⚠️ **入参是 `Option<&Value>` 而不是某个 DTO，且逐条窄化**：真实输入来自
/// `storage.get`，运行期什么形状都可能是。写成 DTO 就是在签名上撒谎。
/// **`malformed` 是给面板看的：静默丢弃就是撒谎。**
///
/// 🔴 **键不存在（`None`）不计 `malformed`**：那是完全正常的路径（这把键还没被写过），
/// 把它计进去会让一个全新部署一上来就报「有畸形数据」。
///
/// ⚠️ **TS 侧那一整段「四个 map 必须是无原型对象」的论证在 Rust 侧结构性消失**：
/// 那条缺陷的成因是 JS 的 `{}` 上 `out["__proto__"] = b` 命中 `Object.prototype`
/// 的访问器、那一条就此消失。Rust 的 `BTreeMap` / `serde_json::Map` 没有原型链，
/// `__proto__` 只是一个普通键。**照旧有一格判据钉着**
/// （[`tests::model_names_that_used_to_break_javascript_are_just_ordinary_keys`]）——
/// 不是因为这里还危险，而是因为那几个名字会从客户端的请求体一路进到这里。
#[must_use]
pub fn merge_day_shards(raw: &[Option<Value>]) -> MergedUsage {
    let mut m = MergedUsage::default();
    let merge = |into: &mut BTreeMap<String, UsageBucket>, from: &BTreeMap<String, UsageBucket>| {
        for (k, v) in from {
            let cur = into.get(k).copied().unwrap_or_default();
            into.insert(k.clone(), cur.plus(*v));
        }
    };
    for item in raw {
        let Some(sh) = narrow_shard(item.as_ref()) else {
            if item.is_some() {
                m.malformed += 1;
            }
            continue;
        };
        m.shards += 1;
        m.total = m.total.plus(sh.total);
        let cur = m.by_day.get(&sh.day).copied().unwrap_or_default();
        m.by_day.insert(sh.day, cur.plus(sh.total));
        merge(&mut m.hours, &sh.hours);
        merge(&mut m.by_model, &sh.by_model);
        merge(&mut m.by_protocol, &sh.by_protocol);
        merge(&mut m.by_api_key, &sh.by_api_key);
    }
    m
}

/// [`merge_day_shards`] 的结果。
#[derive(Debug, Default)]
pub struct MergedUsage {
    pub by_day: BTreeMap<i64, UsageBucket>,
    pub hours: BTreeMap<String, UsageBucket>,
    pub by_model: BTreeMap<String, UsageBucket>,
    pub by_protocol: BTreeMap<String, UsageBucket>,
    pub by_api_key: BTreeMap<String, UsageBucket>,
    pub total: UsageBucket,
    pub shards: usize,
    pub malformed: usize,
}

// ── 3. `note` 的闭集 ───────────────────────────────────────────────────────

/// `note` 的取值全集。
///
/// 🔴 **是 code 不是句子**：面板是五语言的，自由文本没法 i18n，前端拿它去查字典。
/// 🔴 **九条不许混成同一句「没有数据」** —— 那正是「绝不伪造」要挡的那件事。
///
/// ⚠️ **`note` 是摘要，不是第二个真源**：每一条 code 的判据全部来自同一份响应体里
/// 的其它字段（`tier` / `range.clamped` / `days === null` / `shards`）。
/// 前端读 note 还是读那些字段都行，**但两者不许打架** ——
/// 由 [`tests::note_never_contradicts_the_fields_it_summarises`] 钉着。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageNote {
    /// Tier-2 没打开，这个部署根本没在记账。
    Tier2Off,
    /// 服务端时钟给不出有限数字。**这不是「没有数据」。**
    ///
    /// ⚠️⚠️ **Rust 侧今天没有任何路径能产出它，如实登记**：本仓的时钟是
    /// `i64`（`now_ms()`），不存在 `NaN` / `±Infinity` 这一档，
    /// 而 TS 侧那个分支正是靠 `Number.isFinite(nowMs)` 触发的。
    /// **留着这一格是因为它是已发布契约的词汇表的一部分**（面板的五语言字典里有它），
    /// 删掉它等于让面板字典多一条对不上的键。
    /// **别为了「让它可达」去把时钟改成浮点。**
    ClockUnavailable,
    /// 读存储那一块抛了 ⇒ 数据整块是 `null`，面板显示 `—` 而不是 0。
    ReadFailed,
    /// 请求的区间被夹到了保留期起点或 `now`。
    RangeClamped,
    /// 单日下钻要的那一天整个落在保留期之外。
    /// **不是那天没有请求，是那天已经不在了。**
    DateOutOfRetention,
    /// 读成功了，区间里**一个分片都没有**（合法的没有、畸形的也没有）。
    /// ⚠️ 判据是 `shards == 0 && malformed == 0` **两个一起**。
    NoShards,
    /// 读成功了，**读到了分片，但每一个都是畸形的**。
    ///
    /// ⚠️⚠️ **缺陷只有一条：在这条 code 出现之前，`note` 那句话是假的** ——
    /// 它与「一个分片都没有」同报 `no_shards`，于是面板照 note 分支就会把
    /// 「读到了 N 个分片、但每一个都坏」说成「这段时间一个分片都没有」。
    /// 两者的处置完全不同（前者「没人用」，后者「存储里的东西坏了，去看是谁写的」）。
    /// ⭐ 连带记一条形状：**给一个真缺陷编一个更吓人的理由，会把它变成假的**
    /// —— 当年立案时写的「两者响应体逐字相同」是假的（`malformed` 一直在响应里），
    /// 而理由一被证伪，缺陷本身也会跟着被人当成没有。
    AllMalformed,
    /// 读成功了，**一部分分片好、另一部分畸形**。
    ///
    /// ⚠️⚠️ 它上一版走的是 `note: null`（「一切正常」那一支）—— 那不只是少一条 code：
    /// 它让面板把**一份缺了几个分片的数据**渲染成完整的。
    /// **伪造的不只是 `0`，还有「这份数据是全的」这个印象。**
    PartialMalformed,
    /// 单日下钻的常态：**没有逐请求流水**，面板据它渲染那句指路的话。
    NoRequestDetail,
}

impl UsageNote {
    /// 对外的字面量。**与 `#[serde(rename_all = "snake_case")]` 产出的串逐字相同。**
    ///
    /// 🔴 它存在的唯一理由：[`crate::core::admin::view`] 那一层是**纯的**，
    /// 不许反向依赖 `crate::admin` ⇒ `note` 在那边是 `Option<&str>`。
    /// 这个方法是两侧唯一的转换点。
    /// ⚠️ 改这里的任何一个串就是**改对外契约**（面板按它查字典），
    /// 而它与 serde 的一致性由 [`tests::the_note_wire_names_match_what_serde_emits`] 钉着
    /// —— 手写一张表与自动派生的那张漂了，这条端点会一边发 `read_failed`
    /// 一边把横幅算成另一档。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tier2Off => "tier2_off",
            Self::ClockUnavailable => "clock_unavailable",
            Self::ReadFailed => "read_failed",
            Self::RangeClamped => "range_clamped",
            Self::DateOutOfRetention => "date_out_of_retention",
            Self::NoShards => "no_shards",
            Self::AllMalformed => "all_malformed",
            Self::PartialMalformed => "partial_malformed",
            Self::NoRequestDetail => "no_request_detail",
        }
    }
}

/// `note` 的**全集**。判据拿它做「每一条 code 都有人认得」的对表。
///
/// 🔴 **加一条 code 就要加进这里** —— 否则 [`crate::core::admin::view::usage::note_severity`]
/// 会静默地把它归进「表外 ⇒ warn」，而那正是那条兜底**刻意保守**要盖住的情形。
/// 对表判据在 [`tests::every_note_code_has_an_explicit_banner_severity`]。
pub const USAGE_NOTES: [UsageNote; 9] = [
    UsageNote::Tier2Off,
    UsageNote::ClockUnavailable,
    UsageNote::ReadFailed,
    UsageNote::RangeClamped,
    UsageNote::DateOutOfRetention,
    UsageNote::NoShards,
    UsageNote::AllMalformed,
    UsageNote::PartialMalformed,
    UsageNote::NoRequestDetail,
];

/// `note` 的优先级。
///
/// `read_failed` > 畸形族 > `range_clamped` > `no_shards` > `null`。
///
/// 🔴 **畸形那两条压过 `range_clamped`，这个顺序订正过一次，别照上一版读。**
/// 上一版是反的（判据写的是「你要的窗口被缩小了更该先说」），
/// ⚠️ **那个顺序与档位映射那个场景叠起来会变成永久静音**：面板的 `30d` 按钮只要
/// 把 `from` 发成 `now − 30 天`（差一天），`clamped` 就**恒为真**
/// ⇒ 畸形那两条在那一档上**永远不可达**，存储坏成什么样面板都只会说
/// 「这段时间早于保留期」。
/// ⇒ **判据换成「谁需要人去查」**：畸形要人去查存储，被夹只是一句提示。
///
/// ⚠️ **代价明写**：被夹过**且**有畸形分片时，`clamped` 那句提示不再由 `note` 说 ——
/// 但 `range.clamped` 这个布尔一直在响应里，面板照样渲染得出来。
fn summarise(shards: usize, malformed: usize, clamped: bool) -> Option<UsageNote> {
    if malformed > 0 {
        return Some(if shards == 0 {
            UsageNote::AllMalformed
        } else {
            UsageNote::PartialMalformed
        });
    }
    if clamped {
        return Some(UsageNote::RangeClamped);
    }
    if shards == 0 {
        return Some(UsageNote::NoShards);
    }
    None
}

// ── 4. 参数解析 ────────────────────────────────────────────────────────────

/// 服务端真正会用的那一对时刻。
struct Range {
    from: i64,
    to: i64,
    from_day: i64,
    to_day: i64,
    clamped: bool,
}

fn query_param<'a>(raw: Option<&'a str>, name: &str) -> Option<&'a str> {
    raw?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// 单个 epoch 毫秒参数。**缺席返回 `None`，非法直接 400，不做「善意纠正」。**
///
/// ⚠️ **负数一律 400，不当成缺失**：epoch 毫秒没有负数这回事，而「悄悄纠正」会让
/// 「前端把参数发成 `-1`」这件事永远不被发现。
/// ⚠️ **与 `events` 的 `after` 刻意不同口径**：那一条是客户端回传的游标，
/// 容错是它语义的一部分；这两条是人点按钮选出来的时间范围，**发错就是 bug**。
fn ms_param(raw: Option<&str>, name: &str) -> Result<Option<i64>, GatewayError> {
    let Some(s) = raw.filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    // ⚠️ TS 那边判的是 `Number.isSafeInteger`（`1e24` 会通过 `isInteger` 但在那个
    //    量级上 `d + 1 == d`）。Rust 侧 `i64` 的解析天然拒掉非整数与超界值，
    //    **而 `i64::MAX` 仍然够荒诞** ⇒ 上界那一半由下面的夹逼 + 计数闸兜着。
    match s.parse::<i64>() {
        Ok(n) if n >= 0 => Ok(Some(n)),
        _ => Err(GatewayError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            format!("{name} 必须是不小于 0 的整数（epoch 毫秒），收到的是：{s}"),
        )),
    }
}

/// 把查询串解析成服务端真正会用的那一对时刻。
///
/// | 参数 | 缺省 | 越界处理 |
/// |---|---|---|
/// | `from` | `to - 86400000` | 早于保留期的夹到保留期起点，`clamped` 为真 |
/// | `to` | 服务端 `now()` | 晚于 `now()` 的夹到 `now()`，`clamped` 为真 |
///
/// **`days` 不是参数**：它是前端按钮的档位，服务端不认。
///
/// ⚠️⚠️ **档位 → `(from, to)` 的映射在这里钉死**：服务端不认档位，
/// 但两边必须对同一个映射有共识 ⇒ **`from = to − (N − 1) × 86400000`**
/// （服务端按整 UTC 天取，而「N 天」含今天）。
/// **差一天的后果不是「多一天数据」，是那句警告永久常驻**：发
/// `from = now − 30 × 86400000` 的话 `asked_from_day = 今天 − 30` 而保留期起点是
/// `今天 − 29` ⇒ **每一次点 30d 都 `clamped: true`**，面板于是每一次都说
/// 「这段时间早于保留期」—— **说到运维不再看它**。
/// 由 [`tests::the_four_preset_ranges_are_not_clamped_but_an_off_by_one_day_always_is`]
/// **两个方向都断言**（只写正向的话「clamped 恒为 false」的实现也全绿）。
///
/// ── 三处顺序是有讲究的 ────────────────────────────────────────────────────
/// ① **先校验客户端真的发了的那两个值** —— 它不需要时钟；
/// ② **`from > to` 用夹逼之前的那一对判**。夹逼之后再判会把「客户端发了一个自洽的
///    未来区间」误报成 400：`to` 被夹到 `now` 之后确实 `from > to`，**但那不是
///    客户端的错**；
/// ③ ⚠️ **夹完之后有效窗口为空是允许的，而那时回读的 `range.from` 会大于 `range.to`**
///    （例如整段区间都在未来）。那不是编出来的一对，它逐字就是服务端算出来的那个
///    空窗口。**不把它「修正」成一段非空区间** —— 那才是编，而面板会照着那段区间
///    说「这几天没有请求」，一句关于**根本没被查过的日子**的断言。
///
/// ── 滚动窗口 vs UTC 日键的错位，在这里一并裁掉 ────────────────────────────
/// `now − 30 天` 是一个**滚动**时刻，而日桶是 **UTC 日**，滚动 30 天会横跨 **31** 个
/// UTC 日。⇒ 服务端一律按「`from` 所在的 UTC 日」到「`to` 所在的 UTC 日」取整天，
/// 并在 `range` 里回读**取整之后**的那一对。**日桶的粒度就是天，装作能给出半天是撒谎。**
/// ⚠️ **取整本身不算 `clamped`**：它是无条件适用的口径（每一次请求都会发生），
/// 而 `clamped` 要回答的是「服务端有没有因为拿不到而缩小了你要的窗口」。
fn parse_range(raw: Option<&str>, now_ms: i64) -> Result<Range, GatewayError> {
    let raw_from = ms_param(query_param(raw, "from"), "from")?;
    let raw_to = ms_param(query_param(raw, "to"), "to")?;

    let to = raw_to.unwrap_or(now_ms);
    let from = raw_from.unwrap_or_else(|| to.saturating_sub(USAGE_DAY_MS));
    if from > to {
        return Err(GatewayError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            format!("from 不得晚于 to（from={from}, to={to}）"),
        ));
    }

    // 夹逼。**两个方向各自记一笔**，合起来才是 `clamped`。
    let clamped_to = to.min(now_ms);
    let oldest_allowed_day = usage_day_index(now_ms) - USAGE_DAY_RETAIN + 1;
    let asked_from_day = usage_day_index(from.max(0));
    let from_day = asked_from_day.max(oldest_allowed_day);
    let to_day = usage_day_index(clamped_to);
    Ok(Range {
        from,
        to,
        from_day,
        to_day,
        clamped: clamped_to != to || from_day != asked_from_day,
    })
}

/// UTC 日序号 → `YYYY-MM-DD`。
///
/// **日期一律 UTC**：与部署在哪个时区无关，也与运维改容器 `TZ` 无关 ——
/// 按本地时区分桶会让同一份数据在改时区之后**整体错位**。
///
/// ⚠️ 自己算民用历，**不引日期库**：这里只需要「从 1970-01-01 起第 N 天」这一种
/// 换算，而它是确定性的整数算术。`day` 超出 `0000-01-01 ~ 9999-12-31` 时返回
/// `None` —— **不返回一个「大概对」的串**。
#[must_use]
pub fn utc_date_string(day: i64) -> Option<String> {
    // 民用历（proleptic Gregorian）的标准 days-from-civil 逆算法。
    let z = day.checked_add(719_468)?;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    if !(0..=9999).contains(&y) {
        return None;
    }
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// `YYYY-MM-DD` → UTC 日序号。**认不出来返回 `None`。**
///
/// ⚠️⚠️ **必须回读一遍再比对，而理由恰好与直觉相反**：
/// 「日历上不存在的一天」（`2024-02-31`）在朴素解析下**拿得到一个正常数字**
/// （滚到 3 月 2 日），而「月份越界」（`2026-13-01`）才是一眼可拒的。
/// ⇒ **`2024-02-31` 这一档完全靠回读比对挡着。**
/// 「解析出来的那一天再格式化回去必须逐字等于客户端给的串」同时也是一条
/// **与实现无关**的判据。
#[must_use]
pub fn parse_utc_date(raw: &str) -> Option<i64> {
    let b = raw.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return None;
    }
    let y: i64 = raw[0..4].parse().ok()?;
    let m: i64 = raw[5..7].parse().ok()?;
    let d: i64 = raw[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // days-from-civil 正算。
    let yy = if m <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 }.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let day = era * 146_097 + doe - 719_468;
    // 🔴 **回读比对** —— 见上方那段，`2024-02-31` 这一档只靠它。
    (utc_date_string(day)?.as_str() == raw).then_some(day)
}

// ── 5. 读扇出 ──────────────────────────────────────────────────────────────

/// 把候选键读回来并合并。
///
/// 🔴 **一个键都不许吞**：任何一次 `get` 抛错都让整块失败（返回 `None`），
/// **绝不把读到的那部分当成全份交出去**。
///
/// ⚠️⚠️ **代价再说一遍**（文件头写过，这里是它的落点）：30 天那一档是 60 次
/// `get`，而 `FileStorage` 上每一次 `get` 都是整份 `store.json` 的读 + 解析 ——
/// **60 遍**。
///
/// ⚠️ **这里是顺序读，TS 那边是 `Promise.all`。差别只有并发度，`get` 的次数、
/// 顺序、归并结果逐字相同**（`Promise.all` 不去重也不短路）。
/// `FileStorage` 上并发也不会变便宜（同一个文件、同一条写队列），
/// 而顺序读让「读到第 k 个才失败」这件事在判据里可构造。
async fn read_shards(storage: &dyn Storage, keys: &[String]) -> Option<MergedUsage> {
    let mut raw: Vec<Option<Value>> = Vec::with_capacity(keys.len());
    for k in keys {
        match storage.get(k).await {
            Ok(v) => raw.push(v),
            Err(_) => return None,
        }
    }
    Some(merge_day_shards(&raw))
}

/// Tier-2 读侧的接线。**`None` = 这个部署没建 sink，也就是 Tier-2 关着。**
///
/// ⚠️ **整块可空，而不是「存储恒在 + 一个 `enabled` 布尔」**：
/// 关闭时读路径在**结构上不存在**，不是一个 `if` 挡着 ——
/// 一个「反正 enabled 是 false」的读路径迟早会被某次改动接上。
struct UsageWiring<'a> {
    storage: &'a dyn Storage,
}

impl AppState {
    /// 这个部署的 Tier-2 接线。🔴 **P1 恒为 `None`**（`usage_sink` 恒为 `None`）。
    fn usage_wiring(&self) -> Option<UsageWiring<'_>> {
        self.usage_sink.as_ref().map(|_| UsageWiring {
            storage: self.storage.as_ref(),
        })
    }
}

// ── 6. 响应体（🔴 一律结构体 —— 键序是对外契约）────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RangeWire {
    from: i64,
    to: i64,
    clamped: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DayWire {
    date: String,
    total: UsageBucket,
}

/// 未落盘的尾巴。
///
/// ⚠️⚠️ **`ms` 数的是「距上一次落盘*尝试*多久」，不是「距上一次写成功多久」**
/// ⇒ `count > 0 && ms ≈ 0` 要读作「刚试过、没写成」，**不是「没有尾巴」**。
/// 判别未落盘尾巴的字段是 `count`，**不是 `ms`**。
/// ⚠️ `budgetExhausted` 必须一起发：那两种失败态（预算耗尽 / 存储抛错）
/// 在 `count` + `ms` 上长得一模一样，砍掉它面板就只能猜。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingWire {
    count: i64,
    ms: i64,
    budget_exhausted: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageBody {
    /// 🔴 **永远不许是 `null`**：它是「统计开没开」，来自内存里的接线，
    /// 读不出来是不可能的。写成可空会让前端多一条永远走不到的分支，
    /// 而那条分支迟早会被误用来兜别的 null。
    tier: &'static str,
    /// 日期一律 UTC，**并且在响应里说出来**。
    timezone: &'static str,
    approximate: bool,
    generated_at: i64,
    range: Option<RangeWire>,
    /// ⚠️ **恒是「每天一格」，不是「只列有数据的那几天」**：一个 30 天的区间恒给
    /// 30 格，缺数据的那天是 0 桶。让面板去补缺口等于把「哪些天该出现」
    /// 这条知识复制一份到前端。
    ///
    /// ⚠️ 那个 0 桶**不是「伪造 0」**：这一天的两个候选键都**真的被读过**、
    /// 都返回了「没有」，「没有记录」就是这次读的真实结果。
    /// 真正被禁止的是「读失败了却报 0」，而那一档由整块 `null` 承担。
    days: Option<Vec<DayWire>>,
    /// 整段区间的合计。
    ///
    /// ⚠️ **别把它写成「恒等于 Σdays」**（全称量词落笔前先找反例，这里有一个）：
    /// `total` 数的是**读到的每一个分片**，而 `days` 按分片自报的 `day` 分键并只列
    /// 区间内的那些天。一个自报 `day` 与它所在的键对不上的分片（手改存储、
    /// 写到一半的 `store.json`）会进 `total` 而进不了任何一格 `days`。
    /// **方向是「`total` 只多不少」**，而那正是想要的：少报一段真实发生过的流量，
    /// 比多报一段更难被发现。
    total: Option<UsageBucket>,
    /// 整段区间按「哪一把对外 API 密钥」分。
    ///
    /// ⚠️⚠️ **它为什么在汇总这条端点上而不只在单日下钻那条**：「API 密钥」板块
    /// 的每张卡要显示这把密钥的用量，而那个板块**没有「某一天」这个概念**。
    /// 让它去点一天再下钻是错的界面，让它自己把 30 天逐天拉一遍则是 30 次请求。
    /// ⇒ 汇总这条原样把这一维交出去，**零新增存储读**。
    /// ⚠️ **与 `days` / `total` / `shards` / `malformed` 同生同死**：
    /// 一个「读失败但按密钥分解写着 `{}`」的响应体，面板照着渲染就是
    /// 「这段时间每一把密钥都是 0 次」—— 那正是伪造 0。
    by_api_key: Option<BTreeMap<String, UsageBucket>>,
    shards: Option<usize>,
    malformed: Option<usize>,
    pending: Option<PendingWire>,
    note: Option<UsageNote>,
    /// 🔴 **已经判好的视图**（06 §5.3 的 A 档）。
    ///
    /// 它不是上面那些字段的复述：`state` / `cards.*` 是**五态互不重叠**那套判定的
    /// 结论，而那套判定在 TS 版里是面板自己算的、算错过五次
    /// （每一次的形态都逐条记在 [`crate::core::admin::view::usage`] 里）。
    /// ⚠️ **面板一律读这一格，别自己从 `tier` / `shards` / `total` 反推** ——
    /// 反推就是把那套判定复制了第二份。
    view: UsageViewWire,
}

/// 汇总那份响应的视图 DTO。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageViewWire {
    /// 五态之一：`off` / `unavailable` / `no-shards` / `empty` / `data`。
    state: &'static str,
    /// **这一块读成功了没有** —— 两张表拿它决定「空」的时候该说哪一句。
    read_succeeded: bool,
    /// 分片畸形到什么程度：`unknown` / `none` / `all` / `partial`。
    malformed_kind: &'static str,
    /// `note` 该用哪一档横幅：`error` / `warn` / `info`；`null` = 不出横幅。
    note_severity: Option<&'static str>,
    /// 🔴 **这份数字完不完整。** `false` ⇒ 六张卡必须标成不完整的
    /// ——「伪造的不只是 0，还有『这份数据是全的』这个印象」。
    complete: bool,
    cards: UsageCardsWire,
    /// 「API 密钥」板块每张卡上那个用量数该怎么读。见
    /// [`crate::core::admin::view::usage::api_key_usage_kind`]。
    api_key_usage_kind: &'static str,
    /// 日汇总表逐行的状态。**与 `days` 同序同长**；`days` 是 `null` 时是空数组
    /// （表本身不渲染，那一档由顶部横幅承担）。
    day_rows: Vec<DayRowWire>,
}

/// 六张汇总卡各自该渲染成哪一档（`value` / `none` / `none-no-shards` / `unknown`）。
///
/// ⚠️ **计数类与比率类在同一个状态下刻意长得不一样**：`empty` 态下计数是
/// `value`（真的是 0），而比率与平均延迟是 `none`（分母是 0、样本是 0）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageCardsWire {
    requests: &'static str,
    success_rate: &'static str,
    error_rate: &'static str,
    avg_latency: &'static str,
    /// 平均延迟那个数（毫秒）。`null` = **没有样本，不是零延迟**。
    avg_latency_ms: Option<i64>,
    tokens: &'static str,
    streaming: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DayRowWire {
    date: String,
    /// `unavailable` / `no-shards` / `data`。
    state: &'static str,
}

/// 拼汇总那份视图。
///
/// 🔴 **判定一律转调 [`crate::core::admin::view::usage`]，这里只做搬运**：
/// 在这个函数里写哪怕一个 `if`，那条判定就有了第二份实现。
fn usage_view(
    tier: &str,
    days: Option<&[DayWire]>,
    total: Option<&UsageBucket>,
    by_api_key_present: bool,
    shards: Option<usize>,
    malformed: Option<usize>,
    note: Option<UsageNote>,
) -> UsageViewWire {
    use crate::core::admin::view::usage as v;

    let note_str = note.map(UsageNote::as_str);
    let state = v::usage_state(&v::UsageFacts {
        tier,
        days_present: days.is_some(),
        total_requests: total.map(|t| t.requests),
        shards,
        malformed,
        note: note_str,
    });
    let kind = v::malformed_kind(shards, malformed);
    let avg = total.and_then(|t| v::avg_latency(t.latency_sum, t.latency_count));

    UsageViewWire {
        state: state.as_str(),
        read_succeeded: v::read_succeeded(state),
        malformed_kind: kind.as_str(),
        note_severity: v::note_severity(note_str).map(v::NoteSeverity::as_str),
        // ⚠️ 整块读不出来时 `complete` 是 `true`：**没有数据就谈不上残缺** ——
        //    那一档六格已经是 EM DASH，再叠一个「不完整」标记只会让运维以为
        //    「有一部分是好的」。
        complete: total.is_none() || kind != v::MalformedKind::Partial,
        cards: UsageCardsWire {
            requests: v::cell_kind(state, total.map(|t| t.requests)).as_str(),
            success_rate: v::ratio_kind(state, total.map(|t| t.requests)).as_str(),
            error_rate: v::ratio_kind(state, total.map(|t| t.requests)).as_str(),
            avg_latency: v::cell_kind(state, avg).as_str(),
            avg_latency_ms: avg,
            tokens: v::cell_kind(state, total.map(|t| t.tokens_in + t.tokens_out)).as_str(),
            streaming: v::cell_kind(state, total.map(|t| t.streaming_requests)).as_str(),
        },
        api_key_usage_kind: v::api_key_usage_kind(state, by_api_key_present).as_str(),
        day_rows: days
            .unwrap_or(&[])
            .iter()
            .map(|d| DayRowWire {
                date: d.date.clone(),
                // 每天一格恒有桶（后端就是这么发的）⇒ 第二个参数恒真；
                // 🔴 **这一行的价值在于把整块状态往下传**，见 `row_state` 上方那段。
                state: v::row_state(state, true).as_str(),
            })
            .collect(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageDateBody {
    tier: &'static str,
    timezone: &'static str,
    date: String,
    approximate: bool,
    generated_at: i64,
    hours: Option<BTreeMap<String, UsageBucket>>,
    by_model: Option<BTreeMap<String, UsageBucket>>,
    by_protocol: Option<BTreeMap<String, UsageBucket>>,
    by_api_key: Option<BTreeMap<String, UsageBucket>>,
    shards: Option<usize>,
    malformed: Option<usize>,
    note: Option<UsageNote>,
    /// 见 [`UsageBody::view`]。
    view: UsageDateViewWire,
}

/// 单日下钻那份响应的视图 DTO。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UsageDateViewWire {
    /// 三态之一：`off` / `unavailable` / `data`。**这条端点没有 `empty`**
    /// —— 三个 map 只含有过流量的那些键，「表是空的」在读成功时就是
    /// 「这一天没有记录」。
    state: &'static str,
    read_succeeded: bool,
    malformed_kind: &'static str,
    note_severity: Option<&'static str>,
    complete: bool,
    /// 🔴 **小时分解表的行序。** `hours` 那个 map 的键是 `"0"`…`"23"`，
    /// 而 JSON 对象没有「数值序」这回事 ⇒ 面板照 map 的自然序读出来是
    /// 0,1,10,11,…,2,20,… 一张**乱序的时间轴**。
    /// ⚠️ 另外两维（模型 / 协议）刻意留在字典序，理由见
    /// [`crate::core::admin::view::usage::hour_order`]。
    hour_order: Vec<String>,
    /// 按密钥那一维里，**被认出来的那几个保留伪桶**的文案键。
    ///
    /// ⚠️ **认不出来的 id 压根不在这张表里** ⇒ 面板照实画那个原始 id，
    /// 不猜、也不标「已删除」：一个已经被删掉的密钥与一个「这一刻那张表读不出来」
    /// 的密钥在面板手上**完全同形**。
    api_key_label_keys: BTreeMap<String, &'static str>,
}

/// 拼单日下钻那份视图。**判定一律转调 [`crate::core::admin::view::usage`]。**
fn usage_date_view(
    tier: &str,
    hours: Option<&BTreeMap<String, UsageBucket>>,
    by_api_key: Option<&BTreeMap<String, UsageBucket>>,
    shards: Option<usize>,
    malformed: Option<usize>,
    note: Option<UsageNote>,
) -> UsageDateViewWire {
    use crate::core::admin::view::usage as v;

    let state = v::detail_state(tier, hours.is_some(), shards, malformed);
    let kind = v::malformed_kind(shards, malformed);
    UsageDateViewWire {
        state: state.as_str(),
        read_succeeded: v::read_succeeded(state),
        malformed_kind: kind.as_str(),
        note_severity: v::note_severity(note.map(UsageNote::as_str)).map(v::NoteSeverity::as_str),
        complete: hours.is_none() || kind != v::MalformedKind::Partial,
        hour_order: v::hour_order(
            hours
                .map(|h| h.keys().map(String::as_str))
                .into_iter()
                .flatten(),
        ),
        api_key_label_keys: by_api_key
            .map(|m| {
                m.keys()
                    .filter_map(|id| v::api_key_row_label_key(id).map(|k| (id.clone(), k)))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// ── 7. handler ─────────────────────────────────────────────────────────────

/// 区间汇总端点在 `/admin/api` 之下的子路径。
///
/// ★ **它必须注册在 [`USAGE_DATE_SUBPATH`] 之前** —— 见
/// [`tests::the_bare_usage_path_is_never_swallowed_by_the_date_route`]。
pub const USAGE_SUBPATH: &str = "/usage";

/// 单日下钻端点的子路径。
pub const USAGE_DATE_SUBPATH: &str = "/usage/{date}";

/// `GET /admin/api/usage`（Tier-2 区间汇总）。
///
/// ── 八种状态必须在响应字段上分得开 ────────────────────────────────────────
///
/// | # | 状态 | HTTP | `tier` | `range` | `days`/`total` | `shards` | `malformed` | `pending` | `note` |
/// |---|---|---|---|---|---|---|---|---|---|
/// | ① | Tier-2 没开 | 200 | **`off`** | 有 | **null** | **null** | **null** | **null** | `tier2_off` |
/// | ② | 读不出来 | 200 | `tier2` | 有 | **null** | **null** | **null** | 有 | `read_failed` |
/// | ③ | 区间里一个分片都没有 | 200 | `tier2` | 有 | 每天一格、全 0 桶 | **0** | **0** | 有 | `no_shards` |
/// | ④ | 有分片，只是请求数是 0 | 200 | `tier2` | 有 | 有 | **> 0** | 0 | 有 | **null** |
/// | ⑤ | 时钟给不出有限数字 | 200 | 有 | **null** | **null** | **null** | **null** | 有 | `clock_unavailable`（🔴 Rust 侧不可达，见 [`UsageNote::ClockUnavailable`]） |
/// | ⑥ | 参数发错了 | **400** | — | — | — | — | — | — | — |
/// | ⑦ | 分片都在但**每一个都畸形** | 200 | `tier2` | 有 | 每天一格、全 0 桶 | **0** | **> 0** | 有 | `all_malformed` |
/// | ⑧ | **一部分分片畸形** | 200 | `tier2` | 有 | 有，**但不完整** | **> 0** | **> 0** | 有 | `partial_malformed` |
///
/// ③ 与 ④ 是两件事，第一版会把它们压成同一个「今日请求数 0」：前者说「这段时间
/// 这个部署没有记下任何东西」（可能没流量，也可能实例没活到一次落盘），
/// 后者说「有实例落过盘，那些盘里的请求数就是 0」。**判据是 `shards`，不是 `total`。**
///
/// ⚠️ **`tokensCoverage` 不在这里**：它只从 `GET /admin/api/capabilities` 发。
/// 同一份知识两个出口，前端就要面对「读哪一个」这个不该存在的问题。
///
/// ⚠️ **关着的时候一次存储访问都不许有**，而这条在本文件里是**结构性**的：
/// [`AppState::usage_wiring`] 为 `None` 时压根没有 `Storage` 可读。
/// ⚠️ **代价明写**：这条性质**不是一行可以被变异掉的代码** —— 要证伪它得先把接线
/// 本身改回「存储恒在 + 一个布尔」。由
/// [`tests::with_tier2_off_the_endpoint_reads_storage_zero_times`] 如实钉着，
/// 那条断言咬的是「谁把接线改回两格」，不是「谁在 handler 里写错了一个 if」。
pub async fn summary(State(st): State<AppState>, RawQuery(q): RawQuery) -> Response {
    let at = crate::adapters::now_ms();
    let wiring = st.usage_wiring();
    let tier = if wiring.is_none() { "off" } else { "tier2" };

    // ★ 参数校验排在最前：客户端发错了参数，**无论 Tier-2 开没开都该立刻拿到 400**。
    let parsed = match parse_range(q.as_deref(), at) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };

    let range = RangeWire {
        from: parsed.from_day * USAGE_DAY_MS,
        // 结束日的**最后一毫秒**：回读的是服务端真正覆盖到的那一段，
        // 不是 `to` 那一天的零点。
        to: (parsed.to_day + 1) * USAGE_DAY_MS - 1,
        clamped: parsed.clamped,
    };

    let Some(wiring) = wiring else {
        // ① Tier-2 没开。**`range` 照常回读**（参数是好的）；
        //    `pending` 是 `null` —— 没有 sink 就没有「未落盘的尾巴」这件事，报 0 是伪造。
        return Json(UsageBody {
            tier,
            timezone: "UTC",
            approximate: true,
            generated_at: at,
            range: Some(range),
            days: None,
            total: None,
            by_api_key: None,
            shards: None,
            malformed: None,
            pending: None,
            note: Some(UsageNote::Tier2Off),
            view: usage_view(
                tier,
                None,
                None,
                false,
                None,
                None,
                Some(UsageNote::Tier2Off),
            ),
        })
        .into_response();
    };

    let keys = usage_candidate_keys(at, parsed.from, parsed.to);
    let block = read_shards(wiring.storage, &keys).await;

    // ⚠️⚠️ **`pending` 今天恒为 `null`，如实登记**：P1 的 `usage_sink` 是一个
    //    占位单元类型（`Arc<()>`），拿不到 `status()`。
    //    🔴 **回 `null` 而不是伪造 `{0, 0, false}`** —— 后者会让面板说
    //    「没有未落盘的尾巴」，而真相是「我们不知道」。
    //    sink 真落地时这里换成 `sink.status()`，**别顺手填一组 0 把这一格「补齐」**。
    let pending: Option<PendingWire> = None;

    let Some(merged) = block else {
        // ② 读不出来。三个字段同生同死。**不是 500**：
        //    `tier` / `range` / `pending` 这几块都还是真的，整条 500 会把它们一起丢掉。
        return Json(UsageBody {
            tier,
            timezone: "UTC",
            approximate: true,
            generated_at: at,
            range: Some(range),
            days: None,
            total: None,
            by_api_key: None,
            shards: None,
            malformed: None,
            pending,
            note: Some(UsageNote::ReadFailed),
            view: usage_view(
                tier,
                None,
                None,
                false,
                None,
                None,
                Some(UsageNote::ReadFailed),
            ),
        })
        .into_response();
    };

    // 格数的上界来自夹逼（`from_day >= today − 29` 且 `to_day <= today`
    // ⇒ 跨度恒 `<= 30`），计数闸是第二道。
    let mut days = Vec::new();
    let mut d = parsed.from_day;
    let mut n = 0i64;
    while d <= parsed.to_day && n < USAGE_DAY_RETAIN {
        let Some(date) = utc_date_string(d) else {
            // 时钟荒诞到民用历表示不出来。**端点必须返回，不许把进程挂死**，
            // 也不许编一个「大概对」的日期串。
            break;
        };
        days.push(DayWire {
            date,
            total: merged.by_day.get(&d).copied().unwrap_or_default(),
        });
        d += 1;
        n += 1;
    }

    let note = summarise(merged.shards, merged.malformed, parsed.clamped);
    let view = usage_view(
        tier,
        Some(&days),
        Some(&merged.total),
        true,
        Some(merged.shards),
        Some(merged.malformed),
        note,
    );
    Json(UsageBody {
        tier,
        timezone: "UTC",
        approximate: true,
        generated_at: at,
        range: Some(range),
        days: Some(days),
        total: Some(merged.total),
        by_api_key: Some(merged.by_api_key),
        shards: Some(merged.shards),
        malformed: Some(merged.malformed),
        pending,
        note,
        view,
    })
    .into_response()
}

/// `GET /admin/api/usage/{date}`（单日下钻，`date` 是 UTC 的 `YYYY-MM-DD`）。
///
/// ⚠️ **与区间端点的口径刻意不同**：那条只认 epoch 毫秒整数，这条只认日期串，
/// **两者不通用**。
///
/// **没有逐请求流水** ⇒ 常态下 `note` 恒是 `no_request_detail`，让面板能渲染那句
/// 「需要逐请求粒度请看容器 stdout」，**而不是给一张永远空着的表**。
/// ⚠️ **代价明写**：`note` 只有一格，出问题时它会被 `read_failed` 这类占掉，
/// 那句指路的话就不显示了。这是刻意的取舍 —— **出问题的时候先说出问题**。
///
/// ⚠️ **只把那一天的分片喂进 [`merge_day_shards`]**：那个函数把每个分片的 `hours`
/// 无差别地加在一起，多日区间下 `hours["13"]` 是**「区间里所有天的 13 点之和」**。
///
/// ⚠️ **缺席的小时不是「读不出来」**：`hours` 只含有过流量的那些小时。
/// 「这一天读成功了没有」由 `hours` **整块**是不是 `null` 承担，
/// 不由某个小时在不在里面承担 —— 面板对缺席的小时画 0 是对的。
pub async fn by_date(State(st): State<AppState>, Path(date): Path<String>) -> Response {
    let at = crate::adapters::now_ms();
    let wiring = st.usage_wiring();
    let tier = if wiring.is_none() { "off" } else { "tier2" };

    let Some(day) = parse_utc_date(&date) else {
        return GatewayError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            format!("date 必须是 UTC 的 YYYY-MM-DD，收到的是：{date}"),
        )
        .into_response();
    };

    let empty = |note: UsageNote| UsageDateBody {
        tier,
        timezone: "UTC",
        date: date.clone(),
        approximate: true,
        generated_at: at,
        hours: None,
        by_model: None,
        by_protocol: None,
        by_api_key: None,
        shards: None,
        malformed: None,
        note: Some(note),
        view: usage_date_view(tier, None, None, None, None, Some(note)),
    };

    let Some(wiring) = wiring else {
        return Json(empty(UsageNote::Tier2Off)).into_response();
    };

    let now_day = usage_day_index(at);
    if day > now_day || day < now_day - USAGE_DAY_RETAIN + 1 {
        // 那一天整个落在保留期之外。
        // **不是「那天没有请求」，是那天已经不在了** —— 报成 `no_shards`
        // 会让运维以为那天真的没人用。
        return Json(empty(UsageNote::DateOutOfRetention)).into_response();
    }

    let day_ms = day * USAGE_DAY_MS;
    let keys = usage_candidate_keys(at, day_ms, day_ms);
    let Some(merged) = read_shards(wiring.storage, &keys).await else {
        return Json(empty(UsageNote::ReadFailed)).into_response();
    };

    // 成功那一支的 `note` 恒是 `no_request_detail`：本端点给的是聚合，没有逐请求明细。
    let view = usage_date_view(
        tier,
        Some(&merged.hours),
        Some(&merged.by_api_key),
        Some(merged.shards),
        Some(merged.malformed),
        Some(UsageNote::NoRequestDetail),
    );
    Json(UsageDateBody {
        tier,
        timezone: "UTC",
        date,
        approximate: true,
        generated_at: at,
        hours: Some(merged.hours),
        by_model: Some(merged.by_model),
        by_protocol: Some(merged.by_protocol),
        by_api_key: Some(merged.by_api_key),
        shards: Some(merged.shards),
        malformed: Some(merged.malformed),
        note: Some(UsageNote::NoRequestDetail),
        view,
    })
    .into_response()
}

/// 一个分片落盘时长什么样。**只在判据里用**（写侧本轮不存在）。
///
/// 放在这里而不是测试模块里，是因为它同时是**存储形态的文档**：
/// 「日键 + 值里装 24 个小时槽」这个形状是刻意的 ——
/// 写路径按「天」分键所以便宜（一次 flush 恒 1 个 put），
/// 读路径按「天」取键所以扇出小（30 天 = 60 次 get），
/// 而「小时」这一维**住在值里面、不额外花键**。
/// 按小时分键的代价算过：读 30 天要 `720 × 2 = 1440` 次 get。
#[must_use]
pub fn shard_value(day: i64, total: UsageBucket, extra: &[(&str, Value)]) -> Value {
    let mut m = Map::new();
    m.insert("shardId".into(), Value::String("judge".into()));
    m.insert("day".into(), Value::from(day));
    m.insert("updatedAt".into(), Value::from(0));
    m.insert(
        "total".into(),
        serde_json::to_value(total).expect("桶一定序列化得出来"),
    );
    for (k, v) in extra {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

// ── 判据 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::server::app::build_router;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Method, Request};
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    struct Harness {
        app: Router,
        storage: Arc<dyn Storage>,
    }

    /// `tier2` = 把 `usage_sink` 接上。
    ///
    /// ⚠️ **这里接的是真的 sink，不再是一个 `()` 占位。**
    /// 从前它是占位是因为写端还没写；写端落地之后再留占位，
    /// 等于让这一族读路径的判据**在一个永远不会产出数据的形态上跑** ——
    /// 而那正是「用量是 0」与「用量没记上」在屏幕上分不开的来源。
    async fn harness_with(storage: Arc<dyn Storage>, tier2: bool) -> Harness {
        let TestApp { mut state, .. } = state_with_storage(storage.clone()).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        if tier2 {
            state.usage_sink = Some(Arc::new(crate::server::usage_sink::UsageSink::new(
                storage.clone(),
                Arc::new(crate::ports::logger::NullLogger),
                "test-shard",
                crate::server::usage_sink::USAGE_FLUSH_DEFAULT_MS,
                0,
            )));
        }
        let app = build_router(state);
        Harness { app, storage }
    }

    async fn tier2_harness() -> Harness {
        harness_with(Arc::new(MemoryStorage::new()), true).await
    }

    async fn get_json(h: &Harness, uri: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("x-admin-key", ADMIN)
            .body(Body::empty())
            .unwrap();
        let res = h.app.clone().oneshot(req).await.unwrap();
        let s = res.status();
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    fn bucket(requests: i64) -> UsageBucket {
        UsageBucket {
            requests,
            success: requests,
            ..UsageBucket::default()
        }
    }

    async fn seed(h: &Harness, day: i64, slot: usize, v: Value) {
        h.storage
            .put(&usage_day_key(day, slot), v, None)
            .await
            .unwrap();
    }

    fn today() -> i64 {
        usage_day_index(crate::adapters::now_ms())
    }

    // ── 键空间与扇出 ───────────────────────────────────────────────────────

    /// 🔴 **候选键数对任意入参都 `<= 60`。**
    ///
    /// 失效形态是「不返回」而不是「数字变大」—— 当年的第一次探针把运行时跑成了 OOM。
    #[test]
    fn candidate_keys_are_hard_bounded_for_any_input() {
        let now = 1_800_000_000_000i64;
        let bound = (USAGE_DAY_RETAIN as usize) * USAGE_SLOTS;
        let cases: [(i64, i64); 6] = [
            (0, now),                                           // 从纪元零点要到现在
            (now - 365 * USAGE_DAY_MS, now),                    // 要一整年
            (i64::MIN / 2, i64::MAX / 2),                       // 荒诞区间
            (now, now),                                         // 单点
            (now + 40 * USAGE_DAY_MS, now + 41 * USAGE_DAY_MS), // 整段在未来
            (now - USAGE_DAY_MS, now - USAGE_DAY_MS),           // 昨天
        ];
        for (from, to) in cases {
            let k = usage_candidate_keys(now, from, to);
            assert!(k.len() <= bound, "({from}, {to}) 打出了 {} 把键", k.len());
        }
        // 一整年那一档正好顶到上界 —— 这一格同时证明上界不是因为「总是很小」才成立。
        assert_eq!(
            usage_candidate_keys(now, now - 365 * USAGE_DAY_MS, now).len(),
            bound
        );
    }

    /// ⚠️ **被夹逼排除掉的那一天其实还没过期。**
    ///
    /// TTL 多留了一整天余量 ⇒ `今天 − 30` 那把键在任何时刻都还活着，
    /// 却永远不会被 [`usage_candidate_keys`] 取到。
    /// **别照「反正过期了」把那天加回来 —— 那会直接打破 60 这条上界。**
    #[test]
    fn the_day_excluded_by_clamping_has_not_actually_expired_yet() {
        let now = 1_800_000_000_000i64;
        let today = usage_day_index(now);
        let excluded = today - USAGE_DAY_RETAIN;
        assert!(
            usage_expires_at(excluded) > now,
            "那把键还活着（TTL 到 {}，now 是 {now}）",
            usage_expires_at(excluded)
        );
        let keys = usage_candidate_keys(now, 0, now);
        assert!(
            !keys.contains(&usage_day_key(excluded, 0)),
            "但它永远取不到"
        );
        assert!(keys.contains(&usage_day_key(today - USAGE_DAY_RETAIN + 1, 0)));
    }

    /// 日键里的日序号对负时间戳向下取整（1970 年之前也不许错一格）。
    #[test]
    fn day_index_floors_even_for_pre_epoch_timestamps() {
        assert_eq!(usage_day_index(0), 0);
        assert_eq!(usage_day_index(USAGE_DAY_MS - 1), 0);
        assert_eq!(usage_day_index(USAGE_DAY_MS), 1);
        assert_eq!(usage_day_index(-1), -1, "向零截断会给出 0，那是错一格");
        assert_eq!(usage_day_index(-USAGE_DAY_MS), -1);
    }

    // ── 日期换算 ───────────────────────────────────────────────────────────

    /// 🔴 **「日历上不存在的一天」只靠回读比对挡着。**
    ///
    /// `2024-02-31` 在朴素解析下拿得到一个正常数字（滚到 3 月 2 日）；
    /// 只有「解析出来的那一天再格式化回去逐字相等」这一步能拒掉它。
    #[test]
    fn a_date_that_does_not_exist_on_the_calendar_is_refused_not_rolled_over() {
        assert_eq!(parse_utc_date("2024-02-31"), None);
        assert_eq!(parse_utc_date("2026-02-31"), None);
        assert_eq!(parse_utc_date("2026-04-31"), None);
        // 闰日：2024 是闰年、2026 不是 —— 两个方向都要对。
        assert!(parse_utc_date("2024-02-29").is_some());
        assert_eq!(parse_utc_date("2026-02-29"), None);
        // 一眼可拒的那几档。
        assert_eq!(parse_utc_date("2026-13-01"), None);
        assert_eq!(parse_utc_date("2026-00-01"), None);
        assert_eq!(parse_utc_date("2026-9-10"), None);
        assert_eq!(parse_utc_date(""), None);
        assert_eq!(parse_utc_date("summary"), None);
    }

    /// 日序号 ↔ 日期串双向往返。**期望值手写**（纪元零点、一个闰年、一个近期日期）。
    #[test]
    fn the_civil_calendar_conversion_round_trips() {
        assert_eq!(utc_date_string(0).as_deref(), Some("1970-01-01"));
        assert_eq!(parse_utc_date("1970-01-01"), Some(0));
        assert_eq!(utc_date_string(-1).as_deref(), Some("1969-12-31"));
        assert_eq!(parse_utc_date("2000-02-29"), Some(11_016));
        assert_eq!(utc_date_string(11_016).as_deref(), Some("2000-02-29"));
        assert_eq!(parse_utc_date("2026-09-10"), Some(20_706));
        assert_eq!(utc_date_string(20_706).as_deref(), Some("2026-09-10"));
        // 表示不出来就 `None`，**不返回一个「大概对」的串**。
        assert_eq!(utc_date_string(10_000_000), None);
    }

    // ── 窄化与合并 ─────────────────────────────────────────────────────────

    /// 🔴 **键不存在不算畸形**；分片里某几条坏了才算。
    ///
    /// 反向同样要：把「不存在」计进 `malformed` 会让一个全新部署一上来就报
    /// 「有畸形数据」。
    #[test]
    fn absent_keys_are_not_malformed_but_broken_shards_are() {
        let m = merge_day_shards(&[None, None, None]);
        assert_eq!(m.malformed, 0);
        assert_eq!(m.shards, 0);

        let m = merge_day_shards(&[
            Some(shard_value(5, bucket(3), &[])),
            Some(json!("a string")),
            Some(json!({ "day": 5 })),               // 没有 total
            Some(json!({ "total": [] , "day": 5 })), // total 是数组
            None,
        ]);
        assert_eq!(m.shards, 1);
        assert_eq!(m.malformed, 3);
        assert_eq!(m.total.requests, 3);
    }

    /// ⚠️ **`total` 是数组时必须计进 `malformed`，不许当成一个全 0 桶。**
    ///
    /// 一个畸形分片伪装成合法的比被丢掉更糟：它还会把 `shards` 那个数一起变假。
    #[test]
    fn an_array_total_is_malformed_not_an_empty_bucket() {
        let m = merge_day_shards(&[Some(json!({ "day": 1, "total": [] }))]);
        assert_eq!(m.shards, 0, "不许算成合法分片");
        assert_eq!(m.malformed, 1);
    }

    /// `by_day` 按分片**自报的** `day` 分键 —— 而 `total` 数的是读到的每一个分片。
    ///
    /// ⇒ 一个自报 `day` 与它所在的键对不上的分片会进 `total` 而进不了那一天的 `days`。
    /// **方向是「`total` 只多不少」**，这一格把它钉住。
    #[test]
    fn total_counts_every_shard_while_by_day_keys_on_the_self_reported_day() {
        let m = merge_day_shards(&[
            Some(shard_value(10, bucket(2), &[])),
            Some(shard_value(999, bucket(5), &[])), // 自报的日子对不上
        ]);
        assert_eq!(m.total.requests, 7);
        assert_eq!(m.by_day.get(&10).unwrap().requests, 2);
        assert_eq!(m.by_day.get(&999).unwrap().requests, 5);
    }

    /// ⚠️ **`hours` 是跨天求和的 —— 这是契约不是实现细节。**
    ///
    /// 多日区间下 `hours["13"]` 是「区间里所有天的 13 点之和」。
    /// 拿多日区间的返回值去画单日小时图是错的，单日下钻因此必须只喂那一天的分片。
    #[test]
    fn hours_are_summed_across_days_which_is_why_the_drilldown_feeds_one_day_only() {
        let h = |n: i64| json!({ "13": serde_json::to_value(bucket(n)).unwrap() });
        let m = merge_day_shards(&[
            Some(shard_value(1, bucket(2), &[("hours", h(2))])),
            Some(shard_value(2, bucket(3), &[("hours", h(3))])),
        ]);
        assert_eq!(
            m.hours.get("13").unwrap().requests,
            5,
            "两天的 13 点被加在了一起 —— 这正是那条契约说的事"
        );
    }

    /// 桶里的坏值按 0 记，负数也按 0 记（计数没有负数这回事）。
    #[test]
    fn broken_numbers_in_a_bucket_degrade_to_zero() {
        let b = narrow_bucket(Some(&json!({
            "requests": 5, "success": "many", "errors": -3,
            "tokensIn": 1.9, "tokensOut": null,
        })))
        .unwrap();
        assert_eq!(b.requests, 5);
        assert_eq!(b.success, 0);
        assert_eq!(b.errors, 0);
        assert_eq!(b.tokens_in, 1, "截断，不是四舍五入");
        assert_eq!(b.tokens_out, 0);
    }

    /// 那几个曾经在 JS 里把 map 搞坏的模型名，在这里只是普通的键。
    ///
    /// ⚠️ 留着这一格不是因为这里还危险（`BTreeMap` 没有原型链），
    /// 而是因为这几个名字会**从客户端的请求体一路进到这里** ——
    /// 哪天有人把这一维改成某种「更聪明」的容器，它会当场红。
    #[test]
    fn model_names_that_used_to_break_javascript_are_just_ordinary_keys() {
        let names = ["__proto__", "toString", "hasOwnProperty", "constructor"];
        let mut by_model = Map::new();
        for n in names {
            by_model.insert(n.to_string(), serde_json::to_value(bucket(1)).unwrap());
        }
        let m = merge_day_shards(&[Some(shard_value(
            1,
            bucket(4),
            &[("byModel", Value::Object(by_model))],
        ))]);
        for n in names {
            assert_eq!(m.by_model.get(n).map(|b| b.requests), Some(1), "{n}");
        }
    }

    // ── 参数与区间 ─────────────────────────────────────────────────────────

    /// ⑥ 参数发错了 ⇒ **400（不带 `code`，走网关信封）**，不做善意纠正。
    #[tokio::test]
    async fn bad_range_parameters_are_a_400_not_a_silent_correction() {
        let h = tier2_harness().await;
        for q in [
            "?from=abc",
            "?to=abc",
            "?from=-1",
            "?to=-1",
            "?from=1.5",
            "?from=2026-09-10",
        ] {
            let (s, v) = get_json(&h, &format!("/admin/api/usage{q}")).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{q}");
            assert_eq!(v["error"]["type"], json!("invalid_request_error"), "{q}");
            assert!(v["error"]["code"].is_null(), "这一族刻意不带 code");
        }
        // `from > to` 用**夹逼之前**的那一对判。
        let now = crate::adapters::now_ms();
        let (s, _) = get_json(&h, &format!("/admin/api/usage?from={now}&to={}", now - 1)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    /// 🔴 **参数校验排在最前：Tier-2 关着时同样是 400，不是 200 + `tier:"off"`。**
    #[tokio::test]
    async fn parameters_are_validated_even_when_tier2_is_off() {
        let h = harness_with(Arc::new(MemoryStorage::new()), false).await;
        let (s, _) = get_json(&h, "/admin/api/usage?from=abc").await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    /// ⚠️⚠️ **档位映射：`from = to − (N−1) 天` 全部不夹；按 `N` 天发时 30d 恒被夹。**
    ///
    /// **两个方向都断言** —— 只写正向的话「clamped 恒为 false」的实现也全绿。
    /// 差一天的后果不是「多一天数据」，是那句警告**永久常驻**，说到运维不再看它。
    #[tokio::test]
    async fn the_four_preset_ranges_are_not_clamped_but_an_off_by_one_day_always_is() {
        let h = tier2_harness().await;
        let now = crate::adapters::now_ms();
        for n in [1i64, 3, 7, 30] {
            let from = now - (n - 1) * USAGE_DAY_MS;
            let (_, v) = get_json(&h, &format!("/admin/api/usage?from={from}&to={now}")).await;
            assert_eq!(
                v["range"]["clamped"],
                json!(false),
                "{n}d 档按 (N−1) 天发不该被夹"
            );
            assert_eq!(v["days"].as_array().unwrap().len() as i64, n, "{n}d");
        }
        // 差一天：`30d` 那一档恒被夹。
        let from = now - 30 * USAGE_DAY_MS;
        let (_, v) = get_json(&h, &format!("/admin/api/usage?from={from}&to={now}")).await;
        assert_eq!(v["range"]["clamped"], json!(true));
        assert_eq!(v["note"], json!("range_clamped"));
    }

    /// 晚于 `now` 的 `to` 被夹到 `now` ⇒ `clamped` 为真。
    ///
    /// ⚠️ 而整段区间都在未来时，回读的 `range.from` 会**大于** `range.to`：
    /// 那不是编出来的一对，它逐字就是服务端算出来的那个空窗口。
    /// **不把它「修正」成一段非空区间** —— 那才是编。
    #[tokio::test]
    async fn a_window_entirely_in_the_future_is_reported_as_the_empty_window_it_is() {
        let h = tier2_harness().await;
        let now = crate::adapters::now_ms();
        let from = now + 40 * USAGE_DAY_MS;
        let to = now + 41 * USAGE_DAY_MS;
        let (s, v) = get_json(&h, &format!("/admin/api/usage?from={from}&to={to}")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["range"]["clamped"], json!(true));
        assert!(
            v["range"]["from"].as_i64().unwrap() > v["range"]["to"].as_i64().unwrap(),
            "空窗口如实回读：{v}"
        );
        assert_eq!(v["days"], json!([]));
        assert_eq!(v["shards"], json!(0));
        assert_eq!(v["note"], json!("range_clamped"), "它压过 no_shards");
    }

    /// 不带参数 ⇒ 默认 `to = now`、`from = to − 1 天`，回读的是**取整之后**的那一对。
    ///
    /// ⚠️ **取整本身不算 `clamped`**：它是无条件适用的口径，
    /// 混进 `clamped` 会让面板对每一次正常请求都说「这段时间早于保留期」。
    #[tokio::test]
    async fn the_default_window_is_two_utc_days_and_rounding_is_not_clamping() {
        let h = tier2_harness().await;
        let (s, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["range"]["clamped"], json!(false));
        let days = v["days"].as_array().unwrap();
        assert_eq!(days.len(), 2, "昨天 + 今天");
        assert_eq!(days[1]["date"], json!(utc_date_string(today()).unwrap()));
        // 回读的是整天边界：起始日零点 ~ 结束日最后一毫秒。
        assert_eq!(
            v["range"]["from"].as_i64().unwrap() % USAGE_DAY_MS,
            0,
            "起点必须是某一天的零点"
        );
        assert_eq!(
            (v["range"]["to"].as_i64().unwrap() + 1) % USAGE_DAY_MS,
            0,
            "终点必须是某一天的最后一毫秒"
        );
    }

    // ── 八种状态 ───────────────────────────────────────────────────────────

    /// ① Tier-2 没开：**200 + `tier:"off"`**，数据块整块 `null`，`pending` 也是 `null`。
    ///
    /// 🔴 **不是 404、不是 503** —— 503 会让面板把「运维没打开统计」渲染成「后端坏了」。
    #[tokio::test]
    async fn tier2_off_is_a_200_with_tier_off_and_everything_else_null() {
        let h = harness_with(Arc::new(MemoryStorage::new()), false).await;
        let (s, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["tier"], json!("off"));
        assert_eq!(v["note"], json!("tier2_off"));
        for f in [
            "days",
            "total",
            "byApiKey",
            "shards",
            "malformed",
            "pending",
        ] {
            assert!(v[f].is_null(), "{f} 必须是 null：{v}");
        }
        // `range` 照常回读（参数是好的）。
        assert!(v["range"].is_object());
        assert_eq!(v["timezone"], json!("UTC"));
        assert_eq!(v["approximate"], json!(true));
    }

    /// 🔴 **关着的时候一次存储访问都不许有。**
    ///
    /// ⚠️ **代价明写**：这条性质**不是一行可以被变异掉的代码** —— 要证伪它得先把
    /// [`AppState::usage_wiring`] 改回「存储恒在 + 一个布尔」。
    /// 这条断言咬的是「谁把接线改回两格」，不是「谁在 handler 里写错了一个 if」。
    #[tokio::test]
    async fn with_tier2_off_the_endpoint_reads_storage_zero_times() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, false).await;
        let before = counts.snapshot();
        for uri in ["/admin/api/usage", "/admin/api/usage/2026-09-10"] {
            let (s, v) = get_json(&h, uri).await;
            assert_eq!(s, StatusCode::OK);
            assert_eq!(v["tier"], json!("off"));
        }
        assert_eq!(counts.snapshot(), before, "关着就不许碰存储");
    }

    /// ② 读不出来 ⇒ `read_failed`，五个字段**同生同死**地是 `null`。
    ///
    /// 🔴 **不是 500**：`tier` / `range` 这两块都还是真的，整条 500 会把它们一起丢掉。
    /// 🔴 **更不许把读到的那部分当成全份交出去。**
    #[tokio::test]
    async fn a_partial_read_failure_nulls_the_whole_block_rather_than_reporting_what_it_got() {
        let storage = Arc::new(MemoryStorage::new());
        let h = harness_with(storage.clone(), true).await;
        let d = today();
        seed(&h, d, 0, shard_value(d, bucket(9), &[])).await;
        // 扇出里的某一次抛错 —— **整块失败**，那 9 次请求不许被端出去。
        storage.fail_next("磁盘坏了");
        let (s, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["tier"], json!("tier2"));
        assert_eq!(v["note"], json!("read_failed"));
        for f in ["days", "total", "byApiKey", "shards", "malformed"] {
            assert!(v[f].is_null(), "{f} 必须一起变 null：{v}");
        }
        assert!(v["range"].is_object(), "range 还是真的，不许一起丢掉");
        assert!(!v.to_string().contains("磁盘坏了"), "内部错误文本不许回显");
    }

    /// ③ 区间里一个分片都没有 ⇒ `no_shards`，`days` **每天一格、全是 0 桶**。
    ///
    /// 那些 0 **不是伪造**：这一天的候选键都真的被读过，「没有记录」就是这次读的
    /// 真实结果。真正被禁止的是「读失败了却报 0」，而那一档由整块 `null` 承担。
    #[tokio::test]
    async fn an_empty_range_gives_one_zero_bucket_per_day_and_says_no_shards() {
        let h = tier2_harness().await;
        let (_, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(v["note"], json!("no_shards"));
        assert_eq!(v["shards"], json!(0));
        assert_eq!(v["malformed"], json!(0));
        let days = v["days"].as_array().unwrap();
        assert_eq!(days.len(), 2);
        for d in days {
            assert_eq!(d["total"]["requests"], json!(0));
        }
    }

    /// ③ 与 ④ 是两件事 —— **判据是 `shards`，不是 `total`。**
    ///
    /// ④：有实例落过盘，那些盘里的请求数就是 0 ⇒ `shards > 0`、`note` 是 `null`。
    /// 压成同一个「今日请求数 0」会把「没人用」与「实例没活到一次落盘」混成一件事。
    #[tokio::test]
    async fn a_shard_that_recorded_zero_requests_is_not_the_same_as_no_shard_at_all() {
        let h = tier2_harness().await;
        let d = today();
        seed(&h, d, 0, shard_value(d, UsageBucket::default(), &[])).await;
        let (_, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(v["shards"], json!(1));
        assert_eq!(v["total"]["requests"], json!(0));
        assert!(v["note"].is_null(), "有分片就不是 no_shards：{v}");
    }

    /// ⑦ 分片都在、但**每一个都是畸形的** ⇒ `all_malformed` 而不是 `no_shards`。
    ///
    /// 🔴 后者是**一句假话**：面板照 note 分支会把「读到了 N 个分片、但每个都坏」
    /// 说成「这段时间一个分片都没有」，而两者的处置完全不同。
    #[tokio::test]
    async fn shards_that_are_all_broken_say_all_malformed_not_no_shards() {
        let h = tier2_harness().await;
        let d = today();
        seed(&h, d, 0, json!("garbage")).await;
        seed(&h, d, 1, json!({ "day": d })).await;
        let (_, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(v["shards"], json!(0));
        assert_eq!(v["malformed"], json!(2));
        assert_eq!(v["note"], json!("all_malformed"));
    }

    /// ⑧ **一部分**分片畸形 ⇒ `partial_malformed`，**不是 `null`**。
    ///
    /// 🔴 走 `null`（「一切正常」那一支）就是把**一份缺了几个分片的数据**
    /// 渲染成完整的 —— **伪造的不只是 `0`，还有「这份数据是全的」这个印象**。
    #[tokio::test]
    async fn a_partially_broken_range_is_not_reported_as_perfectly_fine() {
        let h = tier2_harness().await;
        let d = today();
        seed(&h, d, 0, shard_value(d, bucket(4), &[])).await;
        seed(&h, d, 1, json!("garbage")).await;
        let (_, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(v["shards"], json!(1));
        assert_eq!(v["malformed"], json!(1));
        assert_eq!(v["note"], json!("partial_malformed"));
    }

    /// 🔴🔴 **畸形那两条压过 `range_clamped`，顺序订正过一次。**
    ///
    /// 反的那个顺序与「30d 按钮差一天」那个场景叠起来会变成**永久静音**：
    /// `clamped` 恒为真 ⇒ 存储坏成什么样面板都只会说「这段时间早于保留期」。
    /// 判据是「谁需要人去查」：畸形要人去查存储，被夹只是一句提示。
    #[tokio::test]
    async fn malformed_outranks_range_clamped_so_a_broken_store_is_never_silenced() {
        let h = tier2_harness().await;
        let d = today();
        seed(&h, d, 0, json!("garbage")).await;
        let now = crate::adapters::now_ms();
        // 差一天的 30d 发法 ⇒ `clamped` 恒真。
        let from = now - 30 * USAGE_DAY_MS;
        let (_, v) = get_json(&h, &format!("/admin/api/usage?from={from}&to={now}")).await;
        assert_eq!(v["range"]["clamped"], json!(true));
        assert_eq!(
            v["note"],
            json!("all_malformed"),
            "被夹的提示不许把「存储坏了」盖掉：{v}"
        );
    }

    /// 🔴 **`note` 与它自己的判据字段恒一致 —— `note` 是摘要不是第二个真源。**
    ///
    /// 每一条 code（含 `null`）都有一个干净的双条件。**顺序一改这一格就红。**
    #[test]
    fn note_never_contradicts_the_fields_it_summarises() {
        // (shards, malformed, clamped) → note
        //
        // ⚠️ **`shards == 1` 那一行是变异复检补进来的**：原表只有 0 和 3，
        //    于是把 `shards == 0` 写成 `shards <= 1`（一次很自然的「差一」笔误）
        //    **这一格完全测不出来**（实测 ESCAPED，只有 ④ 那一格红了）。
        //    边界值要自己在表里出现，不能指望「0 和 3 之间的都一样」。
        let cases: [(usize, usize, bool, Option<UsageNote>); 10] = [
            (0, 0, false, Some(UsageNote::NoShards)),
            (0, 0, true, Some(UsageNote::RangeClamped)),
            (1, 0, false, None),
            (1, 0, true, Some(UsageNote::RangeClamped)),
            (3, 0, false, None),
            (3, 0, true, Some(UsageNote::RangeClamped)),
            (0, 2, false, Some(UsageNote::AllMalformed)),
            (0, 2, true, Some(UsageNote::AllMalformed)),
            (3, 2, false, Some(UsageNote::PartialMalformed)),
            (3, 2, true, Some(UsageNote::PartialMalformed)),
        ];
        for (shards, malformed, clamped, want) in cases {
            assert_eq!(
                summarise(shards, malformed, clamped),
                want,
                "({shards}, {malformed}, {clamped})"
            );
        }
    }

    /// 🔴 **手写的 `as_str` 与 serde 派生的那张表必须逐字相同。**
    ///
    /// 两侧漂了的后果是**静默的**：这条端点会一边往 `note` 里发 `read_failed`，
    /// 一边把 `view.noteSeverity` 算成「表外 ⇒ warn」那一档 ——
    /// 面板于是拿一条黄提示去说一件要人去查存储的事。
    ///
    /// 回归对象：加一条 code 时改了枚举名而忘了改 [`UsageNote::as_str`]。
    #[test]
    fn the_note_wire_names_match_what_serde_emits() {
        for n in USAGE_NOTES {
            let emitted = serde_json::to_value(n).expect("note 可序列化");
            assert_eq!(
                emitted,
                Value::String(n.as_str().to_string()),
                "{n:?} 的手写字面量与 serde 产出的串对不上"
            );
        }
    }

    /// 🔴 **`USAGE_NOTES` 必须是全集**（不多不少），而且**每一条都有一档显式的横幅**。
    ///
    /// 「全集」那一半靠 serde：把一个不在表里的枚举成员漏掉时，`USAGE_NOTES`
    /// 的长度与实际成员数就对不上 —— 这里用「九条互不相同的串」把它钉住。
    ///
    /// 「每一条都有显式一档」那一半盯的是
    /// [`crate::core::admin::view::usage::note_severity`] 的兜底：那条兜底**刻意保守**
    /// （表外归 `warn`），它要盖的是「后端将来加了第十条」，**不是**「今天这九条里
    /// 有几条没人管」。少一条 `match` 臂在行为上看不出来，只能靠对表。
    #[test]
    fn every_note_code_has_an_explicit_banner_severity() {
        use crate::core::admin::view::usage::{NoteSeverity, note_severity};

        let mut names: Vec<&str> = USAGE_NOTES.iter().map(|n| n.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 9, "九条 code 必须互不相同");

        // 表外的那一档长什么样 —— 拿它当对照组。
        let fallback = note_severity(Some("definitely_not_a_real_code"));
        assert_eq!(fallback, Some(NoteSeverity::Warn));

        // 九条里恰好一条（`range_clamped`）与兜底同档，其余八条都必须与它不同。
        // ⚠️ 这一格因此不能写成「九条都 != 兜底」—— 那条断言今天就是假的。
        let same_as_fallback: Vec<&str> = USAGE_NOTES
            .iter()
            .filter(|n| note_severity(Some(n.as_str())) == fallback)
            .map(|n| n.as_str())
            .collect();
        assert_eq!(
            same_as_fallback,
            vec!["range_clamped"],
            "只有 range_clamped 允许与「表外兜底」同档；多出来的那几条八成是漏写了 match 臂"
        );
    }

    /// 🔴 **Tier-2 关着时，`view.state` 说的是「没开」，不是「没人用」。**
    ///
    /// 这是整块视图存在的理由：六张卡在这一档下全是 EM DASH（`unknown`），
    /// 而「API 密钥」那一屏也拿得到一句独立的「没开」——**不是一个 0**。
    #[tokio::test]
    async fn with_tier2_off_the_view_says_off_rather_than_an_empty_day() {
        let h = harness_with(Arc::new(MemoryStorage::new()), false).await;
        let (st, body) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(st, StatusCode::OK);
        let v = &body["view"];
        assert_eq!(v["state"], "off");
        assert_eq!(v["readSucceeded"], false);
        assert_eq!(v["apiKeyUsageKind"], "off");
        assert_eq!(v["noteSeverity"], "info");
        assert_eq!(v["malformedKind"], "unknown");
        for card in [
            "requests",
            "successRate",
            "errorRate",
            "avgLatency",
            "tokens",
            "streaming",
        ] {
            assert_eq!(v["cards"][card], "unknown", "{card}");
        }
        assert!(
            v["dayRows"]
                .as_array()
                .expect("dayRows 恒是数组")
                .is_empty()
        );
    }

    /// 🔴 **「一条分片都还没落盘」与「这段时间真的是 0 次」在 `view` 上分得开。**
    ///
    /// 回归对象：两者压成同一个「今日请求数 0」。前者 `no-shards`（六张卡照旧写 0，
    /// 但没有数字可写的那三格换一句 tooltip），后者 `empty`。
    #[tokio::test]
    async fn the_view_splits_not_landed_yet_from_a_real_zero() {
        let h = tier2_harness().await;
        let (_, empty_range) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(empty_range["view"]["state"], "no-shards");
        assert_eq!(empty_range["view"]["apiKeyUsageKind"], "no-shards");
        assert_eq!(empty_range["view"]["readSucceeded"], true);
        // 计数格照旧写 0（`value`），没有样本的那几格换第四档。
        assert_eq!(empty_range["view"]["cards"]["requests"], "value");
        assert_eq!(empty_range["view"]["cards"]["avgLatency"], "none-no-shards");
        assert_eq!(
            empty_range["view"]["cards"]["successRate"],
            "none-no-shards"
        );
        for row in empty_range["view"]["dayRows"].as_array().expect("有行") {
            assert_eq!(row["state"], "no-shards");
        }

        // 落一个「确实读到了分片，只是请求数是 0」的分片。
        seed(&h, today(), 0, shard_value(today(), bucket(0), &[])).await;
        let (_, real_zero) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(real_zero["view"]["state"], "empty");
        assert_eq!(real_zero["view"]["apiKeyUsageKind"], "value");
        assert_eq!(real_zero["view"]["cards"]["requests"], "value");
        assert_eq!(
            real_zero["view"]["cards"]["avgLatency"], "none",
            "读成功、确实没有样本 —— 与上面那一档同字形、不同 tooltip"
        );
    }

    /// 🔴 **分片全坏时整块判「读不出来」，而那个判断必须一路传到日表的每一行。**
    ///
    /// 回归对象（TS 侧终检点名的那一处）：顶部六张卡刚说完「我们不知道」，
    /// 紧挨着的日表把同一段区间写成「请求 0 次」——因为那一版的行状态**完全不看
    /// 整块状态**。
    #[tokio::test]
    async fn an_all_malformed_verdict_reaches_every_row_of_the_day_table() {
        let h = tier2_harness().await;
        seed(&h, today(), 0, Value::String("这不是一个分片".into())).await;
        let (_, body) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(body["note"], "all_malformed");
        assert_eq!(body["view"]["state"], "unavailable");
        assert_eq!(body["view"]["malformedKind"], "all");
        assert_eq!(body["view"]["noteSeverity"], "error");
        assert_eq!(body["view"]["readSucceeded"], false);
        assert_eq!(body["view"]["cards"]["requests"], "unknown");
        let rows = body["view"]["dayRows"].as_array().expect("有行");
        assert!(!rows.is_empty(), "日表照常有行，只是每一行都说「不知道」");
        for row in rows {
            assert_eq!(row["state"], "unavailable", "{row}");
        }
    }

    /// 🔴 **一部分分片坏 ⇒ 数字是真的、只是不全** ⇒ `complete: false`，**不是早退**。
    ///
    /// 「伪造的不只是 0，还有『这份数据是全的』这个印象。」
    #[tokio::test]
    async fn a_partially_broken_range_is_marked_incomplete_rather_than_unavailable() {
        let h = tier2_harness().await;
        seed(&h, today(), 0, shard_value(today(), bucket(5), &[])).await;
        seed(&h, today(), 1, Value::String("坏的".into())).await;
        let (_, body) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(body["view"]["state"], "data", "好分片里的数字照常展示");
        assert_eq!(body["view"]["malformedKind"], "partial");
        assert_eq!(body["view"]["complete"], false);
        assert_eq!(body["view"]["noteSeverity"], "error");
    }

    /// 整块读不出来时 `complete` 是 `true` —— **没有数据就谈不上残缺**。
    ///
    /// 回归对象：在 EM DASH 的六张卡上再叠一个「不完整」标记，
    /// 让运维以为「有一部分是好的」。
    #[tokio::test]
    async fn an_unreadable_block_is_not_also_labelled_incomplete() {
        let h = harness_with(Arc::new(MemoryStorage::new()), false).await;
        let (_, body) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(body["view"]["state"], "off");
        assert_eq!(body["view"]["complete"], true);
    }

    /// 🔴 **单日下钻的小时表行序是数值序，不是 map 的字典序。**
    ///
    /// 回归对象：面板照 `hours` 那个对象的自然序读出来是 0,1,10,11,…,2,20,…
    /// —— 一张乱序的时间轴，而且它看起来完全像是数据错了。
    #[tokio::test]
    async fn the_drilldown_hands_the_panel_a_numerically_ordered_hour_axis() {
        let h = tier2_harness().await;
        let day = today();
        let mut hours = Map::new();
        for hour in ["2", "10", "9", "23", "0"] {
            hours.insert(hour.to_string(), serde_json::to_value(bucket(1)).unwrap());
        }
        seed(
            &h,
            day,
            0,
            shard_value(day, bucket(5), &[("hours", Value::Object(hours))]),
        )
        .await;
        let date = utc_date_string(day).expect("日期串");
        let (st, body) = get_json(&h, &format!("/admin/api/usage/{date}")).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["view"]["state"], "data");
        assert_eq!(
            body["view"]["hourOrder"],
            serde_json::json!(["0", "2", "9", "10", "23"]),
            "字典序会给出 0,10,2,23,9"
        );
    }

    /// 单日下钻**没有 `empty` 这一档**，而 `date_out_of_retention` 归 `unavailable`
    /// ——**那天的记录已经不在了，不是那天没有请求。**
    #[tokio::test]
    async fn a_drilldown_outside_retention_is_unavailable_not_an_idle_day() {
        let h = tier2_harness().await;
        let old = utc_date_string(today() - USAGE_DAY_RETAIN).expect("日期串");
        let (_, body) = get_json(&h, &format!("/admin/api/usage/{old}")).await;
        assert_eq!(body["note"], "date_out_of_retention");
        assert_eq!(body["view"]["state"], "unavailable");
        assert_eq!(body["view"]["readSucceeded"], false);
        assert_eq!(body["view"]["noteSeverity"], "info");
        assert!(
            body["view"]["hourOrder"]
                .as_array()
                .expect("恒是数组")
                .is_empty()
        );
    }

    /// 三个保留伪桶各有一条文案键；**认不出来的 id 压根不进这张表**
    /// —— 面板照实画那个原始 id，不猜、也不标「已删除」。
    #[tokio::test]
    async fn the_drilldown_labels_only_the_reserved_api_key_buckets() {
        let h = tier2_harness().await;
        let day = today();
        let mut by_key = Map::new();
        for id in ["master", "__other__", "a1b2c3d4e5f6"] {
            by_key.insert(id.to_string(), serde_json::to_value(bucket(1)).unwrap());
        }
        seed(
            &h,
            day,
            0,
            shard_value(day, bucket(3), &[("byApiKey", Value::Object(by_key))]),
        )
        .await;
        let date = utc_date_string(day).expect("日期串");
        let (_, body) = get_json(&h, &format!("/admin/api/usage/{date}")).await;
        let labels = body["view"]["apiKeyLabelKeys"]
            .as_object()
            .expect("恒是对象");
        assert_eq!(labels["master"], "usage.key.master");
        assert_eq!(labels["__other__"], "usage.key.other");
        assert!(
            !labels.contains_key("a1b2c3d4e5f6"),
            "认不出来的 id 不许被猜成任何一档"
        );
    }

    /// 🔴 **30 天那一档恰好 60 次 `get`，一次不多一次不少。**
    ///
    /// 这条代价必须是可观测的：`FileStorage` 上每一次 `get` 都是整份 `store.json`
    /// 的读 + 解析 —— **60 次就是 60 遍**。
    #[tokio::test]
    async fn a_thirty_day_range_costs_exactly_sixty_gets() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, true).await;
        let now = crate::adapters::now_ms();
        let from = now - 29 * USAGE_DAY_MS;
        let before = counts.snapshot();
        let (s, v) = get_json(&h, &format!("/admin/api/usage?from={from}&to={now}")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["days"].as_array().unwrap().len(), 30);
        let after = counts.snapshot();
        assert_eq!(
            after.0 - before.0,
            (USAGE_DAY_RETAIN as u64) * (USAGE_SLOTS as u64),
            "30 天 = 30 × 槽位数 次 get"
        );
        assert_eq!(after.3, before.3, "读路径不许调 list");
        assert_eq!(after.1, before.1, "读路径不许写");
    }

    /// ⚠️ **本端点不带 `tokensCoverage`** —— 同一份知识只许有一个出口
    /// （那个出口是 `capabilities`）。
    #[tokio::test]
    async fn the_usage_endpoint_does_not_carry_tokens_coverage() {
        let h = tier2_harness().await;
        let (_, v) = get_json(&h, "/admin/api/usage").await;
        assert!(
            !v.as_object().unwrap().contains_key("tokensCoverage"),
            "两个出口 = 前端要面对「读哪一个」这个不该存在的问题"
        );
    }

    /// ⚠️ **`pending` 今天恒为 `null`，而这是「我们不知道」不是「没有尾巴」。**
    ///
    /// 回归对象：顺手把它填成 `{count:0, ms:0, budgetExhausted:false}` 把这一格
    /// 「补齐」⇒ 面板会说「没有未落盘的尾巴」，而真相是我们根本拿不到那个数。
    #[tokio::test]
    async fn pending_is_null_because_the_sink_placeholder_cannot_report_its_tail() {
        let h = tier2_harness().await;
        let (_, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(v["tier"], json!("tier2"));
        assert!(
            v.as_object().unwrap().contains_key("pending"),
            "字段要在，值是 null"
        );
        assert!(v["pending"].is_null());
    }

    // ── 单日下钻 ───────────────────────────────────────────────────────────

    /// `date` 只认 UTC 的 `YYYY-MM-DD`，其余 400。
    ///
    /// ⚠️ **与区间端点的口径刻意不同**：那条只认 epoch 毫秒，两者不通用。
    #[tokio::test]
    async fn the_drilldown_only_accepts_a_utc_date_string() {
        let h = tier2_harness().await;
        for d in ["1735689600000", "2024-02-31", "2026-13-01", "yesterday"] {
            let (s, v) = get_json(&h, &format!("/admin/api/usage/{d}")).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{d}");
            let msg = v["error"]["message"].as_str().unwrap();
            assert!(msg.contains("YYYY-MM-DD"), "{msg}");
            assert!(msg.contains(d), "报文里要带上收到的那个串：{msg}");
        }
    }

    /// ★ **`/usage` 必须排在 `/usage/{date}` 之前。**
    ///
    /// 实测过的失效形态：追加一条 `GET /admin/api/usage/summary` 到 `{date}` 之后，
    /// 它**恒回**一个看起来很合理的
    /// `400 date 必须是 UTC 的 YYYY-MM-DD，收到的是：summary`。
    /// axum 的 matchit 让静态段天然优先于 `{param}`，Rust 侧这条风险本身会消失 ——
    /// **但必须写判据钉住，否则将来换匹配器会复发。**
    #[tokio::test]
    async fn the_bare_usage_path_is_never_swallowed_by_the_date_route() {
        let h = tier2_harness().await;
        let (s, v) = get_json(&h, "/admin/api/usage").await;
        assert_eq!(s, StatusCode::OK);
        assert!(
            v.as_object().unwrap().contains_key("range"),
            "落到 {{date}} 上的话这里会是一条 400：{v}"
        );
        assert!(
            !v.as_object().unwrap().contains_key("date"),
            "区间端点没有 date 这一格：{v}"
        );
    }

    /// 那一天整个落在保留期之外 ⇒ `date_out_of_retention`。
    ///
    /// 🔴 **不是「那天没有请求」，是那天已经不在了** ——
    /// 报成 `no_shards` 会让运维以为那天真的没人用。
    /// 未来的日子走同一支（那天还没到）。
    #[tokio::test]
    async fn a_day_outside_the_retention_window_says_so_instead_of_claiming_it_was_idle() {
        let h = tier2_harness().await;
        for day in [today() - USAGE_DAY_RETAIN, today() + 1] {
            let date = utc_date_string(day).unwrap();
            let (s, v) = get_json(&h, &format!("/admin/api/usage/{date}")).await;
            assert_eq!(s, StatusCode::OK, "{date}");
            assert_eq!(v["note"], json!("date_out_of_retention"), "{date}");
            for f in ["hours", "byModel", "byProtocol", "byApiKey", "shards"] {
                assert!(v[f].is_null(), "{f} @ {date}");
            }
        }
        // 边界内那一天走的是另一支。
        let date = utc_date_string(today() - USAGE_DAY_RETAIN + 1).unwrap();
        let (_, v) = get_json(&h, &format!("/admin/api/usage/{date}")).await;
        assert_eq!(v["note"], json!("no_request_detail"));
    }

    /// 成功那一支：`note` 恒是 `no_request_detail`，且 `hours` **只含有过流量的小时**。
    ///
    /// ⚠️ **缺席的小时不是「读不出来」**：「这一天读成功了没有」由 `hours`
    /// **整块**是不是 `null` 承担，面板对缺席的小时画 0 是对的。
    #[tokio::test]
    async fn a_successful_drilldown_reports_only_the_hours_that_saw_traffic() {
        let h = tier2_harness().await;
        let d = today();
        seed(
            &h,
            d,
            0,
            shard_value(
                d,
                bucket(4),
                &[
                    (
                        "hours",
                        json!({ "13": serde_json::to_value(bucket(4)).unwrap() }),
                    ),
                    (
                        "byProtocol",
                        json!({ "openai": serde_json::to_value(bucket(4)).unwrap() }),
                    ),
                ],
            ),
        )
        .await;
        let date = utc_date_string(d).unwrap();
        let (s, v) = get_json(&h, &format!("/admin/api/usage/{date}")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["tier"], json!("tier2"));
        assert_eq!(v["date"], json!(date));
        assert_eq!(v["note"], json!("no_request_detail"));
        assert_eq!(v["shards"], json!(1));
        let hours = v["hours"].as_object().unwrap();
        assert_eq!(hours.len(), 1, "只有 13 点有流量：{v}");
        assert_eq!(hours["13"]["requests"], json!(4));
        assert_eq!(v["byProtocol"]["openai"]["requests"], json!(4));
        // 这一天没有模型维数据 ⇒ 空对象，**不是 null**（读成功了）。
        assert_eq!(v["byModel"], json!({}));
    }

    /// 单日下钻只读那一天的候选键（2 次 get），**不是整段保留期**。
    ///
    /// 它同时是「`hours` 跨天求和」那条契约的落点：多读一天就会把别的天的 13 点
    /// 加进来，而面板画的是单日小时图。
    #[tokio::test]
    async fn the_drilldown_reads_exactly_one_days_worth_of_keys() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let h = harness_with(counting, true).await;
        let date = utc_date_string(today()).unwrap();
        let before = counts.snapshot();
        let (s, _) = get_json(&h, &format!("/admin/api/usage/{date}")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(counts.snapshot().0 - before.0, USAGE_SLOTS as u64);
    }

    /// 单日下钻读不出来 ⇒ `read_failed`，六个字段一起 `null`。
    #[tokio::test]
    async fn a_failed_drilldown_reports_read_failed_with_everything_null() {
        let storage = Arc::new(MemoryStorage::new());
        let h = harness_with(storage.clone(), true).await;
        storage.fail_next("磁盘坏了");
        let date = utc_date_string(today()).unwrap();
        let (s, v) = get_json(&h, &format!("/admin/api/usage/{date}")).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["note"], json!("read_failed"));
        for f in [
            "hours",
            "byModel",
            "byProtocol",
            "byApiKey",
            "shards",
            "malformed",
        ] {
            assert!(v[f].is_null(), "{f}");
        }
    }
}
