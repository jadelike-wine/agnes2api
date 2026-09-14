//! 全局 `x-content-type-options: nosniff`。

use axum::extract::Request;
use axum::http::{HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;

/// 给**每一条**响应加 nosniff。
///
/// 🔴 **必须在 `next()` 之后加。** 写在之前时只对框架自己构造的 JSON 响应生效，
/// 而 handler 直接返回裸 `Response` 的那一族 —— **流式转发、dispatcher 的错误响应、
/// 静态面板的 200/301/304/404、兜底 404/405** —— 整条头被静默丢掉。
///
/// 🔴 **它刻意覆盖非 `/admin` 路径。** `security-headers` 那 10 格判据专门抽查了
/// 转发路径：只给管理面加 nosniff 的实现能让那族判据里的一半全绿，
/// 而真正危险的恰恰是**转发路径**把上游的 content-type 带成可渲染的同源文档。
///
/// **刻意只加 nosniff，不加 CSP / X-Frame-Options**：后两条防的是
/// 「文档被渲染 / 被套 iframe」，而 `/v1/*` 与 `/health` 从不返回文档；
/// 面板那棵树的全套安全头归它自己（P2）。
///
/// ⚠️ **nosniff 挡不住「显式声明为 `text/html`」那一半** ——
/// 那一半由 [`crate::pool::dispatcher::clamp_content_type`] 负责。两者缺一不可。
pub async fn nosniff(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;

    /// 🔴 **裸 `Response` 的那一族也必须带上头。**
    ///
    /// 回归对象：写在 `next()` **之前**的实现对这一族静默失效，
    /// 而转发路径返回的正是裸 `Response`。
    #[tokio::test]
    async fn even_a_hand_built_response_gets_the_header() {
        let app = Router::new()
            .route(
                "/raw",
                get(|| async {
                    let mut r = Response::new(Body::from("<html>x</html>"));
                    *r.status_mut() = StatusCode::IM_A_TEAPOT;
                    r
                }),
            )
            .layer(axum::middleware::from_fn(nosniff));
        let res = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/raw")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    }

    /// 连兜底 404 也带（它同样是「框架之外」的那一族）。
    #[tokio::test]
    async fn the_fallback_response_gets_it_too() {
        let app = Router::new()
            .route("/x", get(|| async { "hi" }))
            .layer(axum::middleware::from_fn(nosniff));
        let res = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    }

    /// **只加这一条**，不顺手加 CSP / X-Frame-Options（那是面板那棵树的事）。
    #[tokio::test]
    async fn nothing_else_is_added() {
        let app = Router::new()
            .route("/x", get(|| async { "hi" }))
            .layer(axum::middleware::from_fn(nosniff));
        let res = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/x")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(res.headers().get("content-security-policy").is_none());
        assert!(res.headers().get("x-frame-options").is_none());
    }
}
