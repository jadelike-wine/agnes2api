//! Anthropic `/v1/messages`（`protocol/anthropic.ts`，01 §6.5）。
//!
//! 请求整形 → [`InternalRequest`]；上游 OpenAI 响应 → Anthropic 形状；流式 → 七段事件序列。

use super::request_shape::{
    RequestShapeError, ShapeResult, WHY_NO_TOOL_CALLS, WHY_NOT_IN_UPSTREAM, opt_f64,
    opt_string_array, opt_u32, reject_param, require_array, require_object, require_string,
};
use super::sse::{ByteChunk, UPSTREAM_STREAM_INTERRUPTED, parse_sse_stream, sse_event};
use super::{InternalRequest, Message};
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::{Value, json};

/// 上游 `finish_reason` → Anthropic `stop_reason`。
///
/// ⚠️ 🔴 **`content_filter` 映到 `refusal`，不是 `stop_sequence`**（2026-09-10 实测缺陷）：
/// 从前映成 `stop_sequence`，而本模块两条出口都把配套的 `stop_sequence` 字段写死成 `null`
/// ——**一个说「命中了停止词」、一个说「没有停止词」，自相矛盾**。
/// 后果是上游内容过滤 / 拒答被下游读成「正常因停止词结束」：**不触发重试、不触发告警，
/// 做审计的下游把「被过滤」记成「正常结束」。**
///
/// ⚠️ 而且经本网关「真的命中停止词」这件事**从来就发生不了**——`stop_sequences`
/// 直到本轮才开始往上游转。
#[must_use]
pub fn stop_reason(finish: &str) -> &'static str {
    match finish {
        "length" => "max_tokens",
        "content_filter" => "refusal",
        // `stop` 与一切没见过的值都走这里 —— 与 TS 的 `?? "end_turn"` 逐字同义。
        _ => "end_turn",
    }
}

/// 把一条 `content` 压平成纯文本。
///
/// 字符串直接返回；数组逐块，**`type != "text"` ⇒ [`RequestShapeError::UnsupportedContent`]**。
///
/// ⚠️ 两处对 TS 的偏离，都是「JS 病不必移植、但语义必须保留」那一族：
/// * 块**不是对象**或**没有 `type`** 时，TS 会拿 `undefined` 去比、报文里印出
///   `不支持的内容块类型: undefined`。Rust 侧报 `"unknown"`（与 Gemini 那条的
///   空对象处置同一个词）—— 说的是同一件事，但不把一个 JS 值名字泄给客户端。
/// * `content` 既不是字符串也不是数组时，TS 会在 `for...of` 上抛裸 `TypeError` ⇒ **500**。
///   Rust 侧是一句点名的 400：它是客户端写错了，不是网关坏了。
fn flatten(content: Option<&Value>, what: &str) -> ShapeResult<String> {
    match content {
        // TS 里 `system: null` 会在 for-of 上抛 TypeError ⇒ 500。空文本是它本来的意思。
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for p in parts {
                let kind = p.get("type").and_then(Value::as_str);
                if kind != Some("text") {
                    return Err(RequestShapeError::UnsupportedContent {
                        block_type: kind.unwrap_or("unknown").to_string(),
                    });
                }
                out.push_str(p.get("text").and_then(Value::as_str).unwrap_or(""));
            }
            Ok(out)
        }
        Some(_) => Err(RequestShapeError::Invalid(format!(
            "{what} 必须是一个字符串或内容块数组"
        ))),
    }
}

/// Anthropic 请求体 → [`InternalRequest`]（`anthropic.ts:49-68`）。
///
/// 顺序是有意义的：**形状校验最前**（否则下面的字段访问在请求体不是对象 / 漏写
/// `messages` 时会走进一条本不该存在的路），**三档参数判在压平之前**
/// （纯字段判断比逐块压平便宜，也让报文只说一件事）。
pub fn to_internal_request(req: &Value) -> ShapeResult<InternalRequest> {
    let o = require_object(req, "请求体")?;
    let model = require_string(o.get("model"), "model")?.to_string();
    let raw_messages = require_array(o.get("messages"), "messages")?;

    // 三档参数裁定表（三条协议共用那一张，表在 `request_shape` 里）。
    reject_param(o, "tools", "tools", WHY_NO_TOOL_CALLS)?;
    reject_param(o, "tool_choice", "tool_choice", WHY_NO_TOOL_CALLS)?;
    reject_param(o, "top_k", "top_k", WHY_NOT_IN_UPSTREAM)?;

    let mut messages = Vec::with_capacity(raw_messages.len() + 1);

    // ⚠️ `system` 必须**同时接受字符串与内容块数组**：官方允许两种，而
    // **所有开启 prompt caching 的 SDK 都是用数组形式发的**。原实现把它标成 string
    // 并直接塞进 messages，数组就会以裸数组的形式发给上游（content 不是字符串），
    // 上游只能报错。
    let system = flatten(o.get("system"), "system")?;
    if !system.is_empty() {
        messages.push(Message {
            role: "system".into(),
            content: system,
        });
    }

    for (i, m) in raw_messages.iter().enumerate() {
        let mo = require_object(m, &format!("messages[{i}] "))?;
        messages.push(Message {
            role: require_string(mo.get("role"), &format!("messages[{i}].role"))?.to_string(),
            content: flatten(mo.get("content"), &format!("messages[{i}].content"))?,
        });
    }

    Ok(InternalRequest {
        model,
        messages,
        max_tokens: opt_u32(o, "max_tokens", "max_tokens")?,
        stream: o.get("stream") == Some(&Value::Bool(true)),
        temperature: opt_f64(o, "temperature")?,
        top_p: opt_f64(o, "top_p")?,
        stop: opt_string_array(o, "stop_sequences", "stop_sequences")?,
    })
}

/// 上游 OpenAI 响应 → Anthropic 非流式响应（`anthropic.ts:91-106`）。
///
/// ⚠️ **上游哪一格缺席都不许让这个函数失败**：它跑在响应路径上，
/// 这时候客户端已经等了一次完整的上游往返，把「上游回了个我们没见过的形状」
/// 变成 500 只是把一次可用的降级变成一次彻底失败。缺席一律取零值 / 空串。
#[must_use]
pub fn to_anthropic_response(openai: &Value, model: &str) -> Value {
    let choice = openai.pointer("/choices/0");
    let text = choice
        .and_then(|c| c.pointer("/message/content"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let finish = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(Value::as_str)
        .unwrap_or("stop");
    let id = openai
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    json!({
        "id": format!("msg_{id}"),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [{ "type": "text", "text": text }],
        "stop_reason": stop_reason(finish),
        "stop_sequence": null,
        "usage": {
            "input_tokens": usage_field(openai, "prompt_tokens"),
            "output_tokens": usage_field(openai, "completion_tokens"),
        },
    })
}

/// 取上游 `usage` 里的一格，缺席 / 读不懂一律 0。
///
/// ⚠️ **「读不懂 ⇒ 0」与「上游真给了 0」在这里是同一个结果**，这是有意的：
/// 两者对下游的意思一样（这一格没有可用的计量），而分出第三态要在响应形状里
/// 加一个 Anthropic 协议里不存在的字段。
fn usage_field(openai: &Value, key: &str) -> u64 {
    openai
        .pointer("/usage")
        .and_then(|u| u.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// 上游 SSE 流 → Anthropic 事件序列（`anthropic.ts:108-194`）。
///
/// ```text
/// message_start          ← 传进来的 message_id
/// content_block_start    ← index 0, { type: "text", text: "" }
/// content_block_delta ×N ← 上游 choices[0].delta.content（**非空才发**）
/// content_block_stop     ← index 0
/// message_delta          ← { stop_reason, stop_sequence: null } + **真实的** usage
/// message_stop
/// ```
///
/// 🔴 **头两条必须在读取上游之前产出**，否则客户端要等到上游有数据甚至结束才能看到
/// 第一个字节，这条流就退化成了「攒完再吐」——与本函数存在的意义相悖。
///
/// 🔴 **上游中途断掉必须说出来**（实测缺陷）：从前断流时生成器**静默退出**、
/// 照常补一个 `message_stop` 并声称 `stop_reason: "end_turn"`
/// ⇒ **客户端把「被截断」当成「正常说完」**。现在发一条 `error` 事件后立刻停。
///
/// * `message_id` —— 由调用方合成（core 不许随机）。形如 `msg_<uuid>`；
///   ⚠️ **每个流式响应必须各不相同**，否则下游按 message id 做的日志 / 追踪 / 去重 / 缓存
///   会互相撞车。这条性质由路由层的判据钉住，本层只保证它出现在该出现的每一格上。
pub fn to_anthropic_stream<S>(
    upstream: S,
    model: String,
    message_id: String,
) -> impl Stream<Item = String>
where
    S: Stream<Item = ByteChunk>,
{
    async_stream::stream! {
        let mut finish = "stop".to_string();
        // 上游把 usage 放在**末块**（实测：`{"choices":[{"delta":{}}],"usage":{...}}`，
        // **不传 `stream_options.include_usage` 也照给**）。边读边攒，收尾时如实报出去。
        let mut in_tok: u64 = 0;
        let mut out_tok: u64 = 0;

        yield sse_event(Some("message_start"), &json!({
            "type": "message_start",
            "message": {
                "id": message_id, "type": "message", "role": "assistant", "model": model,
                "content": [], "stop_reason": null, "stop_sequence": null,
                // ⚠️ **这两个 0 是「此刻还不知道」，不是「永远是 0」**：本事件必须在读上游
                // 之前发出，而 usage 要等上游末块才到。真实值在收尾的 `message_delta.usage`
                // 里给（**真 Anthropic 也是在那里给累计 output_tokens 的**）。
                "usage": { "input_tokens": 0, "output_tokens": 0 },
            },
        }));
        yield sse_event(Some("content_block_start"), &json!({
            "type": "content_block_start", "index": 0,
            "content_block": { "type": "text", "text": "" },
        }));

        let payloads = parse_sse_stream(upstream);
        futures_util::pin_mut!(payloads);
        while let Some(item) = payloads.next().await {
            let raw = match item {
                Ok(raw) => raw,
                Err(_) => {
                    // ⚠️ **只报这一句，不回显上游异常**：它可能带上游 URL 与栈帧。
                    yield sse_event(Some("error"), &json!({
                        "type": "error",
                        "error": { "type": "api_error", "message": UPSTREAM_STREAM_INTERRUPTED },
                    }));
                    return;
                }
            };
            // 解析不了的上游块跳过这一行（与 TS 的 `catch { continue }` 逐字同义）。
            let Ok(chunk) = serde_json::from_str::<Value>(&raw) else { continue };
            if let Some(f) = chunk.pointer("/choices/0/finish_reason").and_then(Value::as_str) {
                finish = f.to_string();
            }
            if let Some(n) = chunk.pointer("/usage/prompt_tokens").and_then(Value::as_u64) {
                in_tok = n;
            }
            if let Some(n) = chunk.pointer("/usage/completion_tokens").and_then(Value::as_u64) {
                out_tok = n;
            }
            let text = chunk.pointer("/choices/0/delta/content").and_then(Value::as_str);
            if let Some(text) = text && !text.is_empty() {
                yield sse_event(Some("content_block_delta"), &json!({
                    "type": "content_block_delta", "index": 0,
                    "delta": { "type": "text_delta", "text": text },
                }));
            }
        }

        yield sse_event(Some("content_block_stop"), &json!({
            "type": "content_block_stop", "index": 0,
        }));
        yield sse_event(Some("message_delta"), &json!({
            "type": "message_delta",
            "delta": { "stop_reason": stop_reason(&finish), "stop_sequence": null },
            // 🔴 **真实值，不是硬编码 0**（实测缺陷）：从前这里写死 0，而非流式路径证明
            // 这两个数网关明明拿得到 ⇒ 任何靠流式 usage 做计费 / 配额 / 统计的下游读到的
            // 永远是 0。**0 比缺字段更坏 —— 它长得像一个真值。**
            "usage": { "input_tokens": in_tok, "output_tokens": out_tok },
        }));
        yield sse_event(Some("message_stop"), &json!({ "type": "message_stop" }));
    }
}

#[cfg(test)]
mod tests {
    use super::super::sse::test_support::{chunks_then_error, one_chunk};
    use super::*;

    fn internal(req: Value) -> InternalRequest {
        to_internal_request(&req).expect("应当转换成功")
    }

    async fn frames(s: impl Stream<Item = String>) -> Vec<Value> {
        let wire: String = s.collect::<Vec<_>>().await.join("");
        wire.split("\n\n")
            .filter(|b| !b.trim().is_empty())
            .map(|b| {
                let data = b
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .expect("每一帧都必须有 data 行");
                serde_json::from_str(data).expect("data 必须是合法 JSON")
            })
            .collect()
    }

    // ── 请求整形 ───────────────────────────────────────────────────────────

    /// `system` 提到 `messages` 首位；没有 `system` 时**不插一条空消息**。
    #[test]
    fn system_becomes_the_first_message_and_is_omitted_when_absent() {
        let with = internal(json!({
            "model": "m", "system": "be nice",
            "messages": [{ "role": "user", "content": "hi" }],
        }));
        assert_eq!(with.messages[0].role, "system");
        assert_eq!(with.messages[0].content, "be nice");
        assert_eq!(with.messages.len(), 2);

        let without = internal(json!({
            "model": "m", "messages": [{ "role": "user", "content": "hi" }],
        }));
        assert_eq!(without.messages.len(), 1);
        assert_eq!(without.messages[0].role, "user");
    }

    /// 🔴 **`system` 为内容块数组时压平成字符串，而不是把裸数组发给上游。**
    ///
    /// 回归对象：官方允许两种形态，而**所有开启 prompt caching 的 SDK 都是用数组发的**。
    /// 原实现把它标成 string 直接塞进 messages ⇒ 上游收到一个 `content` 不是字符串的
    /// 消息，只能报错。
    #[test]
    fn a_block_array_system_is_flattened_not_passed_through_raw() {
        let r = internal(json!({
            "model": "m",
            "system": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }],
            "messages": [{ "role": "user", "content": "hi" }],
        }));
        assert_eq!(r.messages[0].content, "ab");
    }

    /// 空的 `system` 数组不插入空消息（压平结果是空串）。
    #[test]
    fn an_empty_system_array_inserts_nothing() {
        let r = internal(json!({
            "model": "m", "system": [],
            "messages": [{ "role": "user", "content": "hi" }],
        }));
        assert_eq!(r.messages.len(), 1);
    }

    /// 🔴 **非文本内容块一律 400 点名，绝不静默丢掉再照常回 200。**
    ///
    /// 回归对象（2026-09-10 实测）：静默过滤时，客户端发了图片却得到一个只看了文字的
    /// 回答，**既无从察觉也无从排查**。`system` 那一侧同样要拦。
    #[test]
    fn non_text_blocks_are_named_in_a_400_not_silently_dropped() {
        let err = to_internal_request(&json!({
            "model": "m",
            "messages": [{ "role": "user", "content": [{ "type": "image", "source": {} }] }],
        }))
        .unwrap_err();
        assert_eq!(
            err,
            RequestShapeError::UnsupportedContent {
                block_type: "image".into()
            }
        );
        assert_eq!(
            err.to_string(),
            "不支持的内容块类型: image（本网关仅支持 text）"
        );

        // system 那一侧同样拦。
        assert!(
            to_internal_request(&json!({
                "model": "m", "system": [{ "type": "image" }],
                "messages": [{ "role": "user", "content": "hi" }],
            }))
            .is_err()
        );
    }

    /// 块里没有 `type`（或块根本不是对象）时报 `unknown`，**不把 JS 的 `undefined` 泄出去**。
    #[test]
    fn a_typeless_block_is_reported_as_unknown() {
        let err = to_internal_request(&json!({
            "model": "m",
            "messages": [{ "role": "user", "content": [{ "text": "x" }] }],
        }))
        .unwrap_err();
        assert_eq!(
            err,
            RequestShapeError::UnsupportedContent {
                block_type: "unknown".into()
            }
        );
    }

    /// ① 档三格真的带到了上游请求体上（`stop_sequences` → `stop`）。
    #[test]
    fn the_three_passthrough_params_really_reach_the_upstream_body() {
        let r = internal(json!({
            "model": "m", "messages": [{ "role": "user", "content": "hi" }],
            "temperature": 0.3, "top_p": 0.8, "stop_sequences": ["</s>"],
            "max_tokens": 64, "stream": true,
        }));
        assert_eq!(r.temperature, Some(0.3));
        assert_eq!(r.top_p, Some(0.8));
        assert_eq!(r.stop.as_deref(), Some(&["</s>".to_string()][..]));
        assert_eq!(r.max_tokens, Some(64));
        assert!(r.stream);
    }

    /// 🔴 **②/③ 档一律 400 点名**，且**判在压平之前**。
    ///
    /// 回归对象：从前 `tools` / `tool_choice` / `top_k` 一个字都不往下带，而同一台网关的
    /// `/v1/chat/completions` 是整包透传 ⇒ 同一份请求，一条回真的工具调用、
    /// 另一条回 200 + 一段 `<tool_call>` 乱码 + `stop_reason: "end_turn"`，
    /// **网关一句提示都没有**。
    #[test]
    fn undeliverable_params_are_named_in_a_400() {
        for (key, field, why) in [
            ("tools", "tools", WHY_NO_TOOL_CALLS),
            ("tool_choice", "tool_choice", WHY_NO_TOOL_CALLS),
            ("top_k", "top_k", WHY_NOT_IN_UPSTREAM),
        ] {
            // 连内容块都是非法的，但报的必须是参数那一句 —— 证明「判在压平之前」。
            let mut body = json!({
                "model": "m",
                "messages": [{ "role": "user", "content": [{ "type": "image" }] }],
            });
            body[key] = json!("whatever");
            assert_eq!(
                to_internal_request(&body).unwrap_err(),
                RequestShapeError::UnsupportedParam {
                    field: field.into(),
                    why: why.into()
                },
                "{key}"
            );
        }
    }

    /// `stream` 只认布尔 `true`。
    #[test]
    fn stream_defaults_to_false_unless_it_is_boolean_true() {
        let base = json!({ "model": "m", "messages": [{ "role": "user", "content": "hi" }] });
        assert!(!internal(base.clone()).stream);
        let mut t = base.clone();
        t["stream"] = json!(true);
        assert!(internal(t).stream);
        let mut s = base;
        s["stream"] = json!("true");
        assert!(!internal(s).stream);
    }

    /// 形状不对一律 400（不是 500）—— 客户端错误必须是 4xx，否则官方 SDK 会重试 2 次，
    /// 把一个永远修不好的请求放大成 3 倍，去烧**整个网关共享**的限流额度。
    #[test]
    fn structurally_broken_bodies_are_400_never_a_panic() {
        for bad in [
            json!(null),
            json!([]),
            json!("hello"),
            json!({ "model": "m" }),
            json!({ "messages": [{ "role": "user", "content": "hi" }] }),
            json!({ "model": "m", "messages": [] }),
            json!({ "model": "m", "messages": ["not an object"] }),
            json!({ "model": "m", "messages": [{ "content": "role 没了" }] }),
            json!({ "model": "m", "messages": [{ "role": "user", "content": 42 }] }),
        ] {
            assert!(to_internal_request(&bad).is_err(), "{bad} 不该被放行");
        }
    }

    // ── 非流式响应 ─────────────────────────────────────────────────────────

    #[test]
    fn openai_response_is_rebuilt_into_anthropic_content_blocks() {
        let v = to_anthropic_response(
            &json!({
                "id": "abc",
                "choices": [{ "message": { "content": "hello" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 11, "completion_tokens": 22 },
            }),
            "agnes-2.0-flash",
        );
        assert_eq!(v["id"], "msg_abc");
        assert_eq!(v["type"], "message");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["model"], "agnes-2.0-flash");
        assert_eq!(v["content"], json!([{ "type": "text", "text": "hello" }]));
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["stop_sequence"], json!(null));
        assert_eq!(
            v["usage"],
            json!({ "input_tokens": 11, "output_tokens": 22 })
        );
    }

    /// 🔴 **`content_filter` → `refusal`，不是 `stop_sequence`。**
    ///
    /// 回归对象：映成 `stop_sequence` 会与恒为 `null` 的 `stop_sequence` 字段**自相矛盾**
    /// （一个说「命中了停止词」、一个说「没有停止词」）。后果是上游内容过滤被下游读成
    /// 「正常结束」：不触发重试、不触发告警，做审计的下游把「被过滤」记成「正常结束」。
    #[test]
    fn content_filter_maps_to_refusal_so_it_never_contradicts_the_null_stop_sequence() {
        let v = to_anthropic_response(
            &json!({ "choices": [{ "finish_reason": "content_filter" }] }),
            "m",
        );
        assert_eq!(v["stop_reason"], "refusal");
        assert_eq!(v["stop_sequence"], json!(null));
    }

    /// `finish_reason` 全表 + 没见过的值走 `end_turn`。
    #[test]
    fn the_stop_reason_table_is_complete_and_has_a_safe_default() {
        assert_eq!(stop_reason("stop"), "end_turn");
        assert_eq!(stop_reason("length"), "max_tokens");
        assert_eq!(stop_reason("content_filter"), "refusal");
        assert_eq!(stop_reason("tool_calls"), "end_turn");
        assert_eq!(stop_reason(""), "end_turn");
    }

    /// 上游什么都没给时给出零值 / 空串，**不失败**：这时客户端已经等过一次完整往返，
    /// 把「上游形状意外」变成 500 只是把一次可用的降级变成一次彻底失败。
    #[test]
    fn a_shapeless_upstream_response_degrades_instead_of_failing() {
        let v = to_anthropic_response(&json!({}), "m");
        assert_eq!(v["id"], "msg_unknown");
        assert_eq!(v["content"][0]["text"], "");
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["usage"], json!({ "input_tokens": 0, "output_tokens": 0 }));
    }

    // ── 流式 ───────────────────────────────────────────────────────────────

    /// 完整且顺序正确的七段事件序列。
    #[tokio::test]
    async fn the_full_event_sequence_comes_out_in_order() {
        let up = one_chunk(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"},\"finish_reason\":\"stop\"}],",
            "\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4}}\n\n",
            "data: [DONE]\n\n",
        ));
        let got = frames(to_anthropic_stream(up, "m".into(), "msg_x".into())).await;
        let types: Vec<&str> = got.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );
        assert_eq!(got[0]["message"]["id"], "msg_x");
        assert_eq!(got[2]["delta"]["text"], "he");
        assert_eq!(got[3]["delta"]["text"], "llo");
    }

    /// 🔴 **收尾的 `message_delta.usage` 是真实值，不是硬编码 0。**
    ///
    /// 回归对象：从前这里写死 0，而非流式路径证明这两个数网关明明拿得到 ⇒
    /// 任何靠流式 usage 做计费 / 配额 / 统计的下游读到的永远是 0。
    /// **0 比缺字段更坏 —— 它长得像一个真值。**
    #[tokio::test]
    async fn the_final_usage_carries_the_real_numbers_from_the_upstream_tail_chunk() {
        let up = one_chunk(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
            // 上游把 usage 放在末块，且 choices[0].delta 是空的。
            "data: {\"choices\":[{\"delta\":{}}],\"usage\":",
            "{\"prompt_tokens\":123,\"completion_tokens\":456}}\n\n",
        ));
        let got = frames(to_anthropic_stream(up, "m".into(), "msg_x".into())).await;
        let last_delta = got.iter().find(|f| f["type"] == "message_delta").unwrap();
        assert_eq!(
            last_delta["usage"],
            json!({ "input_tokens": 123, "output_tokens": 456 })
        );
        // 而开头那个 0 是「此刻还不知道」，它必须仍然是 0。
        assert_eq!(
            got[0]["message"]["usage"],
            json!({ "input_tokens": 0, "output_tokens": 0 })
        );
    }

    /// 🔴 **上游中途断流：发 `error` 事件后立刻停，绝不补一个 `message_stop`。**
    ///
    /// 回归对象：从前断流时生成器静默退出、照常补 `message_stop` 并声称
    /// `stop_reason: "end_turn"` ⇒ **客户端把「被截断」当成「正常说完」**。
    /// 对做内容完整性校验的下游，这是一种会悄悄产出错误结果的失败。
    #[tokio::test]
    async fn a_broken_upstream_emits_an_error_event_and_never_a_clean_ending() {
        let up =
            chunks_then_error(&["data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n\n"]);
        let got = frames(to_anthropic_stream(up, "m".into(), "msg_x".into())).await;
        let types: Vec<&str> = got.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "error"
            ]
        );
        assert!(!types.contains(&"message_stop"), "断流后不许再补正常收尾");
        assert_eq!(got[3]["error"]["type"], "api_error");
        assert_eq!(got[3]["error"]["message"], UPSTREAM_STREAM_INTERRUPTED);
    }

    /// 🔴 **断流那一句不许回显上游的任何文字**（可能带上游 URL 与栈帧）。
    ///
    /// 这一格在类型层面已经不可能失败（`UpstreamStreamError` 是零大小的），
    /// 留着是因为「有人把 `Err(e)` 换成一个带 message 的类型」这件事要先红在这里。
    #[tokio::test]
    async fn the_break_message_is_exactly_one_sentence_with_nothing_from_upstream() {
        let up = chunks_then_error(&[]);
        let wire: String = to_anthropic_stream(up, "m".into(), "msg_x".into())
            .collect::<Vec<_>>()
            .await
            .join("");
        assert!(wire.contains(UPSTREAM_STREAM_INTERRUPTED));
        assert!(!wire.contains("http"), "上游 URL 一个字节都不许出现");
    }

    /// 上游一个增量都没有时，结构仍然完整（客户端拿到的是一条空回答，不是一条坏流）。
    #[tokio::test]
    async fn an_upstream_with_no_deltas_still_produces_a_structurally_complete_stream() {
        let got = frames(to_anthropic_stream(
            one_chunk("data: [DONE]\n\n"),
            "m".into(),
            "msg_x".into(),
        ))
        .await;
        let types: Vec<&str> = got.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            vec![
                "message_start",
                "content_block_start",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
    }

    /// 空的 delta 不发帧 —— 只带 `role` 或只带 `finish_reason` 的块不许变成一条空文本增量。
    #[tokio::test]
    async fn empty_or_missing_deltas_do_not_produce_frames() {
        let up = one_chunk(concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n",
            "data: 这不是 JSON\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"length\"}]}\n\n",
        ));
        let got = frames(to_anthropic_stream(up, "m".into(), "msg_x".into())).await;
        assert!(!got.iter().any(|f| f["type"] == "content_block_delta"));
        // 而 finish_reason 照样被记住了。
        let d = got.iter().find(|f| f["type"] == "message_delta").unwrap();
        assert_eq!(d["delta"]["stop_reason"], "max_tokens");
    }

    /// 🔴 **头两条事件在读上游之前就已产出。**
    ///
    /// 回归对象：把它们挪到循环之后，客户端要等到上游有数据（甚至上游结束）才能看到
    /// 第一个字节 —— 这条流就退化成了「攒完再吐」，与它存在的意义相悖。
    ///
    /// 装置：上游**永不发任何字节也永不结束**。这时能不能拿到前两帧，才是可观测的。
    #[tokio::test(start_paused = true)]
    async fn the_first_two_events_arrive_before_the_upstream_says_anything() {
        let up = futures_util::stream::pending::<ByteChunk>();
        let s = to_anthropic_stream(up, "m".into(), "msg_x".into());
        futures_util::pin_mut!(s);
        for expected in ["message_start", "content_block_start"] {
            let f = tokio::time::timeout(std::time::Duration::from_secs(2), s.next())
                .await
                .expect("上游还没说话，这一帧就该出来了")
                .unwrap();
            assert!(f.contains(expected), "{f}");
        }
    }
}
