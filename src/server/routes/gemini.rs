//! Gemini 协议：`GET /v1beta/models` 与 `POST /v1beta/models/{model}:{method}`。

use crate::core::protocol::gemini::{
    GeminiPathError, model_list, split_model_and_method, to_gemini_response, to_gemini_stream,
    to_internal_request,
};
use crate::pool::dispatcher::{DispatchArgs, Method, TimeoutProfile, dispatch};
use crate::server::errors::GatewayError;
use crate::server::routes::{
    RawBody, SSE_CONTENT_TYPE, Success, deps, parse_json, respond, set_stream_headers, shape_error,
    split, sse_body, to_byte_chunks,
};
use crate::server::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// `GET /v1beta/models`。零上游、零存储。
pub async fn models() -> Response {
    Json(model_list()).into_response()
}

/// `POST /v1beta/models/{rest}`，其中 `rest` = `模型名:方法名`。
pub async fn generate(
    State(st): State<AppState>,
    Path(rest): Path<String>,
    RawBody(body): RawBody,
) -> Response {
    // 模型名按**最后一个冒号**切（模型名本身可能含冒号）。
    let (model, stream) = match split_model_and_method(&rest) {
        Ok(v) => v,
        Err(GeminiPathError::MissingMethod) => {
            return GatewayError::new(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "路径里缺少方法名（形如 models/{model}:generateContent）",
            )
            .into_response();
        }
        // 🔴 **未知方法是 404 而不是 400**：请求体没有错，是这条端点本网关没实现
        //    —— 真 Gemini 给的也是 NOT_FOUND。
        //
        // 回归对象（实测缺陷）：从前这里一个字都不校验，`:countTokens` 会落进
        // generateContent 的 handler **真打一次上游对话**（200 + 一段真回答 +
        // `candidatesTokenCount: 43`）。代价链是
        // ① 白烧池中一把 key 的一次上游调用；
        // ② 吃掉上游 CF 那层**全网关共享**的限流额度；
        // ③ 客户端解析 `totalTokens` 拿到空 —— 而「计数坏了」与「所有人莫名被限流」
        //    在面板上看不出因果。
        Err(GeminiPathError::UnknownMethod(m)) => {
            return GatewayError::new(
                StatusCode::NOT_FOUND,
                "not_found_error",
                format!("本网关未实现这个方法：{m}"),
            )
            .into_response();
        }
    };

    let req = match parse_json(&body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let internal = match to_internal_request(&req, model, stream) {
        Ok(v) => v,
        Err(e) => return shape_error(&e).into_response(),
    };
    let model = model.to_string();
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
                    serde_json::to_vec(&to_gemini_response(&body, &model))
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
            // ⚠️ Gemini 的帧**不带事件名**（纯 `data:`），与另两条协议不同。
            let s = to_gemini_stream(to_byte_chunks(body), model);
            let mut res = respond(status, &headers, sse_body(s));
            set_stream_headers(&mut res, SSE_CONTENT_TYPE);
            res
        }
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
    use serde_json::{Value, json};

    async fn json_of(res: Response) -> (StatusCode, Value) {
        let status = res.status();
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }

    /// 模型清单按**形态**给方法：媒体模型的 `supportedGenerationMethods` 是空数组，
    /// 而**不是把它从清单里删掉**。
    ///
    /// ⚠️ 少列一个模型对做模型发现的客户端就是「这个模型不存在」；
    /// 空数组说的才是实话。
    #[tokio::test]
    async fn media_models_are_listed_with_no_generation_methods() {
        let (_, v) = json_of(models().await).await;
        let arr = v["models"].as_array().unwrap();
        assert_eq!(arr.len(), 12);
        let chat = arr
            .iter()
            .filter(|m| {
                !m["supportedGenerationMethods"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            })
            .count();
        assert!(chat > 0 && chat < arr.len(), "对话与媒体两类都该在清单里");
    }

    /// 🔴 **未知方法 404，而且一次上游都不打。**
    #[tokio::test]
    async fn an_unknown_method_is_404_without_touching_the_upstream() {
        use crate::adapters::storage_test_doubles::MemoryStorage;
        use crate::server::test_support::{TestApp, state_with_storage};
        use std::sync::Arc;

        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let (status, v) = json_of(
            generate(
                State(state),
                Path("agnes-2.0-flash:countTokens".into()),
                RawBody(Bytes::from_static(br#"{"contents":[]}"#)),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(v["error"]["type"], "not_found_error");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("countTokens")
        );
    }

    /// 🔴 **`:generateContent` 必须把上游那份 OpenAI JSON 转成 Gemini 形状。**
    ///
    /// 与 Anthropic 那两格同源的回归对象：`expect_json: !stream` 写反 ⇒
    /// 未转换的上游 JSON 配上 SSE 响应头直接吐给 Gemini 客户端。
    /// 在这一格落地之前，`generate()` 的**成功路径一格判据都没有**。
    #[tokio::test]
    async fn generate_content_is_converted_into_the_gemini_shape() {
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

        let res = generate(
            State(state),
            Path("agnes-2.0-flash:generateContent".into()),
            RawBody(Bytes::from(
                json!({
                    "systemInstruction": { "parts": [{ "text": "be brief" }] },
                    "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }]
                })
                .to_string(),
            )),
        )
        .await;

        assert_eq!(res.headers()["content-type"], "application/json");
        let (status, v) = json_of(res).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["candidates"][0]["content"]["parts"][0]["text"], "hi");
        assert_eq!(v["candidates"][0]["finishReason"], "STOP");
        assert_eq!(v["modelVersion"], "agnes-2.0-flash");
        assert_eq!(
            v["usageMetadata"],
            json!({ "promptTokenCount": 7, "candidatesTokenCount": 2, "totalTokenCount": 9 })
        );
        assert!(v.get("choices").is_none(), "上游的形状漏出去了：{v}");

        // 反方向：`model` 来自**路径**、`stream` 来自**方法名**，都不在请求体里。
        let sent = up.only();
        assert_eq!(sent.url, "https://apihub.agnes-ai.com/v1/chat/completions");
        assert_eq!(sent.body_json()["model"], "agnes-2.0-flash");
        assert_eq!(sent.body_json()["stream"], json!(false));
        assert_eq!(
            sent.body_json()["messages"],
            json!([
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "hi" }
            ])
        );
    }

    /// 🔴 **`:streamGenerateContent` 是 Gemini 的帧序列，不是上游原样透传。**
    ///
    /// ⚠️ Gemini 的帧**不带事件名**（纯 `data:`），与另两条协议不同 ——
    /// 这一格顺带把这条差异钉住。
    /// 🔴 而**终帧必须带 `finishReason` 与 `usageMetadata`**：
    /// 没有它，一个被截断的半截回答和一个完整回答在客户端看来逐字节不可区分。
    #[tokio::test]
    async fn stream_generate_content_ends_with_a_real_terminal_frame() {
        let (TestApp { state, .. }, up) = state_with_upstream(vec![UpstreamReply::sse(&[
            "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}],\
\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
        ])])
        .await;

        let res = generate(
            State(state),
            Path("agnes-2.0-flash:streamGenerateContent".into()),
            RawBody(Bytes::from(
                json!({ "contents": [{ "parts": [{ "text": "hi" }] }] }).to_string(),
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
        // ⚠️ 纯 `data:`，一条 `event:` 都不许有。
        assert!(!body.contains("event: "), "Gemini 的帧不带事件名：{body}");
        assert!(body.contains(r#""text":"he""#));
        // 🔴 终帧：`finishReason` 与 `usageMetadata` 缺一不可。
        let frames: Vec<Value> = body
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .map(|p| serde_json::from_str(p).expect("每一帧都必须是合法 JSON"))
            .collect();
        let last = frames.last().expect("至少要有终帧");
        assert_eq!(
            last["candidates"][0]["finishReason"], "MAX_TOKENS",
            "终帧必须说出「被截断了」：{body}"
        );
        assert_eq!(
            last["usageMetadata"],
            json!({ "promptTokenCount": 7, "candidatesTokenCount": 2, "totalTokenCount": 9 }),
            "终帧必须带 usageMetadata：{body}"
        );
        assert_eq!(up.only().body_json()["stream"], json!(true));
    }

    /// 🔴 **超时档由路径里的方法名决定：`:streamGenerateContent` 短档、
    /// `:generateContent` 同步档。**
    ///
    /// ⚠️ 这条协议的 `stream` **来自路径而不是请求体** ——
    /// 两支互换的现象是「同一份请求体，换个方法名超时行为反过来」。
    /// dispatcher 那一侧看不见这件事（档位是它的入参）⇒ 从前全量判据一格都不红。
    ///
    /// ⚠️ 期望值**手写字面量**，不从配置或 `SYNC_ATTEMPT_SHARE` 现算。
    #[tokio::test]
    async fn the_timeout_profile_follows_the_method_name_in_the_path() {
        let body = || {
            RawBody(Bytes::from(
                json!({ "contents": [{ "parts": [{ "text": "hi" }] }] }).to_string(),
            ))
        };

        // `:generateContent` ⇒ 同步档。
        let (TestApp { state, .. }, spy) = state_with_timeout_spy(vec![UpstreamReply::json(
            json!({ "id": "chatcmpl-1", "choices": [] }),
        )])
        .await;
        generate(
            State(state),
            Path("agnes-2.0-flash:generateContent".into()),
            body(),
        )
        .await;
        assert_eq!(spy.only_budget(), SYNC_BUDGET_MS);

        // `:streamGenerateContent` ⇒ 首字节档。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::sse(&["data: [DONE]\n\n"])]).await;
        generate(
            State(state),
            Path("agnes-2.0-flash:streamGenerateContent".into()),
            body(),
        )
        .await;
        assert_eq!(spy.only_budget(), FIRST_BYTE_BUDGET_MS);
    }

    /// 路径里没有冒号 ⇒ 400（请求体没错，是路径写错了）。
    #[tokio::test]
    async fn a_path_without_a_method_is_400() {
        use crate::adapters::storage_test_doubles::MemoryStorage;
        use crate::server::test_support::{TestApp, state_with_storage};
        use std::sync::Arc;

        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let (status, _) = json_of(
            generate(
                State(state),
                Path("agnes-2.0-flash".into()),
                RawBody(Bytes::from_static(br#"{"contents":[]}"#)),
            )
            .await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
