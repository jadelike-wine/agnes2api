# agnes2api —— 多协议中转网关，后端 Agnes。单一 Docker 镜像，无前端构建步骤。
#
# ── 为什么没有 frontend-builder 阶段 ────────────────────────────────────────
# 面板（`admin-ui/`）是手写静态资源，由 `rust-embed` 在**编译期**直接读原文件嵌进二进制
# （见 `src/webui/mod.rs`）。没有打包器、没有 `dist/`，因此「产物与源一字不差」这件事
# 在结构上不可能不成立 —— 也就不需要一个 node 阶段，更不需要为它钉 pnpm 版本。

# ---- Rust 构建（交叉编译）------------------------------------------------
# `--platform=$BUILDPLATFORM` 让 builder 永远在构建机上**原生**跑，再按 TARGETARCH 交叉编出
# 目标架构二进制。不这么做的话，多架构构建会在 QEMU 里模拟编译整棵 Rust 依赖树，慢 5–10 倍。
#
# 🔴 **`-bookworm` 必须钉死，且必须与下面的运行镜像同代。** 二进制链接时按 builder 里的
# libc 头定符号版本，运行时却跑在运行镜像的 glibc 上 —— builder 比运行镜像新就会缺符号，
# 表现是「启动即 `version GLIBC_x.yy not found`」，而那句话离真因很远。
# 裸 `rust:1-slim` 已随上游滚到更新的 Debian，所以**不能不写后缀**。换运行镜像时必须同步改这里。
#
# ⚠️ 与 kiro2api 的一处**真实差异，别照抄它的 Dockerfile**：那边默认走 native-tls
#（vendored OpenSSL），因此需要 `perl` + `make`，还会踩 glibc 2.38 才有的 `strlcpy`。
# 本仓是**裁定 K：只用 rustls**（TLS 指纹对抗是 kiro 对它自己上游风控的对抗，
# 在 agnes 这一侧没有证据），于是那两样都不需要、那颗地雷也不存在。
# 仍然要交叉 gcc 与目标 libc 头：rustls 的 crypto 后端 `ring` 带 C/asm，要真编译和链接。
FROM --platform=$BUILDPLATFORM rust:1-slim-bookworm AS builder
ARG TARGETARCH
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
      pkg-config gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
# arm64 目标的链接器 / C 编译器 / 归档器都要指向交叉工具链。
# `AR` 也要给：用宿主 `ar` 会产出 x86 归档，而错误要到链接阶段才暴露。
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar

COPY Cargo.toml Cargo.lock ./
COPY src ./src
# `build.rs` 在编译期捕获 rustc 版本注入 `AGNES_RUST_VERSION`（面板「服务器信息」那一格读它），
# 必须在 `cargo build` 之前拷进来。
COPY build.rs ./
# 🔴 **面板必须在 `cargo build` 之前进构建上下文**：`rust-embed` 是编译期读盘，
# 少了这一行不会报「找不到文件」，而是嵌进一棵**空树** —— 镜像跑起来 `/admin` 回 404，
# 而所有 cargo 门禁在本地都是绿的。
COPY admin-ui ./admin-ui
COPY VERSION ./VERSION

# `--locked` 是刻意的：CI 上不许因为某个依赖发了新版就悄悄换一棵依赖树。
RUN case "$TARGETARCH" in \
      amd64) RUST_TARGET=x86_64-unknown-linux-gnu ;; \
      arm64) RUST_TARGET=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" && exit 1 ;; \
    esac && \
    cargo build --release --locked --target "$RUST_TARGET" && \
    cp "target/$RUST_TARGET/release/agnes2api" /build/agnes2api

# ---- 运行镜像 ------------------------------------------------------------
# 不钉 platform ⇒ 继承 TARGETPLATFORM，产出对应架构的最终镜像。
FROM debian:bookworm-slim
# `ca-certificates`：要打 HTTPS 上游。`wget`：HEALTHCHECK 用。
# `gosu`：见 entrypoint —— 非 root 镜像 + bind mount 必须有它，否则挂载目录属主对不上、
# 网关起来就是 `storage.writable=false`，而那个症状看着像代码 bug。
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates wget gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 1000 appuser
WORKDIR /app
COPY --from=builder /build/agnes2api /usr/local/bin/agnes2api
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh && mkdir -p /app/data && chown -R appuser:appuser /app
EXPOSE 8080

# 🔴 探活端口必须按与应用**完全相同**的优先级解析（`src/cli.rs`：`CLI > PORT > 8080`）。
# 少了 `PORT` 这一层，用户按文档改了端口之后健康检查仍打 8080，容器**永远 unhealthy**，
# 而网关本身一切正常 —— 那是最费时间的一类假故障。
# ⚠️ `${PORT:-8080}` 里的 `:-` 把**空串**也当没设，与 `src/cli.rs` 的 `or_unset` 同口径；
# 两边口径不一致的话，`PORT=` 会让 compose 发布 8080、容器内却监听到别处。
HEALTHCHECK --interval=30s --timeout=10s --start-period=20s --retries=3 \
    CMD wget -q -O /dev/null "http://localhost:${PORT:-8080}/health" || exit 1

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["agnes2api"]
