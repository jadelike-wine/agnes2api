//! OpenAI —— **母协议**，也是四条里最薄的一条（`protocol/openai.ts`，01 §6.4）。
//!
//! # 这条协议的请求体与响应体网关**都不解析**
//!
//! 路由不传 `expect_json` ⇒ `dispatch` 走 `sanitize(res)` **原样搬运**，从头到尾没有解析过。
//! usage 确实在响应里、确实到得了客户端（实测过），**只是网关没读它**。
//! 🔴 **别在热路径上补一次 `res.clone().json()`** —— 那是给每个请求加一次全量解析。
//! 这个缺口由 `GET /admin/api/capabilities` 的 `stats.tokensCoverage` 如实告诉面板。
//!
//! ⚠️ 但「不解析」不等于「不校验」：见 [`check_chat_shape`]。

use super::request_shape::{ShapeResult, require_array, require_object, require_string};
use serde_json::{Value, json};

/// 本网关支持的模型全集。**写死的清单，不是运行时从上游拉回来的。**
///
/// ⚠️ **顺序不是随手排的**：`MODEL_CATALOG[0]` 是验活 handler 与集成示例卡的默认模型，
/// 所以**对话模型排在最前**，媒体模型排在后面；同形态内部保持既有条目在前、新增条目在后，
/// 免得默认值随着补目录漂走。
///
/// ⚠️ **写死是刻意的**：面板的模型表与 Playground 靠它做静态渲染，改成动态拉取要先回答
/// 「上游不可达时画什么、缓不缓存」两个问题。上游此刻真有哪些模型是另一个问题，
/// 走 `core::admin::upstream_models` 的差集呈现。
pub const MODELS: [ModelEntry; 12] = [
    // 对话
    ModelEntry::new("agnes-2.0-flash", Modality::Chat),
    ModelEntry::new("agnes-2.5-flash", Modality::Chat),
    ModelEntry::new("agnes-2.5-pro", Modality::Chat),
    ModelEntry::new("agnes-2.5-pro-alpha", Modality::Chat),
    ModelEntry::new("agnes-2.5-pro-beta", Modality::Chat),
    ModelEntry::new("agnes-3.0-flash", Modality::Chat),
    // 图片
    ModelEntry::new("agnes-image-2.1-flash", Modality::Image),
    ModelEntry::new("agnes-image-2.0-flash", Modality::Image),
    ModelEntry::new("agnes-image-2.5-flash", Modality::Image),
    // 视频
    ModelEntry::new("agnes-video-v2.0", Modality::Video),
    ModelEntry::new("agnes-video-2.5", Modality::Video),
    ModelEntry::new("agnes-video-2.5-flash", Modality::Video),
];

/// 一个模型能干什么。
///
/// 🔴 它不是装饰：Gemini 的 `supportedGenerationMethods` 直接由它决定，
/// 而那一格是**机器可读**的「这个模型能干什么」——写错了，照它渲染下拉框的客户端
/// 会把视频模型列成可对话模型，运维选中发一次对话 ⇒
/// **白烧一把 key + 一次全网关共享的限流额度**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    /// 四条对话协议都能用。
    Chat,
    /// 只能走 `POST /v1/images/generations`。
    Image,
    /// 只能走 `POST /v1/videos` + `GET /v1/videos/{id}` 两段式。
    Video,
}

/// 模型清单里的一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelEntry {
    pub id: &'static str,
    pub modality: Modality,
}

impl ModelEntry {
    const fn new(id: &'static str, modality: Modality) -> Self {
        Self { id, modality }
    }
}

/// `GET /v1/models` 的响应体。
///
/// `created` 是**调用方传进来的** unix 秒 —— core 不读时钟。
#[must_use]
pub fn model_list_response(created: i64) -> Value {
    json!({
        "object": "list",
        "data": MODELS.iter().map(|m| json!({
            "id": m.id,
            "object": "model",
            "created": created,
            "owned_by": "agnes2api",
        })).collect::<Vec<_>>(),
    })
}

/// 🔴 `/v1/chat/completions` 的**本地形状校验**（2026-09-10 实测缺陷）。
///
/// 这条路由是原样透传（不做结构转换），于是从前**一次本地校验都没有**：
/// `{"foo":1}` 会被原封不动送去上游，由上游回 400。证据是那次响应体里带着上游的
/// `AgnesAI_error` 与上游 request id，延迟也对得上（本地拦截稳定 1.5~2ms，走上游是 25ms 量级）。
///
/// **代价不是「多一次往返」那么轻**：上游那层 CF 约 2 次快请求就回 1015，
/// 而限流额度是**整个网关共享**的 ⇒ 一个客户端 bug 循环重试畸形请求，
/// 几秒内就能把所有人的上游通道打死。
///
/// ⚠️ **只校验这三样，不做更严的形状检查**：上游接受哪些字段（`tools` / `response_format` / …）
/// 是它的事，本仓不复刻；校严了就会把上游本来接受的请求挡在门外。
/// 🔴 **特别是：这条协议不走三档参数裁定表。** 它是整包透传，`tools` 在这里**真的能用**
/// （实测上游会回真的 `tool_calls`）—— 那张表管的是另外三条「要把响应转回去」的协议。
pub fn check_chat_shape(req: &Value) -> ShapeResult<()> {
    let o = require_object(req, "请求体")?;
    require_string(o.get("model"), "model")?;
    require_array(o.get("messages"), "messages")?;
    Ok(())
}

/// 请求体里的 `stream` 是不是 `true`。缺席 / 非布尔 ⇒ `false`。
///
/// ⚠️ **这一格刻意不校验类型**（与 [`super::request_shape::opt_f64`] 那几个相反）：
/// 它决定的是**网关自己**走哪条路（首字节档 vs 同步档），不是转给上游的一句话。
/// `stream: "yes"` 的请求照常按非流式转发上去，由上游对那个字段表态 —— 与本条协议
/// 「整包透传、不替上游判」的立身之本一致。
#[must_use]
pub fn wants_stream(req: &Value) -> bool {
    req.get("stream") == Some(&Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 🔴 **12 条模型的 id 与顺序逐字钉住**，期望值手写字面量。
    ///
    /// 不写成 `MODELS.iter().map(...)` 那样从被测对象现算 —— 那是同义反复
    /// （本仓登记的第 ⑥ 种假阳性）。
    ///
    /// 回归对象：① `MODELS[0]` 是验活 handler 与集成示例卡的默认模型，
    /// 补目录时插到最前面会让默认值悄悄漂走；② 这份清单与
    /// `core::admin::protocol_catalog` 的 `MODEL_CATALOG` 必须逐条同序一致。
    #[test]
    fn the_twelve_model_ids_and_their_order_are_pinned_verbatim() {
        let ids: Vec<&str> = MODELS.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            vec![
                "agnes-2.0-flash",
                "agnes-2.5-flash",
                "agnes-2.5-pro",
                "agnes-2.5-pro-alpha",
                "agnes-2.5-pro-beta",
                "agnes-3.0-flash",
                "agnes-image-2.1-flash",
                "agnes-image-2.0-flash",
                "agnes-image-2.5-flash",
                "agnes-video-v2.0",
                "agnes-video-2.5",
                "agnes-video-2.5-flash",
            ]
        );
    }

    /// 形态分档同样手写钉住：6 对话 / 3 图片 / 3 视频，且**对话的全在最前**。
    #[test]
    fn chat_models_come_first_and_the_split_is_six_three_three() {
        let m: Vec<Modality> = MODELS.iter().map(|m| m.modality).collect();
        assert_eq!(&m[..6], &[Modality::Chat; 6]);
        assert_eq!(&m[6..9], &[Modality::Image; 3]);
        assert_eq!(&m[9..], &[Modality::Video; 3]);
    }

    /// 模型 id 不许重复 —— 重复的那一条在面板的模型表里会画两遍，
    /// 而 `models/:id/test` 只会打中第一条。
    #[test]
    fn model_ids_are_unique() {
        let mut ids: Vec<&str> = MODELS.iter().map(|m| m.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    /// `/v1/models` 的响应形状与 `created` 的来源。
    #[test]
    fn model_list_uses_the_injected_clock_and_the_openai_envelope() {
        let v = model_list_response(1_700_000_000);
        assert_eq!(v["object"], "list");
        assert_eq!(v["data"].as_array().unwrap().len(), 12);
        assert_eq!(v["data"][0]["id"], "agnes-2.0-flash");
        assert_eq!(v["data"][0]["object"], "model");
        assert_eq!(v["data"][0]["owned_by"], "agnes2api");
        // core 零 IO：`created` 只可能是传进来的那个值。
        for e in v["data"].as_array().unwrap() {
            assert_eq!(e["created"], 1_700_000_000_i64);
        }
    }

    /// 🔴 三样本地校验：畸形请求不许烧掉**整个网关共享**的限流额度。
    #[test]
    fn malformed_chat_requests_are_stopped_locally() {
        use serde_json::json;
        for bad in [
            json!({ "foo": 1 }),                         // 两样都缺
            json!({ "model": "x" }),                     // 漏写 messages
            json!({ "messages": [{ "role": "user" }] }), // 漏写 model
            json!({ "model": "", "messages": [1] }),     // model 空串
            json!({ "model": "x", "messages": [] }),     // 空数组
            json!([]),                                   // 合法 JSON、非对象
            json!("hello"),
            json!(null),
        ] {
            assert!(check_chat_shape(&bad).is_err(), "{bad} 不该被放行");
        }
    }

    /// 🔴 **反向控制：`tools` 在这条协议上必须放行。**
    ///
    /// 它是整包透传的那一条，实测上游会回**真的** `tool_calls`。
    /// 把三档参数裁定表套到这里来 = 砍掉一个今天真的能用的能力。
    #[test]
    fn tools_are_allowed_here_because_this_route_really_can_serve_them() {
        use serde_json::json;
        let ok = json!({
            "model": "agnes-2.0-flash",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{ "type": "function" }],
            "tool_choice": "required",
            "response_format": { "type": "json_object" },
            "top_k": 40,
        });
        assert!(check_chat_shape(&ok).is_ok());
    }

    /// `stream` 只认布尔 `true`；其余一律当非流式，**且不报错**。
    #[test]
    fn stream_flag_is_true_only_for_a_real_boolean_true() {
        use serde_json::json;
        assert!(wants_stream(&json!({ "stream": true })));
        for v in [
            json!({}),
            json!({ "stream": false }),
            json!({ "stream": "true" }),
            json!({ "stream": 1 }),
            json!({ "stream": null }),
        ] {
            assert!(!wants_stream(&v), "{v} 不该被当成流式");
        }
    }
}
