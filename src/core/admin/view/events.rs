//! 事件（实时日志）板块的取值判定。
//!
//! 对应 TS 侧 `admin-ui/js/pure/events.mjs`（468 行）里的 **A 档**那一半
//! （`effectiveLevel` / `eventLevelLabelKey` / `levelBadgeClass` / `shouldWarn`）。
//! 退避计时（`nextPollDelayMs`）与跨轮状态机（`pollOutcome`）是 **C 档**，留在 JS，
//! 如实登记为无判据欠账（06 §9 R-3）。

/// 四个日志级别，**顺序即级别按钮组的渲染顺序**。不含 `"all"`（那是「不筛」，另处理）。
pub const LEVELS: [&str; 4] = ["debug", "info", "warn", "error"];

/// 一条事件**真实的**级别。
///
/// 🔴 **评审裁定**：一条事件的 `level` 字段来自存储，可能缺失 / 畸形 ——
/// TS 侧原来写的是 `typeof item.level === "string" ? item.level : "info"`，
/// 那会把这类数据**伪装成 info**（绿色徽章），与「绝不伪造」正面冲突。
/// ⇒ 四个已知级别原样透传，其余（缺字段 / 数字 / 畸形字符串）一律归到
/// **显式的 `"unknown"` 档，不冒充任何一个已知级别**。
///
/// ⚠️ Rust 侧这条端点的 `items` 是**原样透传的存储值**（`serde_json::Value`），
/// 窄化在读的那一层只保证「是个对象」——`level` 照样可能是任何东西。
/// **这一格不是防御性编程，是这条路径上真实可达的分支。**
#[must_use]
pub fn effective_level(level: Option<&str>) -> &'static str {
    match level {
        Some("debug") => "debug",
        Some("info") => "info",
        Some("warn") => "warn",
        Some("error") => "error",
        _ => "unknown",
    }
}

/// [`effective_level`] 的结果 → i18n key。**与筛选按钮那一组分开**：这个要接受
/// `"unknown"`，而筛选按钮的入参恒是 [`LEVELS`] 里的一个。
#[must_use]
pub fn event_level_label_key(level: &str) -> &'static str {
    match level {
        "debug" => "ev.level.debug",
        "info" => "ev.level.info",
        "warn" => "ev.level.warn",
        "error" => "ev.level.error",
        _ => "ev.level.unknown",
    }
}

/// 级别徽章颜色。与 [`super::keys::badge_class`] 同一套配色语义。
///
/// ⚠️ **`"unknown"` 单独一档（中性灰）**，不落进任何一个「看起来正常」的颜色
/// —— 给一条读不懂的记录配绿色徽章，正是 [`effective_level`] 要挡的那件事的另一半。
#[must_use]
pub fn level_badge_class(level: &str) -> &'static str {
    match level {
        "error" => "badge badge-danger",
        "warn" => "badge badge-warn",
        "debug" | "info" => "badge badge-ok",
        _ => "badge",
    }
}

/// 顶部黄条要不要出现。
///
/// 🔴 **诚实标记必须由后端字段驱动**（一条评审裁定，隔了一个板块又原样复发过一次）：
/// 判据只看这四个从响应里取出来的值，不许在这里或渲染层硬编码一个 true/false。
///
/// **不含 `buffered`**：单纯「缓冲区里还有事件没落盘」是运行中的正常状态
/// （还没到落盘时机而已），只有搭配「实例可能被回收」这层风险才值得说，
/// 而那条风险不是靠一个数字大小能判断的 ⇒ 交给轮询指示灯的 tooltip 常驻显示。
///
/// **也不含 `malformed`**（刻意的）：本仓 `src/` 里没有任何路径能产出畸形条目，
/// 它**恒为 0**，挂上来只会让黄条的判据多一条**永远为假**的分支
/// ——那是「形状断言冒充行为断言」在产品面上的等价物。它与 `buffered` 一样走 tooltip。
///
/// ⚠️ TS 侧的这个函数还有第五个判据 `cursorBroken`（后端发回来的 `cursor` 既不是
/// 有限数字也不是 `null`）。**Rust 侧的 `cursor` 是 `Option<i64>`，类型上产不出那一档**
/// ⇒ 这里没有它。🔴 **但那条缺陷本身没有消失，只是换了防线**：`admin::events` 那边
/// 有一格判据钉着「谁给 `cursor` 加一句 `skip_serializing_if` 就当场红」——
/// 那个「常规的省字节改动」正是把缺陷搬回来的那一步。
#[must_use]
pub fn should_warn(
    dropped: u64,
    budget_exhausted: bool,
    truncated: bool,
    cursor_ahead: bool,
) -> bool {
    dropped > 0 || budget_exhausted || truncated || cursor_ahead
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔴 **缺失 / 畸形的级别归显式的 `unknown`，不许伪装成 `info`。**
    ///
    /// 回归对象：`level` 读不出来的事件被画成绿色的 info 徽章。
    #[test]
    fn an_unreadable_level_never_disguises_itself_as_info() {
        assert_eq!(effective_level(None), "unknown");
        assert_eq!(effective_level(Some("")), "unknown");
        assert_eq!(effective_level(Some("INFO")), "unknown", "大小写不宽容");
        assert_eq!(effective_level(Some("trace")), "unknown");
    }

    /// 四个已知级别原样透传。
    #[test]
    fn the_four_known_levels_pass_through_verbatim() {
        for l in LEVELS {
            assert_eq!(effective_level(Some(l)), l);
        }
    }

    /// 五条文案 key **两两不同**（含 `unknown` 那一条）。
    #[test]
    fn event_level_label_keys_are_pairwise_distinct() {
        let all = [
            event_level_label_key("debug"),
            event_level_label_key("info"),
            event_level_label_key("warn"),
            event_level_label_key("error"),
            event_level_label_key("unknown"),
        ];
        let mut seen: Vec<&str> = all.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 5, "{all:?}");
    }

    /// 🔴 **`unknown` 是中性灰**，不给任何一种「正常 / 异常」的颜色暗示。
    #[test]
    fn the_unknown_level_gets_a_neutral_badge() {
        assert_eq!(level_badge_class("unknown"), "badge");
        assert_ne!(level_badge_class("unknown"), level_badge_class("info"));
        assert_ne!(level_badge_class("unknown"), level_badge_class("error"));
    }

    /// `debug` 与 `info` 刻意共用一档（都是「正常」），`warn` / `error` 各一档。
    #[test]
    fn level_badge_classes_group_debug_with_info() {
        assert_eq!(level_badge_class("debug"), level_badge_class("info"));
        assert_eq!(level_badge_class("warn"), "badge badge-warn");
        assert_eq!(level_badge_class("error"), "badge badge-danger");
    }

    /// 四条判据**各自单独**都足以点亮黄条。
    ///
    /// 回归对象：把四条写成「且」，于是只有全中时才提示。
    #[test]
    fn each_of_the_four_signals_raises_the_banner_on_its_own() {
        assert!(!should_warn(0, false, false, false));
        assert!(should_warn(1, false, false, false));
        assert!(should_warn(0, true, false, false));
        assert!(should_warn(0, false, true, false));
        assert!(should_warn(0, false, false, true));
    }

    /// `dropped == 0` 不触发 —— 「没丢事件」不是问题。
    #[test]
    fn a_zero_drop_count_is_not_a_problem() {
        assert!(!should_warn(0, false, false, false));
    }
}
