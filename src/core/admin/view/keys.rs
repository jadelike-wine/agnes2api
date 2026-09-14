//! Key 池板块（我们持有的上游 Agnes 凭据）的取值与可用性判定。
//!
//! 对应 TS 侧 `admin-ui/js/pure/keys.mjs`（181 行）与 `keys-write.mjs`（711 行）里
//! 的 **A 档**那一半 —— 06 §5.3 的归档表。
//!
//! ⚠️⚠️ **本板块说的是「我们拿去向 Agnes 证明身份」的那一族**，不是我们签发给别人的
//! `sk-`（那一族在 [`super::apikeys`]）。两者方向相反，名字很像是已知代价。
//!
//! # 为什么这几颗按钮的可用性必须由后端判
//!
//! 它们**本来就是后端写端点的前置校验**（`deletable()` / `PATCH` 的白名单），
//! 前端那一份是「别把一次注定失败的请求发出去」。两份实现同时存在的后果在 TS 侧
//! 逐条留过痕：`canClearCooldown` 曾经拿 `bucket` 当判据，于是一把「已停用且仍在
//! 冷却」的 key 的清冷却按钮**永远禁用**，而后端照样接受 ——
//! **按钮说「不能」，后端说「能」，两边对不上。**
//! ⇒ 上移之后 Rust 是唯一真源，渲染层只负责把布尔画成 `disabled` 属性。

// ── 1. 分档的样式与文案 ────────────────────────────────────────────────────

/// 分档徽章的 CSS 类。**四档必须互不相同**——两档共用一个样式就等于面板分不出它们。
///
/// ⚠️ `disabled` **不带颜色修饰类**，只有底样式 `badge`（中性灰）：它不是故障，
/// 是运维自己按下的开关，用报警色会让人以为出事了。
/// **别为了「看起来对称」再造一个同义类**（TS 侧曾经有过一条 `badge-muted`，
/// 而那条 CSS 的取值与 `.badge` 逐字相同 ⇒ 是条死规则）——那只会让
/// 「四档互不相同」这句话在字符串上成立、在屏幕上不成立。
#[must_use]
pub fn badge_class(bucket: &str) -> &'static str {
    match bucket {
        "disabled" => "badge",
        "evicted" => "badge badge-danger",
        "cooling" => "badge badge-warn",
        _ => "badge badge-ok",
    }
}

/// 分档名的 i18n key。**五条各写一次字面量**，好让 i18n 门禁扫得到
/// （拼出来的 key 与写死的 key 在行为上可以逐字节相同，门禁只认字面量）。
#[must_use]
pub fn bucket_label_key(bucket: &str) -> &'static str {
    match bucket {
        "fresh" => "keys.bucket.fresh",
        "cooling" => "keys.bucket.cooling",
        "evicted" => "keys.bucket.evicted",
        "disabled" => "keys.bucket.disabled",
        _ => "keys.bucket.all",
    }
}

/// 停用/启用那颗按钮该显示哪个 i18n key。**看的是这一条此刻的状态，
/// 不是它将要变成什么。**
#[must_use]
pub fn toggle_disable_label_key(disabled: bool) -> &'static str {
    if disabled {
        "keys.action.enable"
    } else {
        "keys.action.disable"
    }
}

// ── 2. 冷却剩余 ────────────────────────────────────────────────────────────

/// 冷却剩余毫秒。**只有 `cooling` 档才有这个值**，其余一律 `None`（渲染成 `—`）。
///
/// ⚠️ 参照时刻用**服务端的**时钟（响应里那个 `generatedAt`），不是浏览器时钟：
/// 两者不同源，拿浏览器的 now 去减服务端的 `cooldownUntil`，时钟有偏差时会算出
/// 负数或虚高的剩余时间。**上移到 Rust 之后这条偏差从结构上消失了**
/// —— 两个数来自同一次 `now_ms()`。
///
/// ⚠️ **它与 [`can_clear_cooldown`] 刻意用不同的判据**，别合并：这一格是
/// 「这一行该不该显示一个倒计时」（`cooling` 档才显示），那一颗是
/// 「这个动作点下去有没有用」（`cooldown_until > now` 就有用，哪怕这把 key
/// 因为同时被停用而落在 `disabled` 档）。
#[must_use]
pub fn cooldown_remaining(bucket: &str, cooldown_until: i64, now: i64) -> Option<i64> {
    if bucket != "cooling" {
        return None;
    }
    Some(cooldown_until - now)
}

// ── 3. 行内动作的可用性 ────────────────────────────────────────────────────

/// 一把 key 能不能删。**与写端点的 `deletable()` 是同一条判据**：
/// `disabled || evicted`，两条都不成立才拦。
///
/// ⚠️ **只看 `evicted` 是一个已知会咬人的错误写法**——一把被运维手动停用、
/// 但从未被系统剔除的 key 会被前端永远挡在删除按钮外面，即使后端本来放行。
/// 反过来**只看 `disabled` 同样错**：一把被系统剔除但运维没手动停用过的 key
/// 也必须删得掉。
#[must_use]
pub fn is_deletable(disabled: bool, evicted: bool) -> bool {
    disabled || evicted
}

/// 「清冷却」按钮只在这把 key 确实处在冷却期时才有意义。
///
/// ⚠️⚠️ **判据是 `cooldown_until > now`，不是 `bucket == "cooling"`**
/// （TS 侧评审实测抓到的一个真缺陷）：`bucket` 是按
/// `disabled > evicted > cooling > fresh` 算出来的**优先级投影**，一把「已停用
/// 且仍在冷却」的 key 的 `bucket` 是 `"disabled"`，不是 `"cooling"`——但它的
/// `cooldown_until` 确实还没到，后端的 `PATCH { clearCooldown: true }` 照样接受、
/// 照样清得掉。拿 `bucket` 当判据会让这把 key 的「清冷却」按钮永远禁用，即使
/// 运维刚把它启用回来——**按钮说「不能」，后端说「能」，两边对不上**。
#[must_use]
pub fn can_clear_cooldown(cooldown_until: i64, now: i64) -> bool {
    cooldown_until > now
}

/// 「解除剔除」按钮只在这把 key 确实被剔除时才有意义。
///
/// ⚠️ **判据是 `evicted` 这个事实，不是 `bucket`**，理由与 [`can_clear_cooldown`]
/// 逐字同源：一把「既被剔除又被手动停用」的 key 的 `bucket` 是 `"disabled"`。
#[must_use]
pub fn can_unevict(evicted: bool) -> bool {
    evicted
}

/// 「清 strikes」按钮只在这把 key 确实有连续失败计数时才有意义
/// ——零 strikes 时点它什么都不会变，按钮亮着只是噪音。
///
/// ⚠️ **这不是「清冷却」的同义词，两者的可用性判据也刻意不同**：
/// 冷却会自己到期，`strikes` **不会自动清零**、只能靠这个动作或者一次成功请求
/// 把它降下来。
#[must_use]
pub fn can_clear_strikes(strikes: u32) -> bool {
    strikes > 0
}

/// 哪些单条行内动作在执行前需要一次确认。**两种不同的理由触发它，别混成一条：**
///
/// ① **不可撤销**——只有 `delete` 符合。停用/启用/清冷却/解除剔除随时可以再点一次
///    撤回，弹窗只会让运维在这几个动作上多点一次没有实际保护作用的确认
///    ——那不是谨慎，是让人对真正需要谨慎的那个动作变得麻木。
/// ② **语义容易与相邻动作混淆**——`clearStrikes` 符合。它与「清冷却」长得像
///    「清空某个数字」，但清冷却只是让这把 key **现在**能用，它离下一次被剔除
///    仍然只差一次失败；`clearStrikes` 才会把连续失败计数真正清零。
///    这里的确认保护的不是「能不能撤销」，是「点的时候知不知道自己点的是哪一个」。
#[must_use]
pub fn row_action_needs_confirm(action: &str) -> bool {
    action == "delete" || action == "clearStrikes"
}

/// 同一条判据用在批量动作上。**三个批量动作里只有 `delete` 需要确认。**
#[must_use]
pub fn bulk_needs_confirm(op: &str) -> bool {
    op == "delete"
}

// ── 4. 分页与列表消息 ──────────────────────────────────────────────────────

/// 分页控件的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagerState {
    pub prev_disabled: bool,
    pub next_disabled: bool,
    /// `None` = **把「第 1/2 页 · 共 3 条」整条抹掉**。
    pub info: Option<PagerInfo>,
}

/// 见 [`PagerState`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagerInfo {
    pub page: usize,
    pub pages: usize,
    pub total: usize,
}

/// 分页控件的状态。
///
/// ⚠️ **空列表那一支必须把 `info` 一并复位**，否则下面还挂着上一次的
/// 「第 1/2 页 · 共 3 条」，那是在展示**一份已经不存在的数据**。
/// （读失败那一支在 Rust 侧走 500，压根到不了这里 —— 见
/// [`list_message_key`] 上方那段。）
#[must_use]
pub fn pager_state(item_count: usize, page: usize, pages: usize, total: usize) -> PagerState {
    if item_count == 0 {
        return PagerState {
            prev_disabled: true,
            next_disabled: true,
            info: None,
        };
    }
    PagerState {
        prev_disabled: page <= 1,
        next_disabled: page >= pages,
        info: Some(PagerInfo { page, pages, total }),
    }
}

/// 列表区该显示哪条消息（`None` = 显示表格）。
///
/// 🔴 **「池子里还没有 key」与「没有符合条件的 key」必须分开**：前者是「去导入」，
/// 后者是「把筛选条件放宽」，**指向完全相反的动作**。
///
/// ⚠️ TS 侧这个函数还有第三档（`common.loadFailed`）。**Rust 侧刻意没有**：
/// 读存储失败在这条端点上走的是 **500**，压根产不出一份「200 但读失败了」的响应体
/// ⇒ 在这里留一档等于挂一个**永远为假**的分支。那一档由渲染层接 HTTP 错误时自己说。
#[must_use]
pub fn list_message_key(item_count: usize, total_in_pool: usize) -> Option<&'static str> {
    if item_count > 0 {
        return None;
    }
    Some(if total_in_pool == 0 {
        "keys.empty"
    } else {
        "keys.noMatch"
    })
}

// ── 5. 单条 DELETE 的 409 ──────────────────────────────────────────────────

/// 单条 `DELETE` 的 409 拒绝是不是「必须先停用」这一种。
///
/// ⚠️ **它与批量路径的判据是同一件事的两种形状**：单条是 HTTP 409 + 顶层 `reason`，
/// 批量是 HTTP 200 + 逐项 `reason`。两条路径各自读自己那种形状，但
/// 「reason 是不是 `must_disable_first`」这条比较本身**只应该有一份实现**。
#[must_use]
pub fn is_must_disable_first_conflict(status: u16, reason: Option<&str>) -> bool {
    status == 409 && reason == Some("must_disable_first")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── badge_class ────────────────────────────────────────────────────────

    /// 四档的样式**两两不同**，且 `disabled` 是中性灰（不带任何颜色修饰类）。
    ///
    /// 回归对象：给「运维自己按下的开关」配报警色，让人以为出事了。
    #[test]
    fn badge_class_gives_four_distinguishable_styles_and_keeps_disabled_neutral() {
        assert_eq!(badge_class("disabled"), "badge");
        let all = [
            badge_class("disabled"),
            badge_class("evicted"),
            badge_class("cooling"),
            badge_class("fresh"),
        ];
        let mut seen: Vec<&str> = all.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 4, "四档必须互不相同：{all:?}");
    }

    /// 认不出来的档位落到「看起来正常」那一档 —— 与 TS 侧同语义
    /// （这条端点的 `bucket` 是闭集，认不出来只可能来自将来多一档）。
    #[test]
    fn badge_class_falls_back_to_the_ok_style() {
        assert_eq!(badge_class("something-new"), "badge badge-ok");
    }

    // ── bucket_label_key ───────────────────────────────────────────────────

    /// 五条 key **两两不同**，且认不出来的落到 `all`（「不筛」那一档）。
    #[test]
    fn bucket_label_keys_are_pairwise_distinct() {
        let all = [
            bucket_label_key("fresh"),
            bucket_label_key("cooling"),
            bucket_label_key("evicted"),
            bucket_label_key("disabled"),
            bucket_label_key("all"),
        ];
        let mut seen: Vec<&str> = all.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 5, "{all:?}");
        assert_eq!(bucket_label_key("zzz"), "keys.bucket.all");
    }

    /// 停用/启用那颗按钮说的是**下一步的动作**，而判据是**此刻的状态**。
    #[test]
    fn toggle_label_reads_the_current_state() {
        assert_eq!(toggle_disable_label_key(true), "keys.action.enable");
        assert_eq!(toggle_disable_label_key(false), "keys.action.disable");
    }

    // ── cooldown_remaining ─────────────────────────────────────────────────

    /// 只有 `cooling` 档给倒计时；别的档一律 `None`（渲染成 `—`）。
    ///
    /// 回归对象：给一把不在冷却的 key 画一个负数倒计时。
    #[test]
    fn cooldown_remaining_only_exists_for_the_cooling_bucket() {
        assert_eq!(cooldown_remaining("cooling", 1_500, 1_000), Some(500));
        assert_eq!(cooldown_remaining("fresh", 1_500, 1_000), None);
        assert_eq!(cooldown_remaining("disabled", 1_500, 1_000), None);
        assert_eq!(cooldown_remaining("evicted", 1_500, 1_000), None);
    }

    // ── 行内动作 ───────────────────────────────────────────────────────────

    /// 🔴 **`disabled` 与 `evicted` 是「或」，不是「且」，也不是只看其中一个。**
    ///
    /// 回归对象（两个方向都咬过）：只看 `evicted` ⇒ 手动停用的 key 删不掉；
    /// 只看 `disabled` ⇒ 被系统剔除的 key 删不掉。
    #[test]
    fn is_deletable_is_a_disjunction_of_both_facts() {
        assert!(!is_deletable(false, false));
        assert!(is_deletable(true, false));
        assert!(is_deletable(false, true));
        assert!(is_deletable(true, true));
    }

    /// 🔴 **「清冷却」的判据是时刻比较，不是 `bucket`。**
    ///
    /// 回归对象：一把「已停用且仍在冷却」的 key 的 `bucket` 是 `disabled`，
    /// 拿 `bucket` 判会让它的清冷却按钮永远禁用，而后端照样接受。
    #[test]
    fn can_clear_cooldown_ignores_the_bucket_projection() {
        assert!(can_clear_cooldown(1_001, 1_000));
        assert!(!can_clear_cooldown(1_000, 1_000), "到点了就不再是冷却中");
        assert!(!can_clear_cooldown(999, 1_000));
        assert!(!can_clear_cooldown(0, 1_000), "从来没冷却过");
    }

    /// 「解除剔除」同理：判据是 `evicted` 这个事实。
    #[test]
    fn can_unevict_reads_the_fact_not_the_projection() {
        assert!(can_unevict(true));
        assert!(!can_unevict(false));
    }

    /// 零 strikes 时那颗按钮点了什么都不会变 ⇒ 不给亮。
    #[test]
    fn can_clear_strikes_needs_a_nonzero_counter() {
        assert!(!can_clear_strikes(0));
        assert!(can_clear_strikes(1));
    }

    /// 🔴 **确认弹窗恰好两个动作**：不可撤销的 `delete`，与容易点错的 `clearStrikes`。
    ///
    /// 回归对象：给每个动作都加确认 ⇒ 运维对真正危险的那一个变得麻木。
    #[test]
    fn row_action_confirm_covers_exactly_delete_and_clear_strikes() {
        assert!(row_action_needs_confirm("delete"));
        assert!(row_action_needs_confirm("clearStrikes"));
        for a in ["disable", "enable", "clearCooldown", "unevict", "verify"] {
            assert!(!row_action_needs_confirm(a), "{a} 不该要确认");
        }
    }

    /// 批量那边只有 `delete`。
    #[test]
    fn bulk_confirm_covers_only_delete() {
        assert!(bulk_needs_confirm("delete"));
        assert!(!bulk_needs_confirm("disable"));
        assert!(!bulk_needs_confirm("clearCooldown"));
    }

    // ── pager_state ────────────────────────────────────────────────────────

    /// 🔴 **空列表必须把 `info` 一并复位** —— 否则屏幕上挂着一份已经不存在的数据的页码。
    #[test]
    fn pager_state_resets_the_info_line_when_there_is_nothing_to_show() {
        let p = pager_state(0, 2, 3, 47);
        assert!(p.prev_disabled && p.next_disabled);
        assert_eq!(p.info, None);
    }

    /// 首页禁「上一页」、末页禁「下一页」；中间两颗都亮。
    #[test]
    fn pager_state_disables_the_edges() {
        let first = pager_state(20, 1, 3, 47);
        assert!(first.prev_disabled && !first.next_disabled);
        let mid = pager_state(20, 2, 3, 47);
        assert!(!mid.prev_disabled && !mid.next_disabled);
        let last = pager_state(7, 3, 3, 47);
        assert!(!last.prev_disabled && last.next_disabled);
        assert_eq!(
            last.info,
            Some(PagerInfo {
                page: 3,
                pages: 3,
                total: 47
            })
        );
    }

    /// 只有一页时两颗都禁。
    #[test]
    fn pager_state_with_a_single_page_disables_both() {
        let p = pager_state(3, 1, 1, 3);
        assert!(p.prev_disabled && p.next_disabled);
        assert!(p.info.is_some(), "但页码信息照常显示");
    }

    // ── list_message_key ───────────────────────────────────────────────────

    /// 🔴 **「池子里还没有 key」与「筛不出来」是两句话，指向相反的动作。**
    ///
    /// 回归对象：两者合成一句「暂无数据」，运维不知道该去导入还是去放宽筛选。
    #[test]
    fn list_message_key_splits_an_empty_pool_from_an_empty_filter() {
        assert_eq!(list_message_key(0, 0), Some("keys.empty"));
        assert_eq!(list_message_key(0, 12), Some("keys.noMatch"));
        assert_eq!(list_message_key(5, 12), None, "有条目就显示表格");
    }

    // ── is_must_disable_first_conflict ─────────────────────────────────────

    /// 两个判据都要：**状态码对、`reason` 也对**才是这一种。
    #[test]
    fn must_disable_first_needs_both_the_status_and_the_reason() {
        assert!(is_must_disable_first_conflict(
            409,
            Some("must_disable_first")
        ));
        assert!(!is_must_disable_first_conflict(409, Some("something_else")));
        assert!(!is_must_disable_first_conflict(409, None));
        assert!(!is_must_disable_first_conflict(
            400,
            Some("must_disable_first")
        ));
    }
}
