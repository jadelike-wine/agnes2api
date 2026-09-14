//! 「掩码 / 点击显示明文 / 复制」这一族的**档位判定**。
//!
//! 对应 TS 侧 `admin-ui/js/pure/reveal.mjs`（123 行）里的 **A 档**那一半
//! （`revealOutcome` / `revealErrorOutcome` 四态）。
//!
//! **两个板块共用**：Key 池（上游 Agnes key，字段名 `key`）与 API 密钥
//! （我们签发的 `sk-`，字段名 `secret`）。两者的端点、字段名、「取不到」的成因
//! 各不相同，但**状态机逐字相同** —— 所以状态机住在这里，各自的字段名住在各自的
//! handler 里。
//!
//! # 🔴 为什么端点要直接吐 `kind`，而不是让面板自己从字段组合里反推
//!
//! TS 侧 `revealOutcome` 的判据是「`secret` 是不是非空字符串」+「`reason` 是不是
//! `issued_before_plaintext`」两件事的组合，而组合判据的每一个消费者都要把它抄一遍。
//! 上移之后端点直接给一个闭集的 `kind`，面板只需要 `switch`。

/// 一次 reveal 的结局。**四档互不合并。**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevealKind {
    /// 拿到了明文。
    Ok,
    /// 🔴 **「这把密钥签发在明文落盘之前，服务端从来就没有过它的明文」**
    /// —— 一个**永远不会变**的事实，运维该做的是重发一把。
    Unavailable,
    /// 这条记录已经不在了（另一个标签页删了它 / 别的副本剔除了它）。
    /// 处置是「刷新一下列表」。
    Gone,
    /// **这一次**没拿到，重试可能就好了。
    Failed,
}

impl RevealKind {
    /// 对外的字面量。**面板按它分支，改了就是改对外契约。**
    ///
    /// ⚠️ 它是 `const fn`，好让两条 reveal 端点的那几个 `pub const` **由它派生**
    /// 而不是各抄一份 —— 两张表迟早会漂，而漂的那一格恰好是
    /// 「永远取不回」与「这一次没取到」的分界。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Unavailable => "unavailable",
            Self::Gone => "gone",
            Self::Failed => "failed",
        }
    }

    /// 四态各自的五语言文案键。`Ok` 没有文案（那一档要显示的是明文本身）。
    ///
    /// **表外不兜底** —— 与本仓其余「表外返回 `None`」同一条纪律。
    #[must_use]
    pub fn message_key(self) -> Option<&'static str> {
        match self {
            Self::Ok => None,
            Self::Unavailable => Some("reveal.unavailable"),
            Self::Gone => Some("reveal.gone"),
            Self::Failed => Some("reveal.failed"),
        }
    }
}

/// 200 响应体那一半：有明文就是 [`RevealKind::Ok`]，没有就看后端给没给理由。
///
/// 🔴 **`Unavailable` 与 `Failed` 必须分开，不许并成「取不到」。**
/// 前者永远不会变（该重发一把），后者重试可能就好。把两者混成一句话，
/// 会让人**对着一把永远取不回的密钥反复点**。
///
/// ⚠️ **`Ok` 那一档要求明文是非空字符串**：后端约定 `secret: null` 表示前一档，
/// 而空串既不是 null 也不是可用明文 —— 真出现只可能是坏数据，按失败处置，
/// **绝不把空串当明文交出去**。
#[must_use]
pub fn reveal_kind(secret: Option<&str>, issued_before_plaintext: bool) -> RevealKind {
    match secret {
        Some(s) if !s.is_empty() => RevealKind::Ok,
        _ if issued_before_plaintext => RevealKind::Unavailable,
        _ => RevealKind::Failed,
    }
}

/// **抛出来的那一族**（非 2xx）→ 与上面同一套形状的档位。
///
/// ⚠️ **404 单独一档（[`RevealKind::Gone`]），不许并进 `Failed`。**
/// Key 池那张表是轮询刷新的：一把 key 在另一个标签页被删、被别的副本剔除之后，
/// 这一行还在屏幕上，而它的 reveal 一定是 404。那一档的处置是「刷新一下列表」，
/// `Failed` 那一档的处置是「等一会儿再点」——把它们并成一句话，运维会**对着一条
/// 已经不存在的记录反复点**。
///
/// ⚠️ **401 刻意落在 `Failed`，不另起一档**：401 已经先清凭据 + 弹登录闸了，
/// 那一屏本身就是最强的反馈；而「这一次没取到明文，稍后再试」对它**也是真话**。
///
/// ⚠️ **判据是 `status`，不是错误体里那个 `code`**：那个码由后端定义
/// （`key_not_found` / `apikey_not_found` 两族各一个），照它分档就等于在渲染层
/// 抄一份后端的错误码表，而这一族只需要分「没有这条记录」与「这一次没成」两种处置。
#[must_use]
pub fn reveal_error_kind(status: u16) -> RevealKind {
    if status == 404 {
        RevealKind::Gone
    } else {
        RevealKind::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 非空明文 ⇒ `ok`。
    #[test]
    fn a_non_empty_secret_is_ok() {
        assert_eq!(reveal_kind(Some("sk-abc"), false), RevealKind::Ok);
    }

    /// 🔴 **空串不是明文。** 真出现只可能是坏数据 ⇒ 按失败处置。
    ///
    /// 回归对象：把空串当明文塞进剪贴板，运维粘出来是个空的。
    #[test]
    fn an_empty_string_is_never_treated_as_a_secret() {
        assert_eq!(reveal_kind(Some(""), false), RevealKind::Failed);
        assert_eq!(
            reveal_kind(Some(""), true),
            RevealKind::Unavailable,
            "后端明说了理由时仍然报那一档"
        );
    }

    /// 🔴 **「服务端从来就没有过它的明文」与「这一次没拿到」是两件事。**
    ///
    /// 回归对象：两者并成「取不到」，运维对着一把永远取不回的密钥反复点。
    #[test]
    fn unavailable_is_never_merged_into_failed() {
        assert_eq!(reveal_kind(None, true), RevealKind::Unavailable);
        assert_eq!(reveal_kind(None, false), RevealKind::Failed);
        assert_ne!(
            RevealKind::Unavailable.as_str(),
            RevealKind::Failed.as_str()
        );
    }

    /// 🔴 **404 单独一档**：处置是「刷新列表」，不是「等一会儿再点」。
    #[test]
    fn a_404_is_gone_not_merely_failed() {
        assert_eq!(reveal_error_kind(404), RevealKind::Gone);
        assert_eq!(reveal_error_kind(500), RevealKind::Failed);
        assert_eq!(reveal_error_kind(429), RevealKind::Failed);
    }

    /// 401 刻意落在 `failed` —— 登录闸本身就是最强的反馈。
    #[test]
    fn a_401_stays_in_the_failed_bucket() {
        assert_eq!(reveal_error_kind(401), RevealKind::Failed);
    }

    /// 四个字面量互不相同；`ok` 那一档**没有文案**（要显示的是明文本身）。
    #[test]
    fn wire_values_are_pairwise_distinct_and_only_ok_has_no_message() {
        let all = [
            RevealKind::Ok,
            RevealKind::Unavailable,
            RevealKind::Gone,
            RevealKind::Failed,
        ];
        let mut seen: Vec<&str> = all.iter().map(|k| k.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), all.len());

        assert_eq!(RevealKind::Ok.message_key(), None);
        let msgs: Vec<&str> = all.iter().filter_map(|k| k.message_key()).collect();
        assert_eq!(msgs.len(), 3, "另外三档各有一条、且互不相同：{msgs:?}");
        let mut m = msgs.clone();
        m.sort_unstable();
        m.dedup();
        assert_eq!(m.len(), 3);
    }
}
