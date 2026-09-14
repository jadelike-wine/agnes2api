//! 中间件。**顺序本身是规格**，装配在 [`crate::server::app::build_router`]。
//!
//! # ⚠️⚠️ axum 的方向与 Hono 相反，别照抄 TS 的行序
//!
//! Hono 是 `app.use(a); app.use(b);` ⇒ **a 在外、b 在内**（写的顺序就是执行顺序）。
//! axum 的 `Router::layer` 是**包裹**：`r.layer(a).layer(b)` ⇒ `b(a(r))`
//! ⇒ **最后加的那个在最外层、最先执行**。
//!
//! ⇒ 照抄 TS 的行序会得到一条**方向相反**的中间件链，而它的失效方式是**静默的**
//! （鉴权跑在配置刷新之前 ⇒ 读到的是上一个 TTL 的口令；
//! nosniff 跑在里面 ⇒ 裸响应那一族整条头被丢掉，全都返 200 而不是报错）。
//!
//! 本仓的落法：**在 `app.rs` 里按由内向外的顺序写 `.layer()`，并在那里贴一张
//! 「期望的执行顺序」表**，再用一格判据钉住实际顺序。
//!
//! # 四条豁免 `/health` 的理由（它是唯一同时被三条豁免的路径）
//!
//! 镜像 HEALTHCHECK 每 30 秒打一次。它**不该**变成：
//! - 每 30 秒一次配置存储读（`config_refresh`）；
//! - 每 30 秒一次事件落盘（`log_flush`）；
//! - 一个从不产生任何用量的调用方去消费用量统计的写预算（`usage_flush`）。

pub mod admin_auth;
pub mod auth;
pub mod config_refresh;
pub mod flush;
pub mod nosniff;

/// `/health` 的路径。三条刷盘 / 刷新类中间件都认它。
///
/// ⚠️ **用常量而不是各写各的字面量**：改路径时漏掉一处的现象是
/// 「健康检查悄悄开始打存储」，而那在任何监控上都看不出来。
pub const HEALTH_PATH: &str = "/health";
