//! key 池索引（`pool-index.ts`，01 §4.1）。**存在的唯一理由是把 `list()` 从转发热路径上摘掉。**
//!
//! # 它今天成立的理由（起因已经不成立了，如实登记）
//!
//! 原话是「Cloudflare KV 免费档的 list 配额是 1,000 次/天、与转发请求数 1:1 挂钩」。
//! 那个配额随 Worker/KV 形态一起没了（P0 砍掉 Worker）。
//! **索引没有跟着删，理由换成今天成立的那一条**：`FileStorage::list()` 是一次
//! `read_file` + 整份 `store.json` 反序列化，把它挂在每个转发请求上，
//! 池子一大就是**逐请求的全量反序列化**。索引把它降到 0 次 list。
//!
//! # 为什么不退回「整池存成单条 JSON」
//!
//! 那样确实只要 1 次 get，但**一 key 一记录**正是为了避免整池的 last-write-wins
//! （两个并发请求各改一把 key，后写的覆盖先写的**整份池子**）。
//! 索引只存 id 列表，键本身仍然一 key 一记录，两者兼得。
//!
//! # 🔴 一条原则（01 §4.0）
//!
//! **记录（`key:<id>`）是真源，索引（`pool:index`）是派生缓存。**
//! 所以这个文件里的窄化**方向是分层的**（见 [`parse`]），而对账一律以 `list()` 为准。

use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

/// 单条 key 记录的存储键前缀。
///
/// ⚠️ 别的模块**不许**也起一个 `key:` 开头的存储键：`pool/repo.rs` 兜底那条
/// `list("key:")` 会把它扫进来当成一把 key 记录（TS 那边的域名台账就是为此
/// 刻意避开了这个前缀）。
pub const KEY_PREFIX: &str = "key:";

/// 索引本身的存储键。
pub const POOL_INDEX_KEY: &str = "pool:index";

/// 结构版本。
///
/// 改索引形态时把它 +1 即可：[`parse`] 对不认识的版本返回 `None` ⇒ `all()` 会当作
/// 「索引缺失」回落到 `list()` 并重建，**是一条自动的、不需要迁移脚本的升级路径**。
pub const POOL_INDEX_VERSION: u64 = 1;

/// 索引本体。
///
/// ⚠️ **`ids` 的顺序无语义**：对账会拿 `list()` 的结果整体重排它。
/// 任何依赖这个顺序的东西（面板行号、分页）都必须自己排序
/// （见 [`crate::core::keys::view::to_key_views`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PoolIndex {
    pub v: u64,
    pub ids: Vec<String>,
}

/// 建一个索引。**一律 dedupe**（`pool-index.ts:33-35`）。
///
/// ⚠️ dedupe **保留首次出现的顺序**（与 JS 的 `new Set` 逐字相同）。
/// 顺序本身无语义，但「同一份输入建出来的索引每次都一样」这条要成立 ——
/// 否则对账会因为顺序抖动产生一次无谓的写，而每一次写都是一遍 `store.json` 全量重写。
pub fn make_pool_index(ids: &[String]) -> PoolIndex {
    PoolIndex {
        v: POOL_INDEX_VERSION,
        ids: dedupe(ids.iter().map(String::as_str)),
    }
}

/// 从存储读回来的裸值 → 索引。**🔴 窄化方向是分层的，不许合并成一层。**
///
/// | 坏在哪 | 处置 | 理由 |
/// |---|---|---|
/// | **结构级**（不是对象 / 是数组 / 版本不认 / `ids` 不是数组） | 返回 `None` | 让调用方走「索引缺失」那条**重建**路径 |
/// | **元素级**（非字符串 / 空串 / 重复） | **就地剔掉那一个** | 一条脏数据把整个索引作废 ⇒ **每次 `all()` 都 `list` 一遍**，正是索引存在要避免的事 |
///
/// 🔴 这两条**必须都在**。只做前者 = 一个脏元素让索引天天重建；
/// 只做后者 = 版本升级时读到一份语义已经变了的索引，而没有任何东西会红。
pub fn parse(raw: &Value) -> Option<PoolIndex> {
    let o = raw.as_object()?;
    if o.get("v").and_then(Value::as_u64) != Some(POOL_INDEX_VERSION) {
        return None;
    }
    let ids = o.get("ids")?.as_array()?;
    Some(PoolIndex {
        v: POOL_INDEX_VERSION,
        ids: dedupe(
            ids.iter()
                .filter_map(Value::as_str)
                .filter(|s| !s.is_empty()),
        ),
    })
}

/// `storage.list(KEY_PREFIX)` 的结果 → id 列表。
///
/// 过滤出 `key:` 开头的、切掉前缀、丢掉空 id、dedupe。
/// ⚠️ **不是 `key:` 开头的一律跳过**（而不是报错）：同一个存储里还住着 `config`、
/// `apikeys`、`event:*` 等等，它们出现在这里是正常的。
pub fn ids_from_key_names(names: &[String]) -> Vec<String> {
    dedupe(
        names
            .iter()
            .filter_map(|n| n.strip_prefix(KEY_PREFIX))
            .filter(|id| !id.is_empty()),
    )
}

/// 集合相等。**顺序无语义。**
///
/// 按顺序比会让对账在「顺序碰巧不同」时产生一次无谓的存储写，
/// 而每一次写都是一遍 `store.json` 全量重写。
///
/// ⚠️ **两边各自先去重再比，不许图省事「先比长度 + 单向包含」**：
/// 那个写法对含重复元素的入参**不对称** —— `same_id_set(["x","y"], ["x","x"])`
/// 会返回 `true`。它的表现形式是**「对账认为一致因而不修」**，
/// 正好是最难查的那种沉默。
pub fn same_id_set(a: &[String], b: &[String]) -> bool {
    let sa: HashSet<&str> = a.iter().map(String::as_str).collect();
    let sb: HashSet<&str> = b.iter().map(String::as_str).collect();
    sa == sb
}

/// 去重，**保留首次出现的顺序**。
fn dedupe<'a>(it: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for s in it {
        if seen.insert(s) {
            out.push(s.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    /// 建构一律去重，且**顺序稳定**（同一份输入每次给同一份索引）。
    #[test]
    fn building_dedupes_and_keeps_first_occurrence_order() {
        let idx = make_pool_index(&ids(&["b", "a", "b", "c", "a"]));
        assert_eq!(idx.v, POOL_INDEX_VERSION);
        assert_eq!(idx.ids, ids(&["b", "a", "c"]));
    }

    /// 一份正常索引原样读回来。
    #[test]
    fn a_healthy_index_round_trips() {
        let raw = json!({ "v": 1, "ids": ["a", "b"] });
        assert_eq!(parse(&raw).unwrap().ids, ids(&["a", "b"]));
    }

    /// 🔴 **结构级错误 ⇒ `None`**（调用方据此走重建路径）。
    #[test]
    fn structural_damage_asks_for_a_rebuild() {
        for raw in [
            json!(null),
            json!(42),
            json!("nope"),
            json!(["a", "b"]),              // 数组不是对象
            json!({ "ids": ["a"] }),        // 没有版本
            json!({ "v": 2, "ids": [] }),   // 版本不认（升级路径）
            json!({ "v": 1 }),              // 没有 ids
            json!({ "v": 1, "ids": "a" }),  // ids 不是数组
            json!({ "v": "1", "ids": [] }), // 版本是字符串
        ] {
            assert_eq!(parse(&raw), None, "{raw} 该走重建");
        }
    }

    /// 🔴 **元素级脏数据就地剔掉，索引照用。**
    ///
    /// 回归对象：把一条脏元素升级成「整个索引作废」，会让 `all()` 每次都 `list` 一遍
    /// —— 那正是索引存在要避免的那件事。
    #[test]
    fn dirty_elements_are_dropped_without_invalidating_the_whole_index() {
        let raw = json!({ "v": 1, "ids": ["a", 7, "", null, "b", "a", { "x": 1 }] });
        let idx = parse(&raw).expect("一条脏元素不许让整个索引作废");
        assert_eq!(idx.ids, ids(&["a", "b"]));
    }

    /// 空 `ids` 是**合法**的（池子真的是空的），不是「坏了」。
    /// 混淆这两者会让空池每次 `all()` 都去 `list` 一遍。
    #[test]
    fn an_empty_index_is_valid_not_broken() {
        let idx = parse(&json!({ "v": 1, "ids": [] })).expect("空索引是合法的");
        assert!(idx.ids.is_empty());
    }

    /// 版本 +1 就是一条自动升级路径：旧进程写的新版本索引会被当成「缺失」而重建。
    #[test]
    fn an_unknown_version_degrades_to_missing_so_it_can_be_rebuilt() {
        assert_eq!(
            parse(&json!({ "v": POOL_INDEX_VERSION + 1, "ids": ["a"] })),
            None
        );
    }

    /// `list()` 的结果里混着别的键 —— 只认 `key:` 前缀。
    #[test]
    fn only_key_prefixed_names_become_ids() {
        let names = ids(&[
            "key:aaa",
            "config",
            "apikeys",
            "pool:index",
            "key:bbb",
            "key:", // 空 id
            "keyless",
            "key:aaa", // 重复
        ]);
        assert_eq!(ids_from_key_names(&names), ids(&["aaa", "bbb"]));
    }

    /// 集合相等：顺序不算数。
    #[test]
    fn set_equality_ignores_order() {
        assert!(same_id_set(&ids(&["a", "b"]), &ids(&["b", "a"])));
        assert!(!same_id_set(&ids(&["a", "b"]), &ids(&["a"])));
        assert!(same_id_set(&[], &[]));
    }

    /// 🔴 **重复元素的那个陷阱**：`["x","y"]` 与 `["x","x"]` 绝不是同一个集合。
    ///
    /// 回归对象：「先比长度 + 单向包含」的写法在这里会返回 `true`，
    /// 表现形式是「对账认为一致因而不修」—— 最难查的那种沉默。
    #[test]
    fn duplicates_do_not_fool_set_equality() {
        assert!(!same_id_set(&ids(&["x", "y"]), &ids(&["x", "x"])));
        assert!(!same_id_set(&ids(&["x", "x"]), &ids(&["x", "y"])));
        // 两边各自去重之后**真的**相等时照常为真。
        assert!(same_id_set(&ids(&["x", "x", "y"]), &ids(&["y", "x"])));
    }

    /// 存储键前缀与索引键是**存储契约**，改了就读不回既有部署的数据。
    #[test]
    fn storage_key_literals_are_a_contract() {
        assert_eq!(KEY_PREFIX, "key:");
        assert_eq!(POOL_INDEX_KEY, "pool:index");
        assert_eq!(POOL_INDEX_VERSION, 1);
    }

    /// 索引序列化出去的形状就是 `{v, ids}` —— 存储里读回来的那一份认得它。
    #[test]
    fn index_serializes_into_the_shape_parse_accepts() {
        let idx = make_pool_index(&ids(&["a", "b"]));
        let raw = serde_json::to_value(&idx).unwrap();
        assert_eq!(raw, json!({ "v": 1, "ids": ["a", "b"] }));
        assert_eq!(parse(&raw).unwrap(), idx, "写出去的必须读得回来");
    }
}
