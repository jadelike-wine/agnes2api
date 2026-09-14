//! 构建脚本。**全部 best-effort，任何一环失败都不许让构建失败**（对标 kiro2api 03 §11.1）。
//!
//! 唯一职责：把构建用的 rustc 版本注入成一个编译期环境变量，给面板的
//! 「服务器信息」那一格展示用（P2 的消费者）。
//!
//! 纪律：解析不出来一律回落 `"unknown"`，因此下游 `env!("AGNES_RUST_VERSION")`
//! **始终有值**，不需要任何一处 `option_env!` 或 `unwrap_or`。

use std::process::Command;

fn main() {
    // cargo 会把当前用的编译器路径放在 RUSTC 里；拿不到就退回 PATH 上的 `rustc`。
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=AGNES_RUST_VERSION={version}");

    // 只有构建脚本本身变了才需要重跑：rustc 版本变了 cargo 本来就会重建全仓。
    println!("cargo:rerun-if-changed=build.rs");
}
