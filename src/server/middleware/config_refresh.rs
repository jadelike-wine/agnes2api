//! 每请求把配置刷到「不早于一个 TTL」。

use crate::adapters::now_ms;
use crate::server::middleware::HEALTH_PATH;
use crate::server::state::AppState;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

/// 🔴 **它必须跑在鉴权之前**：`/v1` 的鉴权口令就在配置里，顺序反不过来。
/// 反过来的后果是「面板撤销了口令，被撤销的那把还能再用一个 TTL」。
///
/// 刷新次数是 **TTL 界定**的（默认 30 秒 ⇒ 2,880 次/天/副本），
/// **与请求数无关、与是否鉴权无关** —— 这是它敢挂全局的前提。
///
/// ⚠️ `/health` 豁免：镜像 HEALTHCHECK 每 30 秒一次，
/// 不该变成每 30 秒一次存储读。
pub async fn config_refresh(State(st): State<AppState>, req: Request, next: Next) -> Response {
    if req.uri().path() != HEALTH_PATH {
        st.config.ensure_fresh(now_ms()).await;
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::get;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn app(st: AppState) -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .route("/v1/x", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                st.clone(),
                config_refresh,
            ))
            .with_state(st)
    }

    async fn hit(app: &Router, uri: &str) {
        app.clone()
            .oneshot(HttpRequest::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
    }

    /// 🔴 **`/health` 一次存储读都不许产生。**
    ///
    /// 回归对象：HEALTHCHECK 每 30 秒一次，挂上去就是每 30 秒一次存储读 ——
    /// 而这条端点本来的承诺就是「零存储 I/O」。
    #[tokio::test]
    async fn health_is_exempt_from_config_refresh() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        let app = app(state);
        let before = counts.snapshot().0;
        for _ in 0..20 {
            hit(&app, "/health").await;
        }
        assert_eq!(counts.snapshot().0, before, "/health 不许碰存储");
    }

    /// 别的路径会触发刷新，**但被 TTL 摊平**：20 次请求只读一次。
    #[tokio::test]
    async fn other_paths_refresh_but_the_ttl_flattens_it() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { state, .. } = state_with_storage(counting).await;
        let app = app(state);
        let before = counts.snapshot().0;
        for _ in 0..20 {
            hit(&app, "/v1/x").await;
        }
        let after = counts.snapshot().0;
        assert!(
            after - before <= 1,
            "刷新次数该由 TTL 界定，而不是由请求数：读了 {} 次",
            after - before
        );
    }
}
