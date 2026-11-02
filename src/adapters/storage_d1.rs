//! Cloudflare D1 存储：`store` 表一把 key 一行。
//!
//! # 为什么是 D1 而不是 KV / Durable Objects
//!
//! KV 是**最终一致**（写后读最长 ~60 秒才收敛），key 池冷却、子密钥吊销这类
//! 「写完就要被下一个请求看见」的状态在它上面是错的；DO 强一致但要绑定一个
//! actor 命名空间、且免费层没有 DO —— 而 D1（SQLite）**读自己的写**，
//! 行级 `INSERT OR REPLACE` 原子，`batch()` 跨语句事务，免费层 5GB/每天百万读，
//! 与 `FileStorage` 的「读自己的写」语义对齐，是成本与正确性之间唯一同时成立的选项。
//!
//! # 表结构
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS store (
//!   key        TEXT PRIMARY KEY,
//!   value      TEXT NOT NULL,
//!   expires_at INTEGER          -- epoch 毫秒；NULL = 永不过期
//! );
//! ```
//!
//! 🔴 **`expires_at` 是绝对的 epoch 毫秒**，与 `now_ms()` 同一基准 ——
//! `Storage::put` 的契约就是这么写的，这里不另设相对 TTL。
//!
//! # 与 `FileStorage` 的语义差
//!
//! | 面 | FileStorage | D1Storage |
//! |---|---|---|
//! | 并发 | 单进程一把写锁 | D1 自己保证行级原子；跨 key 的读改写**不**串行 |
//! | 原子写 | tmp→fsync→rename | `INSERT OR REPLACE` 单语句 |
//! | 过期清理 | `get`/`list` 惰性判断 | 同（SQL `WHERE` 过滤） |
//!
//! ⚠️ **池索引对账**（`pool:index` 的漂移修复）在多实例并发 `put` 下仍是
//! last-write-wins —— 这与 FileStorage 相同，靠 `reconcile` + `stale_write_dropped`
//! 消化，不是 D1 引入的新问题。
//!
//! # 一个刻意的取舍：不预写 `store` 表
//!
//! `CREATE TABLE IF NOT EXISTS` 放在**第一次真实读写时惰性执行**，而不是 Worker
//! 启动时 —— Workers 没有「启动」这个时刻，全局构造函数跑在冷启动里，
//! 而那时还没有 `env`。⇒ 每个 op 都先 `ensure()` 一次，内部用 `OnceCell` 去重。
//! 代价：同一实例内至多一次多余的 `CREATE TABLE`（幂等）。

use crate::ports::storage::{Storage, StorageError, StorageResult};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::OnceLock;
use worker::{D1Database, Env};

/// 建表 DDL。**幂等**，可重复执行。
///
/// 🔴 **必须与仓库根的 `schema.sql` 逐字同构**：那边是部署前的显式建表
/// （`wrangler d1 execute --file=schema.sql`），这里是冷启动兜底。
/// 两处结构不一致会造出「本地能跑、线上缺列/缺索引」这种最难查的故障。
///
/// ⚠️ **单行 + 逐条 `prepare().run()`**，两个理由：
/// 一是多行字符串经 `D1Database::exec` 会被截断成 `... store (` 并报
/// `incomplete input: SQLITE_ERROR`（Miniflare 实测）；
/// 二是 `exec` 把入参当单条语句解析，多条也用不了。
/// 所以这里是一条语句一个元素，且各自压成一行 —— 注释里的换行解释得清就够了。
const DDL_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS store (key TEXT PRIMARY KEY, value TEXT NOT NULL, expires_at INTEGER)",
    "CREATE INDEX IF NOT EXISTS idx_store_expires_at ON store (expires_at) WHERE expires_at IS NOT NULL",
    "CREATE INDEX IF NOT EXISTS idx_store_key ON store (key)",
];

/// D1 适配器。
///
/// `D1Database` 不是 `Send + Sync`（它是 JsValue 封装），所以不能直接塞进 `Arc<dyn Storage>`
/// 再被 axum 跨 await 持有 —— 这里把它包在 `SendWrapper` 里，**Workers 是单线程**，
/// `unsafe impl Send` 是 workers-rs 自己的既定做法（`SendWrapper`）。
pub struct D1Storage {
    db: worker::send::SendWrapper<D1Database>,
    /// `CREATE TABLE` 只跑一次。`SendWrapper` 套 `OnceLock`，同理单线程成立。
    ensured: worker::send::SendWrapper<OnceLock<()>>,
}

impl D1Storage {
    /// 从 Worker 环境取 D1 绑定。**绑定名写死 `"DB"`**，与 wrangler.toml 对齐。
    pub fn from_env(env: &Env) -> Result<Self, StorageError> {
        let db = env
            .d1("DB")
            .map_err(|e| StorageError::new(format!("D1 binding `DB` 不可用: {e}")))?;
        Ok(Self {
            db: worker::send::SendWrapper::new(db),
            ensured: worker::send::SendWrapper::new(OnceLock::new()),
        })
    }

    /// 确保表存在。**同一实例内至多执行一次**，之后短路。
    ///
    /// 失败不记 `OnceLock`，下次调用会再试 —— 建表失败是部署错误，
    /// 不该被一个瞬时失败永久压死。
    ///
    /// ⚠️ `SendFuture`：D1 的 await 内部是 `JsFuture`（`Rc<RefCell>`，`!Send`），
    /// 而 `Storage` 端口要求 `Send`。Workers 单线程 ⇒ `SendFuture` 是 workers-rs
    /// 官方的桥，语义上成立（没有第二个线程会真的拿到它）。
    async fn ensure(&self) -> StorageResult<()> {
        if self.ensured.get().is_some() {
            return Ok(());
        }
        // 逐条 `prepare().run()`：比 `exec` 稳（`exec` 的多语句/多行行为在
        // Miniflare 与线上不一致，实测会 `incomplete input`）。
        // `run()` 的 future 是 `!Send`（内部 `JsFuture`）⇒ 套 `SendFuture` 桥。
        for sql in DDL_STATEMENTS {
            let stmt = self.db.prepare(*sql);
            worker::send::SendFuture::new(async {
                stmt.run()
                    .await
                    .map_err(|e| StorageError::new(format!("建 store 表失败: {e}")))
            })
            .await?;
        }
        let _ = self.ensured.set(());
        Ok(())
    }

    fn db_err(e: impl std::fmt::Display) -> StorageError {
        StorageError::new(format!("D1: {e}"))
    }
}

#[async_trait]
impl Storage for D1Storage {
    async fn get(&self, key: &str) -> StorageResult<Option<Value>> {
        self.ensure().await?;
        let now = crate::adapters::now_ms();
        // 读：未过期（expires_at 为空或在将来）。🔴 **过滤条件在 SQL 里**，
        // 不是读回来再判 —— 过期行留在库里等惰性删，但读路径必须先看不到它。
        let stmt = self
            .db
            .prepare("SELECT value FROM store WHERE key = ?1 AND (expires_at IS NULL OR expires_at > ?2)")
            .bind(&[
                wasm_bindgen::JsValue::from(key),
                wasm_bindgen::JsValue::from_f64(now as f64),
            ])
            .map_err(Self::db_err)?;
        let row: Option<String> = worker::send::SendFuture::new(stmt.first(Some("value")))
            .await
            .map_err(Self::db_err)?;
        match row {
            None => Ok(None),
            Some(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| StorageError::new(format!("store[{key}] JSON 损坏: {e}"))),
        }
    }

    async fn put(&self, key: &str, value: Value, expires_at: Option<i64>) -> StorageResult<()> {
        self.ensure().await?;
        let text = serde_json::to_string(&value)
            .map_err(|e| StorageError::new(format!("store[{key}] 序列化失败: {e}")))?;
        // `INSERT OR REPLACE`：upsert，行级原子。
        // 🔴 `expires_at` 用 `NULL` 表示永不过期，与 FileStorage 的 `ttl` 表语义一致。
        // `bind` 收 `&[JsValue]`：三个参数统一转成 `JsValue`。
        // `expires_at` 的 `NULL` 用 `JsValue::null()`（不是 `D1Type` —— 它走 `bind_refs`，
        // 这里用 `bind` 更直白）。i64 → f64 在 ±2^53 内精确，epoch 毫秒远在界内。
        let expires_js: wasm_bindgen::JsValue = match expires_at {
            Some(t) => wasm_bindgen::JsValue::from_f64(t as f64),
            None => wasm_bindgen::JsValue::NULL,
        };
        let stmt = self
            .db
            .prepare("INSERT OR REPLACE INTO store (key, value, expires_at) VALUES (?1, ?2, ?3)")
            .bind(&[
                wasm_bindgen::JsValue::from(key),
                wasm_bindgen::JsValue::from(text.as_str()),
                expires_js,
            ])
            .map_err(Self::db_err)?;
        worker::send::SendFuture::new(stmt.run())
            .await
            .map_err(Self::db_err)?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> StorageResult<()> {
        self.ensure().await?;
        let stmt = self
            .db
            .prepare("DELETE FROM store WHERE key = ?1")
            .bind(&[wasm_bindgen::JsValue::from(key)])
            .map_err(Self::db_err)?;
        worker::send::SendFuture::new(stmt.run())
            .await
            .map_err(Self::db_err)?;
        Ok(())
    }

    async fn list(&self, prefix: &str) -> StorageResult<Vec<String>> {
        self.ensure().await?;
        let now = crate::adapters::now_ms();
        // `LIKE 'prefix%'`。前缀里的 `%`/`_`/`\` 必须转义，否则前缀会被当成通配符 —
        // key 前缀全是 ASCII（`key:`/`event:`/`config`…），但转义没有代价。
        let esc = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{esc}%");
        let stmt = self
            .db
            .prepare(
                "SELECT key FROM store \
                 WHERE key LIKE ?1 ESCAPE '\\' \
                   AND (expires_at IS NULL OR expires_at > ?2)",
            )
            .bind(&[
                wasm_bindgen::JsValue::from(pattern.as_str()),
                wasm_bindgen::JsValue::from_f64(now as f64),
            ])
            .map_err(Self::db_err)?;
        let result = worker::send::SendFuture::new(stmt.all())
            .await
            .map_err(Self::db_err)?;
        let rows: Vec<String> = result
            .results::<serde_json::Value>()
            .map_err(Self::db_err)?
            .into_iter()
            .filter_map(|v| v.get("key").and_then(|k| k.as_str()).map(str::to_string))
            .collect();
        Ok(rows)
    }
}
