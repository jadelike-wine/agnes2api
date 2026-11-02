//! agnes2api —— Agnes-AI 上游的多协议中转网关（Rust 版）。
//!
//! # 这个 crate 为什么是 lib 而不是只有 main
//!
//! 全部逻辑住在 lib crate 里，`src/main.rs` 只有装配。**这条照抄 kiro2api**
//! （03 §2「边界规矩」第 5 条）：否则 `tests/` 下的集成测试只能打 HTTP 端口，
//! 而本仓有一整族判据要观察的是「这次请求产生了几次存储 `put`」这类
//! **进程内可观测量**，打端口根本看不见。
//!
//! # 模块树与它们的边界（边界 = 这个模块不许依赖谁）
//!
//! | 模块 | 职责 | 不许依赖 |
//! |---|---|---|
//! | [`cli`] | clap 参数。**不做任何默认值推导** | — |
//! | [`ports`] | 三条 trait + 它们的 DTO。不含任何实现、任何业务 | 除 serde_json/bytes/futures 外的一切 |
//! | [`adapters`] | 端口的生产实现。文件 IO、reqwest、时钟、随机**全住在这里** | `core` / `server` / `admin` |
//! | [`core`] | 纯函数与纯数据。🔴 **零 IO**：不读时钟（收 `now: i64`）、不随机、不定时、不网络、不读 env、不碰存储 | `ports` / `adapters` / `tokio` / `reqwest` / `std::fs` / `std::env` / `SystemTime` |
//! | [`pool`] | key 池的存储形态与转发热路径 | `server` / `admin` |
//! | [`apikey`] | 我们签发的 `sk-*` 的存储侧 | `pool` |
//! | [`server`] | 装配与闸。业务一律在各子模块 | `registrar` 的实现 |
//! | [`admin`] | 管理面 REST。handler 薄，逻辑在 `core::admin` | `adapters` 的具体类型（只经端口） |
//! | [`registrar`] | 注册机：装载、补池一轮、两条邮箱通道、域名台账、跨副本锁 | 一切（它是叶子） |
//!
//! 三条从 kiro2api 学来、必须原样落地的规矩：
//!
//! 1. **只有一条 relay 内核**（将来的 `pool::dispatcher::dispatch`）。四条协议路由是
//!    它的适配器，不许任何协议自己拉一条到上游的链路。
//! 2. **`AppState` 是唯一的状态载体**：全 `Arc`、克隆廉价、`#[derive(Clone)]`，
//!    协议组与管理组两组路由 `.with_state()` **同一份**，不建第二份存储。
//! 3. **原子写 + 停机显式刷盘**是通用基建，住在 [`adapters::storage_file`]，
//!    它不知道任何业务。
//!
//! # 📜 「其余模块是占位」那一段已经作废
//!
//! 它写于 P1 骨架那一轮。今天转发、管理面、面板、注册机、用量都已落地，
//! 仓里**不再有**「有边界、无实现」的模块。
//!
//! ⚠️ 那一段立的规矩照旧有效，而且比那时更要紧：
//! **一律不导出假函数、不返回假数据 —— 空壳比缺失更坏。**
//! 今天它的形态是：没配的能力回一个**说得出原因**的状态码
//!（`503 not_wired` / `200 tier:"off"` / `409` 三档各有各的含义），
//! 而不是回一个长得像成功的空对象。

// ── 全仓 clippy allow 清单（集中声明，🔴 禁止在函数上零散 `#[allow]`）─────────
// CI 跑 `cargo clippy --all-targets -- -D warnings`，所以「允许什么」必须在这里
// 一处说清，否则下一个人只会在报错的那一行上贴一个 allow 了事。
//
// axum 的 handler 错误类型天然较大（错误信封自带 message/params），
// 统一装箱会把 `Box<...>` 污染进大量签名，而这些类型全部只在错误路径上构造一次。
#![allow(clippy::result_large_err)]
// `adapters::sleep_ms` 刻意写 `impl Future + Send` 而不是 `async fn`：
// Workers（wasm）那支返回 `worker::send::SendFuture`，它包着 `Rc` 的 JS 对象 —
// 只有「显式声明 `Send`」这个签名能让两种形态共用同一个调用点。
// `async fn` 的返回类型由函数体决定，给不出那个 bound。
#![allow(clippy::manual_async_fn)]

pub mod adapters;
pub mod admin;
pub mod apikey;
#[cfg(not(target_arch = "wasm32"))]
pub mod cli;
pub mod core;
pub mod pool;
pub mod ports;
pub mod registrar;
pub mod server;
/// `/admin` 那棵静态树与 `/favicon.ico`（P2b）。
///
/// 🔴 **整棵树跟着 `/admin` 一起存在或消失**：没配 `ADMIN_TOKEN` 时这几条路由
/// **根本不注册** —— 不泄漏「这里有个后台」这条性质在 P2b 里没有被破掉。
pub mod webui;
#[cfg(all(target_arch = "wasm32", feature = "worker"))]
pub mod worker;

// 🔴 两条部署形态在**同一个 crate** 里共存，靠 `target_arch` + feature 切开：
//
// | 形态 | target | feature | 入口 | 存储 | 出站 |
// |---|---|---|---|---|---|
// | Docker / 裸机 | 非 wasm32 | （默认） | `main.rs` + `server::serve` | `FileStorage`（store.json） | `ReqwestFetcher` |
// | Cloudflare Workers | wasm32 | `worker` | `worker::fetch` | `D1Storage`（D1 表） | `CfFetcher`（worker::Fetch） |
//
// wasm 目标**不带** `worker` feature 时，下面这条直接把构建打停：
// 半套依赖是编得过的，编过之后会产出一个「没有入口、没有存储」的 wasm，
// 那比「编不过」更难排障。
#[cfg(all(target_arch = "wasm32", not(feature = "worker")))]
compile_error!(
    "wasm32-unknown-unknown 必须带 `--features worker` 构建；\
     原生（Docker）构建请直接 `cargo build`（不带该 feature）。"
);

/// 测试临时目录的统一出口。**不进产物**（`#[cfg(test)]`）。
///
/// 这个模块是照抄 kiro2api 的（03 §10.3），它挡住的是一个实测过的运维问题：
/// 各模块测试各自 `std::env::temp_dir().join(...)` 建完从不删，
/// 跑一轮完整测试会在 `/tmp` 顶层留下成千个永不回收的目录。
#[cfg(test)]
pub mod test_tmp;

/// 本网关的版本。
///
/// 🔴 **唯一来源是 `Cargo.toml` 的 `version`，而它是 `./VERSION` 的副本**
/// （由 `scripts/set-version.sh` 同步）。**不许另存一个 Rust 常量**：
/// 那会变成第四处会漂的地方，而「/health 的版本号骗得了人」这个形态踩过。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 构建这份二进制用的 rustc 版本。由 `build.rs` 注入，解析失败时是 `"unknown"`。
/// 消费者是 P2 面板的「服务器信息」那一格。
pub const RUST_VERSION: &str = env!("AGNES_RUST_VERSION");

#[cfg(test)]
mod version_tests {
    /// 🔴 `VERSION` 文件与 `Cargo.toml` 必须一致。
    ///
    /// 这是「三处一致」里机器能查的那两处（第三处 `Cargo.lock` 由
    /// `cargo build` 自己保证）。它用 `include_str!` 读磁盘上的 `VERSION`，
    /// 而 `CARGO_PKG_VERSION` 来自 `Cargo.toml` —— **两侧不互相推导**，
    /// 否则就是同义反复（04 §3.3 第 ⑥ 条假阳性）。
    ///
    /// 回归对象：发版时改了 `VERSION` 忘了跑 `scripts/set-version.sh`，
    /// 于是 `/health` 报的是旧版本号 —— 而运维正是靠它确认「线上是不是最新」。
    #[test]
    fn version_file_matches_cargo_manifest() {
        let on_disk = include_str!("../VERSION").trim();
        assert_eq!(
            on_disk,
            super::VERSION,
            "VERSION 文件与 Cargo.toml 不一致，跑一下 scripts/set-version.sh"
        );
    }

    /// `build.rs` 的 best-effort 约定：**永远有值**，最差是 `"unknown"`。
    #[test]
    fn rust_version_is_always_populated() {
        assert!(!super::RUST_VERSION.is_empty(), "build.rs 必须保证它非空");
    }
}
