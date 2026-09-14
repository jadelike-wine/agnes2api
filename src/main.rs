//! 进程入口。**只做装配与 fail-closed，逻辑一行都不许写在这里。**
//!
//! 逻辑全在 lib crate 里（见 [`agnes2api`] 的模块头），这样集成测试才能直接调内部
//! 函数而不是只能打 HTTP 端口。

use agnes2api::adapters::logger_console::ConsoleLogger;
use agnes2api::adapters::storage_file::{FileStorage, StoreIntegrity};
use agnes2api::cli;
use agnes2api::ports::logger::{LogEntry, LogLevel, Logger};
use agnes2api::ports::storage::Storage;
use agnes2api::server;
use agnes2api::server::wire::{BuildOptions, build_app};
use clap::Parser;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    // 1. CLI。**不设 default_value**，默认值在用完所有层之后才兜底（见 `cli.rs`）。
    let args = cli::Cli::parse();

    // 2. 日志。必须在**任何一条可能要报错的路径之前**初始化，
    //    否则「起不来」的原因会丢在没人看得见的地方 —— 而 `docker logs` 是运维的第一现场。
    init_tracing();

    // 3. fail-closed 的唯一出口。
    //    ⚠️ 任何一条起不来的路径都走这里：打一行人能读的原因 + `exit(1)`。
    //    **两类错误（运维配错 / 代码 bug）同样让进程起不来，这是 Docker 形态正确的
    //    fail-fast** —— 容器起不来是运维立刻看得见的信号。
    if let Err(msg) = run(args).await {
        tracing::error!("{msg}");
        std::process::exit(1);
    }
}

async fn run(args: cli::Cli) -> Result<(), String> {
    let env = cli::env_snapshot();
    let data_dir = cli::data_dir(&args, &env);
    let port = cli::port(&args, &env)?;

    let logger: Arc<dyn Logger> = Arc::new(ConsoleLogger);
    let storage = FileStorage::new(&data_dir);

    // 上一次崩溃留下的孤儿临时文件。只删够老的，绝不误删别人正在写的。
    let swept = storage.cleanup_orphan_tmp().await;
    if swept > 0 {
        logger.log(
            LogEntry::new(
                LogLevel::Info,
                "storage.tmp_swept",
                "清理了上次留下的临时文件",
            )
            .with("count", swept),
        );
    }

    // 🔴 坏文件体检：**绝不静默当空**。
    //    备份之后**原件原地不动**，于是此后每一次读都继续如实失败 ——
    //    而写是「先读后写」，读不动就根本走不到写，坏文件不会被一次写覆盖掉。
    //    ⚠️ 这里**不拒绝启动**：让容器起来、让运维 curl 得到原因，
    //       比崩溃重启循环好排障。
    match storage.preflight().await {
        StoreIntegrity::Missing | StoreIntegrity::Ok => {}
        StoreIntegrity::Corrupt { backup } => logger.log(
            LogEntry::new(
                LogLevel::Error,
                "storage.corrupt",
                "数据文件解析不动，已原样备份；网关不会用空数据覆盖它，请人工处置后重启",
            )
            .with("path", storage.path().display().to_string())
            .with("backup", backup.display().to_string()),
        ),
    }

    let storage: Arc<dyn Storage> = Arc::new(storage);

    // 装配。🔴 **缺 `GATEWAY_TOKEN` 在这里拒绝服务**，message 逐字来自 core，
    //       五语言部署文档的故障排查条目引的就是那句原文。
    let built = build_app(
        &env,
        storage,
        BuildOptions {
            probe_storage: true,
        },
        logger.clone(),
    )
    .await
    .map_err(|e| e.to_string())?;

    // `0.0.0.0`：容器里必须监听所有接口，只听回环的话端口映射打空。
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    server::serve(&env, addr, built.router, logger)
        .await
        .map_err(|e| format!("{e:#}"))
}

/// 日志初始化。
///
/// 用 `registry` 而不是最简的 `fmt::init()`：P1.5 要在这里挂一个自定义捕获层
/// （面板的实时日志），**现在先按那个形状留好**，届时不用改初始化的写法。
fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    // 默认 `info`：`debug` 会把每一次转发的细节打进 `docker logs`，
    // 而那里同时是运维排障的第一现场 —— 刷爆它等于把别的线索一起冲掉。
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
}
