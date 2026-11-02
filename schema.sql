-- agnes2api —— Cloudflare D1 表结构。
--
-- 🔴 **与 `src/adapters/storage_d1.rs` 的 `DDL` 常量必须是同一份结构**：
-- 那边是「表不存在就建」（`CREATE TABLE IF NOT EXISTS`，冷启动兜底），
-- 这里是「部署前显式建」（`wrangler d1 execute --file=schema.sql`，推荐做法）。
-- 两处不一致会让「本地能跑、线上缺列」这种最难查的问题出现。
--
-- 用法：
--   npx wrangler d1 execute agnes2api-store --remote --file=schema.sql
-- 本地 Miniflare：
--   npx wrangler d1 execute agnes2api-store --local --file=schema.sql
--
-- 表设计说明：
--   key        存储键。沿用 FileStorage 的键空间（`key:` 上游 key、`client:` 子密钥、
--              `config` 面板配置、`event:`/`usage:` …），见 `src/ports/storage.rs`。
--   value      JSON 文本（`serde_json::Value` 序列化）。**不拆成多列**：
--              存储端口对上层是「键 → JSON 值」，拆列会把 schema 知识漏进业务层。
--   expires_at 绝对 epoch 毫秒；`NULL` = 永不过期。
--              🔴 `get`/`list` 都必须在 SQL 里过滤它 —— 过期行惰性保留，但读不到。

CREATE TABLE IF NOT EXISTS store (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  expires_at INTEGER
);

-- 过期清理用（Cron 里可选对账时扫）。不是热路径，但少了它全表扫。
CREATE INDEX IF NOT EXISTS idx_store_expires_at
  ON store (expires_at)
  WHERE expires_at IS NOT NULL;

-- 前缀列举用（`pool` 的 `key:` / `client:` 列表）。SQLite 的 `LIKE 'x%'`
-- 在无索引时会全表扫；这个索引让前缀走 range scan。
CREATE INDEX IF NOT EXISTS idx_store_key
  ON store (key);
