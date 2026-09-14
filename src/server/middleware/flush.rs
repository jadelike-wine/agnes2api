//! 两条同形的「收尾刷盘」中间件：事件落库与（Tier-2 的）用量落盘。
//!
//! # 三条对两者都成立的硬规则
//!
//! 1. 🔴 **必须在 `next()` 之后**：写在前面的话，本次请求自己产生的事件还没进缓冲。
//! 2. 🔴 **必须真的 `await`**：不 await 的后果是
//!    ① 落盘失败的诊断与引发它的请求对不上号；
//!    ② 收到 `SIGTERM` 时最后一批事件直接丢。
//! 3. **永不让旁路影响响应**：刷盘失败一律吞掉（sink 内部自己记）。
//!
//! # ⚠️ 注册顺序 ≠ 落盘顺序
//!
//! 中间件的收尾段**由内向外**跑 ⇒ **挂得越内层，收尾越早**。
//! 本仓照 TS 的既有顺序：`log_flush` 在外、`usage_flush` 在内
//! ⇒ **今天是用量先落、事件后落**。改这个相对顺序会改变
//! 「进程被杀时哪一批更可能活下来」，不是无关紧要的排版。

use crate::server::middleware::HEALTH_PATH;
use crate::server::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

/// 事件落库的收尾刷盘。
///
/// ⚠️ `/health` 豁免同 `config_refresh`。
pub async fn log_flush(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let is_health = req.uri().path() == HEALTH_PATH;
    let res = next.run(req).await;
    if !is_health {
        // sink 内部有最小间隔闸（默认 60 秒），所以这里每请求调一次是廉价的 ——
        // 🔴 而那道闸挡的是「未鉴权请求驱动存储写」这根杠杆，**不是省点写**。
        st.event_sink.maybe_flush().await;
    }
    res
}

/// Tier-2 用量的收尾刷盘。**sink 不存在时这条中间件根本不挂**（见 `app.rs`）。
///
/// ⚠️ **`/health` 豁免对用量比对事件更要紧**：HEALTHCHECK 是一个
/// **从不产生任何用量**的调用方，不该消费用量统计的写预算。
///
/// 🔴 P1 的 sink 恒为 `None`，所以这条路径今天走不到。
/// 留着它**不是空壳**：它承载的是「关 = 零成本」这条可观测性质 ——
/// 关着时这条中间件不在链上，于是**零存储调用、零累加器**，
/// 而不是「建好再用一个 `if` 拦住写」。
pub async fn usage_flush(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let is_health = req.uri().path() == HEALTH_PATH;
    let res = next.run(req).await;
    if !is_health && let Some(sink) = &st.usage_sink {
        // 🔴 **必须真的 await** —— 不 await 的后果是
        // ① 落盘失败的诊断与引发它的请求对不上号；② 收到 SIGTERM 时最后一批直接丢。
        // ⚠️ 它内部自己吞掉一切失败，**永不让旁路影响响应**。
        sink.maybe_flush(crate::adapters::now_ms()).await;
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::ports::logger::{LogEntry, LogLevel};
    use crate::ports::storage::Storage;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use std::sync::Arc;
    use tower::ServiceExt;

    /// 🔴 **刷盘必须发生在 handler 之后。**
    ///
    /// 回归对象：写在 `next()` 之前的话，本次请求自己产生的事件还没进缓冲 ——
    /// 而「未预期异常必须留下一条事件」那条判据要的正是**本次**这一条。
    #[tokio::test]
    async fn an_event_emitted_by_the_handler_is_flushed_by_the_same_request() {
        let mem = Arc::new(MemoryStorage::new());
        let test_app = state_with_storage(mem.clone()).await;
        // ⚠️ sink 的最小间隔闸把 `last_flush_at` 钉在构造那一刻 ——
        //    不把时钟推过那道闸，这一格测到的就只是「闸生效了」，不是被测的那件事。
        test_app.advance(10 * 60_000);
        let state = test_app.state.clone();
        let logger = state.logger.clone();
        let app = Router::new()
            .route(
                "/boom",
                get(move || {
                    let logger = logger.clone();
                    async move {
                        logger.log(LogEntry::new(
                            LogLevel::Error,
                            "http.unhandled_error",
                            "炸了",
                        ));
                        "ok"
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                log_flush,
            ))
            .with_state(state);

        app.oneshot(
            HttpRequest::builder()
                .uri("/boom")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        let keys = mem.list("event:").await.unwrap();
        assert!(
            !keys.is_empty(),
            "本次请求产生的事件必须在这次响应返回前落盘"
        );
    }

    /// 🔴 **`/health` 一次存储写都不许产生。**
    #[tokio::test]
    async fn health_never_drives_an_event_flush() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        let logger = state.logger.clone();
        let app = Router::new()
            .route(
                "/health",
                get(move || {
                    let logger = logger.clone();
                    async move {
                        logger.log(LogEntry::new(LogLevel::Info, "health.checked", "ok"));
                        "ok"
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                log_flush,
            ))
            .with_state(state);

        let before = counts.snapshot();
        for _ in 0..20 {
            app.clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri("/health")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
        }
        assert_eq!(counts.snapshot(), before, "/health 不许碰存储");
    }

    /// Tier-2 关着时，用量中间件是**零成本**的：一次存储调用都没有。
    #[tokio::test]
    async fn a_disabled_usage_sink_costs_nothing() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        assert!(state.usage_sink.is_none(), "P1 的用量 sink 恒为 None");
        let app = Router::new()
            .route("/v1/x", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                usage_flush,
            ))
            .with_state(state);
        let before = counts.snapshot();
        app.oneshot(
            HttpRequest::builder()
                .uri("/v1/x")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(counts.snapshot(), before);
    }
}
