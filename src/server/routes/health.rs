//! `GET /health` —— 不鉴权、**零存储 I/O**。

use crate::server::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;

/// 存储不可写时对外的**固定**说明。
///
/// 🔴 **刻意不回显底层异常**（`EACCES: permission denied, open '/app/data/...'`）：
/// `/health` 是不鉴权端点，不该把内部路径与原始错误暴露给任何人。
/// 真正的异常写在容器日志里，排障看日志即可。
const UNWRITABLE_DETAIL: &str = "数据目录不可写，key 池无法持久化。Docker 部署常见于绑定挂载的宿主目录属主与容器内运行用户不一致，详见容器日志";

/// 响应体。**字段顺序即序列化顺序**，与 TS 版逐字节对齐。
#[derive(Serialize)]
struct HealthBody {
    status: &'static str,
    version: &'static str,
    storage: StorageBlock,
}

#[derive(Serialize)]
struct StorageBlock {
    writable: bool,
    /// 可写时**这一格根本不出现**（不是 `null`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'static str>,
}

/// 健康检查路由。
pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/health", get(health))
}

/// 只读一个内存里的布尔值。
///
/// 🔴 **零存储 I/O**：HEALTHCHECK 每 30 秒来一次也不会产生任何读写。
/// 这不是性能优化 —— 健康检查里写盘会让「磁盘满了」这类故障**自我放大**。
async fn health(State(state): State<AppState>) -> Response {
    let writable = state.health.status().writable;
    if writable {
        let body = HealthBody {
            status: "ok",
            version: crate::VERSION,
            storage: StorageBlock {
                writable: true,
                detail: None,
            },
        };
        return (StatusCode::OK, Json(body)).into_response();
    }

    // 🔴 **503 而不是 200**：镜像内置的 HEALTHCHECK 按响应是否 ok 判定，
    // 只有这样容器才会被标成 unhealthy。原实现里这种容器一直报 healthy，
    // **而每一次 API 调用都在失败**。
    let body = HealthBody {
        status: "degraded",
        version: crate::VERSION,
        storage: StorageBlock {
            writable: false,
            detail: Some(UNWRITABLE_DETAIL),
        },
    };
    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::core::storage_health::StorageHealth;
    use crate::server::app::build_router;
    use crate::server::state::AppState;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// ⚠️ 走**共用的**装配替身而不是手搓一份 `AppState`：
    /// 手搓的那一份会在每次给 `AppState` 加字段时各改一遍，
    /// 而改着改着就会出现「某一份的默认值和真实装配不一样」。
    async fn state_with(health: Arc<StorageHealth>, storage: Arc<CountingStorage>) -> AppState {
        let TestApp { mut state, .. } = state_with_storage(storage).await;
        state.health = health;
        state
    }

    async fn call(state: AppState) -> (StatusCode, Value) {
        let res = build_router(state)
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// 可写时：200 + 这个**逐字节**的形状。
    #[tokio::test]
    async fn ok_body_shape_is_exact() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let (status, body) = call(state_with(Arc::new(StorageHealth::new()), counting).await).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({ "status": "ok", "version": crate::VERSION, "storage": { "writable": true } })
        );
    }

    /// 🔴 **不可写时是 503，不是 200。**
    ///
    /// 回归对象：镜像 HEALTHCHECK 按响应是否 ok 判定。回 200 的话容器一直报
    /// healthy，而每一次 API 调用都在失败 —— 编排层永远不会把它换掉。
    #[tokio::test]
    async fn degraded_is_503_so_the_container_is_marked_unhealthy() {
        let health = Arc::new(StorageHealth::new());
        health.record(false, 1);
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let (status, body) = call(state_with(health, counting).await).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "degraded");
        assert_eq!(body["storage"]["writable"], false);
    }

    /// 🔴 **不回显底层异常**：它是不鉴权端点。
    ///
    /// 文案是固定的一句话，不含路径、不含 errno、不含任何原始错误文本。
    #[tokio::test]
    async fn degraded_detail_leaks_nothing() {
        let health = Arc::new(StorageHealth::new());
        health.record(false, 1);
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let (_, body) = call(state_with(health, counting).await).await;
        let detail = body["storage"]["detail"].as_str().unwrap();
        assert_eq!(detail, super::UNWRITABLE_DETAIL);
        for leak in ["EACCES", "/app/data", "permission denied", "store.json"] {
            assert!(!detail.contains(leak), "不鉴权端点泄漏了 {leak}: {detail}");
        }
    }

    /// 可写时**不出现** `detail` 这一格（不是 `null`）。
    #[tokio::test]
    async fn detail_is_absent_not_null_when_healthy() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let (_, body) = call(state_with(Arc::new(StorageHealth::new()), counting).await).await;
        assert!(body["storage"].get("detail").is_none());
    }

    /// 🔴🔴 **`/health` 零存储 I/O，四种操作全数。**
    ///
    /// 这一格用四个桶分开数的计数桩：上一版只数 `get`，而被违反的恰恰是**写**那一侧
    /// （有人在健康检查里顺手写了一次探针）。健康检查写盘会让「磁盘满了」自我放大。
    #[tokio::test]
    async fn health_touches_storage_exactly_zero_times() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let state = state_with(Arc::new(StorageHealth::new()), counting).await;

        // 连打三次：一次可能碰巧为零，三次还为零才说明它真的不碰存储。
        for _ in 0..3 {
            call(state.clone()).await;
        }
        assert!(
            counts.is_silent(),
            "/health 碰了存储: {:?}",
            counts.snapshot()
        );
    }

    /// 版本号来自 `CARGO_PKG_VERSION`，不是硬编码的第四处。
    #[tokio::test]
    async fn version_comes_from_the_single_source() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let (_, body) = call(state_with(Arc::new(StorageHealth::new()), counting).await).await;
        assert_eq!(
            body["version"].as_str().unwrap(),
            include_str!("../../../VERSION").trim()
        );
    }
}
