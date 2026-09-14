//! 「支持模型」板块的取值判定。
//!
//! 对应 TS 侧 `admin-ui/js/pure/models.mjs`（277 行）里的 **A 档**那一半
//! （`protocolBadges` / `modalityLabelKey`）。
//!
//! ⚠️ TS 侧那一份里还有 `catalogProtocols` / `catalogModels` 两个**窄化**函数
//! （「响应读不出来时整份判成 `null`，不是空数组」）。**Rust 侧刻意没有对应物**：
//! 那两个函数窄化的是**自己后端刚发出去的**那份 JSON —— 在 Rust 里它是
//! `#[derive(Serialize)]` 的结构体，「格式不对」这一档在类型上就不存在。
//! 🔴 **别为了「对齐 TS」在这里补一层反序列化再校验**：那会造出一族永远为假的分支。

/// 一个模型在一条协议上的格子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolBadge<'a> {
    pub id: &'a str,
    /// 协议的**专名**（"OpenAI Chat Completions"）。
    ///
    /// ⚠️ **它刻意不进 i18n**：翻译一个专名只会制造歧义。
    pub label: &'a str,
    pub available: bool,
}

impl ProtocolBadge<'_> {
    /// 这个格子的 CSS 类。
    ///
    /// ⚠️ **不可用那一档有自己的类（`badge-off`），不是「不画」**，见
    /// [`protocol_badges`] 上方那段。渲染层在 `badge-off` 上加删除线。
    #[must_use]
    pub fn badge_class(self) -> &'static str {
        if self.available {
            "badge badge-success"
        } else {
            "badge badge-off"
        }
    }
}

/// 一个模型在**每一条**协议上各自可不可用。
///
/// ⚠️⚠️ **恒返回与目录等长的一串，不可用的那几项 `available: false`。**
/// 🔴 **不许把不可用的过滤掉**：格子恒在，运维一眼就能看出「这个模型只在一条上可用」；
/// 过滤掉之后图片 / 视频模型那一行会是**空的**，**与「读不出来」长得一模一样**
/// —— 而那是两件事。
///
/// ⚠️ 夹具要用**图片或视频模型**：对话模型四条协议全有，
/// 「过滤」与「不过滤」在它身上**数学上等价** ⇒ 拿它做夹具这条不变量不可观测。
///
/// `catalog` 的**顺序即渲染顺序**。
#[must_use]
pub fn protocol_badges<'a>(
    model_protocols: &[&'a str],
    catalog: &[(&'a str, &'a str)],
) -> Vec<ProtocolBadge<'a>> {
    catalog
        .iter()
        .map(|(id, label)| ProtocolBadge {
            id,
            label,
            available: model_protocols.contains(id),
        })
        .collect()
}

/// 形态 → 那一列上的文案 key。**列的显示名叫「类型」，字段名一律 `modality`。**
///
/// ⚠️ **这三个字面量是本模块唯一一处「要翻译的词汇表」，它为什么不像 `label`
/// 那样来自目录**：协议的 `label` 是**专名**，翻译它只会制造歧义；
/// 「对话 / 图片 / 视频」不是专名，五种语言各不相同，而协议目录里没有、
/// 也不该有一份五语言表。
///
/// **代价明写**：真源新增一个 modality 时这张表不会自动跟上。所以它 **fail-open**
/// —— 表外的值返回 `None`，渲染层把原值照实画出来，**绝不冒充任何一档已知形态**。
/// 🔴 别把它改成「认不出来就当 chat」：那会让一个新增的形态在面板上伪装成对话模型。
#[must_use]
pub fn modality_label_key(modality: &str) -> Option<&'static str> {
    match modality {
        "chat" => Some("models.modality.chat"),
        "image" => Some("models.modality.image"),
        "video" => Some("models.modality.video"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: [(&str, &str); 4] = [
        ("openai", "OpenAI Chat Completions"),
        ("anthropic", "Anthropic Messages"),
        ("responses", "OpenAI Responses"),
        ("gemini", "Gemini generateContent"),
    ];

    /// 🔴 **格子恒与目录等长，不可用的画灰不隐藏。**
    ///
    /// 回归对象：把不可用的过滤掉 ⇒ 一个只在一条协议上可用的模型那一行看起来
    /// 与「这一行读不出来」一模一样。
    ///
    /// ⚠️ 夹具刻意用一个**只在一条协议上可用**的模型：四条全有的对话模型身上
    /// 「过滤」与「不过滤」数学上等价，这条不变量在它身上不可观测。
    #[test]
    fn every_protocol_keeps_its_cell_even_when_the_model_cannot_use_it() {
        let got = protocol_badges(&["gemini"], &CATALOG);
        assert_eq!(got.len(), CATALOG.len(), "四个格子恒在");
        let avail: Vec<bool> = got.iter().map(|b| b.available).collect();
        assert_eq!(avail, vec![false, false, false, true]);
        let ids: Vec<&str> = got.iter().map(|b| b.id).collect();
        assert_eq!(
            ids,
            vec!["openai", "anthropic", "responses", "gemini"],
            "顺序即目录顺序"
        );
    }

    /// 四条全有的模型：四个格子全亮。
    #[test]
    fn a_model_available_everywhere_lights_up_every_cell() {
        let got = protocol_badges(&["openai", "anthropic", "responses", "gemini"], &CATALOG);
        assert!(got.iter().all(|b| b.available));
    }

    /// 一条都不可用的模型**照样有四个格子**（全灰）——不是空行。
    #[test]
    fn a_model_available_nowhere_still_renders_four_cells() {
        let got = protocol_badges(&[], &CATALOG);
        assert_eq!(got.len(), 4);
        assert!(got.iter().all(|b| !b.available));
    }

    /// 两档样式互不相同，且「不可用」有自己的类（渲染层据它加删除线）。
    #[test]
    fn the_two_badge_styles_are_distinguishable() {
        let on = ProtocolBadge {
            id: "x",
            label: "X",
            available: true,
        };
        let off = ProtocolBadge {
            available: false,
            ..on
        };
        assert_eq!(on.badge_class(), "badge badge-success");
        assert_eq!(off.badge_class(), "badge badge-off");
        assert_ne!(on.badge_class(), off.badge_class());
    }

    /// 三档形态各有一条文案；**表外的值返回 `None`**（照实画原值）。
    ///
    /// 回归对象：把兜底写成「认不出来就当 chat」⇒ 一个新增的形态在面板上
    /// 伪装成对话模型。
    #[test]
    fn an_unknown_modality_is_never_disguised_as_a_known_one() {
        assert_eq!(modality_label_key("chat"), Some("models.modality.chat"));
        assert_eq!(modality_label_key("image"), Some("models.modality.image"));
        assert_eq!(modality_label_key("video"), Some("models.modality.video"));
        assert_eq!(modality_label_key("audio"), None);
        assert_eq!(modality_label_key(""), None);
    }

    /// 三条 key 两两不同。
    #[test]
    fn modality_label_keys_are_pairwise_distinct() {
        let all: Vec<&str> = ["chat", "image", "video"]
            .iter()
            .filter_map(|m| modality_label_key(m))
            .collect();
        let mut seen = all.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3, "{all:?}");
    }
}
