//! OpenAI Responses `/v1/responses`（`protocol/responses.ts`，01 §6.7）。
//!
//! 这一条的流式事件序列是四条里**最不能糊弄**的一条：官方 SDK 的累积器会**直接读**
//! 若干属性去重建它自己的事件，少一条事件或少一个字段都不是「少点信息」，
//! 而是 SDK 在**它自己的文件里**抛异常 —— 用户第一反应是「我 SDK 装坏了」。

use super::request_shape::{
    RequestShapeError, ShapeResult, WHY_NO_TOOL_CALLS, opt_f64, opt_u32, reject_param,
    require_array, require_object, require_string,
};
use super::sse::{ByteChunk, UPSTREAM_STREAM_INTERRUPTED, parse_sse_stream, sse_event};
use super::{InternalRequest, Message};
use futures_core::Stream;
use futures_util::StreamExt;
use serde_json::{Value, json};

/// Responses 的 `content` 数组里**只有这两种块带正文**：`input_text`（用户侧）与
/// `output_text`（把上一轮回答原样喂回去时用的那种）。
///
/// 其余（`input_image` / `input_file` / `refusal` / …）本网关都转不成内部纯文本。
const TEXT_PARTS: [&str; 2] = ["input_text", "output_text"];

/// 把一条 `content` 压平成纯文本。
///
/// 🔴 **非文本块一律 400，不再求值成空串**（2026-09-10 实测缺陷）：从前是
/// `c.map(p => p.text ?? "").join("")`，`input_image` / `input_file` **直接蒸发**，
/// 网关照回 200，客户端拿到的是一段模型在**完全没看到图**的前提下编出来的答案。
fn flatten(content: Option<&Value>, what: &str) -> ShapeResult<String> {
    match content {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Array(parts)) => {
            let mut out = String::new();
            for p in parts {
                let kind = p.get("type").and_then(Value::as_str);
                match kind {
                    Some(k) if TEXT_PARTS.contains(&k) => {
                        out.push_str(p.get("text").and_then(Value::as_str).unwrap_or(""));
                    }
                    other => {
                        return Err(RequestShapeError::UnsupportedContent {
                            block_type: other.unwrap_or("unknown").to_string(),
                        });
                    }
                }
            }
            Ok(out)
        }
        Some(_) => Err(RequestShapeError::Invalid(format!(
            "{what} 必须是一个字符串或内容块数组"
        ))),
    }
}

/// Responses 请求体 → [`InternalRequest`]（`responses.ts:47-67`）。
///
/// ⚠️ **`input` 两种合法形态都要放行**（字符串 / 非空数组），所以**只在它不是字符串时**
/// 才要求是非空数组 —— 直接 `require_array` 会把官方 SDK 最常见的那种用法挡在门外。
pub fn to_internal_request(req: &Value) -> ShapeResult<InternalRequest> {
    let o = require_object(req, "请求体")?;
    let model = require_string(o.get("model"), "model")?.to_string();
    let input = o.get("input");
    let input_is_string = matches!(input, Some(Value::String(_)));
    if !input_is_string {
        require_array(input, "input")?;
    }

    // 三档参数裁定表（三条协议共用那一张）。
    reject_param(o, "tools", "tools", WHY_NO_TOOL_CALLS)?;
    reject_param(o, "tool_choice", "tool_choice", WHY_NO_TOOL_CALLS)?;

    let mut messages = Vec::new();
    // ⚠️ `instructions` 是**字符串**格（不是内容块数组），与 Anthropic 的 `system` 不同。
    // 空串不插入一条空消息（与 TS 的 `if (req.instructions)` 逐字同义）。
    if let Some(s) = o.get("instructions").and_then(Value::as_str)
        && !s.is_empty()
    {
        messages.push(Message {
            role: "system".into(),
            content: s.to_string(),
        });
    }

    match input {
        Some(Value::String(s)) => messages.push(Message {
            role: "user".into(),
            content: s.clone(),
        }),
        _ => {
            for (i, m) in require_array(input, "input")?.iter().enumerate() {
                let mo = require_object(m, &format!("input[{i}] "))?;
                messages.push(Message {
                    role: require_string(mo.get("role"), &format!("input[{i}].role"))?.to_string(),
                    content: flatten(mo.get("content"), &format!("input[{i}].content"))?,
                });
            }
        }
    }

    Ok(InternalRequest {
        model,
        messages,
        max_tokens: opt_u32(o, "max_output_tokens", "max_output_tokens")?,
        stream: o.get("stream") == Some(&Value::Bool(true)),
        temperature: opt_f64(o, "temperature")?,
        top_p: opt_f64(o, "top_p")?,
        // ⚠️ Responses 这条协议**没有 `stop` 那一格**（三档表里它只给了 Anthropic 与 Gemini）。
        stop: None,
    })
}

/// 上游 OpenAI 响应 → Responses 非流式响应（`responses.ts:69-87`）。
#[must_use]
pub fn to_responses_response(openai: &Value, model: &str) -> Value {
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
    let input = usage_field(openai, "prompt_tokens");
    let output = usage_field(openai, "completion_tokens");
    json!({
        "id": format!("resp_{id}"),
        "object": "response",
        "model": model,
        // 🔴 `length` ⇒ `incomplete`：这是非流式路径上「被截断」的唯一信号。
        "status": if finish == "length" { "incomplete" } else { "completed" },
        "output": [{
            "type": "message",
            "id": format!("msg_{id}"),
            "role": "assistant",
            "status": "completed",
            "content": [{ "type": "output_text", "text": text, "annotations": [] }],
        }],
        "usage": {
            "input_tokens": input,
            "output_tokens": output,
            "total_tokens": input + output,
        },
    })
}

fn usage_field(openai: &Value, key: &str) -> u64 {
    openai
        .pointer("/usage")
        .and_then(|u| u.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn part(text: &str) -> Value {
    json!({ "type": "output_text", "text": text, "annotations": [] })
}

fn item(item_id: &str, status: &str, content: Value) -> Value {
    json!({
        "type": "message", "id": item_id, "status": status,
        "role": "assistant", "content": content,
    })
}

/// 上游 SSE 流 → Responses 事件序列（`responses.ts:89-216`）。
///
/// 🔴 **官方 SDK 能用的最小事件序列，一条都不许少**：
///
/// ```text
/// response.created            seq=0
/// response.output_item.added  seq=1   ← SDK 靠它建出 snapshot.output[0]
/// response.content_part.added seq=2   ← SDK 靠它建出 output.content[0]
/// response.output_text.delta  ×N
/// response.output_text.done
/// response.content_part.done
/// response.output_item.done
/// response.completed
/// ```
///
/// **缺陷定位**（2026-09-10 本地回放定案，零上游请求）：从前只发三类事件
/// （`created` / 若干 `delta` / `completed`），delta 上没有 `item_id`，
/// `completed.response` 里**没有 `output`**。把线上真吐的那串字节原样喂给官方 `openai` 3.11.0：
///
/// * `client.responses.stream(...)` ⇒ **`IndexError: list index out of range`**，抛在
///   `openai/lib/streaming/responses/_responses.py` 的
///   `output = snapshot.output[event.output_index]` 那一行，**栈里全是 openai 包的文件**
///   ——用户第一反应是「我 SDK 装坏了」，排查方向整个被带偏；
/// * 对照组 `client.responses.create(stream=True)` 裸迭代**是通的**。
///
/// 而 `stream()` 正是 OpenAI 文档里 Responses 流式的推荐写法，也是 Agents SDK 一类框架的底座。
///
/// ⚠️ **`sequence_number` / `item_id` / `logprobs` 三格不是装饰**：SDK 的 `handle_event()`
/// 在 delta 与 done 两档上**直接读这三个属性**，缺一个就是一次 `AttributeError`（实测过）。
///
/// ⚠️ **`response_id` 与 `item_id` 是两个不同的 id**：官方事件里 `item_id` 指的是**这一项**，
/// 下游按它把增量归到某一项上。两者都由调用方生成（core 不许随机）。
///
/// ⚠️⚠️ **`completed` 里仍然不带 `usage`，这是一条被裁定过的取舍、不是漏的。**
/// 官方 `Response.usage` 本来就是可选格，缺它不会让 SDK 崩。
/// **要动它得先动那一段裁定，别在这里顺手加。**
pub fn to_responses_stream<S>(
    upstream: S,
    model: String,
    response_id: String,
    item_id: String,
) -> impl Stream<Item = String>
where
    S: Stream<Item = ByteChunk>,
{
    async_stream::stream! {
        // 官方事件带的是一个**全流单调递增**的序号。本地自己数，不依赖上游。
        let mut seq: u64 = 0;
        let mut text = String::new();

        // 前三条必须在读取上游之前产出，否则这条流退化成「攒完再吐」。
        // 它们也都不依赖上游的任何内容。
        let n = seq; seq += 1;
        yield sse_event(Some("response.created"), &json!({
            "type": "response.created", "sequence_number": n,
            "response": {
                "id": response_id, "object": "response", "model": model,
                "status": "in_progress", "output": [],
            },
        }));
        let n = seq; seq += 1;
        yield sse_event(Some("response.output_item.added"), &json!({
            "type": "response.output_item.added", "sequence_number": n,
            "output_index": 0, "item": item(&item_id, "in_progress", json!([])),
        }));
        let n = seq; seq += 1;
        yield sse_event(Some("response.content_part.added"), &json!({
            "type": "response.content_part.added", "sequence_number": n,
            "item_id": item_id, "output_index": 0, "content_index": 0, "part": part(""),
        }));

        let payloads = parse_sse_stream(upstream);
        futures_util::pin_mut!(payloads);
        while let Some(step) = payloads.next().await {
            let raw = match step {
                Ok(raw) => raw,
                Err(_) => {
                    // 🔴 **`response.failed` 之后绝不再发 `completed`**：官方 SDK 的
                    // `get_final_response()` 会明确抛「没收到 completed」——
                    // **一次响亮的失败，好过一份看起来完整的残缺回答。**
                    //
                    // ⚠️ 只报这一句，不回显上游异常（可能带上游 URL 与栈帧）。
                    let n = seq;
                    yield sse_event(Some("response.failed"), &json!({
                        "type": "response.failed", "sequence_number": n,
                        "response": {
                            "id": response_id, "object": "response", "model": model,
                            "status": "failed",
                            "output": [item(&item_id, "incomplete", json!([part(&text)]))],
                            "error": {
                                "code": "upstream_error",
                                "message": UPSTREAM_STREAM_INTERRUPTED,
                            },
                        },
                    }));
                    return;
                }
            };
            let Ok(chunk) = serde_json::from_str::<Value>(&raw) else { continue };
            let delta = chunk.pointer("/choices/0/delta/content").and_then(Value::as_str);
            if let Some(delta) = delta && !delta.is_empty() {
                text.push_str(delta);
                let n = seq; seq += 1;
                yield sse_event(Some("response.output_text.delta"), &json!({
                    "type": "response.output_text.delta", "sequence_number": n,
                    "item_id": item_id, "output_index": 0, "content_index": 0,
                    "delta": delta, "logprobs": [],
                }));
            }
        }

        let n = seq; seq += 1;
        yield sse_event(Some("response.output_text.done"), &json!({
            "type": "response.output_text.done", "sequence_number": n,
            "item_id": item_id, "output_index": 0, "content_index": 0,
            "text": text, "logprobs": [],
        }));
        let n = seq; seq += 1;
        yield sse_event(Some("response.content_part.done"), &json!({
            "type": "response.content_part.done", "sequence_number": n,
            "item_id": item_id, "output_index": 0, "content_index": 0, "part": part(&text),
        }));
        let n = seq; seq += 1;
        yield sse_event(Some("response.output_item.done"), &json!({
            "type": "response.output_item.done", "sequence_number": n,
            "output_index": 0, "item": item(&item_id, "completed", json!([part(&text)])),
        }));
        let n = seq;
        yield sse_event(Some("response.completed"), &json!({
            "type": "response.completed", "sequence_number": n,
            "response": {
                "id": response_id, "object": "response", "model": model, "status": "completed",
                // 「最终对象」就是 `get_final_response()` 的返回值本身，缺了它下游拿到的是空。
                "output": [item(&item_id, "completed", json!([part(&text)]))],
            },
        }));
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

    fn stream_of(up: &str) -> impl Stream<Item = String> {
        to_responses_stream(one_chunk(up), "m".into(), "resp_x".into(), "msg_y".into())
    }

    // ── 请求整形 ───────────────────────────────────────────────────────────

    /// ⚠️ **`input` 的两种合法形态都要放行。**
    ///
    /// 回归对象：直接 `require_array` 会把官方 SDK 最常见的
    /// `client.responses.create(input="hi")` 挡在门外。
    #[test]
    fn both_input_shapes_are_accepted() {
        let s = internal(json!({ "model": "m", "input": "hi" }));
        assert_eq!(
            s.messages,
            vec![Message {
                role: "user".into(),
                content: "hi".into()
            }]
        );

        let a = internal(json!({
            "model": "m",
            "input": [{ "role": "user", "content": [
                { "type": "input_text", "text": "a" },
                { "type": "input_text", "text": "b" },
            ]}],
        }));
        assert_eq!(a.messages[0].content, "ab");
    }

    /// `output_text` 也带正文（把上一轮回答喂回去时用的那种）。
    #[test]
    fn output_text_blocks_carry_content_too() {
        let r = internal(json!({
            "model": "m",
            "input": [
                { "role": "assistant", "content": [{ "type": "output_text", "text": "prev" }] },
                { "role": "user", "content": "next" },
            ],
        }));
        assert_eq!(r.messages[0].content, "prev");
        assert_eq!(r.messages[1].content, "next");
    }

    /// `instructions` 转为首条 system 消息；没有就不插。
    #[test]
    fn instructions_become_the_first_system_message_when_present() {
        let with = internal(json!({ "model": "m", "input": "hi", "instructions": "be nice" }));
        assert_eq!(with.messages[0].role, "system");
        assert_eq!(with.messages.len(), 2);

        for without in [
            json!({ "model": "m", "input": "hi" }),
            json!({ "model": "m", "input": "hi", "instructions": "" }),
        ] {
            assert_eq!(internal(without).messages.len(), 1);
        }
    }

    /// `max_output_tokens` → `max_tokens`；`stream` 只认布尔 `true`；
    /// ⚠️ 这条协议**没有 `stop` 那一格**。
    #[test]
    fn field_mapping_and_the_absent_stop_slot() {
        let r = internal(json!({
            "model": "m", "input": "hi", "max_output_tokens": 32,
            "stream": true, "temperature": 0.4, "top_p": 0.6,
        }));
        assert_eq!(r.max_tokens, Some(32));
        assert!(r.stream);
        assert_eq!(r.temperature, Some(0.4));
        assert_eq!(r.top_p, Some(0.6));
        assert_eq!(r.stop, None);
    }

    /// 🔴 **非文本块一律 400 点名，绝不静默丢掉一张图再照常回 200。**
    ///
    /// 回归对象（2026-09-10 实测）：`input_image` / `input_file` 直接蒸发，
    /// 网关照回 200，客户端拿到的是一段模型在**完全没看到图**的前提下编出来的答案。
    #[test]
    fn non_text_blocks_are_named_in_a_400_instead_of_evaporating() {
        for (kind, expected) in [
            ("input_image", "input_image"),
            ("input_file", "input_file"),
            ("refusal", "refusal"),
        ] {
            let body = json!({
                "model": "m",
                "input": [{ "role": "user", "content": [{ "type": kind }] }],
            });
            assert_eq!(
                to_internal_request(&body).unwrap_err(),
                RequestShapeError::UnsupportedContent {
                    block_type: expected.into()
                }
            );
        }
    }

    /// 🔴 ③ 档：`tools` / `tool_choice` 一律 400 点名。
    #[test]
    fn tool_params_are_named_in_a_400() {
        for key in ["tools", "tool_choice"] {
            let mut body = json!({ "model": "m", "input": "hi" });
            body[key] = json!("whatever");
            assert_eq!(
                to_internal_request(&body).unwrap_err(),
                RequestShapeError::UnsupportedParam {
                    field: key.into(),
                    why: WHY_NO_TOOL_CALLS.into()
                },
                "{key}"
            );
        }
    }

    /// 形状不对一律 400，不 panic。
    #[test]
    fn structurally_broken_bodies_are_400_never_a_panic() {
        for bad in [
            json!(null),
            json!([]),
            json!({ "input": "hi" }),
            json!({ "model": "m" }),
            json!({ "model": "m", "input": [] }),
            json!({ "model": "m", "input": 7 }),
            json!({ "model": "m", "input": [{ "content": "role 没了" }] }),
        ] {
            assert!(to_internal_request(&bad).is_err(), "{bad}");
        }
    }

    // ── 非流式响应 ─────────────────────────────────────────────────────────

    #[test]
    fn openai_response_is_rebuilt_into_the_output_array_shape() {
        let v = to_responses_response(
            &json!({
                "id": "abc",
                "choices": [{ "message": { "content": "hello" }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 2, "completion_tokens": 3 },
            }),
            "agnes-2.0-flash",
        );
        assert_eq!(v["id"], "resp_abc");
        assert_eq!(v["object"], "response");
        assert_eq!(v["status"], "completed");
        assert_eq!(v["output"][0]["id"], "msg_abc");
        assert_eq!(v["output"][0]["type"], "message");
        assert_eq!(v["output"][0]["content"][0]["type"], "output_text");
        assert_eq!(v["output"][0]["content"][0]["text"], "hello");
        assert_eq!(v["output"][0]["content"][0]["annotations"], json!([]));
        assert_eq!(
            v["usage"],
            json!({ "input_tokens": 2, "output_tokens": 3, "total_tokens": 5 })
        );
    }

    /// 🔴 `length` ⇒ `incomplete` —— 非流式路径上「被截断」的唯一信号。
    #[test]
    fn a_length_finish_makes_the_response_incomplete() {
        let v = to_responses_response(&json!({ "choices": [{ "finish_reason": "length" }] }), "m");
        assert_eq!(v["status"], "incomplete");
        let v = to_responses_response(&json!({ "choices": [{ "finish_reason": "stop" }] }), "m");
        assert_eq!(v["status"], "completed");
    }

    // ── 流式 ───────────────────────────────────────────────────────────────

    /// 🔴 **官方最小事件序列：两条 added 打头、三条 done 收尾。**
    ///
    /// 回归对象（本地回放定案，零上游请求）：少了 `output_item.added` /
    /// `content_part.added`，官方 `openai` 3.11.0 的 `responses.stream(...)` 抛
    /// **`IndexError: list index out of range`**，栈里**全是 openai 包的文件** ——
    /// 用户第一反应是「我 SDK 装坏了」，排查方向整个被带偏。
    #[tokio::test]
    async fn the_official_minimal_event_sequence_is_emitted_in_full() {
        let got = frames(stream_of(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
            "data: [DONE]\n\n",
        )))
        .await;
        let types: Vec<&str> = got.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            vec![
                "response.created",
                "response.output_item.added",
                "response.content_part.added",
                "response.output_text.delta",
                "response.output_text.delta",
                "response.output_text.done",
                "response.content_part.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
    }

    /// 🔴 **`sequence_number` 全流单调递增、从 0 开始，本地自己数。**
    #[tokio::test]
    async fn sequence_numbers_start_at_zero_and_increase_by_one_across_the_whole_stream() {
        let got = frames(stream_of(
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
        ))
        .await;
        let seqs: Vec<u64> = got
            .iter()
            .map(|f| f["sequence_number"].as_u64().expect("每条都必须带"))
            .collect();
        assert_eq!(seqs, (0..got.len() as u64).collect::<Vec<_>>());
    }

    /// 🔴 **delta 与 done 两档必须带齐 SDK 会**直接读**的三个属性。**
    ///
    /// 回归对象：SDK 的 `handle_event()` 直接读 `item_id` / `sequence_number` / `logprobs`
    /// 去重建它自己的事件，**缺一个就是一次 `AttributeError`**（同一次回放里实测过）。
    #[tokio::test]
    async fn delta_and_done_events_carry_the_three_attributes_the_sdk_reads_directly() {
        let got = frames(stream_of(
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\n\n",
        ))
        .await;
        for f in got.iter().filter(|f| {
            f["type"] == "response.output_text.delta" || f["type"] == "response.output_text.done"
        }) {
            assert_eq!(f["item_id"], "msg_y", "{}", f["type"]);
            assert!(f["sequence_number"].is_u64(), "{}", f["type"]);
            assert_eq!(f["logprobs"], json!([]), "{}", f["type"]);
            assert_eq!(f["output_index"], 0);
            assert_eq!(f["content_index"], 0);
        }
    }

    /// ⚠️ **`response_id` 与 `item_id` 是两个不同的 id**：官方事件里 `item_id` 指的是
    /// **这一项**，下游按它把增量归到某一项上。混用会让下游把两件事当成一件。
    #[tokio::test]
    async fn the_response_id_and_the_item_id_are_two_different_identifiers() {
        let got = frames(stream_of("data: [DONE]\n\n")).await;
        assert_eq!(got[0]["response"]["id"], "resp_x");
        assert_eq!(got[1]["item"]["id"], "msg_y");
        assert_eq!(got[2]["item_id"], "msg_y");
    }

    /// 🔴 **`response.completed.response` 带着完整的 `output`** —— 那就是
    /// `get_final_response()` 返回的那个对象，缺了它下游拿到的是空。
    #[tokio::test]
    async fn the_completed_event_carries_the_final_object_with_its_output() {
        let got = frames(stream_of(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
        )))
        .await;
        let done = got.last().unwrap();
        assert_eq!(done["type"], "response.completed");
        assert_eq!(done["response"]["status"], "completed");
        assert_eq!(done["response"]["output"][0]["status"], "completed");
        assert_eq!(done["response"]["output"][0]["content"][0]["text"], "hello");
    }

    /// ⚠️⚠️ **`completed` 里一个 `usage` 字段都没有 —— 这是裁定过的取舍，不是漏的。**
    ///
    /// 官方 `Response.usage` 本来就是可选格，缺它不会让 SDK 崩。
    /// **要动它得先动那一段裁定，别在这里顺手加。**
    #[tokio::test]
    async fn not_a_single_usage_field_appears_anywhere_in_this_stream() {
        let wire: String = stream_of(concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}],",
            "\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n",
        ))
        .collect::<Vec<_>>()
        .await
        .join("");
        assert!(
            !wire.contains("usage"),
            "这条流刻意不报 usage，加之前先改那段裁定"
        );
    }

    /// 🔴 **断流：发 `response.failed` 且之后绝不再发 `completed`。**
    ///
    /// 回归对象：从前静默退出、照常补一个 `completed` 并声称 `status: "completed"`
    /// ⇒ **客户端把「被截断」当成「正常说完」**。现在 `get_final_response()` 会明确抛
    /// 「没收到 completed」——**一次响亮的失败，好过一份看起来完整的残缺回答。**
    #[tokio::test]
    async fn a_broken_upstream_fails_loudly_and_never_completes() {
        let up =
            chunks_then_error(&["data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n\n"]);
        let got = frames(to_responses_stream(
            up,
            "m".into(),
            "resp_x".into(),
            "msg_y".into(),
        ))
        .await;
        let types: Vec<&str> = got.iter().map(|f| f["type"].as_str().unwrap()).collect();
        assert_eq!(*types.last().unwrap(), "response.failed");
        assert!(
            !types.contains(&"response.completed"),
            "failed 之后绝不许再发 completed"
        );
        let failed = got.last().unwrap();
        assert_eq!(failed["response"]["status"], "failed");
        assert_eq!(failed["response"]["error"]["code"], "upstream_error");
        assert_eq!(
            failed["response"]["error"]["message"],
            UPSTREAM_STREAM_INTERRUPTED
        );
        // 已攒到的文本照常交出去，但那一项的状态是 `incomplete` —— 说的是实话。
        assert_eq!(failed["response"]["output"][0]["status"], "incomplete");
        assert_eq!(
            failed["response"]["output"][0]["content"][0]["text"],
            "half"
        );
    }

    /// 没有文本增量的 chunk 跳过（只带 `role` 或只带 `finish_reason` 的那些），
    /// 而结构照样完整。
    #[tokio::test]
    async fn chunks_without_text_deltas_are_skipped_but_the_sequence_stays_complete() {
        let got = frames(stream_of(concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"\"}}]}\n\n",
            "data: 这不是 JSON\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
        )))
        .await;
        assert!(
            !got.iter()
                .any(|f| f["type"] == "response.output_text.delta")
        );
        assert_eq!(got.len(), 7);
        assert_eq!(
            got.last().unwrap()["response"]["output"][0]["content"][0]["text"],
            ""
        );
    }

    /// 🔴 **前三条事件在读上游之前就已产出。**
    ///
    /// 装置：上游**永不发任何字节也永不结束**。回归对象是「这条流退化成攒完再吐」。
    #[tokio::test(start_paused = true)]
    async fn the_first_three_events_arrive_before_the_upstream_says_anything() {
        let up = futures_util::stream::pending::<ByteChunk>();
        let s = to_responses_stream(up, "m".into(), "resp_x".into(), "msg_y".into());
        futures_util::pin_mut!(s);
        for expected in [
            "response.created",
            "response.output_item.added",
            "response.content_part.added",
        ] {
            let f = tokio::time::timeout(std::time::Duration::from_secs(2), s.next())
                .await
                .expect("上游还没说话，这一帧就该出来了")
                .unwrap();
            assert!(f.contains(expected), "{f}");
        }
    }
}
