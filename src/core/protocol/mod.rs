//! 四条对外协议 ↔ [`InternalRequest`] 的双向转换 + SSE 原语。**这里不发 HTTP。**
//!
//! # 母协议是 **OpenAI**（裁定 D；与 kiro2api 的 Anthropic 不同，理由在这里）
//!
//! kiro 的母协议是 Anthropic，因为它的上游语义就是 Anthropic 形状。
//! agnes 的上游是 **OpenAI 兼容的 `/chat/completions`**，所以：
//!
//! - [`InternalRequest`] 就是 OpenAI chat 请求体的子集；
//! - [`openai`] 是**最薄的一条**：不解析请求体、不解析响应体，原样搬运
//!   （它唯一做的事是三样本地形状校验，理由见那边）；
//! - [`anthropic`] / [`gemini`] / [`responses`] 三条 = 自家 DTO + 自家双向转换；
//! - 🔴 **只有一条 relay 内核**（`pool::dispatcher::dispatch`），
//!   **不许任何协议自己拉一条到上游的链路**。本模块一个出站调用都没有，
//!   这条边界因此是**结构性**的：`core` 连 `ports` 都不许依赖，想违反得先改模块树。
//!
//! # 文件分工
//!
//! | 文件 | 放什么 | 它最要紧的那一条 |
//! |---|---|---|
//! | [`request_shape`] | 🔴 三档参数裁定表（三条协议**共用这一张**）+ 三个守卫 + 错误枚举 | **做不到就明说，别假装做到了** |
//! | [`sse`] | `frame_end` / `extract_payloads` / `sse_event` / `parse_sse_stream` | 帧边界认 CRLF；收尾 flush 不许省 |
//! | [`openai`] | 写死的 12 条模型清单 + `/v1/models` 响应 + 本地形状校验 | 清单顺序不是随手排的 |
//! | [`anthropic`] | 请求整形 / 非流式响应 / 🔴 流式七段事件序列 | `content_filter → refusal`；末尾 usage 是真值 |
//! | [`gemini`] | 同上 + 方法名白名单 + `/v1beta/models` | 🔴 **终帧必须有 `finishReason` 与 `usageMetadata`** |
//! | [`responses`] | 同上 + 🔴 官方 SDK 能用的最小事件序列 | `item_id`/`sequence_number`/`logprobs` 一格都不许少 |
//! | [`count_tokens`] | 纯本地估算（裁定 G） | **零上游、零存储**，口径登记为 `assumed` |
//!
//! # 🔴 四条流**都必须有终帧**
//!
//! 这是本模块最贵的一条判据，买单的是一条实测缺陷：Gemini 那条流从前全程不出现
//! `finishReason` 与 `usageMetadata` ⇒ **一个被截断的半截回答和一个完整回答，
//! 在客户端看来逐字节不可区分**。四条协议各按自己的形状说同一句话：
//!
//! | 协议 | 正常收尾 | 上游中途断流 |
//! |---|---|---|
//! | OpenAI | 上游原样透传（网关不解析） | 上游原样透传 |
//! | Anthropic | `content_block_stop` / `message_delta`（带真 usage）/ `message_stop` | `error` 事件后**立刻停** |
//! | Gemini | 终帧带 `finishReason` + `usageMetadata` | 终帧 `finishReason: "OTHER"`，**不报 token 数** |
//! | Responses | `output_text.done` / `content_part.done` / `output_item.done` / `completed` | `response.failed`，**之后绝不再发 `completed`** |
//!
//! # 零 IO 岛在这一层的具体样子
//!
//! 三条协议的流式响应都要一个**本地合成的随机 id**（`msg_…` / `resp_…`），而 core 不许随机。
//! ⇒ **id 由调用方生成并传进来**。这不是绕路：它顺手让「每次流式响应的 id 都各自生成」
//! 这条判据可以在路由层用一个确定性的替身去钉，而本层的判据只管「传进来的 id 有没有
//! 出现在该出现的每一格上」。
//!
//! ⚠️ P1 保留 TS 版「内部规范格式只有纯文本」这条现状：`tools` / `tool_choice` /
//! 多模态块**一律 400 并逐字说明哪一格不支持**。工具调用的双向映射是 P1.5，
//! 而多模态那一半**前提还没核实**（上游 `/chat/completions` 认不认 `image_url` 块，
//! 规格 §8.3 第 2 条明记「没核实」）。**不许因为「看起来该支持」就先写上。**

pub mod anthropic;
pub mod count_tokens;
pub mod gemini;
pub mod openai;
pub mod request_shape;
pub mod responses;
pub mod sse;

use serde::Serialize;

/// 内部规范格式的一条消息。
///
/// ⚠️ **`content` 只有纯文本，这是现状不是终局**：四条协议的多模态内容块一律在转换时
/// 400（见 [`request_shape::RequestShapeError::UnsupportedContent`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    /// `system` / `user` / `assistant`。
    ///
    /// ⚠️ **网关不校验角色取值**：上游认哪些角色是它的事，本仓不复刻它的清单
    /// （校严了就会把上游本来接受的请求挡在门外）。
    pub role: String,
    pub content: String,
}

/// 四条协议 `to_internal_request` 的**公共产物** —— 也就是发给上游 `/chat/completions`
/// 的请求体（01 §6 开头那个结构逐字照搬）。
///
/// # 为什么 `Option` 的那几格要 `skip_serializing_if`
///
/// TS 侧 `JSON.stringify` 会把 `undefined` 的键**整个丢掉**，上游因此收到的是一份
/// 「没有这一格」的请求体。Rust 侧若序列化成 `"temperature": null`，那是一句**不同的话**
/// （「显式设成空」），上游完全可能因此报错。⇒ 逐格 `skip_serializing_if`。
///
/// # `stream` 为什么**不是** `Option`
///
/// 三条协议是 `req.stream === true`（缺席 = false），Gemini 那条根本不看请求体、
/// 由**方法名**决定（`:streamGenerateContent`）。两种来路合成同一个 `bool`，
/// ⚠️ 所以 [`gemini::to_internal_request`] 的 `stream` 是**显式入参**，不从请求体里读。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InternalRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bare() -> InternalRequest {
        InternalRequest {
            model: "agnes-2.0-flash".into(),
            messages: vec![Message {
                role: "user".into(),
                content: "hi".into(),
            }],
            max_tokens: None,
            stream: false,
            temperature: None,
            top_p: None,
            stop: None,
        }
    }

    /// 🔴 **没设的可选参数在上游请求体里必须「不存在」，而不是 `null`。**
    ///
    /// 回归对象：TS 的 `JSON.stringify` 丢掉 `undefined` 键，上游收到的是「没有这一格」。
    /// Rust 默认会写成 `"temperature": null` —— 那是**一句不同的话**（「显式设成空」），
    /// 上游完全可能因此报错，而现象是「换了个网关版本，同一份请求突然 400」。
    #[test]
    fn unset_optionals_vanish_from_the_upstream_body_instead_of_becoming_null() {
        let v = serde_json::to_value(bare()).unwrap();
        let o = v.as_object().unwrap();
        for k in ["max_tokens", "temperature", "top_p", "stop"] {
            assert!(!o.contains_key(k), "{k} 不该出现在上游请求体里");
        }
        // 反向控制：设了的那一格必须真的在。
        let mut req = bare();
        req.temperature = Some(0.5);
        assert_eq!(
            serde_json::to_value(req).unwrap()["temperature"],
            json!(0.5)
        );
    }

    /// `stream` **恒出现**（它是 `bool` 不是 `Option`）：上游要靠它决定回不回 SSE，
    /// 缺席与 `false` 在上游那边不一定是同一件事，不赌。
    #[test]
    fn stream_is_always_present_even_when_false() {
        let v = serde_json::to_value(bare()).unwrap();
        assert_eq!(v["stream"], json!(false));
    }

    /// 上游请求体的字段名是**契约**，逐字钉住（它们就是 OpenAI chat 的那几个名字）。
    #[test]
    fn upstream_field_names_are_the_openai_chat_ones() {
        let mut req = bare();
        req.max_tokens = Some(7);
        req.temperature = Some(0.1);
        req.top_p = Some(0.9);
        req.stop = Some(vec!["\n\n".into()]);
        let v = serde_json::to_value(req).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "max_tokens",
                "messages",
                "model",
                "stop",
                "stream",
                "temperature",
                "top_p"
            ]
        );
        assert_eq!(v["messages"], json!([{ "role": "user", "content": "hi" }]));
    }
}
