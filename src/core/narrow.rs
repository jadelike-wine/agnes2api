//! 【裁定 B】**存储窄化**：从 `serde_json::Value` 到业务类型的那一步。
//!
//! # 为什么不直接 `#[derive(Deserialize)]`
//!
//! TS 里 `disabled: "true"` 被 `JSON.parse` 原样带进来、代码显式判真值性，**记录仍然可用**。
//! Rust 的 `serde` 遇到 `"true"` 给 `bool` 字段会**整条记录反序列化失败**
//! ⇒ **这把 key 直接从池子里消失**（而它在存储里好端端地躺着）。
//!
//! ⇒ 两段式：`Storage::get` 只回 `Option<Value>`，再由本模块**手写**窄化。
//! 持久化 DTO **只派生 `Serialize`**。
//!
//! # 🔴 方向逐处不同，不许统一成一种（01 §12 那张《存储窄化方向表》）
//!
//! 这是原仓散在 10 个文件里、**最容易在移植时被「统一成一种写法」而丢掉**的地方。
//! 每一处「坏数据往哪个方向倒」都是一次有代价的取舍：
//!
//! | 数据 | 整体读不出来 | 单条 / 单字段坏 | 方向的理由 | 实现在哪 |
//! |---|---|---|---|---|
//! | `pool:index` | 当「索引缺失」⇒ 回落 `list()` 并重建 | **就地剔掉那一个元素** | 一条脏数据把整个索引作废 ⇒ 每次 `all()` 都 `list` 一遍 | [`narrow_pool_index`] |
//! | `key:<id>` | 那一条被丢掉（留下幽灵索引项，fail-safe） | **逐字段降级**，`stats` 逐字段补零 | 整块丢弃会让一次坏写把攒了几天的计数清零，**而面板上看不出发生过** | [`narrow_key_record`] |
//! | `apikeys` | 🔴 **`Invalid`，绝不当空表** | **坏一条 = 整张表不认** | 当空表 ⇒ **全部客户 401**，且下一次面板写会把幸存记录整份覆掉，不可逆 | [`crate::core::keys::api_keys::parse_api_key_table`] |
//! | `config` | 冷启动降级到 env+默认；**热路径如实抛** | 字段级降级回默认值 + `degraded=true` | 抛错 ⇒ 重启循环，**而且没有面板可以进去改回来** | [`crate::core::config_provenance`]（已落） |
//! | `event:*` | 非数组 ⇒ 空分片，`malformed` **计 0** | `ts` 非有限数 ⇒ 丢；其余一律留 | `null` 是完全正常的路径；**丢 level/msg = 在一次上游异常之后把整段证据清空** | `core::admin::event_entry`（**本轮不在本人射程内**） |
//! | `usage:*` | 非对象 / `total` 坏 / `day` 坏 ⇒ 整条 `malformed` | 单个桶坏 ⇒ 那个键静默消失 | **数组必须单独拒掉**，否则畸形分片会伪装成合法的并把 `shards` 一起变假 | P1.5（Tier-2） |
//! | `tend:history` | 非数组 ⇒ 空 | **逐字段校验，任一不合整条丢** | 它是**定长表的一行**：`minted` 坏掉的那行**显示比丢掉更容易误导运维** | [`crate::registrar::runtime::history`] |
//! | `registrar_manual_guard` | — | **任一字段读不得 ⇒ 整条 `None`**（= 今天还没点过） | 多放行一轮（有锁与冷却兜着）好过**把按钮永久锁死而看不出原因** | [`crate::registrar::guard::narrow_manual_guard`] |
//! | `registrar:backoff` | — | **任一字段读不得 ⇒ `None`**（= 放行） | 当成「还在退避」会让注册机**静默停摆**；放行的代价只是多打一轮 | [`crate::registrar::backoff::narrow_backoff`] |
//! | `registrar:domains` | **整个对象读不得 ⇒ 空台账** | 单条读不得 ⇒ **丢那一条** | 「什么都没记住，多探几次」；**绝不返回「全被屏蔽」** | [`crate::registrar::runtime::narrow_domain_ledger`] |
//!
//! 🔴 **这张表是全表，本模块只实现前三行（key 池那条主链）。**
//! 其余各行的实现住在各自的模块里（第 5 列点名），**本模块刻意不建它们的空壳** ——
//! 一个返回默认值的占位函数会让「这块做完了吗」这个问题需要读代码才能回答。

use serde_json::Value;

use crate::core::keys::api_keys::{ApiKeyTableRead, parse_api_key_table};
use crate::core::pool_index::{self, PoolIndex};
use crate::core::stats::normalize_stats;
use crate::core::types::{DEFAULT_KEY_PRIORITY, DEFAULT_KEY_WEIGHT, KeyRecord};

/// JS 的真值性（`!!v`）。
///
/// **falsy 只有五种**：`false` / `0`（含 `-0`、`0.0`）/ `""` / `null` / 缺席。
/// 其余一律真 —— ⚠️ **包括字符串 `"false"`**，这不是笔误：
/// 它是 TS 侧 `!!r.disabled` 的逐字语义，而那条语义的依据是
/// **「对一个安全开关，读不懂就当停用是安全的那一侧」**。
///
/// ⚠️ `NaN` 在 JSON 里表达不出来（serde 给出 `null`），所以那一档由 `null` 覆盖。
fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        // 数组与对象在 JS 里恒为真（连 `[]` 与 `{}` 都是）。
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `key:<id>` 一条记录的窄化。
///
/// # 丢整条的条件：**只有 `id` 与 `key` 两格**
///
/// 这两格缺一不可：没有 `id` 就没法指认它，没有 `key` 就没有可以拿去用的材料。
/// 丢掉它留下的是一个**幽灵索引项** —— 01 §4.0 明写这是 fail-safe 状态
/// （读到 `None` 被过滤掉，对账以 `list()` 为准）。
///
/// 🔴 **其余每一格都只降级、不丢整条。** 理由是这条路径的失败形态很硬：
/// 丢一条记录 = 一把**在存储里好端端躺着的 key 从池子里消失**，
/// 而池子空了会让整个网关 503。**为了一格坏掉的 `added_at` 付这个代价是荒唐的。**
///
/// # 逐格的方向（每一格都单独说明为什么倒向这一边）
///
/// | 字段 | 坏了怎么办 | 为什么是这个方向 |
/// |---|---|---|
/// | `id` / `key` | **丢整条** | 见上 |
/// | `disabled` | **真值性**（不是 `false`/`0`/`""` 就当**停用**） | 安全开关：读不懂就当停用是安全的那一侧 |
/// | `evicted` | **真值性** | 与 `disabled` 同一条 JS 语义。⚠️ 代价明写：一格脏数据把 key 判成 evicted 会**让出一个 `target_keys` 名额**，而那意味着补池会去真的建一个号。这条不对称如实登记，方向仍按 TS 逐字 |
/// | `cooldown_until` | **0**（= 不在冷却） | 反方向（当成永久冷却）会让一格脏数据**把 key 永久移出池子**；判成 0 最坏是多打一次上游、当场重新进冷却，**自愈** |
/// | `strikes` | **0** | 同上，自愈 |
/// | `added_at` | **0** | 纯展示。面板上显示 1970 年**是显眼的**，而丢掉这条 key 是不可见的 |
/// | `last_used_at` | `None` | 纯展示，`null` 本来就是合法取值 |
/// | `cooldown_reason` / `evicted_reason` / `note` | `None` | 纯展示；非字符串当没有 |
/// | `stats` | **缺席 ⇒ `None`；在场 ⇒ 逐字段补零** | 🔴 整块丢弃会让一次坏写把攒了几天的计数清零，而面板上看不出发生过。**缺席与坏掉必须分开**：存量记录没有这一格，硬补一个全零会让「从没记过」与「记录被写坏」长得一样 |
/// | `priority` | `0` | 与新记录的默认值同；读不出来时按「最普通的那一档」处理 |
/// | `weight` | `1` | **0 在任何按权重展开的实现里都意味着永不被选中** |
pub fn narrow_key_record(raw: &Value) -> Option<KeyRecord> {
    let o = raw.as_object()?;
    let id = o
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let key = o
        .get("key")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    Some(KeyRecord {
        id: id.to_string(),
        key: key.to_string(),
        added_at: int_or(o.get("addedAt"), 0),
        last_used_at: opt_int(o.get("lastUsedAt")),
        cooldown_until: int_or(o.get("cooldownUntil"), 0),
        cooldown_reason: opt_str(o.get("cooldownReason")),
        strikes: int_or(o.get("strikes"), 0).clamp(0, i64::from(u32::MAX)) as u32,
        evicted: o.get("evicted").is_some_and(js_truthy),
        evicted_reason: opt_str(o.get("evictedReason")),
        priority: int_or(o.get("priority"), DEFAULT_KEY_PRIORITY),
        weight: int_or(o.get("weight"), i64::from(DEFAULT_KEY_WEIGHT)).clamp(0, i64::from(u32::MAX))
            as u32,
        // **缺席与坏掉必须分开**：缺席 = 存量记录从没记过 ⇒ `None`。
        stats: o.get("stats").map(|v| normalize_stats(Some(v))),
        disabled: o.get("disabled").map(js_truthy),
        note: opt_str(o.get("note")),
    })
}

/// 一整批记录的窄化，**并把丢了几条数出来**。
///
/// 🔴 **「丢了几条」必须可数**（04 的纪律：读侧畸形条目逐条丢**且丢了几条要数得出来**）。
/// 调用方（`pool/repo.rs`）拿这个数去打一条 `pool.load_failed` 级别的事件 ——
/// 否则「一把 key 悄悄消失」就是一件没有任何现象的事。
///
/// ⚠️ 入参是 `Option<Value>` 的序列：`None` 是**幽灵索引项**（存储里没这条），
/// 它**不算「坏数据」** —— 那是 `add` / `delete` 写序中途失败留下的正常 fail-safe 残留。
/// 两者混在一起数，会让一次正常的删除看起来像一次数据损坏。
pub fn narrow_key_records<'a>(
    raws: impl IntoIterator<Item = Option<&'a Value>>,
) -> (Vec<KeyRecord>, NarrowLoss) {
    let mut out = Vec::new();
    let mut loss = NarrowLoss::default();
    for raw in raws {
        match raw {
            None => loss.missing += 1,
            Some(v) => match narrow_key_record(v) {
                Some(r) => out.push(r),
                None => loss.malformed += 1,
            },
        }
    }
    (out, loss)
}

/// 一次批量窄化丢了些什么。**两个数分开，不许合并成一个**（理由见 [`narrow_key_records`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NarrowLoss {
    /// 索引里有、存储里没有（幽灵索引项）。**正常的 fail-safe 残留，不是损坏。**
    pub missing: usize,
    /// 存储里有、但连 `id` / `key` 都读不出来。**这一条才是「数据损坏」。**
    pub malformed: usize,
}

impl NarrowLoss {
    /// 有没有值得打一条事件的东西。
    pub fn is_clean(self) -> bool {
        self.missing == 0 && self.malformed == 0
    }
}

/// `pool:index` 的窄化。**分层：结构级 ⇒ `None`（重建），元素级 ⇒ 剔掉那一个。**
///
/// 实现在 [`crate::core::pool_index::parse`]；这里只是《方向表》的统一入口，
/// 让「想知道某种存储数据往哪个方向倒」的人**只需要读这一个文件**。
pub fn narrow_pool_index(raw: Option<&Value>) -> Option<PoolIndex> {
    pool_index::parse(raw?)
}

/// `apikeys` 的窄化。**三档：`Absent` / `Ok` / `Invalid`，🔴 绝不当空表。**
///
/// 实现在 [`crate::core::keys::api_keys::parse_api_key_table`]；同上，这里是入口。
pub fn narrow_api_key_table(raw: Option<&Value>) -> ApiKeyTableRead {
    parse_api_key_table(raw)
}

/// 有限数字 ⇒ 截断取整；否则取 `fallback`。
fn int_or(v: Option<&Value>, fallback: i64) -> i64 {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => n.trunc() as i64,
        _ => fallback,
    }
}

/// 有限数字 ⇒ `Some`；否则 `None`（含 `null` 与缺席）。
fn opt_int(v: Option<&Value>) -> Option<i64> {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => Some(n.trunc() as i64),
        _ => None,
    }
}

/// 字符串 ⇒ `Some`；否则 `None`。
fn opt_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keypool::{is_available, is_disabled};
    use crate::core::types::{KeyRecord, KeyStats};
    use serde_json::json;

    const NOW: i64 = 1_700_000_000_000;

    fn healthy() -> Value {
        json!({
            "id": "abc0123456789def",
            "key": "sk-plaintext-value",
            "addedAt": 1_000,
            "lastUsedAt": 2_000,
            "cooldownUntil": 0,
            "cooldownReason": null,
            "strikes": 0,
            "evicted": false,
            "evictedReason": null,
            "priority": 0,
            "weight": 1,
        })
    }

    /// 🔴 **窄化覆盖断言（裁定 B 的第 4 条）。**
    ///
    /// 期望侧从 `KeyRecord` 的**序列化结果**现算字段数，实际侧是本模块手写的那张清单
    /// —— **两侧不互相推导**。加一个字段而不来这里表态，这一格当场红。
    ///
    /// 回归对象：新字段被 `..Default` 之类的写法悄悄带过去，于是存储里那一格
    /// **永远读不回来**，而任何一格行为断言都不会红（它读的是内存里那份）。
    #[test]
    fn every_key_record_field_is_covered_by_the_narrowing_function() {
        /// 本模块**逐格表过态**的字段（顺序同 `KeyRecord` 的声明）。
        const COVERED: &[&str] = &[
            "id",
            "key",
            "addedAt",
            "lastUsedAt",
            "cooldownUntil",
            "cooldownReason",
            "strikes",
            "evicted",
            "evictedReason",
            "priority",
            "weight",
            "stats",
            "disabled",
            "note",
        ];
        // 一条**每一格都在场**的记录：`skip_serializing_if` 才不会把可选那三格藏起来。
        let full = KeyRecord {
            stats: Some(KeyStats::empty()),
            disabled: Some(false),
            note: Some(String::new()),
            ..KeyRecord::fresh("id", "key", 0)
        };
        let v = serde_json::to_value(&full).unwrap();
        let mut actual: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        let mut expected = COVERED.to_vec();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(
            actual, expected,
            "KeyRecord 的字段与 narrow_key_record 覆盖的字段对不上：\
             加字段时必须来这里表一次态（它坏了往哪个方向倒）"
        );
    }

    /// 一条健康记录原样读回来。
    #[test]
    fn a_healthy_record_narrows_without_losing_anything() {
        let r = narrow_key_record(&healthy()).unwrap();
        assert_eq!(r.id, "abc0123456789def");
        assert_eq!(r.key, "sk-plaintext-value");
        assert_eq!(r.added_at, 1_000);
        assert_eq!(r.last_used_at, Some(2_000));
        assert_eq!(r.strikes, 0);
        assert!(!r.evicted);
        assert_eq!(r.stats, None, "没有 stats 这一格就是 None，不是全零");
        assert_eq!(r.disabled, None, "缺席 ≠ Some(false)");
    }

    /// 写出去的必须读得回来（存储往返闭合）。
    #[test]
    fn records_round_trip_through_serialize_and_narrow() {
        let mut before = KeyRecord::fresh("id0", "plain", 42);
        before.stats = Some(KeyStats {
            requests: 9,
            success: 8,
            failed: 1,
            client_errors: 0,
            last_error_at: Some(7),
            last_error_kind: Some("timeout".into()),
        });
        before.disabled = Some(true);
        before.note = Some("运维备注".into());
        before.cooldown_reason = Some("rate limited".into());
        before.last_used_at = Some(99);
        let raw = serde_json::to_value(&before).unwrap();
        assert_eq!(narrow_key_record(&raw).as_ref(), Some(&before));
    }

    /// 🔴 **`disabled: "true"` 这条脏数据不许让一把 key 消失。**
    ///
    /// 这一格是裁定 B 的**全部理由**：`#[derive(Deserialize)]` 在这里会整条失败。
    #[test]
    fn a_dirty_disabled_flag_does_not_make_the_key_disappear() {
        let mut raw = healthy();
        raw["disabled"] = json!("true");
        let r = narrow_key_record(&raw).expect("🔴 一条脏数据不许让整把 key 读不出来");
        assert!(is_disabled(&r), "读不懂就当停用 —— 安全的那一侧");
        assert_eq!(r.id, "abc0123456789def", "其余字段照常");
    }

    /// 真值性的**每一档**都钉住（含那个反直觉的 `"false"`）。
    #[test]
    fn disabled_follows_javascript_truthiness_to_the_letter() {
        let cases: [(Value, Option<bool>); 10] = [
            (json!(true), Some(true)),
            (json!(false), Some(false)),
            (json!("true"), Some(true)),
            // ⚠️ 反直觉但正确：非空字符串恒为真。
            (json!("false"), Some(true)),
            (json!(""), Some(false)),
            (json!(1), Some(true)),
            (json!(0), Some(false)),
            (json!(null), Some(false)),
            (json!({}), Some(true)),
            (json!([]), Some(true)),
        ];
        for (v, expect) in cases {
            let mut raw = healthy();
            raw["disabled"] = v.clone();
            assert_eq!(narrow_key_record(&raw).unwrap().disabled, expect, "{v}");
        }
    }

    /// `evicted` 走同一条真值性。
    #[test]
    fn evicted_follows_the_same_truthiness_rule() {
        let mut raw = healthy();
        raw["evicted"] = json!("yes");
        assert!(narrow_key_record(&raw).unwrap().evicted);
        raw["evicted"] = json!(0);
        assert!(!narrow_key_record(&raw).unwrap().evicted);
    }

    /// 🔴 **坏掉的 `cooldown_until` 倒向「不在冷却」。**
    ///
    /// 反方向（当成永久冷却）会让一格脏数据把 key **永久移出池子**；
    /// 判成 0 最坏是多打一次上游、当场重新进冷却 —— **自愈**。
    #[test]
    fn a_broken_cooldown_makes_the_key_usable_not_permanently_dead() {
        for bad in [json!("soon"), json!(null), json!({}), json!([])] {
            let mut raw = healthy();
            raw["cooldownUntil"] = bad.clone();
            let r = narrow_key_record(&raw).unwrap();
            assert_eq!(r.cooldown_until, 0, "{bad}");
            assert!(is_available(&r, NOW), "一格脏数据不许永久移出一把 key");
        }
    }

    /// 坏掉的 `strikes` / `added_at` 也只是降级，**不丢整条**。
    #[test]
    fn broken_telemetry_fields_degrade_without_dropping_the_record() {
        let mut raw = healthy();
        raw["strikes"] = json!("lots");
        raw["addedAt"] = json!(null);
        raw["lastUsedAt"] = json!("yesterday");
        raw["note"] = json!(42);
        raw["cooldownReason"] = json!(7);
        let r = narrow_key_record(&raw).unwrap();
        assert_eq!(r.strikes, 0);
        assert_eq!(r.added_at, 0);
        assert_eq!(r.last_used_at, None);
        assert_eq!(r.note, None);
        assert_eq!(r.cooldown_reason, None);
    }

    /// 负数 / 超界的 `strikes` 与 `weight` 钳住，**不许绕成一个巨大的正数**。
    #[test]
    fn out_of_range_numbers_are_clamped_not_wrapped() {
        let mut raw = healthy();
        raw["strikes"] = json!(-3);
        raw["weight"] = json!(-1);
        let r = narrow_key_record(&raw).unwrap();
        assert_eq!(r.strikes, 0);
        assert_eq!(r.weight, 0);

        raw["strikes"] = json!(9_999_999_999_i64);
        raw["weight"] = json!(9_999_999_999_i64);
        let r = narrow_key_record(&raw).unwrap();
        assert_eq!(r.strikes, u32::MAX);
        assert_eq!(r.weight, u32::MAX);
    }

    /// 两个调度默认值：`priority = 0`、`weight = 1`（**不是 0**）。
    #[test]
    fn scheduling_defaults_keep_the_key_schedulable() {
        let mut raw = healthy();
        raw.as_object_mut().unwrap().remove("priority");
        raw.as_object_mut().unwrap().remove("weight");
        let r = narrow_key_record(&raw).unwrap();
        assert_eq!(r.priority, 0);
        assert_eq!(r.weight, 1, "0 在按权重展开的实现里意味着永不被选中");
    }

    /// 🔴 **`stats` 逐字段补零，不整块丢弃。**
    #[test]
    fn a_single_broken_counter_does_not_wipe_the_others() {
        let mut raw = healthy();
        raw["stats"] = json!({ "requests": 900, "success": "oops", "failed": 3 });
        let s = narrow_key_record(&raw).unwrap().stats.unwrap();
        assert_eq!(s.requests, 900);
        assert_eq!(s.success, 0);
        assert_eq!(s.failed, 3);
    }

    /// **`stats` 缺席与 `stats` 坏掉是两回事**，不许合并。
    ///
    /// 缺席 = 存量记录从没记过；坏掉 = 有这一格但读不动。
    /// 硬把缺席补成全零，「从没记过」与「被写坏」就长得一模一样了。
    #[test]
    fn an_absent_stats_field_is_not_the_same_as_a_broken_one() {
        let mut raw = healthy();
        assert_eq!(narrow_key_record(&raw).unwrap().stats, None);
        raw["stats"] = json!("garbage");
        assert_eq!(
            narrow_key_record(&raw).unwrap().stats,
            Some(KeyStats::empty()),
            "在场但读不动 ⇒ 全零基线（而不是缺席）"
        );
    }

    /// 丢整条的**只有两格**：`id` 与 `key`。
    #[test]
    fn only_a_missing_id_or_key_drops_the_whole_record() {
        for bad in [
            json!({ "key": "k" }),           // 没有 id
            json!({ "id": "", "key": "k" }), // 空 id
            json!({ "id": 7, "key": "k" }),  // id 不是字符串
            json!({ "id": "a" }),            // 没有 key
            json!({ "id": "a", "key": "" }), // 空 key（没有可用的材料）
            json!({ "id": "a", "key": null }),
            json!("not an object"),
            json!([{ "id": "a", "key": "k" }]),
        ] {
            assert!(narrow_key_record(&bad).is_none(), "{bad} 该被丢掉");
        }
        // 反面：只有 id + key 也足够读出一条（其余全走默认值）。
        let minimal = narrow_key_record(&json!({ "id": "a", "key": "k" })).unwrap();
        assert_eq!(minimal.id, "a");
        assert_eq!(minimal.cooldown_until, 0);
        assert!(is_available(&minimal, NOW));
    }

    /// 🔴 **「幽灵索引项」与「数据损坏」分开数。**
    ///
    /// 前者是 `add` / `delete` 写序中途失败留下的**正常 fail-safe 残留**；
    /// 混在一起数会让一次正常的删除看起来像一次数据损坏。
    #[test]
    fn ghost_index_entries_are_counted_apart_from_real_corruption() {
        let good = healthy();
        let broken = json!({ "id": "", "key": "k" });
        let (records, loss) = narrow_key_records([Some(&good), None, Some(&broken), Some(&good)]);
        assert_eq!(records.len(), 2);
        assert_eq!(loss.missing, 1, "索引里有、存储里没有");
        assert_eq!(loss.malformed, 1, "存储里有、但读不出来");
        assert!(!loss.is_clean());
        let (_, clean) = narrow_key_records([Some(&good)]);
        assert!(clean.is_clean());
    }

    // ── 方向表的另外两行（入口在本模块，实现在各自的文件里） ───────────────

    /// `pool:index`：**结构级坏 ⇒ 重建；元素级坏 ⇒ 剔掉那一个。**
    #[test]
    fn pool_index_narrowing_is_layered() {
        assert_eq!(narrow_pool_index(None), None, "缺席 = 索引缺失 ⇒ 重建");
        assert_eq!(narrow_pool_index(Some(&json!("garbage"))), None);
        let idx = narrow_pool_index(Some(&json!({ "v": 1, "ids": ["a", 7, "", "b"] })))
            .expect("一条脏元素不许让整个索引作废");
        assert_eq!(idx.ids, vec!["a".to_string(), "b".to_string()]);
    }

    /// `apikeys`：🔴 **三档，绝不当空表。**
    #[test]
    fn api_key_table_narrowing_keeps_absent_and_invalid_apart() {
        assert_eq!(narrow_api_key_table(None), ApiKeyTableRead::Absent);
        assert_eq!(
            narrow_api_key_table(Some(&json!("garbage"))),
            ApiKeyTableRead::Invalid,
            "🔴 读不动 ≠ 空表（当空表 = 全部客户 401 + 不可逆覆写）"
        );
    }

    /// 三条方向**互不相同**，这一格把它们摆在一起，免得后人「统一成一种写法」。
    #[test]
    fn the_three_directions_are_deliberately_different() {
        // 同样是「整体读不出来」，三处的处置各不相同：
        assert_eq!(narrow_pool_index(Some(&json!("x"))), None); // 重建
        assert!(narrow_key_record(&json!("x")).is_none()); // 丢那一条（幽灵索引项）
        assert_eq!(
            narrow_api_key_table(Some(&json!("x"))),
            ApiKeyTableRead::Invalid // 整张表不认，且**不许**退化成空表
        );
    }
}
