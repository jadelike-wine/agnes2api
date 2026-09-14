//! Tier-1 用量记账（`admin/stats.ts`，01 §5）。**近似值，且它自己知道自己是近似的。**
//!
//! 两条误差要在面板上打 `≈`：并发请求下会少计（存储没有 CAS），
//! 且计数最多晚一个 `POOL_TOUCH_INTERVAL_MS` 落盘。
//!
//! # 这一族函数为什么全是「取大 / 取非负差」
//!
//! 计数只会涨。于是「取大」等价于「**绝不回退**」，而回退正是这一族唯一真正的失败形态：
//! 一份比存储旧的快照被合并回去，就是把别的副本（或自己上一次）的计数**抹掉**，
//! **而面板上看不出发生过这件事**。

use serde_json::Value;

use crate::core::types::{KeyRecord, KeyStats};

/// 一笔增量。字段与 [`KeyStats`] 一一对应，但语义是「涨了多少」而不是「一共多少」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsDelta {
    pub requests: u64,
    pub success: u64,
    pub failed: u64,
    pub client_errors: u64,
    pub last_error_at: Option<i64>,
    pub last_error_kind: Option<String>,
}

impl StatsDelta {
    /// 全零增量。
    pub const fn zero() -> Self {
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

impl Default for StatsDelta {
    fn default() -> Self {
        Self::zero()
    }
}

/// 这一笔增量什么都没带。
///
/// ⚠️ **按值判，不按引用判**：TS 那边写成 `d === ZERO_DELTA` 是一行永远为假的死代码
/// （`statsDelta` 每次都新建对象）。Rust 侧本来就没有那个陷阱，
/// 但这条判据的**内容**要照抄 —— 注意它**不看 `last_error_kind`**：
/// 没有 `last_error_at` 就没有可合并的错误，kind 单独出现是坏数据的形态，不是增量。
pub fn is_zero_delta(d: &StatsDelta) -> bool {
    d.requests == 0
        && d.success == 0
        && d.failed == 0
        && d.client_errors == 0
        && d.last_error_at.is_none()
}

/// 一次请求的终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 上游 2xx。
    Success,
    /// 归因到这把 key 的失败。**只有它记 `last_error_*`。**
    Failed,
    /// 上游 4xx 直通。**客户端的错，不是这把 key 的错。**
    ClientError,
}

/// 存储里那一格 `stats` → 一份可用的计数。**逐字段补零，而不是整块丢弃。**
///
/// 🔴 整块丢弃会让**一次坏写把已经攒了几天的计数清零**，而面板上看不出发生过这件事。
///
/// ⚠️ **形参是 `Option<&Value>` 而不是 `Option<&KeyStats>`，这是刻意的**：
/// 真实输入来自 `JSON.parse` / `serde_json`，运行期什么形状都可能。
/// 写成后者就是在签名上撒谎 —— TS 那边的证据是测试当时必须靠
/// `as unknown as KeyStats` 强转才写得出「存储被写坏」那几格，
/// 而**需要强转才能表达的输入，正说明签名把它排除在外了**。
///
/// 数字规则：**有限、非负 ⇒ 截断取整；否则 0。**
/// `last_error_at` 只收有限数字，`last_error_kind` 只收字符串，否则 `None`。
pub fn normalize_stats(raw: Option<&Value>) -> KeyStats {
    let Some(Value::Object(o)) = raw else {
        return KeyStats::empty();
    };
    KeyStats {
        requests: nonneg_count(o.get("requests")),
        success: nonneg_count(o.get("success")),
        failed: nonneg_count(o.get("failed")),
        client_errors: nonneg_count(o.get("clientErrors")),
        last_error_at: finite_int(o.get("lastErrorAt")),
        last_error_kind: o
            .get("lastErrorKind")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// 已经窄化过的那一侧：`None`（存量记录没有这一格）⇒ 全零基线。
///
/// 这是 [`normalize_stats`] 在「类型已经对了」之后的那一半。分成两个函数而不是一个，
/// 是因为**裁定 B 把窄化挤到了存储边界上** —— 记录一旦进了 `core`，
/// 它的 `stats` 就已经是 `Option<KeyStats>`，再去走一遍 `Value` 是白付的代价。
pub fn stats_or_empty(stats: Option<&KeyStats>) -> KeyStats {
    stats.cloned().unwrap_or_else(KeyStats::empty)
}

/// 逐字段取较大者。**计数只会涨，所以「取大」等价于「绝不回退」。**
///
/// 它是落盘基线（`pool/repo.rs` 的 `pending_stats[].base`）唯一的更新方式，
/// 同时处理两个方向、**且不需要知道是哪一种**：
/// ① 调用方交上来的 `prev` 比存储**旧**（`dispatch` 把未合并的 next 写回自己的视图）；
/// ② 快照过了 TTL 之后带回**别的副本写得更高**的值。
///
/// ⚠️⚠️ **它对 `last_error_kind` 不可交换，这是有意的**：判据是
/// `b.last_error_at > a.last_error_at`（**严格大于**），所以两条错误**同毫秒**时保留
/// `a` 的 kind ⇒ `max_stats(x, y)` 与 `max_stats(y, x)` 会给出不同的 kind。
/// 计数那四项是真正的 max（可交换），**只有这一项不是**。
/// 取严格大于是因为「同毫秒」下没有任何依据判定谁更新，
/// 而稳定地保留左侧（= 已有基线）比让顺序决定结果更可预期。
pub fn max_stats(a: &KeyStats, b: &KeyStats) -> KeyStats {
    let newer = is_newer(a.last_error_at, b.last_error_at);
    KeyStats {
        requests: a.requests.max(b.requests),
        success: a.success.max(b.success),
        failed: a.failed.max(b.failed),
        client_errors: a.client_errors.max(b.client_errors),
        last_error_at: if newer {
            b.last_error_at
        } else {
            a.last_error_at
        },
        last_error_kind: if newer {
            b.last_error_kind.clone()
        } else {
            a.last_error_kind.clone()
        },
    }
}

/// 记一次终态。
///
/// ⚠️ **只碰 `stats`，一个调度字段都不动** —— 它要在
/// [`apply_success`](crate::core::keypool::apply_success) /
/// [`apply_strike`](crate::core::keypool::apply_strike) /
/// [`apply_evict`](crate::core::keypool::apply_evict) **之后**叠加。
/// 顺手改调度状态就会把两套语义混在一起，而调度那套是「丢一次就是事故」的。
///
/// ⚠️ **[`Outcome::ClientError`] 不记 `last_error_*`**：那是客户端发错了请求，
/// 把它记成这把 key 的「最近错误」会让运维去查一把其实没问题的 key。
pub fn with_outcome(
    rec: &KeyRecord,
    outcome: Outcome,
    now: i64,
    err_kind: Option<&str>,
) -> KeyRecord {
    let s = stats_or_empty(rec.stats.as_ref());
    let failed = outcome == Outcome::Failed;
    let next = KeyStats {
        requests: s.requests.saturating_add(1),
        success: s.success + u64::from(outcome == Outcome::Success),
        failed: s.failed + u64::from(failed),
        client_errors: s.client_errors + u64::from(outcome == Outcome::ClientError),
        last_error_at: if failed { Some(now) } else { s.last_error_at },
        last_error_kind: if failed {
            err_kind.map(str::to_string)
        } else {
            s.last_error_kind
        },
    };
    KeyRecord {
        stats: Some(next),
        ..rec.clone()
    }
}

/// 两份计数之间的增量。**计数一律取非负差。**
///
/// 🔴 快照比存储旧时会算出负增量，合并回去就是把别人的计数抹掉。
/// `last_error_*` 的判据同 [`max_stats`]（严格大于才带过来，否则 `None`）。
pub fn stats_delta(prev: Option<&KeyStats>, next: Option<&KeyStats>) -> StatsDelta {
    let a = stats_or_empty(prev);
    let b = stats_or_empty(next);
    let newer = is_newer(a.last_error_at, b.last_error_at);
    StatsDelta {
        requests: b.requests.saturating_sub(a.requests),
        success: b.success.saturating_sub(a.success),
        failed: b.failed.saturating_sub(a.failed),
        client_errors: b.client_errors.saturating_sub(a.client_errors),
        last_error_at: if newer { b.last_error_at } else { None },
        last_error_kind: if newer { b.last_error_kind } else { None },
    }
}

/// 两笔增量相加（写被消除时把增量攒起来，见 01 §4.3：
/// **`stats` 是唯一一个「消除 ≠ 丢弃」的字段**）。
pub fn add_delta(a: &StatsDelta, b: &StatsDelta) -> StatsDelta {
    let newer = is_newer(a.last_error_at, b.last_error_at);
    StatsDelta {
        requests: a.requests.saturating_add(b.requests),
        success: a.success.saturating_add(b.success),
        failed: a.failed.saturating_add(b.failed),
        client_errors: a.client_errors.saturating_add(b.client_errors),
        last_error_at: if newer {
            b.last_error_at
        } else {
            a.last_error_at
        },
        last_error_kind: if newer {
            b.last_error_kind.clone()
        } else {
            a.last_error_kind.clone()
        },
    }
}

/// 把攒下来的增量补回基线。
pub fn apply_delta(base: Option<&KeyStats>, d: Option<&StatsDelta>) -> KeyStats {
    let s = stats_or_empty(base);
    let Some(d) = d else { return s };
    let newer = is_newer(s.last_error_at, d.last_error_at);
    KeyStats {
        requests: s.requests.saturating_add(d.requests),
        success: s.success.saturating_add(d.success),
        failed: s.failed.saturating_add(d.failed),
        client_errors: s.client_errors.saturating_add(d.client_errors),
        last_error_at: if newer {
            d.last_error_at
        } else {
            s.last_error_at
        },
        last_error_kind: if newer {
            d.last_error_kind.clone()
        } else {
            s.last_error_kind.clone()
        },
    }
}

/// 整池汇总（面板概览卡的那四个数）。
///
/// 实现方式与 TS 逐字相同：**把每一份都当成「相对全零基线的增量」累加**，
/// 于是 `last_error_*` 走的还是同一条「严格大于」规则，不多一份实现。
pub fn sum_stats<'a>(list: impl IntoIterator<Item = Option<&'a KeyStats>>) -> KeyStats {
    let mut acc = KeyStats::empty();
    for s in list {
        let d = stats_delta(Some(&KeyStats::empty()), s);
        acc = apply_delta(Some(&acc), Some(&d));
    }
    acc
}

/// 「b 那条错误比 a 那条新吗」。🔴 **严格大于** —— 见 [`max_stats`] 那段。
fn is_newer(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(x), Some(y)) => y > x,
    }
}

/// 计数字段：**有限、非负 ⇒ 截断取整；否则 0。**
///
/// ⚠️ `as u64` 对浮点是**饱和**转换（Rust 1.45 起），
/// 所以 `1e300` 会变成 `u64::MAX` 而不是未定义值 —— 荒唐但有界，且不会 panic。
fn nonneg_count(v: Option<&Value>) -> u64 {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() && n >= 0.0 => n.trunc() as u64,
        _ => 0,
    }
}

/// 时刻字段：只收有限数字，截断取整。
fn finite_int(v: Option<&Value>) -> Option<i64> {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => Some(n.trunc() as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_700_000_000_000;

    fn stats(requests: u64, success: u64, failed: u64, client_errors: u64) -> KeyStats {
        KeyStats {
            requests,
            success,
            failed,
            client_errors,
            ..KeyStats::empty()
        }
    }

    // ── normalize_stats ────────────────────────────────────────────────────

    /// 🔴 **逐字段补零，不是整块丢弃。**
    ///
    /// 回归对象：整块丢弃会让一次坏写把攒了几天的计数清零，而面板上看不出来。
    /// 这一格里 `success` 被写坏了，**其余三个字段必须原样活下来**。
    #[test]
    fn a_single_broken_field_does_not_wipe_the_other_counters() {
        let raw = json!({
            "requests": 900,
            "success": "oops",
            "failed": 3,
            "clientErrors": 7,
        });
        let s = normalize_stats(Some(&raw));
        assert_eq!(s.requests, 900, "别的字段不许被一格坏数据带走");
        assert_eq!(s.success, 0, "坏的那一格补零");
        assert_eq!(s.failed, 3);
        assert_eq!(s.client_errors, 7);
    }

    /// 整块读不出来（不是对象 / 缺席）⇒ 全零基线，**不 panic、不抛**。
    #[test]
    fn non_object_shapes_degrade_to_the_empty_baseline() {
        for raw in [json!(null), json!(3), json!("x"), json!([1, 2])] {
            assert_eq!(normalize_stats(Some(&raw)), KeyStats::empty(), "{raw}");
        }
        assert_eq!(normalize_stats(None), KeyStats::empty());
    }

    /// 数字规则：负数 / NaN / 无穷 ⇒ 0；小数**截断**不是四舍五入。
    #[test]
    fn counters_are_truncated_and_never_negative() {
        let raw = json!({
            "requests": 9.9,
            "success": -1,
            // 非有限数在 JSON 里根本表达不出来，serde 给出 `null` ⇒ 走补零那一支。
            "failed": f64::INFINITY,
            "clientErrors": 0.4,
        });
        let s = normalize_stats(Some(&raw));
        assert_eq!(s.requests, 9, "截断，不是四舍五入");
        assert_eq!(s.success, 0, "负数补零");
        assert_eq!(s.failed, 0, "非有限数补零");
        assert_eq!(s.client_errors, 0);
    }

    /// `last_error_*` 各自的类型闸：坏了就是 `None`，**不许拿一个占位字符串顶上**。
    #[test]
    fn last_error_fields_are_type_gated_independently() {
        let raw = json!({ "lastErrorAt": "yesterday", "lastErrorKind": 42 });
        let s = normalize_stats(Some(&raw));
        assert_eq!(s.last_error_at, None);
        assert_eq!(s.last_error_kind, None);

        let ok = json!({ "lastErrorAt": NOW, "lastErrorKind": "timeout" });
        let s = normalize_stats(Some(&ok));
        assert_eq!(s.last_error_at, Some(NOW));
        assert_eq!(s.last_error_kind.as_deref(), Some("timeout"));
    }

    /// 字段名走的是**存储契约**那一套（camelCase）。
    /// snake_case 的 `client_errors` 在存储里不存在 ⇒ 读到 0，而不是「碰巧也认」。
    #[test]
    fn normalize_reads_the_camel_case_storage_names() {
        let s = normalize_stats(Some(&json!({ "client_errors": 5 })));
        assert_eq!(s.client_errors, 0);
        let s = normalize_stats(Some(&json!({ "clientErrors": 5 })));
        assert_eq!(s.client_errors, 5);
    }

    // ── max_stats ──────────────────────────────────────────────────────────

    /// 计数逐字段取大 —— 「绝不回退」的那个性质。
    #[test]
    fn counters_never_go_backwards() {
        let a = stats(10, 8, 2, 0);
        let b = stats(3, 3, 0, 5);
        let m = max_stats(&a, &b);
        assert_eq!(
            (m.requests, m.success, m.failed, m.client_errors),
            (10, 8, 2, 5)
        );
    }

    /// ⚠️ **`last_error_kind` 不可交换，这是有意的**：同毫秒保留左侧。
    #[test]
    fn same_millisecond_errors_keep_the_left_hand_kind() {
        let a = KeyStats {
            last_error_at: Some(NOW),
            last_error_kind: Some("timeout".into()),
            ..KeyStats::empty()
        };
        let b = KeyStats {
            last_error_at: Some(NOW),
            last_error_kind: Some("network error".into()),
            ..KeyStats::empty()
        };
        assert_eq!(
            max_stats(&a, &b).last_error_kind.as_deref(),
            Some("timeout")
        );
        assert_eq!(
            max_stats(&b, &a).last_error_kind.as_deref(),
            Some("network error"),
            "不可交换：换个顺序就是另一个结果，这一点必须被钉住而不是被修掉"
        );
    }

    /// 严格更新的那一条会带过来（含 kind）。
    #[test]
    fn a_strictly_newer_error_wins_together_with_its_kind() {
        let a = KeyStats {
            last_error_at: Some(NOW),
            last_error_kind: Some("timeout".into()),
            ..KeyStats::empty()
        };
        let b = KeyStats {
            last_error_at: Some(NOW + 1),
            last_error_kind: Some("network error".into()),
            ..KeyStats::empty()
        };
        let m = max_stats(&a, &b);
        assert_eq!(m.last_error_at, Some(NOW + 1));
        assert_eq!(m.last_error_kind.as_deref(), Some("network error"));
    }

    /// 一侧没有错误时不许把对面的抹掉。
    #[test]
    fn a_side_without_errors_does_not_erase_the_other_side() {
        let a = KeyStats {
            last_error_at: Some(NOW),
            last_error_kind: Some("timeout".into()),
            ..KeyStats::empty()
        };
        let m = max_stats(&a, &KeyStats::empty());
        assert_eq!(m.last_error_at, Some(NOW));
        assert_eq!(m.last_error_kind.as_deref(), Some("timeout"));
    }

    // ── with_outcome ───────────────────────────────────────────────────────

    fn rec() -> KeyRecord {
        KeyRecord::fresh("id", "plain", NOW - 1)
    }

    /// 三条终态各自落在自己那一格，且 `requests` 每次都 +1。
    #[test]
    fn each_outcome_lands_in_its_own_bucket() {
        let r = with_outcome(&rec(), Outcome::Success, NOW, None);
        let s = r.stats.clone().unwrap();
        assert_eq!(
            (s.requests, s.success, s.failed, s.client_errors),
            (1, 1, 0, 0)
        );

        let r = with_outcome(&r, Outcome::Failed, NOW, Some("timeout"));
        let s = r.stats.clone().unwrap();
        assert_eq!(
            (s.requests, s.success, s.failed, s.client_errors),
            (2, 1, 1, 0)
        );

        let r = with_outcome(&r, Outcome::ClientError, NOW, None);
        let s = r.stats.clone().unwrap();
        assert_eq!(
            (s.requests, s.success, s.failed, s.client_errors),
            (3, 1, 1, 1)
        );
        assert_eq!(
            s.requests,
            s.success + s.failed + s.client_errors,
            "不变式：requests == success + failed + clientErrors"
        );
    }

    /// 🔴 **只有 `Failed` 记 `last_error_*`。**
    ///
    /// 回归对象：把 4xx 直通记成这把 key 的「最近错误」，
    /// 会让运维去查一把其实没问题的 key。
    #[test]
    fn client_errors_do_not_touch_the_last_error_fields() {
        let failed = with_outcome(&rec(), Outcome::Failed, NOW, Some("timeout"));
        let after = with_outcome(&failed, Outcome::ClientError, NOW + 5_000, None);
        let s = after.stats.unwrap();
        assert_eq!(s.last_error_at, Some(NOW), "4xx 不许把时间戳往前推");
        assert_eq!(s.last_error_kind.as_deref(), Some("timeout"));
    }

    /// 一次成功**不清**历史错误（它是「最近一次错误」，不是「当前是否有错」）。
    #[test]
    fn a_success_does_not_clear_the_error_history() {
        let failed = with_outcome(&rec(), Outcome::Failed, NOW, Some("network error"));
        let ok = with_outcome(&failed, Outcome::Success, NOW + 1, None);
        let s = ok.stats.unwrap();
        assert_eq!(s.last_error_at, Some(NOW));
        assert_eq!(s.last_error_kind.as_deref(), Some("network error"));
    }

    /// ⚠️ **只碰 `stats`，一个调度字段都不动。**
    #[test]
    fn with_outcome_touches_nothing_but_stats() {
        let mut before = rec();
        before.strikes = 2;
        before.cooldown_until = NOW + 10_000;
        before.cooldown_reason = Some("rate limited".into());
        before.disabled = Some(true);
        let after = with_outcome(&before, Outcome::Failed, NOW, Some("timeout"));
        assert_eq!(
            KeyRecord {
                stats: before.stats.clone(),
                ..after
            },
            before,
            "除 stats 之外任何一格变了都是回归"
        );
    }

    // ── delta ──────────────────────────────────────────────────────────────

    /// 🔴 **增量取非负差**：快照比存储旧时算出负增量，合并回去就是抹掉别人的计数。
    #[test]
    fn deltas_are_clamped_at_zero() {
        let d = stats_delta(Some(&stats(10, 10, 0, 0)), Some(&stats(3, 3, 0, 0)));
        assert_eq!(d.requests, 0);
        assert_eq!(d.success, 0);
        assert!(is_zero_delta(&d));
    }

    #[test]
    fn deltas_report_real_growth() {
        let d = stats_delta(Some(&stats(1, 1, 0, 0)), Some(&stats(4, 2, 1, 1)));
        assert_eq!(
            (d.requests, d.success, d.failed, d.client_errors),
            (3, 1, 1, 1)
        );
        assert!(!is_zero_delta(&d));
    }

    /// `prev` 缺席（存量记录没有 `stats`）⇒ 整份 next 都是增量。
    #[test]
    fn a_missing_baseline_means_everything_is_new() {
        let d = stats_delta(None, Some(&stats(5, 5, 0, 0)));
        assert_eq!(d.requests, 5);
    }

    /// `is_zero_delta` **不看 kind**：只有 kind 没有 at 仍然是「什么都没带」。
    #[test]
    fn a_dangling_error_kind_is_still_a_zero_delta() {
        let d = StatsDelta {
            last_error_kind: Some("timeout".into()),
            ..StatsDelta::zero()
        };
        assert!(is_zero_delta(&d));
    }

    /// 写被消除时把增量攒起来：两笔 +1 攒成 +2（`stats` 是唯一「消除≠丢弃」的字段）。
    #[test]
    fn elided_writes_accumulate_instead_of_vanishing() {
        let one = stats_delta(Some(&KeyStats::empty()), Some(&stats(1, 1, 0, 0)));
        let acc = add_delta(&one, &one);
        assert_eq!(acc.requests, 2);
        let merged = apply_delta(Some(&stats(50, 50, 0, 0)), Some(&acc));
        assert_eq!(merged.requests, 52, "攒下来的必须真的补回基线");
    }

    /// `apply_delta(base, None)` = 原样返回基线（别顺手清零）。
    #[test]
    fn applying_no_delta_keeps_the_baseline_intact() {
        let base = stats(7, 7, 0, 0);
        assert_eq!(apply_delta(Some(&base), None), base);
    }

    /// 整池汇总：四个计数相加，`last_error_*` 取最新那条。
    #[test]
    fn sum_stats_adds_counters_and_keeps_the_newest_error() {
        let a = KeyStats {
            last_error_at: Some(NOW),
            last_error_kind: Some("timeout".into()),
            ..stats(3, 2, 1, 0)
        };
        let b = KeyStats {
            last_error_at: Some(NOW + 10),
            last_error_kind: Some("network error".into()),
            ..stats(4, 4, 0, 0)
        };
        let total = sum_stats([Some(&a), Some(&b), None]);
        assert_eq!(total.requests, 7);
        assert_eq!(total.success, 6);
        assert_eq!(total.failed, 1);
        assert_eq!(total.last_error_at, Some(NOW + 10));
        assert_eq!(total.last_error_kind.as_deref(), Some("network error"));
    }

    /// 空池汇总是全零 —— 而不是「没有数据」这种要靠 `Option` 表达的东西。
    #[test]
    fn summing_an_empty_pool_is_all_zeroes() {
        assert_eq!(sum_stats(std::iter::empty()), KeyStats::empty());
    }
}
