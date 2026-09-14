//! 接线层的判据：锁、历史、装载。
//!
//! 这一层不含业务规则，但它**决定业务规则跑不跑得起来** ——
//! 装载判错一次，整个注册机就是关着的，而面板上看不出原因。

use super::*;
use crate::adapters::storage_test_doubles::MemoryStorage;
use std::collections::BTreeMap;

fn mem() -> Arc<dyn Storage> {
    Arc::new(MemoryStorage::new())
}
fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

// ── 补池锁 ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_lock_keeps_a_second_round_out_until_it_expires() {
    let s = mem();
    assert!(
        acquire_tend_lock(s.as_ref(), 1_000, 5_000).await.is_ok(),
        "第一轮该拿到锁"
    );
    assert!(
        acquire_tend_lock(s.as_ref(), 2_000, 5_000).await.is_err(),
        "锁还在，第二轮却也拿到了",
    );
    // 过期之后放行。
    assert!(
        acquire_tend_lock(s.as_ref(), 6_001, 5_000).await.is_ok(),
        "🔴 锁过期了还挡着 —— 进程被硬杀时收尾一行都不跑，只能靠自然过期放开",
    );
}

#[tokio::test]
async fn releasing_the_lock_lets_the_next_round_in_right_away() {
    let s = mem();
    assert!(acquire_tend_lock(s.as_ref(), 1_000, 900_000).await.is_ok());
    release_tend_lock(s.as_ref()).await;
    assert!(acquire_tend_lock(s.as_ref(), 1_001, 900_000).await.is_ok());
}

/// 🔴 **锁被占着时要把「它什么时候放开」一起带出来。**
///
/// 只回一个「没拿到」的话，面板那颗按钮只能说「现在不行」，
/// 说不出「还要等多久」—— 而运维唯一想知道的就是后面那句。
#[tokio::test]
async fn a_held_lock_reports_when_it_will_be_released() {
    let s = mem();
    assert!(acquire_tend_lock(s.as_ref(), 1_000, 5_000).await.is_ok());
    assert_eq!(
        acquire_tend_lock(s.as_ref(), 2_000, 5_000).await,
        Err(Some(6_000)),
        "🔴 只说「不行」说不出「还要等多久」",
    );
}

/// 一把读不出来的锁**不许把补池永久挡住**。
#[tokio::test]
async fn an_unreadable_lock_value_does_not_wedge_the_registrar_forever() {
    let s = mem();
    s.put(TEND_LOCK_KEY, serde_json::json!("这不是一个对象"), None)
        .await
        .unwrap();
    assert!(
        acquire_tend_lock(s.as_ref(), 0, 1_000).await.is_ok(),
        "🔴 一把写坏的锁把补池永久挡住了 —— 那是个没有任何征兆的停摆",
    );
}

// ── 补池历史 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_history_ring_is_bounded_and_keeps_the_newest() {
    let s = mem();
    for i in 0..(TEND_HISTORY_SIZE + 7) {
        append_history(s.as_ref(), serde_json::json!({ "at": i })).await;
    }
    let v = s.get(TEND_HISTORY_KEY).await.unwrap().unwrap();
    // 🔴 **裸数组**，不是 `{"entries": [...]}` —— 见 `append_history` 上那段。
    let entries = v.as_array().expect("落盘的是裸数组");
    assert_eq!(entries.len(), TEND_HISTORY_SIZE);
    assert_eq!(
        entries.last().unwrap()["at"],
        (TEND_HISTORY_SIZE + 6) as i64,
        "环里留的不是最新那几条",
    );
    assert_eq!(entries[0]["at"], 7, "丢的不是最旧那几条");
}

/// 🔴 **存量部署里这把键是裸数组，读得出来才算移植对了。**
///
/// 这一格钉的是一份**真实部署里的形状**（线上那台 0.4.0 的 `store.json`，
/// 50 条、每条十个字段）。它是被「拿真镜像跑生产数据的副本」这一步抓出来的：
/// 当时的实现读的是 `{"entries": [...]}`，于是那 50 条**一条都读不到**，
/// 而全部 1367 条判据是绿的 —— 它们照着实现写，不是照着存储写。
///
/// ⚠️ 更贵的那一半：写出去的那份包了一层**上一版也读不懂**
/// ⇒ 回滚回去历史同样是空的，而回滚恰恰是升级唯一的兜底。
#[tokio::test]
async fn the_legacy_bare_array_shape_is_read_and_written_back_the_same_way() {
    let s = mem();
    // 照抄线上那一条的字段（值改成无意义的占位）。
    let legacy = serde_json::json!([{
        "skipped": false, "available": 20, "attempted": 0, "minted": 0,
        "mintedByChannel": {}, "failures": [], "at": 1, "primaryChannel": "moemail",
        "durationMs": 4, "trigger": "cron"
    }]);
    s.put(TEND_HISTORY_KEY, legacy, None).await.unwrap();

    append_history(s.as_ref(), serde_json::json!({ "at": 2 })).await;
    let v = s.get(TEND_HISTORY_KEY).await.unwrap().unwrap();
    let entries = v
        .as_array()
        .expect("🔴 写回去的不是裸数组 —— 回滚到上一版之后历史同样读不到");
    assert_eq!(entries.len(), 2, "🔴 存量那一条被吞掉了：{entries:?}");
    assert_eq!(
        entries[0]["primaryChannel"], "moemail",
        "存量那条要原样留着"
    );
}

/// ⚠️ **坏掉的那几条丢掉，不是整份丢掉。**
#[tokio::test]
async fn a_few_malformed_entries_do_not_wipe_the_whole_history() {
    let s = mem();
    s.put(
        TEND_HISTORY_KEY,
        serde_json::json!([{"at": 1}, "坏的", 42, {"at": 2}]),
        None,
    )
    .await
    .unwrap();
    append_history(s.as_ref(), serde_json::json!({ "at": 3 })).await;
    let v = s.get(TEND_HISTORY_KEY).await.unwrap().unwrap();
    let entries = v.as_array().expect("落盘的是裸数组");
    assert_eq!(entries.len(), 3, "好的那两条也被丢掉了：{entries:?}");
}

/// ⚠️ 落盘的 `trigger` 是 `cron` 而不是 `scheduled`。
///
/// 那是**已经写进存量历史**的字面量 —— 改它会让升级前的条目在面板上对不上。
/// 名字与今天的形态有出入是**知情的**，不是漏改。
#[test]
fn the_scheduled_trigger_is_persisted_under_its_historical_name() {
    assert_eq!(TendTrigger::Scheduled.as_str(), "cron");
    assert_eq!(TendTrigger::Manual.as_str(), "manual");
}

// ── 装载 ──────────────────────────────────────────────────────────────────

/// 🔴 **默认是关着的。** 一个没配过注册机的部署不许自己开始打上游。
#[test]
fn a_deployment_that_never_configured_the_registrar_stays_off() {
    let l = load(&env(&[]), &Value::Null);
    assert!(!l.config.enabled);
    assert!(
        l.blockers.is_empty(),
        "关着的注册机不该产 blocker：{:?}",
        l.blockers
    );
    assert!(l.warnings.is_empty(), "关着的注册机不该打启动告警");
    assert!(
        l.config.yyds.is_none() && l.config.moemail.is_none(),
        "关着时不该解析任何凭据",
    );
}

/// 打开但缺凭据 ⇒ **产 blocker，而 `enabled` 一个字都不改**。
///
/// 🔴 「运维明明打开了，面板却说未启用」是另一种撒谎。
/// **真话是「已启用 · 本次没跑起来」。**
#[test]
fn enabling_without_credentials_blocks_the_round_but_never_flips_enabled() {
    let l = load(&env(&[("REGISTRAR_ENABLED", "true")]), &Value::Null);
    assert!(
        l.config.enabled,
        "🔴 被拦下就把 enabled 改成了 false —— 那是另一种撒谎"
    );
    assert!(l.config.blocked);
    assert!(
        l.blockers
            .iter()
            .any(|b| b.code == "registrar_missing_api_key"),
        "{:?}",
        l.blockers,
    );
}

/// 🔴 **通道名写错一个字母，停在这一级，不往下穿透。**
#[test]
fn a_typo_in_the_channel_name_blocks_instead_of_silently_using_another_channel() {
    let l = load(
        &env(&[
            ("REGISTRAR_ENABLED", "true"),
            ("REGISTRAR_CHANNEL", "yydss"),
        ]),
        &serde_json::json!({ "channel": "moemail" }),
    );
    assert!(
        l.blockers
            .iter()
            .any(|b| b.code == "registrar_channel_unknown"),
        "写错一个字母却静默用了存储里那条通道：{:?}",
        l.blockers,
    );
}

/// 🔴 **`min > max` 那条不受 `enabled` 门控。**
///
/// 运维恰恰是在「打开之前先把参数填好」的时候需要看见它。
#[test]
fn an_inverted_delay_range_is_reported_even_while_the_registrar_is_off() {
    let l = load(
        &env(&[
            ("REGISTRAR_MINT_DELAY_MIN_MS", "90000"),
            ("REGISTRAR_MINT_DELAY_MAX_MS", "60000"),
        ]),
        &Value::Null,
    );
    assert!(!l.config.enabled);
    assert!(
        l.blockers
            .iter()
            .any(|b| b.code == "registrar_delay_min_gt_max"),
        "关着的时候就不说了 —— 而那正是运维在填参数的时刻：{:?}",
        l.blockers,
    );
    assert!(l.config.blocked);
}

/// 非法数值 ⇒ **回落默认值 + 一条 notice**，不产 blocker、不 panic。
#[test]
fn an_illegal_number_degrades_and_only_says_so() {
    let l = load(&env(&[("REGISTRAR_TARGET_KEYS", "abc")]), &Value::Null);
    assert_eq!(
        l.config.target_keys,
        super::super::config::DEFAULT_TARGET_KEYS
    );
    assert!(l.notices.iter().any(|n| n.code == "config.invalid"));
    assert!(
        l.blockers.is_empty(),
        "字段回落之后注册机照样跑得起来，不该拦人"
    );
}

/// env 压过存储。
#[test]
fn the_environment_outranks_the_stored_configuration() {
    let l = load(
        &env(&[("REGISTRAR_TARGET_KEYS", "7")]),
        &serde_json::json!({ "targetKeys": "99" }),
    );
    assert_eq!(l.config.target_keys, 7);
}

/// 旧主备键被规整成 `channel`，并留一条 notice。
#[test]
fn the_legacy_primary_key_is_migrated_and_reported() {
    let l = load(&env(&[]), &serde_json::json!({ "primary": "moemail" }));
    assert_eq!(l.config.channel, Channel::Moemail);
    assert!(
        l.notices
            .iter()
            .any(|n| n.code == "registrar_legacy_primary_key"),
        "{:?}",
        l.notices,
    );
}

/// 🔴 **凭据是嵌套的两层，不是扁平键。**
///
/// 存量部署（被移植的那一版）把它们写成 `registrar.yyds.apiKey`，
/// 而 02 §9.18 那张可编辑字段表逐字也是两层。
/// 读成扁平的后果**不是报错，是静默读不到**：
/// 切换那一刻，配好的注册机凭据一把都找不到，
/// 面板上两张卡同时变成「未配置」，而注册机一声不响地停摆。
#[test]
fn the_credentials_are_read_from_the_nested_shape_the_panel_writes() {
    let l = load(
        &env(&[("REGISTRAR_ENABLED", "true")]),
        &serde_json::json!({
            "channel": "yyds",
            "yyds": { "apiKey": "yk" },
            "moemail": { "baseUrl": "https://m.test", "apiKey": "mk" },
        }),
    );
    let y = l
        .config
        .yyds
        .expect("🔴 嵌套的 yyds 凭据没读出来 —— 切换那一刻注册机会静默停摆");
    assert_eq!(y.api_key, "yk");
    let m = l.config.moemail.expect("🔴 moemail 同上");
    assert_eq!(
        (m.base_url.as_str(), m.api_key.as_str()),
        ("https://m.test", "mk")
    );
    assert!(l.blockers.is_empty(), "{:?}", l.blockers);
}

/// 🔴 **存储里的值可能是 JSON 原生类型，不只是字符串。**
///
/// 配置面 `PUT /admin/api/config` 写进去的 `enabled` 是**真布尔**、
/// `targetKeys` 是**真数字**（`Spec` 那张表要求的）。
/// 只认字符串的话，运维在面板上把注册机打开、保存成功，
/// 而装载器读不出那个 `true` ⇒ **它一直是关着的，面板上却显示已启用**。
#[test]
fn native_json_types_from_the_settings_page_are_read_the_same_as_strings() {
    let native = load(
        &env(&[]),
        &serde_json::json!({
            "enabled": true,
            "targetKeys": 33,
            "yyds": { "apiKey": "yk" },
        }),
    );
    assert!(
        native.config.enabled,
        "🔴 面板存的真布尔被当成没设 —— 保存成功而注册机一直关着",
    );
    assert_eq!(native.config.target_keys, 33, "🔴 真数字同理");

    // 存量那份（字符串）照旧读得出来 —— 两种都要认。
    let legacy = load(
        &env(&[]),
        &serde_json::json!({
            "enabled": "true",
            "targetKeys": "33",
            "yyds": { "apiKey": "yk" },
        }),
    );
    assert_eq!(
        (legacy.config.enabled, legacy.config.target_keys),
        (native.config.enabled, native.config.target_keys),
        "存量的字符串形态不许因为这次兼容而失效",
    );
}

/// 🔴 **两条通道都解析** —— 面板要能读出「另一条也配好了」。
#[test]
fn both_channels_are_parsed_so_the_panel_can_offer_a_pre_switch_test() {
    let l = load(
        &env(&[
            ("REGISTRAR_ENABLED", "true"),
            ("REGISTRAR_CHANNEL", "yyds"),
            ("YYDS_API_KEY", "yk"),
            ("MOEMAIL_BASE_URL", "https://m.test"),
            ("MOEMAIL_API_KEY", "mk"),
        ]),
        &Value::Null,
    );
    assert!(l.config.yyds.is_some());
    assert!(
        l.config.moemail.is_some(),
        "🔴 未选中的通道没被解析 —— 「想在切换前先测一下另一条」会变成鸡生蛋",
    );
    assert!(l.blockers.is_empty(), "{:?}", l.blockers);
}

/// 启动告警在**跑得起来**的配置上才打。
#[test]
fn the_startup_warnings_only_fire_on_a_runnable_configuration() {
    let l = load(
        &env(&[
            ("REGISTRAR_ENABLED", "true"),
            ("YYDS_API_KEY", "yk"),
            ("REGISTRAR_MAX_DOMAIN_ATTEMPTS", "5"),
        ]),
        &Value::Null,
    );
    assert!(
        l.warnings
            .iter()
            .any(|w| w.event == "registrar.domain_attempts_costly"),
        "{:?}",
        l.warnings,
    );
}

// ── 台账窄化 ──────────────────────────────────────────────────────────────

/// 🔴 **失败方向：读不出来 ⇒ 空台账，绝不返回「全被屏蔽」。**
///
/// 后者会让注册机一把 key 都铸不出来，**而面板上看不出任何原因。**
#[test]
fn an_unreadable_ledger_degrades_to_empty_never_to_all_blocked() {
    for bad in [
        Value::Null,
        serde_json::json!("字符串"),
        serde_json::json!([1, 2]),
        serde_json::json!({ "entries": "不是对象" }),
    ] {
        let l = narrow_domain_ledger(&bad);
        assert!(l.entries.is_empty(), "{bad}");
    }
}

/// 坏掉的那一条丢掉，**别的照常留着**。
#[test]
fn one_malformed_entry_does_not_take_the_rest_of_the_ledger_with_it() {
    let v = serde_json::json!({
        "updated_at": 5,
        "total": 3,
        "entries": {
            "good.test": { "s": "ok", "at": 1, "n": 1 },
            "bad1.test": { "s": "不认识的状态", "at": 1, "n": 1 },
            "bad2.test": { "s": "ok", "at": "不是数", "n": 1 },
            "dead.test": { "s": "blocked", "at": 2, "n": 2 },
        },
    });
    let l = narrow_domain_ledger(&v);
    assert_eq!(
        l.entries.len(),
        2,
        "好的那两条也丢了：{:?}",
        l.entries.keys()
    );
    assert_eq!(l.entries["good.test"].s, DomainState::Ok);
    assert_eq!(l.entries["dead.test"].s, DomainState::Blocked);
    assert_eq!(l.total, Some(3));
}

// ── 定时轮的接线 ──────────────────────────────────────────────────────────
//
// 这一组对应 TS 侧 `scheduling-wiring.test.ts`。它钉的三件事在本仓都**曾经是反的**：
// 定时轮被 `runnable()` 门控（关着就永不起）、配置是启动快照（面板改完不生效）、
// 对账排在开关后面（关着就永不对账）。三条各自都能让面板变成一个装饰品。

use crate::pool::repo::KeyPoolRepo;
use crate::ports::logger::NullLogger;
use crate::server::test_support::{RecordingLogger, UnreachableFetcher};

fn runtime_with(
    env_pairs: &[(&str, &str)],
    storage: Arc<dyn Storage>,
    logger: Arc<dyn Logger>,
) -> Arc<RegistrarRuntime> {
    let e = env(env_pairs);
    let l = load(&e, &Value::Null);
    Arc::new(RegistrarRuntime {
        config: l.config,
        blockers: l.blockers,
        notices: l.notices,
        warnings: l.warnings,
        storage,
        logger,
        fetcher: Arc::new(UnreachableFetcher),
        env: e,
    })
}

/// 把存储里那份注册机配置改掉 —— **这正是面板做的事**。
async fn write_stored_registrar(s: &dyn Storage, patch: Value) {
    s.put(
        crate::core::config_provenance::CONFIG_KEY,
        serde_json::json!({ "registrar": patch }),
        None,
    )
    .await
    .unwrap();
}

/// 🔴 **注册机关着，索引照样对账。**
///
/// 孤儿记录与幽灵索引项是 `add`/`delete` 半路失败留下的 —— 转发路径天天在跑，
/// 而注册机可能一天都没开过。把对账放在开关后面 = **关着的部署永远不对账**，
/// 而那正是最可能长期关着的那一类部署。
#[tokio::test(start_paused = true)]
async fn a_disabled_registrar_still_reconciles_the_key_pool_index() {
    let s = mem();
    // 一条幽灵：索引里有、记录没有。
    s.put(
        crate::core::pool_index::POOL_INDEX_KEY,
        serde_json::json!({ "v": 1, "ids": ["ghost"] }),
        None,
    )
    .await
    .unwrap();

    let rt = runtime_with(
        &[("REGISTRAR_TEND_INTERVAL_MS", "1000")],
        Arc::clone(&s),
        Arc::new(NullLogger),
    );
    assert!(!rt.runnable(), "这条判据的前提就是它关着");

    let repo = Arc::new(KeyPoolRepo::new(
        Arc::clone(&s),
        Arc::new(NullLogger),
        0,
        21_600_000,
    ));
    spawn_scheduler(Arc::clone(&rt), repo);
    tokio::time::sleep(std::time::Duration::from_millis(600_000)).await;

    let idx = s
        .get(crate::core::pool_index::POOL_INDEX_KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        idx["ids"].as_array().unwrap().len(),
        0,
        "🔴 关着的部署一次都没对过账 —— 幽灵会一直堆下去：{idx}",
    );
}

/// 🔴 **启动时关着，面板打开之后必须真的开始补池 —— 不必重启容器。**
///
/// 反过来的形态是：启动那一刻 `enabled=false` ⇒ 定时器压根没起
/// ⇒ 此后从面板怎么打开都没有任何东西会去读它。
#[tokio::test(start_paused = true)]
async fn turning_the_registrar_on_from_the_panel_takes_effect_without_a_restart() {
    let s = mem();
    let lg = Arc::new(RecordingLogger::default());
    // env 里只给凭据，**不给 REGISTRAR_ENABLED** —— 否则 env 压过存储，面板改不动。
    let rt = runtime_with(
        &[
            ("REGISTRAR_TEND_INTERVAL_MS", "1000"),
            ("YYDS_API_KEY", "yk"),
        ],
        Arc::clone(&s),
        lg.clone(),
    );
    assert!(!rt.config.enabled, "启动时必须是关着的，否则测不出这件事");

    let repo = Arc::new(KeyPoolRepo::new(
        Arc::clone(&s),
        Arc::new(NullLogger),
        0,
        21_600_000,
    ));
    spawn_scheduler(Arc::clone(&rt), repo);
    tokio::time::sleep(std::time::Duration::from_millis(600_000)).await;
    assert!(
        s.get(TEND_HISTORY_KEY).await.unwrap().is_none(),
        "还关着的时候不许补池",
    );

    // 面板把它打开。
    write_stored_registrar(
        s.as_ref(),
        serde_json::json!({ "enabled": "true", "channel": "yyds" }),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(600_000)).await;

    assert!(
        s.get(TEND_HISTORY_KEY).await.unwrap().is_some(),
        "🔴 面板打开了注册机，而没有任何东西再去读它 —— 只能重启容器",
    );
}

/// 🔴 **启动时开着，面板关掉之后必须真的停下来。**
///
/// 与上一条是同一个缺陷的两个方向：读死一份启动快照，两边都动不了。
#[tokio::test(start_paused = true)]
async fn turning_the_registrar_off_from_the_panel_also_takes_effect() {
    let s = mem();
    let rt = runtime_with(
        &[
            ("REGISTRAR_TEND_INTERVAL_MS", "1000"),
            ("YYDS_API_KEY", "yk"),
        ],
        Arc::clone(&s),
        Arc::new(NullLogger),
    );
    write_stored_registrar(
        s.as_ref(),
        serde_json::json!({ "enabled": "true", "channel": "yyds" }),
    )
    .await;

    let repo = Arc::new(KeyPoolRepo::new(
        Arc::clone(&s),
        Arc::new(NullLogger),
        0,
        21_600_000,
    ));
    spawn_scheduler(Arc::clone(&rt), repo);
    tokio::time::sleep(std::time::Duration::from_millis(600_000)).await;
    assert!(
        s.get(TEND_HISTORY_KEY).await.unwrap().is_some(),
        "开着的时候本来就该补池"
    );

    // 面板关掉它。
    write_stored_registrar(s.as_ref(), serde_json::json!({ "enabled": "false" })).await;
    // ⚠️ **先等在途那一轮跑完再清历史**：一轮补池自己带铸号间隔的睡眠，
    // 关掉的那一刻可能正有一轮在飞。先清后等 = 把在途那一轮的收尾当成
    // 「关不掉」，判据会因为时序而不是因为缺陷变红。
    tokio::time::sleep(std::time::Duration::from_millis(600_000)).await;
    s.delete(TEND_HISTORY_KEY).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(600_000)).await;

    assert!(
        s.get(TEND_HISTORY_KEY).await.unwrap().is_none(),
        "🔴 面板关掉了注册机，它还在打上游 —— 启动时开着就再也关不掉",
    );
}
