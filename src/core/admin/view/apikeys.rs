//! 「API 密钥」板块（我们签发给别人的 `sk-`）的取值判定。
//!
//! 对应 TS 侧 `admin-ui/js/pure/apikeys.mjs`（267 行）里的 **A 档**那一半。
//!
//! ⚠️⚠️ **本板块说的是「别人拿来向我们证明身份」的那一族，不是 Key 池那一族。**
//! 两者方向相反：Key 池装的是**我们持有的**上游凭据（在 [`super::keys`]），
//! 这里是**我们签发出去的**。名字很像是已知代价。

// ── 1. 分档的样式与文案 ────────────────────────────────────────────────────

/// 分档徽章的 CSS 类。**三档互不相同**——两档共用一个样式等于面板分不出它们。
///
/// ⚠️ 「已停用」不带颜色修饰类（中性灰）：它不是故障，是运维自己按下的开关。
/// 与 [`super::keys::badge_class`] 里 Key 池那一族的同名裁定**逐字同源**。
#[must_use]
pub fn ak_badge_class(bucket: &str) -> &'static str {
    match bucket {
        "disabled" => "badge",
        "expired" => "badge badge-warn",
        _ => "badge badge-ok",
    }
}

/// 分档名的 i18n key。**三条各写一次字面量**，好让 i18n 门禁扫得到。
#[must_use]
pub fn ak_bucket_label_key(bucket: &str) -> &'static str {
    match bucket {
        "disabled" => "ak.bucket.disabled",
        "expired" => "ak.bucket.expired",
        _ => "ak.bucket.active",
    }
}

/// 「停用 / 启用」那颗开关上的文案 key。**看的是这一条此刻的状态，
/// 不是它将要变成什么。**
#[must_use]
pub fn ak_toggle_label_key(disabled: bool) -> &'static str {
    if disabled {
        "ak.action.enable"
    } else {
        "ak.action.disable"
    }
}

// ── 2. 列表的三态 ──────────────────────────────────────────────────────────

/// 列表的三种状态，**恒有一种**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AkListState {
    /// 后端说存储里那张表**读不出来**。
    Unreadable,
    /// 拿到了一份列表（可能是空的）。
    Ok,
}

impl AkListState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::Ok => "ok",
        }
    }
}

/// 列表的状态。
///
/// ⚠️⚠️ **`unreadable` 与「一把都没有」必须分开，这是本板块最要紧的一条。**
/// 后端在读不出来时同样返回 `keys: []`，拿 `keys.length == 0` 去判就会把
/// 「表坏了、全部客户端正在 401」画成一句「你还没签发过密钥」。
///
/// ⚠️ TS 侧这个函数还有第三档（`error`，这次请求本身失败了）。**Rust 侧刻意没有**：
/// 那一档在 HTTP 层就是非 2xx，压根产不出一份响应体 ⇒ 留着等于挂一个永远为假的分支。
#[must_use]
pub fn ak_list_state(unreadable: bool) -> AkListState {
    if unreadable {
        AkListState::Unreadable
    } else {
        AkListState::Ok
    }
}

// ── 3. 「清理失效（N）」 ───────────────────────────────────────────────────

/// 「清理失效（N）」那颗按钮上的 N。
///
/// **判据是分档，与后端 `is_api_key_usable` 是同一条**：另写一条判据的话，
/// 那颗按钮删掉的条数会与它自己写的数字对不上。
///
/// ⚠️ **它数的是整张表，不是当前搜索结果**：那颗按钮清的就是整张表里失效的那些，
/// 让它跟着搜索框变会让人以为自己只清掉了看得见的那几条。
#[must_use]
pub fn ak_purge_count<'a, I: IntoIterator<Item = &'a str>>(buckets: I) -> usize {
    buckets
        .into_iter()
        .filter(|b| *b == "disabled" || *b == "expired")
        .count()
}

/// N = 0 时那颗按钮**整个不画**：一颗点了什么都不会发生的按钮比没有更糟。
#[must_use]
pub fn ak_purge_visible(purge_count: usize) -> bool {
    purge_count > 0
}

/// 列表为空时那句话选哪一条。**「没有密钥」与「搜索没命中」是两句不同的话。**
///
/// 🔴 **`Unreadable` 那一档恒返回 `None`，这一条是本函数存在的一半理由**：
/// 表读不出来时后端同样发 `keys: []`，光看条数就会说出
/// 「你还没签发过密钥」——而真相是那张表坏了、全部客户端正在 401。
/// 那一档由 [`ak_list_state`] 独占，**这里一个字都不许替它说**。
///
/// 🔴 搜索是**客户端**做的（这条端点一次发整张表），所以这里只能回答
/// 「整张表空不空」那一半 ⇒ 表非空时返回 `None`，由渲染层在自己筛完之后
/// 决定要不要换成 `ak.emptyFiltered`。**这是 A/C 分档的边界**，别把筛选搬上来。
#[must_use]
pub fn ak_empty_key(state: AkListState, total: usize) -> Option<&'static str> {
    if state != AkListState::Ok {
        return None;
    }
    if total == 0 { Some("ak.empty") } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三档的样式**两两不同**，且 `disabled` 是中性灰。
    #[test]
    fn ak_badge_class_gives_three_distinguishable_styles() {
        assert_eq!(ak_badge_class("disabled"), "badge");
        let all = [
            ak_badge_class("disabled"),
            ak_badge_class("expired"),
            ak_badge_class("active"),
        ];
        let mut seen: Vec<&str> = all.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3, "{all:?}");
    }

    /// 与 Key 池那一族的同名裁定**逐字同源**：两边「已停用」都是中性灰。
    ///
    /// 回归对象：只改其中一边，于是同一条产品判断在两个板块上长得不一样。
    #[test]
    fn the_disabled_badge_matches_the_key_pool_verdict() {
        assert_eq!(
            ak_badge_class("disabled"),
            super::super::keys::badge_class("disabled")
        );
    }

    /// 三条 key 两两不同，认不出来的落到 `active`。
    #[test]
    fn ak_bucket_label_keys_are_pairwise_distinct() {
        let all = [
            ak_bucket_label_key("disabled"),
            ak_bucket_label_key("expired"),
            ak_bucket_label_key("active"),
        ];
        let mut seen: Vec<&str> = all.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3, "{all:?}");
        assert_eq!(ak_bucket_label_key("zzz"), "ak.bucket.active");
    }

    /// 开关上的文案看的是此刻的状态。
    #[test]
    fn ak_toggle_label_reads_the_current_state() {
        assert_eq!(ak_toggle_label_key(true), "ak.action.enable");
        assert_eq!(ak_toggle_label_key(false), "ak.action.disable");
    }

    /// 🔴 **「表读不出来」与「一把都没有」是两件事。**
    ///
    /// 回归对象：拿 `keys.length == 0` 判，于是「表坏了、全部客户端正在 401」
    /// 被画成一句「你还没签发过密钥」。
    #[test]
    fn ak_list_state_splits_unreadable_from_empty() {
        assert_eq!(ak_list_state(true), AkListState::Unreadable);
        assert_eq!(ak_list_state(false), AkListState::Ok);
        assert_ne!(AkListState::Unreadable.as_str(), AkListState::Ok.as_str());
    }

    /// 🔴 **数的是「停用」+「过期」两档，与 `is_api_key_usable` 同一条判据。**
    ///
    /// 回归对象：只数其中一档 ⇒ 按钮上的数字与它真删掉的条数对不上。
    #[test]
    fn ak_purge_count_covers_both_unusable_buckets() {
        assert_eq!(
            ak_purge_count(["active", "disabled", "expired", "active", "expired"]),
            3
        );
        assert_eq!(ak_purge_count(["active", "active"]), 0);
        assert_eq!(ak_purge_count(std::iter::empty::<&str>()), 0);
    }

    /// 一颗点了什么都不会发生的按钮比没有更糟 ⇒ N = 0 时整个不画。
    #[test]
    fn ak_purge_button_hides_itself_when_there_is_nothing_to_purge() {
        assert!(!ak_purge_visible(0));
        assert!(ak_purge_visible(1));
    }

    /// 整张表空 ⇒ 「还没签发过」；非空 ⇒ `None`（筛选那一半归渲染层）。
    #[test]
    fn ak_empty_key_only_answers_the_half_the_server_knows() {
        assert_eq!(ak_empty_key(AkListState::Ok, 0), Some("ak.empty"));
        assert_eq!(ak_empty_key(AkListState::Ok, 3), None);
    }

    /// 🔴 **表读不出来时一个字都不许说** —— 那一档同样是 `keys: []`，
    /// 光看条数就会说出「你还没签发过密钥」，而真相是那张表坏了。
    ///
    /// 回归对象：判据只看条数、不看状态。
    #[test]
    fn ak_empty_key_stays_silent_when_the_table_is_unreadable() {
        assert_eq!(ak_empty_key(AkListState::Unreadable, 0), None);
    }
}
