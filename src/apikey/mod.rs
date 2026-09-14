//! 我们**签发**的那一族 `sk-*` 的存储侧。纯函数那一半在 [`crate::core::keys::api_keys`]。
//!
//! ⚠️⚠️ **本仓有两个都叫 key 的东西，方向相反，别读混**：
//! - `core::types::KeyRecord` —— **上游** Agnes 的 key，我们拿它去调别人；
//! - `core::keys::api_keys::ApiKeyRecord` —— **我们签发**的子密钥，别人拿它来调我们。
//!
//! 本模块只做三件带 IO 的事：**签发（CSPRNG）/ 读表 / 乐观并发写表**。
//! 掩码、分档、`find_by_digest` 这些没有 IO 的一律在 `core` 那边。

/// `/admin/api/apikeys*` 那六条管理端点（02 §9.25 ~ §9.30）。
///
/// ⚠️ 它住在 `apikey/` 而不是 `admin/`：本组 handler 的全部逻辑就是
/// 「窄化请求 → 调本模块的 [`load`] / [`save`] / [`issue`] → 挑信封」，
/// 与存储侧同住一处才看得见「读到写之间那个窗口」的全貌。
pub mod admin;

use crate::core::keys::api_keys::{
    API_KEY_SECRET_BYTES, APIKEY_SECRET_PREFIX, ApiKeyRecord, ApiKeyTable, ApiKeyTableRead, digest,
    hint_of,
};
use crate::core::narrow::narrow_api_key_table;
use crate::ports::storage::{Storage, StorageError};
use crate::server::holders::apikey_holder::APIKEYS_KEY;

/// id 的字节数：6 字节 ⇒ 12 位十六进制（与 `ApiKeyRecord::id` 的文档一致）。
const ID_BYTES: usize = 6;

/// 写表可能的失败。**三档，各自对应一个不同的 HTTP 处置。**
#[derive(Debug, Clone)]
pub enum ApiKeyWriteError {
    /// 调用方手上的版本与存储里的对不上 ⇒ **409**。
    ///
    /// ⚠️ 这**不是 CAS**：读到写之间仍有一个窗口。这条机制把窗口从「整个人类操作时长」
    /// 压到「一次读写之间」，**残余窗口确实存在** —— 任何一处都不许把它说成「不会丢更新」。
    Stale { expected: i64, actual: i64 },
    /// 🔴 存储里那张表结构不认 ⇒ **拒绝落盘**（不是「当空表然后覆盖」）。
    ///
    /// 回归对象：当空表的后果是把幸存记录整份覆掉，**不可逆**。
    /// 拒绝写之后原字节原地不动，人工捞回随时可做。
    Unreadable,
    /// 存储本身出错。
    Storage(StorageError),
}

/// ⚠️ **手写而不是 derive**：`StorageError` 刻意没有 `PartialEq`
/// （它包的是一段人能读的说明，不是一个可比较的错误码）。
/// 这里按文本比，只为让判据能写 `assert_eq!`。
impl PartialEq for ApiKeyWriteError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Stale {
                    expected: a,
                    actual: b,
                },
                Self::Stale {
                    expected: c,
                    actual: d,
                },
            ) => a == c && b == d,
            (Self::Unreadable, Self::Unreadable) => true,
            (Self::Storage(a), Self::Storage(b)) => a.0 == b.0,
            _ => false,
        }
    }
}

impl Eq for ApiKeyWriteError {}

impl std::fmt::Display for ApiKeyWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale { expected, actual } => {
                write!(f, "表版本已变化：期望 {expected}，实际 {actual}")
            }
            Self::Unreadable => f.write_str("存储里的子密钥表结构不认，拒绝覆盖"),
            Self::Storage(e) => write!(f, "写子密钥表失败: {e}"),
        }
    }
}

impl std::error::Error for ApiKeyWriteError {}

/// 读一次表。**三档原样交出去**（`Absent` / `Ok` / `Invalid`）。
///
/// 🔴 调用方不许把 `Invalid` 折叠成 `Ok(empty)`。那一折就是那条不可逆事故。
pub async fn load(storage: &dyn Storage) -> Result<ApiKeyTableRead, StorageError> {
    let raw = storage.get(APIKEYS_KEY).await?;
    Ok(narrow_api_key_table(raw.as_ref()))
}

/// 乐观并发地写回一张表。
///
/// `expected_version` 是调用方读到的版本；落盘的版本是它 **+1**。
///
/// 写之前**必须回读一次**（那一次 `get` 本来就要付 —— 调用方拿到当前表才能改它，
/// 但它读到写之间可能有别人写过）。
pub async fn save(
    storage: &dyn Storage,
    keys: Vec<ApiKeyRecord>,
    expected_version: i64,
) -> Result<ApiKeyTable, ApiKeyWriteError> {
    let current = load(storage).await.map_err(ApiKeyWriteError::Storage)?;
    let actual = match &current {
        // 还没有表 ⇒ 版本按 0 算（`ApiKeyTable::empty()` 的起点）。
        ApiKeyTableRead::Absent => 0,
        ApiKeyTableRead::Ok(t) => t.version,
        ApiKeyTableRead::Invalid => return Err(ApiKeyWriteError::Unreadable),
    };
    if actual != expected_version {
        return Err(ApiKeyWriteError::Stale {
            expected: expected_version,
            actual,
        });
    }
    let table = ApiKeyTable {
        version: actual + 1,
        keys,
    };
    let value = serde_json::to_value(&table).map_err(|e| {
        // 结构体只含 String / i64 / Option，序列化失败只可能是 serde 本身出事。
        ApiKeyWriteError::Storage(StorageError::new(format!("序列化子密钥表失败: {e}")))
    })?;
    storage
        // 🔴 **不传 `expires_at`**：这张表一律不靠 TTL 收敛，
        //    让 TTL 兼任别的职责会让它整份蒸发（见 01 §1 的键表）。
        .put(APIKEYS_KEY, value, None)
        .await
        .map_err(ApiKeyWriteError::Storage)?;
    Ok(table)
}

/// 新签发一把密钥。返回 **（明文, 记录）**。
///
/// 🔴 **明文只在这一次出现**（以及记录里那个由用户拍板保留的 `secret` 格）。
/// 调用方把它放进 201 响应之后就该丢掉。
///
/// 🔴 **CSPRNG，绝不用非密码学随机源**：这把密钥是别人访问本网关的唯一凭据。
/// `getrandom` 直接走操作系统熵池（Linux 上是 `getrandom(2)`）。
///
/// # panic
///
/// 熵池取不到随机数时 panic。**这是有意的**：退化成一个可预测的密钥，
/// 比进程崩掉危险几个量级，而这条路径在 Linux 上不会失败。
#[must_use]
pub fn issue(name: String, now: i64, expires_at: Option<i64>) -> (String, ApiKeyRecord) {
    let secret = format!("{APIKEY_SECRET_PREFIX}{}", random_hex(API_KEY_SECRET_BYTES));
    let record = ApiKeyRecord {
        id: random_hex(ID_BYTES),
        name,
        // 明文落盘 —— **用户拍板的取舍**，代价写在 `ApiKeyRecord::secret` 的文档里
        // （面板被打穿 = 全部明文一次性泄漏；备份与快照的处置级别要跟着升）。
        secret: Some(secret.clone()),
        hash: digest(&secret),
        hint: hint_of(&secret),
        // 🔴 **缺席 = 启用**，刻意不写 `Some(false)`：那会产出一种存量记录里没有的形状。
        disabled: None,
        created_at: now,
        expires_at,
    };
    (secret, record)
}

/// `n` 字节 CSPRNG ⇒ 小写十六进制。
fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::getrandom(&mut buf).expect("系统熵池不可用；宁可崩也不签发可预测的密钥");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::storage_test_doubles::MemoryStorage;
    use crate::core::keys::api_keys::{API_KEY_HINT_LENGTH, find_by_digest};
    use serde_json::json;
    use std::collections::HashSet;
    use std::sync::Arc;

    /// 签发的明文形状：`sk-` + 32 位十六进制（16 字节 = 128 bit）。
    ///
    /// 期望值**手写字面量**，不从被测常量现算 —— 后者是同义反复。
    #[test]
    fn an_issued_secret_has_the_documented_shape() {
        let (secret, _) = issue("n".into(), 0, None);
        assert!(secret.starts_with("sk-"));
        assert_eq!(secret.len(), 3 + 32);
        assert!(secret[3..].bytes().all(|b| b.is_ascii_hexdigit()));
    }

    /// 🔴 **两把密钥不许相同**（CSPRNG 的可观测后果）。
    ///
    /// 1000 把全不同并不能证明随机源是密码学安全的 —— 但它能抓住
    /// 「忘了填充 buf」「用了固定种子」这两种真实会发生的写法。
    #[test]
    fn issued_secrets_and_ids_do_not_repeat() {
        let mut secrets = HashSet::new();
        let mut ids = HashSet::new();
        for _ in 0..1000 {
            let (s, r) = issue("n".into(), 0, None);
            assert!(secrets.insert(s), "签出了重复的明文");
            assert!(ids.insert(r.id), "签出了重复的 id");
        }
    }

    /// 记录里存的是**摘要**与**末四位**，并且和明文对得上。
    #[test]
    fn the_record_carries_a_digest_and_a_hint_that_match_the_secret() {
        let (secret, r) = issue("我的密钥".into(), 42, Some(99));
        assert_eq!(r.hash, digest(&secret));
        assert_eq!(r.hint.len(), API_KEY_HINT_LENGTH);
        assert!(secret.ends_with(&r.hint));
        assert_eq!(r.created_at, 42);
        assert_eq!(r.expires_at, Some(99));
        // 🔴 缺席 = 启用。写 `Some(false)` 会产出存量记录里没有的形状。
        assert_eq!(r.disabled, None);
    }

    /// id **不由密钥派生**：它会进 URL 与事件日志，不该携带密钥的任何函数值。
    #[test]
    fn the_id_is_not_derived_from_the_secret() {
        let (secret, r) = issue("n".into(), 0, None);
        assert!(!secret.contains(&r.id));
        assert!(!r.hash.contains(&r.id));
        assert_eq!(r.id.len(), 12);
    }

    /// 签发 → 落盘 → 读回来 → 用摘要查得到。**端到端那一条**。
    #[tokio::test]
    async fn an_issued_key_round_trips_through_storage() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        let (secret, rec) = issue("n".into(), 1, None);
        let t = save(mem.as_ref(), vec![rec.clone()], 0).await.unwrap();
        assert_eq!(t.version, 1, "第一次写落地的版本是 1");

        let ApiKeyTableRead::Ok(read) = load(mem.as_ref()).await.unwrap() else {
            panic!("应该读回一张表");
        };
        let found = find_by_digest(&read.keys, &digest(&secret)).expect("摘要查不到");
        assert_eq!(found.id, rec.id);
    }

    /// 🔴 **版本对不上 ⇒ 拒绝落盘**（409 的来源）。
    #[tokio::test]
    async fn a_stale_version_is_refused() {
        let mem: Arc<dyn Storage> = Arc::new(MemoryStorage::new());
        save(mem.as_ref(), vec![], 0).await.unwrap(); // 版本变成 1
        let err = save(mem.as_ref(), vec![], 0).await.unwrap_err();
        assert_eq!(
            err,
            ApiKeyWriteError::Stale {
                expected: 0,
                actual: 1
            }
        );
    }

    /// 🔴🔴 **结构不认的表绝不许被覆盖。**
    ///
    /// 回归对象：折叠成「空表 + 覆盖」的实现会把幸存记录整份抹掉，**不可逆**。
    /// 这里同时断言存储里的原字节**一个都没被动过**。
    #[tokio::test]
    async fn a_malformed_table_is_never_overwritten() {
        let mem = Arc::new(MemoryStorage::new());
        let junk = json!("这不是一张表");
        mem.put(APIKEYS_KEY, junk.clone(), None).await.unwrap();

        let err = save(mem.as_ref(), vec![], 0).await.unwrap_err();
        assert_eq!(err, ApiKeyWriteError::Unreadable);
        assert_eq!(
            mem.get(APIKEYS_KEY).await.unwrap(),
            Some(junk),
            "拒绝之后原字节必须原地不动，否则人工捞回也做不到了"
        );
    }

    /// 存储出错时如实回 `Storage`，**不伪装成版本冲突**
    /// —— 两者对运维要做的事完全不同（一个是重试，一个是修磁盘）。
    #[tokio::test]
    async fn a_storage_failure_is_reported_as_itself() {
        let mem = Arc::new(MemoryStorage::new());
        mem.fail_next("EACCES");
        let err = save(mem.as_ref(), vec![], 0).await.unwrap_err();
        assert!(matches!(err, ApiKeyWriteError::Storage(_)));
    }

    /// 🔴 **这张表一律不传 TTL。**
    ///
    /// 用 `MemoryStorage` 的假时钟往前推很久，表必须还在 ——
    /// 让 TTL 兼任别的职责会让它整份蒸发（全部客户端一次性 401）。
    #[tokio::test]
    async fn the_table_is_written_without_an_expiry() {
        let mem = Arc::new(MemoryStorage::new());
        mem.set_clock(0);
        save(mem.as_ref(), vec![], 0).await.unwrap();
        mem.set_clock(86_400_000 * 3650);
        assert!(
            matches!(load(mem.as_ref()).await.unwrap(), ApiKeyTableRead::Ok(_)),
            "十年之后它也必须还在"
        );
    }
}
