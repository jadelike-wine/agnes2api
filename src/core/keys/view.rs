//! 上游 key 的**面板投影**（`admin/key-view.ts`，01 §8.2）。
//!
//! 🔴 **这个结构永不含明文 key，本仓也没有任何 reveal 端点。**
//! 明文只在两个地方存在：存储里的 [`KeyRecord::key`]，与拼 `Authorization` 头的那一瞬间。
//!
//! 🔴 **搜索绝不能变成明文预言机**：见 [`matches_query`]。

use serde::Serialize;

use crate::core::keypool::is_disabled;
use crate::core::stats::stats_or_empty;
use crate::core::types::{KeyRecord, KeyStats};

/// 上游 key 的掩码。**全仓唯一的一份实现。**
///
/// `abcde…wxyz`：前 5 位 + `…` + 后 4 位。**长度 ≤ 10 时只给一个 `…`** ——
/// 再短的 key 掩不住，前 5 后 4 加起来就把它整个露出来了。
///
/// 🔴 **绝不返回原值**：返回原值的「掩码」比没有掩码更糟 —— **调用方会以为它安全了。**
///
/// 🔴 **必须按 `char` 切，不许 `&key[..5]`**（01 §14）：
/// 后者在多字节 UTF-8 上**直接 panic**，而明文 key 是不受控输入 ——
/// [`is_importable_key`](crate::core::keypool) 那道门只拦**导入**那条路，
/// **铸号那条路存进去的可疑 key 照存不误**（01 §4.5：那里拒绝就是销毁凭据）。
/// 所以「池子里每一把 key 都是 ASCII」这个前提**不成立**。
pub fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n <= 10 {
        return "…".to_string();
    }
    let head: String = key.chars().take(5).collect();
    let tail: String = key.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// 面板分档。**顺序即优先级，且与
/// [`pool_health`](crate::core::keypool::pool_health) 逐档同序。**
///
/// 两处不同序就会出现「概览卡说 3 把冷却中、Key 池列表一把冷却中的都没有」
/// 这种自相矛盾。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    Disabled,
    Evicted,
    Cooling,
    Fresh,
}

impl Bucket {
    /// 对外的字面量。**面板按它做 CSS 类名与筛选参数，改了就是改对外契约。**
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Evicted => "evicted",
            Self::Cooling => "cooling",
            Self::Fresh => "fresh",
        }
    }
}

/// 四个档次的闭集，**顺序即优先级**。面板的筛选下拉按它渲染。
pub const BUCKETS: [Bucket; 4] = [
    Bucket::Disabled,
    Bucket::Evicted,
    Bucket::Cooling,
    Bucket::Fresh,
];

/// 见 [`Bucket`]。
///
/// 🔴 `key_bucket(...) == Bucket::Fresh` **当且仅当**
/// [`is_available`](crate::core::keypool::is_available) —— 有判据钉着。
/// 加一档而不改调度，那一格就会变红。
pub fn key_bucket(r: &KeyRecord, now: i64) -> Bucket {
    if is_disabled(r) {
        Bucket::Disabled
    } else if r.evicted {
        Bucket::Evicted
    } else if r.cooldown_until > now {
        Bucket::Cooling
    } else {
        Bucket::Fresh
    }
}

/// 面板上一行 key 的形状。
///
/// ⚠️ **`disabled` 与 `note` 在这里是「恒存在」的**（01 §14 那条「`c.json` 丢掉
/// `undefined` 字段」在 serde 的 `skip_serializing_if` 下会原样复现）：
/// [`KeyRecord`] 那边它们可选，是为了分得清「没停用」与「读不出来」；
/// 到了投影这一层已经**判过**了，所以必须落成实打实的 `bool` / `null`，
/// **一个 `skip_serializing_if` 都不许有**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    pub id: String,
    /// 掩码。🔴 **这个结构永不含明文。**
    pub masked: String,
    pub upstream: Option<crate::core::types::KeyUpstream>,
    /// 面板上的行号。按 `added_at` 升序、`id` 破平。**它随增删而变，不是身份。**
    pub seq: usize,
    pub bucket: Bucket,
    pub added_at: i64,
    pub last_used_at: Option<i64>,
    pub cooldown_until: i64,
    pub cooldown_reason: Option<String>,
    pub evicted_reason: Option<String>,
    pub strikes: u32,
    /// **恒是布尔、恒存在**，即使记录里压根没有这一格。
    pub disabled: bool,
    /// 运维备注。**恒存在、恒是 `string | null`。**
    ///
    /// 🔴 它是本结构里第一个「运维自由输入、又会被投影到面板上」的字段，
    /// **渲染必须走 `textContent`，绝不许拼进 innerHTML**。
    /// 其余字段要么是后端算出来的枚举、要么是数字、要么是掩码，**只有它整串来自请求体**。
    /// 后端刻意不做转义（JSON 响应体的字段边界由 JSON 语法本身保证，转义是渲染层的事）
    /// —— **所以这句话必须在渲染那一侧被执行，别以为后端已经洗过了。**
    pub note: Option<String>,
    /// Tier-1。**近似值**：并发下少计，且最多晚一个 `POOL_TOUCH_INTERVAL_MS` 落盘。
    pub stats: KeyStats,
}

/// 投影成面板要的形状，**并按 `seq` 的顺序返回**。
///
/// 🔴 **返回顺序不是「顺手排一下好看」，分页的正确性建立在它上面。**
/// 传入顺序来自 `pool:index`，而索引**顺序无语义**（对账会拿 `list()` 的结果整体重排它）。
/// 只算 `seq` 却按传入顺序返回，后果有三层：
/// ① 面板上出现 `#7 / #2 / #19`；
/// ② 一次索引对账之后整个列表重排，而什么都没增删；
/// ③ **分页在两次请求之间不稳定**，翻页会重复或漏掉条目。
///
/// ⚠️ `added_at` 相同时**用 `id` 破平**（同一批导入的 key 时间戳常常逐字相同）。
pub fn to_key_views(records: &[KeyRecord], now: i64) -> Vec<KeyView> {
    let mut sorted: Vec<&KeyRecord> = records.iter().collect();
    sorted.sort_by(|a, b| a.added_at.cmp(&b.added_at).then_with(|| a.id.cmp(&b.id)));
    sorted
        .into_iter()
        .enumerate()
        .map(|(i, r)| KeyView {
            id: r.id.clone(),
            masked: mask_key(&r.key),
            upstream: r.upstream,
            seq: i + 1,
            bucket: key_bucket(r, now),
            added_at: r.added_at,
            last_used_at: r.last_used_at,
            cooldown_until: r.cooldown_until,
            cooldown_reason: r.cooldown_reason.clone(),
            evicted_reason: r.evicted_reason.clone(),
            strikes: r.strikes,
            // 🔴 过一道 `is_disabled()` 落成布尔，**不许直接搬 `r.disabled`**。
            disabled: is_disabled(r),
            note: r.note.clone(),
            stats: stats_or_empty(r.stats.as_ref()),
        })
        .collect()
}

/// 汇总卡的五个数。
///
/// ⚠️ **判据只看 `v.bucket`**，不再各自重算一遍 `disabled` / `evicted` ——
/// 重算就是同一条优先级规则的第二份实现，两份迟早会漂。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BucketCounts {
    pub all: usize,
    pub fresh: usize,
    pub cooling: usize,
    pub evicted: usize,
    pub disabled: usize,
}

/// 见 [`BucketCounts`]。
pub fn bucket_counts(views: &[KeyView]) -> BucketCounts {
    let mut c = BucketCounts {
        all: views.len(),
        fresh: 0,
        cooling: 0,
        evicted: 0,
        disabled: 0,
    };
    for v in views {
        match v.bucket {
            Bucket::Disabled => c.disabled += 1,
            Bucket::Evicted => c.evicted += 1,
            Bucket::Cooling => c.cooling += 1,
            Bucket::Fresh => c.fresh += 1,
        }
    }
    c
}

/// 搜索。🔴 **只匹配 id 与掩码后的可见部分，绝不匹配明文 key。**
///
/// 匹配明文等于把「没有 reveal 端点」这条保证降级成一个**慢速预言机**：
/// 提交一段猜测、看返回条数，就能**逐字猜出整把 key**。
///
/// ⚠️ 这条约束的对象是「**投影**」而不是「记录」—— 所以这个函数收的是
/// [`KeyView`]（里面根本没有明文），签名本身就把那条路堵死了。
/// **别为了「搜起来更方便」改成收 `&KeyRecord`。**
pub fn matches_query(v: &KeyView, q: &str) -> bool {
    let s = q.trim().to_lowercase();
    if s.is_empty() {
        return true;
    }
    v.id.to_lowercase().contains(&s) || v.masked.to_lowercase().contains(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keypool::is_available;

    const NOW: i64 = 1_700_000_000_000;

    fn rec(id: &str, key: &str, added_at: i64) -> KeyRecord {
        KeyRecord::fresh(id, key, added_at)
    }

    // ── mask_key ───────────────────────────────────────────────────────────

    /// 🔴 **掩码绝不返回原值。** 一把正常长度的 key：前 5 + `…` + 后 4。
    #[test]
    fn masking_never_returns_the_plaintext() {
        let key = "sk-abcdefghijklmnop";
        let masked = mask_key(key);
        assert_eq!(masked, "sk-ab…mnop");
        assert_ne!(masked, key);
        assert!(!key.contains(&masked));
    }

    /// 短到掩不住的时候只给一个 `…`（前 5 后 4 会把 ≤10 位的整个露出来）。
    #[test]
    fn short_keys_are_fully_hidden() {
        assert_eq!(mask_key(""), "…");
        assert_eq!(mask_key("1234567890"), "…", "10 位是边界，仍然全掩");
        assert_eq!(mask_key("12345678901"), "12345…8901", "11 位才开始露两头");
    }

    /// 🔴 **多字节字符不许 panic**（`&s[..5]` 会）。
    ///
    /// 回归对象：`is_importable_key` 只拦**导入**那条路；铸号那条路存进去的
    /// 可疑 key 照存不误，所以「池子里全是 ASCII」这个前提不成立。
    #[test]
    fn masking_multibyte_keys_does_not_panic() {
        let key = "密钥密钥密钥密钥密钥密钥"; // 12 个字符
        let masked = mask_key(key);
        assert_eq!(masked, "密钥密钥密…密钥密钥");
        assert!(masked.chars().count() < key.chars().count());
        // 混合宽度也要稳（12 个字符：前 5 个 + … + 后 4 个）。
        assert_eq!(mask_key("ab密cd密ef密gh密"), "ab密cd…密gh密");
    }

    // ── key_bucket ─────────────────────────────────────────────────────────

    /// 🔴 **顺序即优先级**：既停用、又剔除、又冷却的那一把只能是 `disabled`。
    #[test]
    fn bucket_order_is_priority_order() {
        let mut r = rec("a", "k", 0);
        r.disabled = Some(true);
        r.evicted = true;
        r.cooldown_until = NOW + 1;
        assert_eq!(key_bucket(&r, NOW), Bucket::Disabled);

        r.disabled = None;
        assert_eq!(key_bucket(&r, NOW), Bucket::Evicted);

        r.evicted = false;
        assert_eq!(key_bucket(&r, NOW), Bucket::Cooling);

        r.cooldown_until = NOW;
        assert_eq!(
            key_bucket(&r, NOW),
            Bucket::Fresh,
            "冷却到期那一刻就是 fresh"
        );
    }

    /// 🔴 **`Fresh` 当且仅当 `is_available`** —— 概览卡与调度器不许各说各话。
    #[test]
    fn fresh_bucket_is_exactly_is_available() {
        let mut shapes = Vec::new();
        for disabled in [None, Some(false), Some(true)] {
            for evicted in [false, true] {
                for cd in [0, NOW - 1, NOW, NOW + 1] {
                    let mut r = rec("x", "k", 0);
                    r.disabled = disabled;
                    r.evicted = evicted;
                    r.cooldown_until = cd;
                    shapes.push(r);
                }
            }
        }
        let mut fresh_seen = 0;
        for r in &shapes {
            let fresh = key_bucket(r, NOW) == Bucket::Fresh;
            assert_eq!(fresh, is_available(r, NOW), "分档与调度判据分歧：{r:?}");
            fresh_seen += usize::from(fresh);
        }
        assert!(
            fresh_seen > 0 && fresh_seen < shapes.len(),
            "夹具必须两侧都有"
        );
    }

    /// 档次的字面量是对外契约（面板按它做 CSS 类名与筛选参数）。
    #[test]
    fn bucket_literals_are_a_contract() {
        assert_eq!(
            BUCKETS.map(Bucket::as_str),
            ["disabled", "evicted", "cooling", "fresh"]
        );
        assert_eq!(
            serde_json::to_value(Bucket::Cooling).unwrap(),
            serde_json::json!("cooling")
        );
    }

    // ── to_key_views ───────────────────────────────────────────────────────

    /// 🔴 **返回顺序必须与 `seq` 一致**（分页的正确性建立在它上面）。
    #[test]
    fn views_are_returned_in_seq_order() {
        let rs = vec![
            rec("c", "kkkkkkkkkkkk", 300),
            rec("a", "kkkkkkkkkkkk", 100),
            rec("b", "kkkkkkkkkkkk", 200),
        ];
        let views = to_key_views(&rs, NOW);
        assert_eq!(
            views.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(views.iter().map(|v| v.seq).collect::<Vec<_>>(), [1, 2, 3]);
    }

    /// `added_at` 相同（同一批导入）时按 `id` 破平 —— 否则列表每次刷新都在跳。
    #[test]
    fn ties_on_added_at_are_broken_by_id() {
        let rs = vec![
            rec("zz", "kkkkkkkkkkkk", 100),
            rec("aa", "kkkkkkkkkkkk", 100),
            rec("mm", "kkkkkkkkkkkk", 100),
        ];
        let views = to_key_views(&rs, NOW);
        assert_eq!(
            views.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["aa", "mm", "zz"]
        );
    }

    /// 🔴 **投影里一个字节的明文都没有。**
    #[test]
    fn a_view_never_carries_the_plaintext() {
        let rs = vec![rec("a", "sk-super-secret-value", 1)];
        let json = serde_json::to_string(&to_key_views(&rs, NOW)).unwrap();
        assert!(!json.contains("super-secret"), "明文泄漏进了面板响应");
        assert!(json.contains("sk-su…alue"));
    }

    /// 🔴 **`disabled` 与 `note` 恒存在**（一个 `skip_serializing_if` 都不许有）。
    ///
    /// 回归对象：让它们跟着 `KeyRecord` 一起可选，面板就再也分不出
    /// 「没停用」与「读不出来」。
    #[test]
    fn view_fields_are_always_present_even_when_the_record_omits_them() {
        let rs = vec![rec("a", "kkkkkkkkkkkk", 1)];
        assert_eq!(rs[0].disabled, None);
        assert_eq!(rs[0].note, None);
        let v = serde_json::to_value(&to_key_views(&rs, NOW)[0]).unwrap();
        assert_eq!(v.get("disabled"), Some(&serde_json::json!(false)));
        assert_eq!(v.get("note"), Some(&serde_json::json!(null)));
        assert!(
            v.get("stats").is_some(),
            "stats 也要恒存在（normalize 过的）"
        );
    }

    /// 存量记录没有 `stats` ⇒ 投影出全零，**不是缺席**。
    #[test]
    fn missing_stats_project_as_zeroes() {
        let rs = vec![rec("a", "kkkkkkkkkkkk", 1)];
        assert_eq!(to_key_views(&rs, NOW)[0].stats, KeyStats::empty());
    }

    /// 投影的字段名是面板契约（camelCase）。
    #[test]
    fn view_field_names_are_camel_case() {
        let rs = vec![rec("a", "kkkkkkkkkkkk", 1)];
        let v = serde_json::to_value(&to_key_views(&rs, NOW)[0]).unwrap();
        for name in [
            "addedAt",
            "lastUsedAt",
            "cooldownUntil",
            "cooldownReason",
            "evictedReason",
        ] {
            assert!(v.get(name).is_some(), "缺字段 {name}");
        }
    }

    // ── bucket_counts ──────────────────────────────────────────────────────

    /// 计数**只看 bucket**，且与 `to_key_views` 的分档一致。
    #[test]
    fn counts_are_derived_from_buckets_only() {
        let mut rs = vec![
            rec("a", "kkkkkkkkkkkk", 1),
            rec("b", "kkkkkkkkkkkk", 2),
            rec("c", "kkkkkkkkkkkk", 3),
            rec("d", "kkkkkkkkkkkk", 4),
        ];
        rs[1].disabled = Some(true);
        rs[2].evicted = true;
        rs[3].cooldown_until = NOW + 1;
        let c = bucket_counts(&to_key_views(&rs, NOW));
        assert_eq!(
            c,
            BucketCounts {
                all: 4,
                fresh: 1,
                cooling: 1,
                evicted: 1,
                disabled: 1
            }
        );
        assert_eq!(c.all, c.fresh + c.cooling + c.evicted + c.disabled);
    }

    // ── matches_query ──────────────────────────────────────────────────────

    /// 🔴 **搜索绝不匹配明文**（否则它就是一个慢速预言机）。
    #[test]
    fn search_never_matches_the_plaintext() {
        let rs = vec![rec("abc123", "sk-super-secret-value", 1)];
        let v = &to_key_views(&rs, NOW)[0];
        assert!(!matches_query(v, "super"), "🔴 明文中段绝不许可搜");
        assert!(!matches_query(v, "secret"));
        assert!(matches_query(v, "abc"), "id 可搜");
        assert!(matches_query(v, "sk-su"), "掩码露出来的那一段可搜");
        assert!(matches_query(v, "alue"), "掩码的尾巴同理");
    }

    /// 空查询放行全部；大小写与两头空白都不算数。
    #[test]
    fn empty_query_matches_everything_and_search_is_case_insensitive() {
        let rs = vec![rec("AbC123", "kkkkkkkkkkkkkk", 1)];
        let v = &to_key_views(&rs, NOW)[0];
        assert!(matches_query(v, ""));
        assert!(matches_query(v, "   "));
        assert!(matches_query(v, "abc"));
        assert!(matches_query(v, "  ABC  "));
        assert!(!matches_query(v, "zzz"));
    }
}
