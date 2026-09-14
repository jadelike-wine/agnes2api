#!/usr/bin/env bash
# 版本同步：`VERSION` 是唯一真源，本脚本把它抄进 `Cargo.toml`，再刷 `Cargo.lock`。
#
# 🔴 **三处，不是四处**（规格 §7 矛盾 8）：VERSION / Cargo.toml / Cargo.lock。
#    代码一律 `env!("CARGO_PKG_VERSION")`，**不另存 Rust 侧版本常量** ——
#    多一处就多一个会漂的地方，而「/health 的版本号骗人」这个形态踩过。
#
# 用法：
#   scripts/set-version.sh 0.2.0   # 写新版本
#   scripts/set-version.sh         # 不改版本，只把 Cargo.toml / Cargo.lock 拉回与 VERSION 一致
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# -ge 1 ]]; then
  new="$1"
  # 只认 x.y.z：带 v 前缀、带后缀的写法一律拒掉，否则 Cargo.toml 会被写成非法值
  # 而错误要到下一次 cargo 命令才报出来。
  [[ "$new" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "版本号必须形如 1.2.3，收到：$new" >&2; exit 1; }
  printf '%s\n' "$new" > VERSION
fi

version="$(tr -d '[:space:]' < VERSION)"
[[ -n "$version" ]] || { echo "VERSION 是空的" >&2; exit 1; }

# 只改 [package] 段里的第一处 version =，不碰依赖的版本号。
awk -v v="$version" '
  /^\[/ { in_pkg = ($0 == "[package]") }
  in_pkg && !done && /^version *=/ { print "version = \"" v "\""; done = 1; next }
  { print }
' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

# 刷 Cargo.lock 里本包自己那一条。--offline 失败就退回联网，两种环境都能用。
cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null

echo "版本已同步为 $version（VERSION / Cargo.toml / Cargo.lock）"
