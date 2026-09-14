//! OpenAI 协议 —— **母协议，也是最薄的一条**。
//!
//! 上游就是 OpenAI 兼容的 `/chat/completions`，所以这条路由**原样透传**：
//! 网关从头到尾**不解析**它的请求体与响应体。
//!
//! 🔴 **`tokensCoverage` 里没有它，那不是漏**：它是唯一不传 `expect_json` 的一条，
//! 网关手上从来没有过一份解析好的响应体。这个缺口由
//! `GET /admin/api/capabilities` 的 `stats.tokensCoverage` 如实告诉面板。
//! 🔴 **别在这里补一次 `res.clone().json()`** —— 那是在热路径上给每个请求加一次全量解析，
//! 而且会把流式那一族整体缓冲掉。

use crate::adapters::now_ms;
use crate::core::protocol::openai::{check_chat_shape, model_list_response, wants_stream};
use crate::pool::dispatcher::{DispatchArgs, Method, TimeoutProfile, dispatch};
use crate::server::errors::GatewayError;
use crate::server::routes::{
    RawBody, SSE_CONTENT_TYPE, Success, deps, parse_json, respond, shape_error, split,
};
use crate::server::state::AppState;
use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

/// `GET /v1/models`。**零上游、零存储。**
pub async fn models() -> Response {
    // `created` 是 unix **秒**（OpenAI 的口径），core 不读时钟所以由这里传。
    Json(model_list_response(now_ms() / 1000)).into_response()
}

/// `POST /v1/chat/completions`。
pub async fn chat_completions(State(st): State<AppState>, RawBody(body): RawBody) -> Response {
    let req = match parse_json(&body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    // 🔴 **本地形状校验不是可选项。**
    //
    // 从前这条路由一次校验都没有：`{"foo":1}` 被原封不动送上游由上游回 400。
    // 代价不是「多一次往返」那么轻 —— 上游那层 CF 约 2 次快请求就回 1015，
    // 而限流额度是**整个网关共享**的 ⇒ 一个客户端 bug 循环重试畸形请求，
    // 几秒内就能把所有人的上游通道打死。
    //
    // ⚠️ **只校验 `model` 与 `messages` 两样，不做更严的形状检查**：
    // 上游接受哪些字段（`tools` / `response_format` / …）是它的事。
    // 🔴 特别是：**这条协议不走三档参数裁定表** —— 它是整包透传，
    // `tools` 在这里**真的能用**（实测上游会回真的 `tool_calls`）。
    if let Err(e) = check_chat_shape(&req) {
        return shape_error(&e).into_response();
    }
    let stream = wants_stream(&req);
    let cfg = st.config.current();

    let out = dispatch(
        &deps(&st, &cfg),
        DispatchArgs {
            path: "/chat/completions".into(),
            method: Method::Post,
            body: Some(req),
            timeout: if stream {
                TimeoutProfile::FirstByte
            } else {
                // 非流式对话：上游要把整段回答生成完才发响应头，
                // 与图片生成是同一种延迟语义。
                TimeoutProfile::Sync
            },
            // **原样搬运**，网关不解析响应体。
            expect_json: false,
        },
    )
    .await;

    match split(out) {
        Err(ready) => ready,
        Ok(Success::Stream {
            status,
            headers,
            body,
        }) => {
            let mut res = respond(status, &headers, Body::from_stream(body));
            if stream {
                // ⚠️ **只补 content-type，不整体替换响应头** —— 整体替换会丢掉
                //    上游的 `cache-control`。
                res.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static(SSE_CONTENT_TYPE),
                );
            }
            res
        }
        // `expect_json: false` ⇒ 结构上到不了这一支。
        Ok(Success::Json { .. }) => GatewayError::internal().into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::timeout_probe::{
        FIRST_BYTE_BUDGET_MS, SYNC_BUDGET_MS, state_with_timeout_spy,
    };
    use crate::server::test_support::{TestApp, UpstreamReply, state_with_upstream};
    use axum::body::Bytes;
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    async fn raw_of(res: Response) -> String {
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&b).into_owned()
    }

    /// 🔴 **非流式 `/v1/chat/completions` 两个方向都原样搬运。**
    ///
    /// 发给上游的请求体逐字节是客户端那一份（**不走内部规范格式**），
    /// 回给客户端的响应体逐字节是上游那一份（**网关不解析、不重排**）。
    ///
    /// ⚠️ 这一格落地之前，`chat_completions()` 的**成功路径一格判据都没有**：
    /// 全仓的路由判据都走 `state_with_storage` ⇒ `UnreachableFetcher`，
    /// 「上游答了 200」那一支永远到不了。
    ///
    /// ⚠️ 请求体里刻意带了 `tools`：**这条协议不走三档参数裁定表**，
    /// `tools` 在这里要真的能过去（另三条协议会 400，那是它们的判据）。
    #[tokio::test]
    async fn a_non_streaming_chat_completion_is_relayed_in_both_directions() {
        let upstream = json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "hi" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 7, "completion_tokens": 2 }
        });
        let (TestApp { state, .. }, up) =
            state_with_upstream(vec![UpstreamReply::json(upstream.clone())]).await;

        let client_body = json!({
            "model": "m",
            "messages": [{ "role": "user", "content": "hi" }],
            "tools": [{ "type": "function", "function": { "name": "f" } }]
        });
        let res =
            chat_completions(State(state), RawBody(Bytes::from(client_body.to_string()))).await;

        assert_eq!(res.status(), StatusCode::OK);
        // 🔴 非流式**不许**被贴上 SSE 的 content-type。
        assert_eq!(res.headers()["content-type"], "application/json");
        assert_eq!(
            raw_of(res).await,
            upstream.to_string(),
            "响应体必须是上游那份原字节"
        );

        let sent = up.only();
        assert_eq!(sent.method, "POST");
        assert_eq!(sent.url, "https://apihub.agnes-ai.com/v1/chat/completions");
        assert_eq!(
            sent.body_json(),
            client_body,
            "请求体必须原样搬运（含 tools），不许过一遍内部规范格式"
        );
    }

    /// 🔴 **流式 `/v1/chat/completions` 逐字节透传，只补 content-type。**
    ///
    /// ⚠️ **不许整体替换响应头**：那会把上游的 `cache-control` 丢掉 ——
    /// 这一格给上游一个**独特的** `cache-control` 就是为了让「丢掉」可观测。
    #[tokio::test]
    async fn a_streaming_chat_completion_keeps_the_upstream_cache_control() {
        let frames = [
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\ndata: [DONE]\n\n",
        ];
        let (TestApp { state, .. }, up) = state_with_upstream(vec![
            UpstreamReply::sse(&frames).with_header("cache-control", "no-store, max-age=0"),
        ])
        .await;

        let res = chat_completions(
            State(state),
            RawBody(Bytes::from(
                json!({
                    "model": "m",
                    "messages": [{ "role": "user", "content": "hi" }],
                    "stream": true
                })
                .to_string(),
            )),
        )
        .await;

        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], SSE_CONTENT_TYPE);
        assert_eq!(
            res.headers()["cache-control"],
            "no-store, max-age=0",
            "整体替换响应头会把上游的 cache-control 丢掉"
        );
        assert_eq!(
            raw_of(res).await,
            frames.concat(),
            "上游的帧必须逐字节到达客户端（本协议不做任何转换）"
        );
        assert_eq!(up.only().body_json()["stream"], json!(true));
    }

    /// 🔴 **`stream` 决定超时档：流式短档、非流式同步档。**
    ///
    /// 依据不是端点的名字而是「上游的第一个字节什么时候才可能到达」：
    /// 流式请求上游立刻开始吐帧（慢 key 要在这里被快速甩掉），
    /// 非流式要把整段回答生成完才发响应头 —— 与图片生成是同一种延迟语义。
    ///
    /// ⚠️ 这一格钉的是**这条路由传给 `dispatch` 的是哪一档**。
    /// dispatcher 那一侧只钉得住「拿到哪一档就发哪个预算」，档位是入参，它看不见 ——
    /// 于是 `if stream { … } else { … }` 这两支互换一下，从前**全量判据一格都不红**。
    ///
    /// ⚠️ 期望值**手写字面量**，不从配置或 `SYNC_ATTEMPT_SHARE` 现算。
    #[tokio::test]
    async fn the_timeout_profile_follows_stream_not_the_endpoint_name() {
        // 非流式 ⇒ 同步档。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::json(json!({ "choices": [] }))]).await;
        chat_completions(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "m", "messages": [{ "role": "user", "content": "hi" }] })
                    .to_string(),
            )),
        )
        .await;
        assert_eq!(
            spy.only_budget(),
            SYNC_BUDGET_MS,
            "非流式要等整段生成完才有响应头，短档会把它整条掐死"
        );

        // 流式 ⇒ 首字节档。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::sse(&["data: [DONE]\n\n"])]).await;
        chat_completions(
            State(state),
            RawBody(Bytes::from(
                json!({
                    "model": "m",
                    "messages": [{ "role": "user", "content": "hi" }],
                    "stream": true
                })
                .to_string(),
            )),
        )
        .await;
        assert_eq!(
            spy.only_budget(),
            FIRST_BYTE_BUDGET_MS,
            "流式必须留在短档：慢 key 要在这里被快速甩掉"
        );
    }

    /// `/v1/models` 的形状：12 条、`object: "list"`。
    #[tokio::test]
    async fn the_model_list_is_openai_shaped() {
        let res = models().await;
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["object"], "list");
        assert_eq!(v["data"].as_array().unwrap().len(), 12);
        assert_eq!(v["data"][0]["object"], "model");
    }

    /// `created` 是 unix **秒**，不是毫秒。
    ///
    /// 回归对象：给毫秒的话客户端会把模型的创建时间显示成公元 5 万年 ——
    /// 那是一条不会有人报的 bug。
    #[tokio::test]
    async fn created_is_in_seconds_not_milliseconds() {
        let res = models().await;
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&b).unwrap();
        let created = v["data"][0]["created"].as_i64().unwrap();
        // 2020-01-01 ~ 2100-01-01 的秒数区间。
        assert!(
            (1_577_836_800..4_102_444_800).contains(&created),
            "created 不像 unix 秒: {created}"
        );
    }
}
