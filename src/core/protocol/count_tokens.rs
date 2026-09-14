//! `POST /v1/messages/count_tokens` —— **纯本地估算，零上游、零存储**（裁定 G）。
//!
//! # 它为什么是新增的（TS 版没有这条端点）
//!
//! kiro2api 有，而实物核对的结论是：那条端点**不打网络、不选账号、空池也 200**。
//! agnes 可以同样诚实地做 —— 而**不做**的代价是具体的：`google-genai` 的
//! `count_tokens()`、以及一切「发正式请求前先探一次 token 数」的框架，
//! 在没有这条端点时会退化成**真发一次对话**去数 token（Gemini 那条的
//! `:countTokens` 方法名白名单缺陷就是这个形状的实测记录）。
//!
//! # 🔴 口径登记为 `assumed`
//!
//! 本网关**没有**上游的 tokenizer，也没有上游的 token 计数接口。
//! 这里的数是**本网关自己的估算**，不是上游计数。
//!
//! * 公式：`总字符数 / CHARS_PER_TOKEN`，**下界 1**；
//! * `CHARS_PER_TOKEN = 4` 是英文经验粗估（照 kiro2api 的同名常量，**非官方**）；
//! * ⚠️ **中文会被低估约三倍**（一个汉字通常 1~2 token，这里按 0.25 算）。
//!   kiro2api 为记账另做了一份按字符类别加权的口径，而把 `chars/4` 那一版**保留**给
//!   `count_tokens`，理由是「对外承诺过那个数值口径，改它属于契约变更」。
//!   agnes 这条端点是新的、还没承诺过任何口径 —— 但仍然照抄 `chars/4`：
//!   **加权那一版的权重表同样是没有真源的估计**，换一份没有依据的公式去修另一份
//!   没有依据的公式，只是把偏差换个方向，却要多背一张表。**偏差如实写进文档，不硬凑。**
//! * 🔴 **它必须写进五语言 API.md：「这是本网关的本地估算，不是上游计数」**，
//!   并在 `upstream_facts` 里标成 `assumed`。客户端拿它做预算规划时会偏。
//!
//! # 🔴 为什么它走与 `/v1/messages` **完全相同**的请求整形
//!
//! 这是一处相对 kiro2api 的刻意偏离：kiro 的 `count_tokens` 自己数图片块与工具定义，
//! 而 agnes 的 `/v1/messages` 对图片块与 `tools` 是 **400**。
//! 两边口径若不一致，就会出现「`count_tokens` 说这份请求 1200 token，
//! 真发过去却 400」——**一个不能被真正发送的请求，估算它的 token 数没有任何意义**，
//! 而客户端会先信前者。
//! ⇒ **能被这条端点估算的请求，必须是这台网关真能转发的请求。**

use super::anthropic;
use super::request_shape::ShapeResult;
use serde_json::{Value, json};

/// 字符 → token 的粗略换算基数（英文经验粗估）。**非官方。**
///
/// 照 kiro2api 的同名常量取 4。改它就是改一条已经对外报出去过的数值口径。
pub const CHARS_PER_TOKEN: usize = 4;

/// 估算一份 Anthropic 请求体的输入 token 数。
///
/// ⚠️ **按 `chars().count()` 数字符，不是 `len()` 数字节。**
/// 用字节数的话，同一段中文会比同一段英文多出三倍的「字符」，
/// 估出来的 token 数**系统性偏高三倍** —— 而这个数是客户端用来做预算规划的。
///
/// ⚠️ **下界 1**：一个合法请求的 token 数不可能是 0，回 0 会让按它做除法 /
/// 做「要不要发」判断的客户端拿到一个不可能的值。
///
/// 零上游、零存储、零时钟 —— 它就是一个纯函数。
pub fn estimate_input_tokens(req: &Value) -> ShapeResult<u32> {
    // 🔴 与 `/v1/messages` 同一道整形：见模块头「为什么走完全相同的请求整形」。
    let internal = anthropic::to_internal_request(req)?;
    let chars: usize = internal
        .messages
        .iter()
        .map(|m| m.content.chars().count())
        .sum();
    Ok(u32::try_from((chars / CHARS_PER_TOKEN).max(1)).unwrap_or(u32::MAX))
}

/// `POST /v1/messages/count_tokens` 的响应体（照 Anthropic 公开规范的形状）。
#[must_use]
pub fn count_tokens_response(input_tokens: u32) -> Value {
    json!({ "input_tokens": input_tokens })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::request_shape::RequestShapeError;

    fn body(text: &str) -> Value {
        json!({ "model": "m", "messages": [{ "role": "user", "content": text }] })
    }

    /// 口径本身：`总字符数 / 4`。
    #[test]
    fn the_estimate_is_total_chars_over_four() {
        // 40 个 ASCII 字符 ⇒ 10。
        assert_eq!(estimate_input_tokens(&body(&"a".repeat(40))).unwrap(), 10);
        // 41 个 ⇒ 仍是 10（整除截断，与 kiro 的 `chars / CHARS_PER_TOKEN` 逐字同义）。
        assert_eq!(estimate_input_tokens(&body(&"a".repeat(41))).unwrap(), 10);
    }

    /// 🔴 **下界 1**：一个合法请求的 token 数不可能是 0。
    ///
    /// 回归对象：回 0 会让按它做除法 / 做「要不要发」判断的客户端拿到一个不可能的值。
    #[test]
    fn the_estimate_never_returns_zero() {
        assert_eq!(estimate_input_tokens(&body("a")).unwrap(), 1);
        assert_eq!(estimate_input_tokens(&body("")).unwrap(), 1);
    }

    /// 🔴 **按字符数，不按字节数。**
    ///
    /// 回归对象：`len()` 下同一段中文的「长度」是字符数的三倍 ⇒ 估值系统性偏高三倍，
    /// 而这个数是客户端用来做预算规划的。
    #[test]
    fn multibyte_text_is_counted_by_chars_not_bytes() {
        // 8 个汉字 = 24 字节。按字符是 8/4 = 2；按字节会是 24/4 = 6。
        let cn = "一二三四五六七八";
        assert_eq!(cn.len(), 24);
        assert_eq!(estimate_input_tokens(&body(cn)).unwrap(), 2);
    }

    /// `system` 与每一条消息都计入（它们都进了上游请求体，都是要付钱的）。
    #[test]
    fn system_and_every_message_are_all_counted() {
        let v = json!({
            "model": "m",
            "system": &"s".repeat(8),
            "messages": [
                { "role": "user", "content": "u".repeat(8) },
                { "role": "assistant", "content": [{ "type": "text", "text": "a".repeat(8) }] },
            ],
        });
        // 24 个字符 ⇒ 6。
        assert_eq!(estimate_input_tokens(&v).unwrap(), 6);
    }

    /// 🔴 **能被估算的请求，必须是这台网关真能转发的请求。**
    ///
    /// 回归对象：若这里自己数图片 / 工具定义而不复用整形，就会出现
    /// 「`count_tokens` 说这份请求 1200 token，真发过去却 400」——
    /// **估算一个发不出去的请求没有任何意义**，而客户端会先信前者。
    #[test]
    fn anything_the_relay_would_reject_is_rejected_here_too() {
        // 非文本内容块。
        let img = json!({
            "model": "m",
            "messages": [{ "role": "user", "content": [{ "type": "image" }] }],
        });
        assert_eq!(
            estimate_input_tokens(&img).unwrap_err(),
            RequestShapeError::UnsupportedContent {
                block_type: "image".into()
            }
        );
        // ③ 档参数。
        let tools = json!({
            "model": "m", "tools": [],
            "messages": [{ "role": "user", "content": "hi" }],
        });
        assert!(estimate_input_tokens(&tools).is_err());
        // 形状不对。
        for bad in [json!(null), json!({ "model": "m" }), json!("hi")] {
            assert!(estimate_input_tokens(&bad).is_err(), "{bad}");
        }
    }

    /// 响应体形状（Anthropic 公开规范：只有 `input_tokens` 一格）。
    #[test]
    fn the_response_body_has_exactly_one_field() {
        let v = count_tokens_response(42);
        assert_eq!(v, json!({ "input_tokens": 42 }));
        assert_eq!(v.as_object().unwrap().len(), 1);
    }

    /// `CHARS_PER_TOKEN` 是对外报出去过的数值口径的一部分，逐字钉住。
    #[test]
    fn the_chars_per_token_constant_is_pinned() {
        assert_eq!(
            CHARS_PER_TOKEN, 4,
            "改它就是改一条已经对外报出去过的数值口径（非官方，登记为 assumed）"
        );
    }

    /// **零上游、零存储、零时钟** —— 同样的输入永远给同样的输出。
    /// 这一格是零 IO 岛在本模块的具体形态。
    #[test]
    fn the_estimate_is_a_pure_function() {
        let v = body("hello world, this is a prompt");
        assert_eq!(
            estimate_input_tokens(&v).unwrap(),
            estimate_input_tokens(&v).unwrap()
        );
    }
}
