//! Anthropic 协议：`POST /v1/messages` 与 `POST /v1/messages/count_tokens`。

use crate::core::protocol::anthropic::{to_anthropic_response, to_anthropic_stream};
use crate::core::protocol::count_tokens::{count_tokens_response, estimate_input_tokens};
use crate::core::protocol::{InternalRequest, anthropic};
use crate::pool::dispatcher::{DispatchArgs, Method, TimeoutProfile, dispatch};
use crate::server::routes::{
    RawBody, SSE_CONTENT_TYPE, Success, deps, parse_json, random_id, respond, set_stream_headers,
    shape_error, split, sse_body, to_byte_chunks,
};
use crate::server::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

/// `POST /v1/messages`。
pub async fn messages(State(st): State<AppState>, RawBody(body): RawBody) -> Response {
    let req = match parse_json(&body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let internal: InternalRequest = match anthropic::to_internal_request(&req) {
        Ok(v) => v,
        // 🔴 零上游：形状不对在这里就结束，**一次上游调用都不发**。
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
            // 非流式时要把上游那份 OpenAI 响应转成 Anthropic 形状 ⇒ 必须是 JSON。
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
                    serde_json::to_vec(&to_anthropic_response(&body, &model))
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
            // 🔴 `message_id` **每个流式响应必须各不相同**：
            //    下游按 message id 做的日志 / 追踪 / 去重 / 缓存会互相撞车。
            let message_id = random_id("msg_");
            let s = to_anthropic_stream(to_byte_chunks(body), model, message_id);
            let mut res = respond(status, &headers, sse_body(s));
            set_stream_headers(&mut res, SSE_CONTENT_TYPE);
            res
        }
    }
}

/// `POST /v1/messages/count_tokens`。
///
/// 🔴 **零上游、零存储、空池也 200。**
///
/// ⚠️ 它是**本网关的本地估算，不是上游计数** —— 这句话要同时出现在
/// `upstream_facts`（标 `assumed`）与五语言 API.md 里。
/// 客户端拿它做预算规划时会偏，而**说清楚**比偷偷打一次上游便宜得多。
pub async fn count_tokens(RawBody(body): RawBody) -> Response {
    let req = match parse_json(&body) {
        Ok(v) => v,
        // ⚠️ 畸形体回的是 **JSON 信封的 400**，不是 axum 默认的 422 纯文本 ——
        //    后者会让 SDK 在解析处二次抛错。
        Err(e) => return e.into_response(),
    };
    match estimate_input_tokens(&req) {
        Ok(n) => Json(count_tokens_response(n)).into_response(),
        Err(e) => shape_error(&e).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::server::routes::timeout_probe::{
        FIRST_BYTE_BUDGET_MS, SYNC_BUDGET_MS, state_with_timeout_spy,
    };
    use crate::server::test_support::{
        TestApp, UpstreamReply, state_with_storage, state_with_upstream,
    };
    use axum::body::Bytes;
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use std::sync::Arc;

    async fn body_of(res: Response) -> (StatusCode, Value) {
        let status = res.status();
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    /// 🔴 **`count_tokens` 零上游、零存储，空池也 200。**
    ///
    /// 这一格用的存储桩会把任何一次调用数出来；上游那一侧用的是
    /// 「永远连不上」的替身 —— 真发了一次调用会以失败收场而不是悄悄成功。
    #[tokio::test]
    async fn count_tokens_touches_neither_storage_nor_upstream() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { .. } = state_with_storage(counting).await;
        let before = counts.snapshot();

        let res = count_tokens(RawBody(Bytes::from(
            json!({ "model": "m", "messages": [{ "role": "user", "content": "hello" }] })
                .to_string(),
        )))
        .await;
        let (status, v) = body_of(res).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["input_tokens"], json!(1));
        assert_eq!(counts.snapshot(), before, "不许碰存储");
    }

    /// ⚠️ 畸形请求体回 **JSON 信封的 400**，不是纯文本。
    #[tokio::test]
    async fn a_malformed_body_gets_a_json_400() {
        let (status, v) = body_of(count_tokens(RawBody(Bytes::from_static(b"{oops"))).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["type"], "invalid_request_error");
        assert!(v["error"]["message"].is_string());
    }

    /// 🔴🔴 **非流式 `/v1/messages` 必须把上游那份 OpenAI JSON 转成 Anthropic 形状。**
    ///
    /// 回归对象（本轮对抗式复核实测）：把上面的 `expect_json: !stream` 写成
    /// `expect_json: stream` ⇒ 非流式请求落进 `Success::Stream` 那一支，
    /// **把上游那份未转换的 OpenAI JSON 配上 SSE 响应头直接吐给 Anthropic 客户端**
    /// —— 而在这一格落地之前，全仓 570 格判据**没有一格会红**：
    /// 所有路由判据都走 `state_with_storage` ⇒ `UnreachableFetcher`，
    /// 「上游答了 200」那一支根本到不了。
    ///
    /// ⇒ 这一格同时钉住三样：**状态码 / content-type / 响应体形状**。
    #[tokio::test]
    async fn a_non_streaming_message_is_converted_into_the_anthropic_shape() {
        let (TestApp { state, .. }, up) = state_with_upstream(vec![UpstreamReply::json(json!({
            "id": "chatcmpl-1",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "hi" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 7, "completion_tokens": 2 }
        }))])
        .await;

        let res = messages(
            State(state),
            RawBody(Bytes::from(
                json!({
                    "model": "claude-x",
                    "system": "be brief",
                    "messages": [{ "role": "user", "content": "hi" }]
                })
                .to_string(),
            )),
        )
        .await;

        // 🔴 非流式 **必须**是 JSON，不是 SSE。
        assert_eq!(res.headers()["content-type"], "application/json");
        assert!(
            res.headers().get("cache-control").is_none(),
            "非流式不该带流式那一族的响应头"
        );
        let (status, v) = body_of(res).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["type"], "message");
        assert_eq!(v["id"], "msg_chatcmpl-1");
        // `model` 取的是**客户端请求里那个**，不是上游的。
        assert_eq!(v["model"], "claude-x");
        assert_eq!(v["content"], json!([{ "type": "text", "text": "hi" }]));
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["usage"], json!({ "input_tokens": 7, "output_tokens": 2 }));
        // 上游那份 OpenAI 形状**一格都不许漏出去**。
        assert!(v.get("choices").is_none(), "上游的形状漏出去了：{v}");

        // 反方向：发给上游的是**转换后的**内部规范格式（system 提到 messages 首位）。
        let sent = up.only();
        assert_eq!(sent.url, "https://apihub.agnes-ai.com/v1/chat/completions");
        assert_eq!(
            sent.body_json()["messages"],
            json!([
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hi" }
            ])
        );
        assert_eq!(sent.body_json()["stream"], json!(false));
    }

    /// 🔴🔴 **流式 `/v1/messages` 必须是 Anthropic 的事件序列，不是上游原样透传。**
    ///
    /// 回归对象（本轮对抗式复核实测）：把上面的 `to_anthropic_stream(...)` 换成
    /// `Body::from_stream(body)` 原样透传 ⇒ Anthropic 客户端收到的是一串 OpenAI 的
    /// `chat.completion.chunk` —— 而在这一格落地之前，全仓 570 格判据**没有一格会红**。
    ///
    /// ⚠️ 上游的帧**刻意切在块边界上**（`{"choi` / `ces":…`），
    /// 顺带钉住「跨块的 SSE 帧也认得出来」。
    #[tokio::test]
    async fn a_streaming_message_is_an_anthropic_event_sequence_not_a_passthrough() {
        let (TestApp { state, .. }, up) = state_with_upstream(vec![UpstreamReply::sse(&[
            "data: {\"choi",
            "ces\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}],\
\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
        ])])
        .await;

        let res = messages(
            State(state),
            RawBody(Bytes::from(
                json!({
                    "model": "claude-x",
                    "stream": true,
                    "messages": [{ "role": "user", "content": "hi" }]
                })
                .to_string(),
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

        // 🔴 上游的形状一个字节都不许漏出去。
        assert!(
            !body.contains("choices") && !body.contains("[DONE]"),
            "上游的 OpenAI 帧被原样透传了：{body}"
        );
        // 事件序列逐条。
        let events: Vec<&str> = body
            .lines()
            .filter_map(|l| l.strip_prefix("event: "))
            .collect();
        assert_eq!(
            events,
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
        assert!(body.contains(r#""text":"he""#) && body.contains(r#""text":"llo""#));
        // 末尾的 usage 是**真值**，`length` 也如实翻成 `max_tokens`。
        assert!(
            body.contains(r#""stop_reason":"max_tokens""#),
            "被截断必须说出来：{body}"
        );
        assert!(
            body.contains(r#""usage":{"input_tokens":7,"output_tokens":2}"#),
            "末尾 usage 必须是真值：{body}"
        );
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
        // 非流式 ⇒ 同步档（上游要把整段回答生成完才发响应头）。
        let (TestApp { state, .. }, spy) = state_with_timeout_spy(vec![UpstreamReply::json(
            json!({ "id": "chatcmpl-1", "choices": [] }),
        )])
        .await;
        messages(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "claude-x", "messages": [{ "role": "user", "content": "hi" }] })
                    .to_string(),
            )),
        )
        .await;
        assert_eq!(spy.only_budget(), SYNC_BUDGET_MS);

        // 流式 ⇒ 首字节档（慢 key 要在这里被快速甩掉）。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::sse(&["data: [DONE]\n\n"])]).await;
        messages(
            State(state),
            RawBody(Bytes::from(
                json!({
                    "model": "claude-x",
                    "stream": true,
                    "messages": [{ "role": "user", "content": "hi" }]
                })
                .to_string(),
            )),
        )
        .await;
        assert_eq!(spy.only_budget(), FIRST_BYTE_BUDGET_MS);
    }

    /// 🔴 **做不到就明说**：多模态块一律 400 并逐字点名是哪一格。
    ///
    /// 回归对象：实测过同一张 PNG 走另外两条协议返回 200 +
    /// 「模型没看见图编出来的答案」，而**网关一句提示都没有**。
    #[tokio::test]
    async fn an_unsupported_content_block_is_refused_in_words() {
        let (status, v) = body_of(
            count_tokens(RawBody(Bytes::from(
                json!({
                    "model": "m",
                    "messages": [{ "role": "user", "content": [{ "type": "image" }] }]
                })
                .to_string(),
            )))
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains("image"), "报文必须点名是哪一种块：{msg}");
    }
}
