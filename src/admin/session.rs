//! `GET /admin/api/session` —— **登录探针**（02 §8.3）。
//!
//! 它是闸背后的第一个端点，面板每次进站都打它一次：口令对不对，全靠它的状态码。
//!
//! # 🔴 **越薄越好，一格与部署有关的东西都不回**
//!
//! 响应体**恰好两格**：`ok` 与 `version`。
//! 池子大小、有没有接对外密钥、Tier-2 开没开 —— 一个都不许加进来，理由是硬的：
//! 这条端点的**每一个字节都会在口令验证通过的那一瞬间被交出去**，
//! 而它同时是唯一一条「面板还没拿到任何别的授权状态」时就会被打的端点。
//! 形态分支要什么，去打 [`crate::admin::capabilities`]（那一条是**专门**回答这个问题的，
//! 而且已经有 8 块结构与一整族判据钉着）。
//! ⇒ 这里多加一格，就是把同一份知识开出第二个出口，
//!    而前端要面对的下一个问题是「读哪一个」—— 那个问题不该存在。
//!
//! # 🔴 **零存储读**
//!
//! 理由与 [`crate::admin::capabilities`] 逐字相同：面板每次进站必调，
//! 让它读一次存储就等于给每次登录加一次整份 `store.json` 的读 + 解析。
//! 这里**结构上**做不到去读 —— 它连 `AppState` 都不取。

use axum::Json;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// 注册用的子树内路径。🔴 `app.rs` 的 `.route()` 从这里取，不许手抄字面量。
pub const SESSION_SUBPATH: &str = "/session";

/// 线格式。**字段顺序 = 序列化顺序**，见 `server::errors` 文件头那段。
#[derive(Serialize)]
struct Wire {
    /// 恒 `true`。
    ///
    /// ⚠️ **不是「这个网关健康吗」**（那是 `/health`），是「你手上这把口令过了闸」。
    /// 闸没过的请求**根本到不了这个函数** ⇒ 结构上不存在 `ok: false` 的响应。
    ok: bool,
    version: &'static str,
}

/// `GET /admin/api/session`。**零存储读、零网络。**
pub async fn session() -> Response {
    Json(Wire {
        ok: true,
        // 版本号取自唯一真源（`./VERSION` → `CARGO_PKG_VERSION`），不另存常量。
        version: crate::VERSION,
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::{CountingStorage, MemoryStorage};
    use crate::server::app::build_router;
    use crate::server::test_support::{TestApp, state_with_storage};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    const ADMIN: &str = "admin-token-0123456789abcdef";

    async fn raw() -> String {
        let res = session().await;
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(b.to_vec()).unwrap()
    }

    /// 契约逐字：`200 { "ok": true, "version": "…" }`。
    #[tokio::test]
    async fn it_answers_the_documented_two_field_body() {
        let raw = raw().await;
        assert_eq!(
            serde_json::from_str::<Value>(&raw).unwrap(),
            json!({ "ok": true, "version": crate::VERSION })
        );
        // 键序也是契约 —— `Value` 是 `BTreeMap`，顺序只能对着原始字节比。
        assert!(
            raw.find("\"ok\"").unwrap() < raw.find("\"version\"").unwrap(),
            "键序：{raw}"
        );
    }

    /// 🔴 **顶层键恰好两格**：它是登录闸背后的第一条端点，越薄越好。
    ///
    /// 这一格拦的是「顺手把池子大小 / 有没有接对外密钥挂上来」——
    /// 那些东西归 `capabilities`，在这里多开一个出口就是给前端造一个
    /// 「读哪一个」的问题。
    #[tokio::test]
    async fn it_leaks_nothing_about_the_deployment() {
        let v: Value = serde_json::from_str(&raw().await).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["ok", "version"], "多一格就是多一个出口：{v}");
    }

    /// 🔴 **走真路由**：口令对 ⇒ 200；口令不对 ⇒ 401（而不是 404）。
    ///
    /// ⚠️ 这一格必须打真实的嵌套路由：只调 [`session`] 函数本身是**看不见闸的**，
    /// 而「这条端点注册在闸之后」正是它唯一的安全性质。
    #[tokio::test]
    async fn it_sits_behind_the_admin_gate() {
        let TestApp { mut state, .. } = state_with_storage(Arc::new(MemoryStorage::new())).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state);

        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/api/session")
                    .header("x-admin-key", ADMIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let b = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&b).unwrap()["ok"],
            json!(true)
        );

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/admin/api/session")
                    .header("x-admin-key", "admin-token-WRONG-0123456789")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    /// 🔴 **零存储 I/O。** 数的是**精确次数**：面板每次进站都会打它。
    #[tokio::test]
    async fn it_never_touches_storage() {
        let counting = Arc::new(CountingStorage::new(Arc::new(MemoryStorage::new())));
        let counts = counting.counts.clone();
        let TestApp { mut state, .. } = state_with_storage(counting).await;
        state.admin_token = Some(Arc::new(ADMIN.to_string()));
        let app = build_router(state);

        // ⚠️ 打**真路由**而不是直调 handler：直调看不见中间件那一侧的存储读，
        //    而「登录探针便宜」这句话说的是整条请求路径。
        //    ⇒ 先让配置刷新那一次发生掉（它是每条请求都可能做的事，不是本端点的账）。
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/admin/api/session")
                    .header("x-admin-key", ADMIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let before = counts.snapshot();
        for _ in 0..10 {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/admin/api/session")
                        .header("x-admin-key", ADMIN)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
        }
        assert_eq!(counts.snapshot(), before, "登录探针必须零存储 I/O");
    }
}
