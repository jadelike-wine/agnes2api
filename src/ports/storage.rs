//! 键值存储端口。

use async_trait::async_trait;
use serde_json::Value;
use std::fmt;

/// 存储错误。
///
/// **刻意只有一个不透明的 message，不做错误码分类**：调用方对存储失败只有两种处置——
/// 「如实抛」或「按本数据的窄化方向降级」，而选哪种取决于**被读的是哪把键**
/// （见 `core::narrow` 的方向表），不取决于失败的原因是 EACCES 还是 JSON 坏了。
///
/// 🔴 `Display` 的内容**绝不许进未鉴权响应体**：`/health` 与转发失败信封里的
/// `reason` 一律是固定枚举串，永不由这里的 message 派生。
#[derive(Debug, Clone)]
pub struct StorageError(pub String);

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StorageError {}

impl StorageError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

pub type StorageResult<T> = Result<T, StorageError>;

/// 键值存储。
///
/// # 为什么 `get` 回的是 `Value` 而不是泛型 `T: DeserializeOwned`
///
/// 🔴 **这是裁定 B，必须在写第一个持久化 struct 之前理解。**
///
/// TS 里 `disabled: "true"`（字符串，不是布尔）被 `JSON.parse` 原样带进来，
/// 代码显式判真值性，**那条记录仍然可用**。Rust 的 `serde` 遇到 `"true"` 给 `bool`
/// 字段会让**整条记录反序列化失败** ⇒ 这把 key 直接从池子里消失，而面板上看不出
/// 发生过什么。
///
/// ⇒ 存储层只回 `serde_json::Value`（**不带泛型、不做类型推导**），
/// 由 `core::narrow` 的 `narrow_*` 函数按《存储窄化方向表》逐处窄化 ——
/// **方向逐处不同，不许统一成一种**：`pool:index` 就地剔除坏元素、`key:<id>` 的
/// `stats` 逐字段补零、`config` 字段级降级、`apikeys` 🔴 **绝不当空表**、
/// `event:*` 只有 `ts` 坏才丢……
///
/// 连带纪律：持久化 DTO **只派生 `Serialize`**，不派生 `Deserialize`
/// （或仅在 `#[cfg(test)]` 下派生）。
#[async_trait]
pub trait Storage: Send + Sync {
    /// 读一把键。**键不存在与键已过期都回 `Ok(None)`**，两者对调用方无差别。
    async fn get(&self, key: &str) -> StorageResult<Option<Value>>;

    /// 写一把键。
    ///
    /// `expires_at`：可选的**绝对**过期时刻（epoch 毫秒，与全仓 `now()` 同一个时钟
    /// 基准 —— 不是相对时长）。`None` = 永不过期。
    ///
    /// 🔴 **有界性必须是存储自己的性质**，不能靠调用方按某种节奏顺手 delete 一个
    /// 算出来的键：那种方案的有界性依赖「落盘节奏恰好规律」这个前提，稀疏落盘时
    /// 前提不成立，清理率会跌到 0。⇒ 传了 `expires_at` 的键，实现必须保证过期之后
    /// [`Storage::get`] **与** [`Storage::list`] 都不再看得到它。
    ///
    /// ⚠️ 这条 Cloudflare KV 时代的纪律在 Docker-only 形态下**照旧成立**，
    /// 只是分母从「KV 每日写配额」换成了「`store.json` 的整份重写开销」。
    async fn put(&self, key: &str, value: Value, expires_at: Option<i64>) -> StorageResult<()>;

    /// 删一把键。**键不存在不是错误**（删除是幂等的）。
    async fn delete(&self, key: &str) -> StorageResult<()>;

    /// 列出所有以 `prefix` 开头、且未过期的键。
    ///
    /// ⚠️ 它在 `FileStorage` 上是**整份文件读 + 解析**，与 `get` 同价。
    /// 转发热路径上被判据钉死为**零次调用**（走进程级快照）。
    async fn list(&self, prefix: &str) -> StorageResult<Vec<String>>;
}
