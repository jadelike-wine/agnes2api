//! key 池的两个持久化结构：[`KeyRecord`]（一把上游 key）与 [`KeyStats`]（它的 Tier-1 计数）。
//!
//! ⚠️⚠️ **本仓有两个都叫 key 的东西，方向相反**（01 §8.0）：
//! 这里是**上游 key 池** —— 我们**持有**、拿去向 Agnes 证明身份的那一族，
//! 它**必须存明文**（要拿去用）。我们**签发**给下游客户的那一族（`sk-*`）在
//! [`crate::core::keys::api_keys`]。读混的后果是把上游 key 的明文当成「自家签发的」展示出去。
//!
//! # 🔴 裁定 B：这两个结构**只派生 `Serialize`**
//!
//! 存储里读回来的东西一律先是 `serde_json::Value`，再由 [`crate::core::narrow`]
//! 手写窄化。理由（规格 §3.6）：TS 里 `disabled: "true"` 被 `JSON.parse` 原样带进来、
//! 代码显式判真值性，**记录仍然可用**；而 Rust 的 `serde` 遇到 `"true"` 给 `bool` 字段会
//! **整条记录反序列化失败** ⇒ **这把 key 直接从池子里消失**。
//! 派生 `Deserialize` 就等于把那条分岔重新打开，所以这里一格都不派生。
//!
//! # 字段名是存储契约
//!
//! `#[serde(rename_all = "camelCase")]` 不是风格选择：TS 版写进 `store.json` 的就是
//! `cooldownUntil` / `lastUsedAt` / `clientErrors` 这些名字，**改了就读不回既有部署的数据**。

use serde::Serialize;

use crate::core::config::GatewayConfig;

/// Tier-1 用量埋点。**近似值**，两条误差要在面板上打 `≈`（`types.ts:1-16`）：
/// 并发请求下会少计（存储没有 CAS），且计数最多晚一个 `POOL_TOUCH_INTERVAL_MS` 落盘。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStats {
    /// 终态归因到这把 key 的请求总数。
    /// **不变式：`requests == success + failed + client_errors`。**
    pub requests: u64,
    /// 上游 2xx。
    pub success: u64,
    /// 归因到**这把 key** 的失败：超时 / 5xx / 非 JSON 响应体 / 凭据失效 / 限流。
    pub failed: u64,
    /// 上游 4xx 直通。**客户端的错，不是这把 key 的错**，所以单列，不并进 `failed`。
    pub client_errors: u64,
    pub last_error_at: Option<i64>,
    pub last_error_kind: Option<String>,
}

impl KeyStats {
    /// 全零基线。
    ///
    /// TS 那边是 `Object.freeze` 的一个模块级常量；Rust 侧用 `const fn` 拿到同一个东西，
    /// 而且天然没有「谁不小心改了这个共享对象」这个问题。
    pub const fn empty() -> Self {
        Self {
            requests: 0,
            success: 0,
            failed: 0,
            client_errors: 0,
            last_error_at: None,
            last_error_kind: None,
        }
    }
}

impl Default for KeyStats {
    fn default() -> Self {
        Self::empty()
    }
}

/// Key 所属的固定上游线路；不接受任意 URL，避免把凭据发往未知服务。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyUpstream {
    International,
    InternationalBackup,
    China,
}

impl KeyUpstream {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "international" => Some(Self::International),
            "internationalBackup" => Some(Self::InternationalBackup),
            "china" => Some(Self::China),
            _ => None,
        }
    }

    pub const fn base_url(self) -> &'static str {
        match self {
            Self::International => "https://apihub.agnes-ai.com/v1",
            Self::InternationalBackup => "https://apihub.agnes-ai.cn/v1",
            Self::China => "https://api.agnes-ai.cn/v1",
        }
    }
}

/// 一把上游 key 的持久化形态（存储键 `key:<id>`，`types.ts:18-52`）。
///
/// # 🔴 `disabled` 与 `evicted` 语义分离，不许合并（`types.ts:31-40`）
///
/// 前者是**人的决定**、随时可撤销；后者是**系统按 401/403 的判定**、要换 key。
/// 混为一谈会让 503 把运维指向一件完全没用的事（「去换 key」，而其实是他自己关的）。
///
/// # ⚠️ 三个可选字段必须**真的可选**
///
/// `stats` / `disabled` / `note` 写成必填的话，存量记录读回来是「没有这一格」，
/// 而 JSON 序列化会把该字段**整个丢掉** ⇒ 面板收到的是「字段不存在」而不是 `false`，
/// **分不清「没停用」和「读不出来」**。所以这三格：
/// `Option<..>` + `skip_serializing_if`，**存储里没有就是没有**。
///
/// 🔴 反过来，投影到面板时（[`crate::core::keys::view::KeyView`]）必须落成实打实的
/// `bool` / `Option<String>` 且**恒存在**（01 §14 那条「`c.json` 丢掉 `undefined`
/// 字段」在 serde 的 `skip_serializing_if` 下会**原样复现**）。
///
/// ⚠️ `last_used_at` / `cooldown_reason` / `evicted_reason` **不在**上面那三格里：
/// 它们在 TS 里是 `number | null` / `string | null`（**必填、可空**），
/// 所以这里是 `Option<T>` 但**照常序列化成 `null`，不许 skip**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyRecord {
    /// SHA-256 前 8 字节的 hex（16 字符）。计算在 `pool/repo.rs`（那里允许摘要）。
    pub id: String,
    /// **明文**。要拿去拼 `authorization: Bearer <key>`，所以必须存。
    /// 🔴 它**永不进** [`crate::core::keys::view::KeyView`]、永不进日志、永不进事件。
    pub key: String,
    /// 缺席表示沿用全局上游，兼容存量记录及旧版导入客户端。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<KeyUpstream>,
    pub added_at: i64,
    pub last_used_at: Option<i64>,
    /// `0` = 不在冷却。
    pub cooldown_until: i64,
    /// 当前这段冷却因何而起（限流 / 欠费 / 瞬时故障累计）。
    /// **仅供排障，不参与调度** —— 调度只看 `cooldown_until`。
    pub cooldown_reason: Option<String>,
    pub strikes: u32,
    /// 系统按 401/403 判定的凭据失效。**永久**，只有人工能撤销。
    pub evicted: bool,
    pub evicted_reason: Option<String>,
    /// 调度优先级，**越小越先用**（裁定 F 第 2 步，[`crate::core::keypool::LbMode::Priority`] 的依据）。
    ///
    /// 默认 0。规格 §3.10 钉的是 0（kiro2api 那边导入的号一律 999「最低」，
    /// 那是它的运营口径，不是本仓的）。**它属于 Scheduling 档**（01 §4.3）：
    /// 改它会改变「先用哪一把」，所以**不许进写消除白名单**。
    pub priority: i64,
    /// 权重。**P1 建了字段、没有任何消费者**（规格 §4.2：B4/B5「P1 只留字段，不接逻辑」）。
    ///
    /// ⚠️ 老实说清它今天的状态：[`LbMode::Balanced`](crate::core::keypool::LbMode::Balanced)
    /// 是 TS 游标轮询的**逐字移植**，**不带权重**；kiro2api 的 balanced 才是按权重展开的。
    /// 🔴 **在它有真正的消费者之前，不许把它投影进任何面板响应** ——
    /// 一个没有生产者也没有消费者的字段画到面板上，正是本仓裁掉过四次的那个形态。
    pub weight: u32,
    /// Tier-1 用量埋点。**可选**：存量记录没有它，读取处统一走
    /// [`crate::core::stats::normalize_stats`] 补默认值。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<KeyStats>,
    /// 管理员**手工**停用。**可选**（缺席 = 没停用过）。
    /// 判据一律走 [`crate::core::keypool::is_disabled`]，**一处都不许自己判**。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// 运维备注。**可选**。
    ///
    /// 🔴 它是唯一一个「运维自由输入、又会被投影到面板」的字段。
    /// 后端刻意**不做转义**（JSON 响应体的字段边界由 JSON 语法本身保证，转义是渲染层的事）
    /// ⇒ **渲染那一侧必须走 `textContent`，绝不许拼进 innerHTML**。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl KeyRecord {
    pub fn upstream_base_url<'a>(&self, fallback: &'a str) -> &'a str {
        match self.upstream {
            Some(upstream) => upstream.base_url(),
            None => fallback,
        }
    }

    /// 一条「刚导入 / 刚铸出来」的干净记录。
    ///
    /// 三个可选字段一律**缺席**（不是 `Some(false)` / `Some("")`）：新记录与存量记录在
    /// 「这一格读不出来吗」这个问题上必须给出同一个答案 —— 都是「没有这一格」。
    pub fn fresh(id: impl Into<String>, key: impl Into<String>, now: i64) -> Self {
        Self {
            id: id.into(),
            key: key.into(),
            upstream: None,
            added_at: now,
            last_used_at: None,
            cooldown_until: 0,
            cooldown_reason: None,
            strikes: 0,
            evicted: false,
            evicted_reason: None,
            priority: DEFAULT_KEY_PRIORITY,
            weight: DEFAULT_KEY_WEIGHT,
            stats: None,
            disabled: None,
            note: None,
        }
    }
}

/// `priority` 缺席 / 读不出来时取的值。规格 §3.10 钉的就是 0。
pub const DEFAULT_KEY_PRIORITY: i64 = 0;

/// `weight` 缺席 / 读不出来时取的值。
///
/// 取 1 而不是 0：0 在任何「按权重展开」的实现里都意味着**永不被选中**，
/// 而「这一格读不出来」绝不该等价于「把这把 key 踢出池子」。
pub const DEFAULT_KEY_WEIGHT: u32 = 1;

/// 调度用得着的那四个配置旋钮的一份投影。
///
/// # 为什么不直接传 [`GatewayConfig`]
///
/// TS 那边每个函数只收它真正要的那几格（`cfg: { maxStrikes, cooldownStrikeMs }`）。
/// Rust 没有结构化类型，照抄的办法只有两个：给每个函数摊平成一串标量参数，
/// 或者建一个这样的小投影。选后者，理由两条：
/// ① 判据写起来不必构造一整份 `GatewayConfig`（那会让每一格单元测试都依赖配置体系）；
/// ② `apply_strike(r, now, 3, 1_800_000, reason)` 这种调用点没人读得懂哪个数是哪个。
///
/// 🔴 **`core` 不许自己去读配置** —— 这份投影由调用方（`pool/dispatcher.rs`）
/// 在装配时从 [`GatewayConfig`] 造好传进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolPolicy {
    /// 累计几次瞬时故障之后进长冷却。
    pub max_strikes: u32,
    /// 429 的冷却时长。
    pub cooldown_rate_limit_ms: i64,
    /// 402 的冷却时长。
    pub cooldown_payment_ms: i64,
    /// strike 攒满之后的长冷却时长。
    pub cooldown_strike_ms: i64,
}

impl PoolPolicy {
    /// 从生效配置里取那四格。
    ///
    /// ⚠️ `max_strikes` 在 [`GatewayConfig`] 里是 `i64`（配置体系统一按 `i64` 收数值，
    /// 下界校验在 `core::config`）。这里**钳到 `u32`**：`strikes` 是计数，负数没有意义，
    /// 而 `strikes >= max_strikes` 在 `max_strikes = 0` 时会让**第一次** strike 就进长冷却
    /// —— 那是配置校验该挡的事，不是这里该偷偷改的。**钳不改语义，只挡溢出。**
    pub fn from_config(cfg: &GatewayConfig) -> Self {
        Self {
            max_strikes: cfg.max_strikes.clamp(0, i64::from(u32::MAX)) as u32,
            cooldown_rate_limit_ms: cfg.cooldown_rate_limit_ms,
            cooldown_payment_ms: cfg.cooldown_payment_ms,
            cooldown_strike_ms: cfg.cooldown_strike_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 🔴 **三个可选字段缺席时，序列化出去必须是「没有这一格」，不是 `null`。**
    ///
    /// 回归对象：把它们写成必填（或者去掉 `skip_serializing_if`）之后，
    /// 面板就再也分不出「没停用」与「这一格读不出来」—— 而那正是
    /// `disabled` 这个安全开关最难被发现的失败形态。
    #[test]
    fn absent_optional_fields_serialize_as_absent_not_null() {
        let r = KeyRecord::fresh("abc", "sk-plain", 1_000);
        let v = serde_json::to_value(&r).unwrap();
        let o = v.as_object().unwrap();
        assert!(!o.contains_key("stats"), "stats 缺席就该真的缺席");
        assert!(!o.contains_key("disabled"), "disabled 缺席就该真的缺席");
        assert!(!o.contains_key("note"), "note 缺席就该真的缺席");
    }

    /// ⚠️ 与上一格相反的那一半：`lastUsedAt` / `cooldownReason` / `evictedReason`
    /// 在 TS 里是**必填可空**，序列化必须给出 `null` 而**不是**缺席。
    ///
    /// 两格分开写，是因为「哪些该 skip、哪些不该」正是这个结构最容易被
    /// 「统一成一种写法」抹平的地方。
    #[test]
    fn required_nullable_fields_serialize_as_null_not_absent() {
        let r = KeyRecord::fresh("abc", "sk-plain", 1_000);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v.get("lastUsedAt"), Some(&json!(null)));
        assert_eq!(v.get("cooldownReason"), Some(&json!(null)));
        assert_eq!(v.get("evictedReason"), Some(&json!(null)));
    }

    /// 字段名是**存储契约**：改成 snake_case 就读不回既有部署写下的 `store.json`。
    #[test]
    fn field_names_are_camel_case_because_they_are_a_storage_contract() {
        let r = KeyRecord::fresh("abc", "sk-plain", 7);
        let v = serde_json::to_value(&r).unwrap();
        for name in ["addedAt", "lastUsedAt", "cooldownUntil", "cooldownReason"] {
            assert!(v.get(name).is_some(), "缺字段 {name}");
        }
        let s = serde_json::to_value(KeyStats::empty()).unwrap();
        for name in ["clientErrors", "lastErrorAt", "lastErrorKind"] {
            assert!(s.get(name).is_some(), "缺字段 {name}");
        }
    }

    /// 新记录的两个调度默认值。**`weight` 取 1 不取 0**（0 = 永不被选中）。
    #[test]
    fn fresh_record_defaults_are_schedulable() {
        let r = KeyRecord::fresh("id", "k", 0);
        assert_eq!(r.priority, 0);
        assert_eq!(r.weight, 1);
        assert_eq!(r.cooldown_until, 0, "新 key 不该一生下来就在冷却里");
        assert!(!r.evicted);
    }

    /// `PoolPolicy` 照搬配置那四格，**一个都不许自己换算**。
    #[test]
    fn policy_is_a_faithful_projection_of_the_four_config_knobs() {
        let cfg = GatewayConfig {
            max_strikes: 3,
            cooldown_rate_limit_ms: 60_000,
            cooldown_payment_ms: 3_600_000,
            cooldown_strike_ms: 1_800_000,
            ..sample_config()
        };
        let p = PoolPolicy::from_config(&cfg);
        assert_eq!(p.max_strikes, 3);
        assert_eq!(p.cooldown_rate_limit_ms, 60_000);
        assert_eq!(p.cooldown_payment_ms, 3_600_000);
        assert_eq!(p.cooldown_strike_ms, 1_800_000);
    }

    /// `max_strikes` 的钳制**只挡溢出，不改语义**：合法值原样过去，
    /// 荒唐值（负数 / 超 `u32`）不许 panic 也不许绕回成一个小正数。
    #[test]
    fn max_strikes_clamps_instead_of_wrapping() {
        let neg = PoolPolicy::from_config(&GatewayConfig {
            max_strikes: -5,
            ..sample_config()
        });
        assert_eq!(neg.max_strikes, 0, "负数钳到 0，不许绕成 4294967291");
        let huge = PoolPolicy::from_config(&GatewayConfig {
            max_strikes: i64::MAX,
            ..sample_config()
        });
        assert_eq!(huge.max_strikes, u32::MAX);
    }

    fn sample_config() -> GatewayConfig {
        GatewayConfig {
            gateway_token: "x".repeat(24),
            agnes_base_url: "https://example.invalid".into(),
            upstream_timeout_ms: 8_000,
            upstream_sync_timeout_ms: 120_000,
            max_strikes: 3,
            cooldown_rate_limit_ms: 60_000,
            cooldown_payment_ms: 3_600_000,
            cooldown_strike_ms: 1_800_000,
            pool_cache_ttl_ms: 60_000,
            pool_touch_interval_ms: 21_600_000,
            usage_stats_enabled: false,
        }
    }
}
