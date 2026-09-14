//! Responses 协议：`POST /v1/responses`。

use crate::core::protocol::responses::{
    to_internal_request, to_responses_response, to_responses_stream,
};
use crate::pool::dispatcher::{DispatchArgs, Method, TimeoutProfile, dispatch};
use crate::server::routes::{
    RawBody, SSE_CONTENT_TYPE, Success, deps, parse_json, random_id, respond, set_stream_headers,
    shape_error, split, sse_body, to_byte_chunks,
};
use crate::server::state::AppState;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

/// `POST /v1/responses`。
pub async fn responses(State(st): State<AppState>, RawBody(body): RawBody) -> Response {
    let req = match parse_json(&body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let internal = match to_internal_request(&req) {
        Ok(v) => v,
        Err(e) => return shape_error(&e).into_response(),
    };
    let stream = internal.stream;
    let model = internal.model.clone();
    let cfg = st.config.current();

    let out = dispatch(
        &deps(&st, &cfg),
        DispatchArgs {
            path: "/chat/completions".into(),
            method: Method::Post,
            body: Some(serde_json::to_value(&internal).unwrap_or(serde_json::Value::Null)),
            timeout: if stream {
                TimeoutProfile::FirstByte
            } else {
                TimeoutProfile::Sync
            },
            expect_json: !stream,
        },
    )
    .await;

    match split(out) {
        Err(ready) => ready,
        Ok(Success::Json { headers, body, .. }) => {
            let mut res = respond(
                200,
                &headers,
                axum::body::Body::from(
                    serde_json::to_vec(&to_responses_response(&body, &model))
                        .unwrap_or_else(|_| b"{}".to_vec()),
                ),
            );
            res.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("application/json"),
            );
            res
        }
        Ok(Success::Stream {
            status,
            headers,
            body,
        }) => {
            // ⚠️ **`response_id` 与 `item_id` 是两个不同的 id**：官方事件里 `item_id`
            //    指的是**这一项**，下游按它把增量归到某一项上。两者都由这里生成
            //    （core 不许随机），且**每个流式响应各不相同**。
            let s = to_responses_stream(
                to_byte_chunks(body),
                model,
                random_id("resp_"),
                random_id("msg_"),
            );
            let mut res = respond(status, &headers, sse_body(s));
            set_stream_headers(&mut res, SSE_CONTENT_TYPE);
            res
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::server::routes::timeout_probe::{
        FIRST_BYTE_BUDGET_MS, SYNC_BUDGET_MS, state_with_timeout_spy,
    };
    use crate::server::test_support::{
        TestApp, UpstreamReply, state_with_storage, state_with_upstream,
    };
    use axum::body::Bytes;
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use std::collections::HashSet;
    use std::sync::Arc;

    /// 结构不对 ⇒ 400，**且一次上游都不打**
    /// （上游替身是「永远连不上」的，真打了会以失败收场而不是 400）。
    #[tokio::test]
    async fn a_structurally_invalid_body_is_refused_locally() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let res = responses(
            State(state),
            RawBody(Bytes::from(json!({ "model": 1 }).to_string())),
        )
        .await;
        let status = res.status();
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["type"], "invalid_request_error");
    }

    /// 🔴 **非流式 `/v1/responses` 必须把上游那份 OpenAI JSON 转成 Responses 形状。**
    ///
    /// 与 Anthropic 那两格同源的回归对象：`expect_json: !stream` 写反 ⇒
    /// 未转换的上游 JSON 配上 SSE 响应头直接吐给 SDK。
    /// 在这一格落地之前，`responses()` 的**成功路径一格判据都没有**。
    ///
    /// ⚠️ 顺带钉住 `length ⇒ incomplete`：那是非流式路径上「被截断」的**唯一**信号。
    #[tokio::test]
    async fn a_non_streaming_response_is_converted_into_the_responses_shape() {
        let (TestApp { state, .. }, up) = state_with_upstream(vec![UpstreamReply::json(json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "hi" },
                "finish_reason": "length"
            }],
            "usage": { "prompt_tokens": 7, "completion_tokens": 2 }
        }))])
        .await;

        let res = responses(
            State(state),
            RawBody(Bytes::from(
                json!({
                    "model": "gpt-x",
                    "instructions": "be brief",
                    "input": "hi"
                })
                .to_string(),
            )),
        )
        .await;

        assert_eq!(res.headers()["content-type"], "application/json");
        let status = res.status();
        let v: Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["object"], "response");
        assert_eq!(v["id"], "resp_chatcmpl-1");
        assert_eq!(v["model"], "gpt-x");
        // 🔴 `length` ⇒ `incomplete`。
        assert_eq!(v["status"], "incomplete");
        assert_eq!(v["output"][0]["content"][0]["text"], "hi");
        assert_eq!(
            v["usage"],
            json!({ "input_tokens": 7, "output_tokens": 2, "total_tokens": 9 })
        );
        assert!(v.get("choices").is_none(), "上游的形状漏出去了：{v}");

        let sent = up.only();
        assert_eq!(sent.url, "https://apihub.agnes-ai.com/v1/chat/completions");
        assert_eq!(sent.body_json()["stream"], json!(false));
        // `instructions` 是**字符串**格，进 messages 首位；`input` 的字符串形态照常放行。
        assert_eq!(
            sent.body_json()["messages"],
            json!([
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hi" }
            ])
        );
    }

    /// 🔴 **流式 `/v1/responses` 是官方 SDK 能用的那串事件，不是上游原样透传。**
    ///
    /// 🔴 **八条事件一条都不许少**：`client.responses.stream(...)` 靠
    /// `output_item.added` / `content_part.added` 建出 snapshot，
    /// 少了它们会在 openai 包里抛 `IndexError`，栈里**全是 SDK 自己的文件** ——
    /// 用户第一反应是「我 SDK 装坏了」。
    #[tokio::test]
    async fn a_streaming_response_is_the_minimal_event_sequence_the_sdk_needs() {
        let (TestApp { state, .. }, up) = state_with_upstream(vec![UpstreamReply::sse(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\ndata: [DONE]\n\n",
        ])])
        .await;

        let res = responses(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "gpt-x", "input": "hi", "stream": true }).to_string(),
            )),
        )
        .await;

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], SSE_CONTENT_TYPE);
        assert_eq!(res.headers()["cache-control"], "no-cache");
        let body = String::from_utf8(
            axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();

        assert!(
            !body.contains("choices") && !body.contains("[DONE]"),
            "上游的 OpenAI 帧被原样透传了：{body}"
        );
        let events: Vec<&str> = body
            .lines()
            .filter_map(|l| l.strip_prefix("event: "))
            .collect();
        assert_eq!(
            events,
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
        // `sequence_number` 全流单调递增，从 0 数起。
        let seqs: Vec<u64> = body
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .filter_map(|p| serde_json::from_str::<Value>(p).ok())
            .filter_map(|v| v["sequence_number"].as_u64())
            .collect();
        assert_eq!(seqs, (0..events.len() as u64).collect::<Vec<_>>());
        // 🔴 `item_id` / `logprobs` 两格缺一个就是 SDK 的一次 `AttributeError`。
        assert!(body.contains(r#""logprobs":[]"#), "{body}");
        assert!(body.contains(r#""item_id":"msg_"#), "{body}");
        assert!(body.contains(r#""text":"hello""#), "增量要攒成整段：{body}");
        assert_eq!(up.only().body_json()["stream"], json!(true));
    }

    /// 🔴 **`stream` 决定超时档：流式短档、非流式同步档。**
    ///
    /// 与另三条协议同形的一格：档位是传给 `dispatch` 的**入参**，
    /// dispatcher 那一侧看不见「路由选了哪一档」⇒ 把
    /// `if stream { FirstByte } else { Sync }` 两支互换，从前全量判据一格都不红。
    ///
    /// ⚠️ 期望值**手写字面量**，不从配置或 `SYNC_ATTEMPT_SHARE` 现算。
    #[tokio::test]
    async fn the_timeout_profile_follows_stream_not_the_endpoint_name() {
        // 非流式 ⇒ 同步档。
        let (TestApp { state, .. }, spy) = state_with_timeout_spy(vec![UpstreamReply::json(
            json!({ "id": "chatcmpl-1", "choices": [] }),
        )])
        .await;
        responses(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "gpt-x", "input": "hi" }).to_string(),
            )),
        )
        .await;
        assert_eq!(spy.only_budget(), SYNC_BUDGET_MS);

        // 流式 ⇒ 首字节档。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::sse(&["data: [DONE]\n\n"])]).await;
        responses(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "gpt-x", "input": "hi", "stream": true }).to_string(),
            )),
        )
        .await;
        assert_eq!(spy.only_budget(), FIRST_BYTE_BUDGET_MS);
    }

    /// 🔴 **两个 id 既互不相同、也不跨请求重复。**
    ///
    /// 回归对象：下游按 message id 做的日志 / 追踪 / 去重 / 缓存会互相撞车。
    #[test]
    fn the_generated_ids_are_distinct_and_never_repeat() {
        let mut seen = HashSet::new();
        for _ in 0..500 {
            let a = random_id("resp_");
            let b = random_id("msg_");
            assert_ne!(a, b);
            assert!(seen.insert(a));
            assert!(seen.insert(b));
        }
    }
}
