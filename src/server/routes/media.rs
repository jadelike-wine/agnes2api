//! 媒体三条：图片生成、视频建任务、视频轮询。
//!
//! 这三条是「同步超时档」与 `SYNC_ATTEMPT_SHARE`、`attribute_sync_timeouts`
//! **唯一的真实消费者** —— 没有它们，那三条判据只能对着一个没有消费者的代码路径写，
//! 而那正是已登记的假阳性形态第 ① 条。
//!
//! # 三条端点的超时档不一样，而依据不是端点的名字
//!
//! | 端点 | 档 | 为什么 |
//! |---|---|---|
//! | `POST /v1/images/generations` | `Sync` | 实测直连上游首字节 **11.99 秒** |
//! | `POST /v1/videos` | `Sync` | 建任务同理 |
//! | `GET /v1/videos/{id}` | `FirstByte` | **唯一的例外**：非流式，但只查一次任务状态，是快接口 |
//!
//! 这就是「档位由调用方显式传入，不在 dispatcher 里按 `stream` 自动推断」的原因。

use crate::pool::dispatcher::{DispatchArgs, Method, TimeoutProfile, dispatch};
use crate::server::errors::GatewayError;
use crate::server::routes::{RawBody, Success, deps, parse_json, respond, split};
use crate::server::state::AppState;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::sync::OnceLock;

/// 🔴 视频任务 id 的形状。**一个字符都不许放宽。**
///
/// 它是一条**未核实的假设**（上游真实 id 的形状还没有实物）：
/// - 放宽了就是给路径穿越留门（这个值直接拼进上游 URL）；
/// - 收紧过头会让合法任务 id 被 400。
///
/// ⇒ 等注册机铸出第一把真 key、拿到一条真任务 id 之后**当场重验**，在那之前不动它。
pub const VIDEO_TASK_ID_RE: &str = r"^[A-Za-z0-9_-]{1,128}$";

/// 建视频任务那条端点的方法 + 路径。
///
/// **报文里的形状串与这一对都从这里派生，不手抄** —— 手抄的那一份迟早与真路由漂开，
/// 而现象是「报错信息教客户端去打一条不存在的端点」。
pub const VIDEO_CREATE_METHOD: &str = "POST";
pub const VIDEO_CREATE_PATH: &str = "/v1/videos";

fn task_id_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(VIDEO_TASK_ID_RE).expect("字面量，编译不失败"))
}

/// `POST /v1/images/generations`。
pub async fn images_generations(State(st): State<AppState>, RawBody(body): RawBody) -> Response {
    forward_sync(st, body, "/images/generations").await
}

/// `POST /v1/videos`（建任务）。
pub async fn videos_create(State(st): State<AppState>, RawBody(body): RawBody) -> Response {
    forward_sync(st, body, "/videos").await
}

async fn forward_sync(st: AppState, body: Bytes, path: &str) -> Response {
    let req = match parse_json(&body) {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let cfg = st.config.current();
    let out = dispatch(
        &deps(&st, &cfg),
        DispatchArgs {
            path: path.to_string(),
            method: Method::Post,
            body: Some(req),
            // 🔴 同步档。用 8 秒去卡图片生成的结果是**100% 失败**，
            //    且每次请求把池中每把 key 各记一次 strike ——
            //    三次请求就能把整个池子打进 30 分钟长冷却，连对话一起拖死。
            timeout: TimeoutProfile::Sync,
            // 媒体路由承诺「上游返回什么就原样转发响应体」⇒ 网关不解析它。
            expect_json: false,
        },
    )
    .await;
    passthrough(out)
}

/// `GET /v1/videos/{id}`（轮询）。
pub async fn videos_poll(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    if !task_id_re().is_match(&id) {
        return GatewayError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            format!(
                "任务 id 形状不合法（应匹配 {VIDEO_TASK_ID_RE}）；任务 id 来自 {VIDEO_CREATE_METHOD} {VIDEO_CREATE_PATH} 的响应"
            ),
        )
        .into_response();
    }
    let cfg = st.config.current();
    let out = dispatch(
        &deps(&st, &cfg),
        DispatchArgs {
            path: format!("/videos/{id}"),
            method: Method::Get,
            body: None,
            // ⚠️ **唯一的「非流式但用首字节档」的例外**：它只查一次任务状态。
            timeout: TimeoutProfile::FirstByte,
            expect_json: false,
        },
    )
    .await;
    passthrough(out)
}

fn passthrough(out: crate::pool::dispatcher::Dispatched) -> Response {
    match split(out) {
        Err(ready) => ready,
        Ok(Success::Stream {
            status,
            headers,
            body,
        }) => respond(status, &headers, Body::from_stream(body)),
        // `expect_json: false` ⇒ 结构上到不了这一支。
        Ok(Success::Json { .. }) => GatewayError::internal().into_response(),
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
    use serde_json::{Value, json};
    use std::sync::Arc;

    /// 🔴 **媒体三条的成功路径：上游返回什么就原样转发响应体。**
    ///
    /// 三条一起走一格，因为它们要钉的是**同一句承诺**，差别只在
    /// 「打到上游哪条路径 / 用哪个方法」——而那两样恰恰是最容易改错的。
    ///
    /// ⚠️ 这一格落地之前，`images_generations()` / `videos_create()` 的**成功路径
    /// 一格判据都没有**：全仓的路由判据都走 `UnreachableFetcher`。
    #[tokio::test]
    async fn the_three_media_endpoints_relay_the_upstream_body_verbatim() {
        // ① 图片生成。
        let png = Bytes::from_static(b"\x89PNG\r\n\x1a\nnot-really");
        let (TestApp { state, .. }, up) = state_with_upstream(vec![
            UpstreamReply::raw("image/png", vec![png.slice(0..4), png.slice(4..)])
                .with_header("content-disposition", "attachment; filename=a.png"),
        ])
        .await;
        let res = images_generations(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "m", "prompt": "a cat" }).to_string(),
            )),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()["content-type"], "image/png");
        // 媒体路由承诺原样转发 ⇒ 剥掉它会让浏览器端下载丢失文件名。
        assert_eq!(
            res.headers()["content-disposition"],
            "attachment; filename=a.png"
        );
        assert_eq!(
            axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
            png,
            "响应体必须逐字节是上游那一份"
        );
        let sent = up.only();
        assert_eq!(sent.method, "POST");
        assert_eq!(
            sent.url,
            "https://apihub.agnes-ai.com/v1/images/generations"
        );
        assert_eq!(sent.body_json(), json!({ "model": "m", "prompt": "a cat" }));

        // ② 视频建任务。
        let (TestApp { state, .. }, up) =
            state_with_upstream(vec![UpstreamReply::json(json!({ "id": "task-1" }))]).await;
        let res = videos_create(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "m", "prompt": "x" }).to_string(),
            )),
        )
        .await;
        assert_eq!(res.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v, json!({ "id": "task-1" }));
        let sent = up.only();
        assert_eq!(sent.method, "POST");
        assert_eq!(sent.url, "https://apihub.agnes-ai.com/v1/videos");

        // ③ 视频轮询。**合法 id 逐字拼进上游路径**，且方法是 GET。
        let (TestApp { state, .. }, up) =
            state_with_upstream(vec![UpstreamReply::json(json!({ "status": "done" }))]).await;
        let res = videos_poll(State(state), Path("task-1_A".into())).await;
        assert_eq!(res.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(
            &axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v, json!({ "status": "done" }));
        let sent = up.only();
        assert_eq!(sent.method, "GET");
        assert_eq!(sent.url, "https://apihub.agnes-ai.com/v1/videos/task-1_A");
        assert!(sent.body.is_none(), "GET 不带请求体");
    }

    /// 🔴 **路径穿越防护**：不合法的任务 id 在本地就被挡住，一次上游都不打。
    #[tokio::test]
    async fn a_traversal_shaped_task_id_never_reaches_the_upstream() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        for bad in [
            "../../admin",
            "a/b",
            "a b",
            "",
            "a?x=1",
            "a#frag",
            "%2e%2e%2f",
        ] {
            let res = videos_poll(State(state.clone()), Path(bad.to_string())).await;
            assert_eq!(
                res.status(),
                StatusCode::BAD_REQUEST,
                "不该放行任务 id {bad:?}"
            );
        }
        // 128 是上界，129 要被挡住。
        // 合法 id ⇒ 走到上游（替身永远连不上）⇒ 503，**不是 400**。
        assert_ne!(
            videos_poll(State(state.clone()), Path("a".repeat(128)))
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "128 个字符是上界之内，不该被挡"
        );
        assert_eq!(
            videos_poll(State(state), Path("a".repeat(129)))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }

    /// 🔴🔴 **媒体三条各选哪一档超时** —— 模块头那张表，逐行钉住。
    ///
    /// 回归对象是 `01 §3.5` 记着的那次**真实事故**：
    /// `POST /v1/images/generations` 拿到 `8000/2 = 4000ms` 预算 ⇒ **图片生成 100% 失败**，
    /// 且每次请求把池中**每把 key 各记一次 strike**，三次请求就把整池打进 30 分钟长冷却、
    /// **连对话一起拖死**。
    ///
    /// ⚠️ 这一格落地之前，那张表是**一张没人守的表**：实测把
    /// `images_generations()` 的 `TimeoutProfile::Sync` 改成 `FirstByte`，
    /// 全量 600 格判据**一格都不红** —— dispatcher 那一侧只钉得住
    /// 「拿到哪一档就发哪个预算」，档位是调用方传进去的，它看不见。
    ///
    /// ⚠️ 期望值**手写字面量**，见 [`FIRST_BYTE_BUDGET_MS`] / [`SYNC_BUDGET_MS`] 的说明。
    #[tokio::test]
    async fn the_three_media_endpoints_each_send_the_budget_their_table_row_promises() {
        // ① 图片生成 ⇒ **同步档**（实测直连上游首字节 11.99 秒）。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::raw("image/png", vec![])]).await;
        images_generations(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "m", "prompt": "a cat" }).to_string(),
            )),
        )
        .await;
        assert_eq!(
            spy.only_budget(),
            SYNC_BUDGET_MS,
            "图片生成拿短预算 = 100% 失败 + 整池 30 分钟长冷却"
        );

        // ② 视频建任务 ⇒ **同步档**（建任务同理）。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::json(json!({ "id": "task-1" }))]).await;
        videos_create(
            State(state),
            RawBody(Bytes::from(
                json!({ "model": "m", "prompt": "x" }).to_string(),
            )),
        )
        .await;
        assert_eq!(spy.only_budget(), SYNC_BUDGET_MS);

        // ③ 视频轮询 ⇒ **首字节档**。🔴 **全网关唯一的「非流式但用短档」的例外**：
        //    它只查一次任务状态，是快接口 —— 给它 60 秒预算会让一把挂起的 key
        //    把一次「查状态」拖满一分钟。
        let (TestApp { state, .. }, spy) =
            state_with_timeout_spy(vec![UpstreamReply::json(json!({ "status": "done" }))]).await;
        videos_poll(State(state), Path("task-1_A".into())).await;
        assert_eq!(
            spy.only_budget(),
            FIRST_BYTE_BUDGET_MS,
            "轮询是唯一的例外，它必须留在短档"
        );
    }

    /// 报错信息里**教客户端去哪儿拿任务 id**，而那一对是从真源常量派生的。
    #[tokio::test]
    async fn the_rejection_tells_the_caller_where_task_ids_come_from() {
        let TestApp { state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        let res = videos_poll(State(state), Path("../etc".into())).await;
        let b = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&b).unwrap();
        let msg = v["error"]["message"].as_str().unwrap();
        assert!(msg.contains(VIDEO_CREATE_PATH));
        assert!(msg.contains(VIDEO_CREATE_METHOD));
    }
}
