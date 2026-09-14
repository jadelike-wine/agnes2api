/**
 * 设置板块（`settings`）。
 *
 * 外观**照 kiro**（06 §3.2 ⑨）：`.settings-grid` 两列放卡、集成示例走
 * `.int-card/.int-codewrap/.int-copy/.int-code`、危险区一条 2px 左红边。
 * 功能**照 agnes**（它强得多）：逐字段四元组 + provenance + 干跑校验 +
 * 凭据清空 + 一键重置 + 三态左色条。
 *
 * # 🔴 一条不可妥协的产品原则：**成功提示不得早于回读**
 *
 * `PUT /admin/api/config` 的回执**就是从存储回读出来的那一份**
 * （`src/admin/config.rs` 纪律 ④：不是把 handler 自己算出来的 `next` 换个形状交回去）。
 * ⇒ 面板必须：`await api.put()` → **拿回执重画整页** → 才允许出现任何「已保存」的迹象。
 * 🔴 **而且不许说「已保存并生效」**：`appliedAt` 是「服务器落盘的那一刻」，
 * 不是「所有实例都看见了」。本文件说的是两句话 ——
 *   ① 「已写入，回读到这几个字段真的变了」（高亮 `changed`）；
 *   ② 「其余实例最多 N 秒后看到」（`propagation.visibilityUpperBoundMs`）。
 *
 * # 🔴 三条贯穿本文件的纪律
 *
 * ① **来源推导一个字都不在这里做。** 「谁压过谁」全在后端
 *    (`crate::core::config::resolve_config`)，四元组里的 `effective` 是**真跑一次**
 *    取回来的。⚠️ 「有 env 就显示 env，否则显示存储」那条推法**在一个具体分支上
 *    会说假话**：存储值非法时网关用的是默认值，而推出来的那份会把那个非法值
 *    显示成「生效值」—— 两份实现之间的差，恰好落在最需要面板说实话的那一格上。
 * ② 🔴 **凭据只写不读。** `GET` 永不返回明文（只有 `{configured, hint, lockedBy}`）；
 *    `PUT` 时凭据字段**空串 = 不改**，清空只有 `POST /config/secrets/clear` 一条路。
 *    ⚠️ 把空串实现成清空的后果：运维保存一次设置页就抹掉 `gatewayToken`，
 *    而热实例因为持有者保留上一份快照**当场看不出任何异常**，直到下一次重启才整个停摆。
 * ③ **一切来自接口的内容一律 `textContent`**（`el()` / `elI18n()` 内部就是它），
 *    全文件零 `innerHTML`。
 *
 * # ⚠️ 密度：25 个可编辑项那条论证，与今天的 12 格
 *
 * 06 §2.11 保留 `.field-grid`（agnes 量出来的那条网格公式）的理由是「设置页有
 * **25 个可编辑项**」。🔴 **那是 TS 版的数**：本仓 `src/admin/config.rs::FIELDS`
 * 今天只有 **12 格**（10 个可编辑 + 2 个 `ReadOnly`），注册机那 15 格**随注册机
 * 子树一起来**（`GatewayConfig` 里没有那棵树，硬编一份出来就是在面板层另写一套
 * 来源推导）。⇒ 本文件照旧用 `.field-grid`（卡内两列），字段表**从响应现读**
 * （`fields` / `credentials` / `editable` / `secrets` 四格），
 * **面板不另存一份字段清单** —— 注册机那 15 格落地那天，这里零改动就长出来。
 *
 * # ⚠️ 板块内不许监听 `langchange`：框架层会 `apply(document)` 之后重跑 `onShow()`。
 */
import { api } from "./api.js";
import { t } from "./i18n.js";
import { el, elI18n, toast, openModal, copy } from "./ui.js";

/* ============================================================
   字段分组 —— **只管「画在哪张卡、叫什么名字」，不管「有没有、能不能改」**
   ============================================================ */

/**
 * 卡片分组。
 *
 * 🔴 **这张表不是字段清单的第二份真源**：
 *   · 「这个字段存不存在」由响应里的 `fields` / `credentials` 决定；
 *   · 「能不能改」由响应里的 `editable` + 那一格的 `lockedBy` 决定；
 *   · 本表只回答「画在哪张卡、标题写什么」。
 * ⚠️ 响应里出现一个本表不认识的字段时**照样画出来**（落进 `other` 那张卡，
 * 标题就是它的路径）——**不许静默丢弃**：那会让一个新加的旋钮在面板上消失，
 * 而运维以为这个部署没有它。反向（表里有、响应里没有）则跳过，不画空格子。
 */
const GROUPS = [
  { id: "upstream", titleKey: "section.set.group.upstream", fields: ["agnesBaseUrl", "upstreamTimeoutMs", "upstreamSyncTimeoutMs"] },
  { id: "resilience", titleKey: "section.set.group.resilience", fields: ["maxStrikes", "cooldownRateLimitMs", "cooldownPaymentMs", "cooldownStrikeMs"] },
  { id: "pool", titleKey: "section.set.group.pool", fields: ["poolCacheTtlMs", "poolTouchIntervalMs"] },
  // 注册机分三组：**开关与节奏 / 两条通道的凭据 / 铸号细节**。
  // ⚠️ 两条通道**字母序**（moemail 在前），与注册机板块那两张卡同一条规矩：
  // 任何顺序都会被读成排名，字母序是唯一可辩护的中立规则。
  { id: "registrar", titleKey: "section.set.group.registrar",
    fields: ["registrar.enabled", "registrar.channel", "registrar.targetKeys", "registrar.tendIntervalMs"] },
  { id: "registrarChannels", titleKey: "section.set.group.registrarChannels",
    fields: ["registrar.moemail.baseUrl", "registrar.moemail.apiKey",
             "registrar.yyds.baseUrl", "registrar.yyds.apiKey"] },
  { id: "registrarMint", titleKey: "section.set.group.registrarMint",
    fields: ["registrar.mintBatch", "registrar.mintDelayMinMs", "registrar.mintDelayMaxMs",
             "registrar.codeTimeoutMs", "registrar.maxDomainAttempts",
             "registrar.tokenName", "registrar.agnesPlatformUrl"] },
  { id: "readonly", titleKey: "section.set.group.readonly", fields: ["degraded", "usageStatsEnabled"] },
];

/** 逐字段的说明文案。认不出来的字段不画说明（**不编一句**）。 */
const FIELD_LABEL = {
  agnesBaseUrl: "section.set.field.agnesBaseUrl",
  upstreamTimeoutMs: "section.set.field.upstreamTimeoutMs",
  upstreamSyncTimeoutMs: "section.set.field.upstreamSyncTimeoutMs",
  maxStrikes: "section.set.field.maxStrikes",
  cooldownRateLimitMs: "section.set.field.cooldownRateLimitMs",
  cooldownPaymentMs: "section.set.field.cooldownPaymentMs",
  cooldownStrikeMs: "section.set.field.cooldownStrikeMs",
  poolCacheTtlMs: "section.set.field.poolCacheTtlMs",
  poolTouchIntervalMs: "section.set.field.poolTouchIntervalMs",
  degraded: "section.set.field.degraded",
  usageStatsEnabled: "section.set.field.usageStatsEnabled",
  gatewayToken: "section.set.field.gatewayToken",
  "registrar.enabled": "section.set.field.registrar.enabled",
  "registrar.channel": "section.set.field.registrar.channel",
  "registrar.targetKeys": "section.set.field.registrar.targetKeys",
  "registrar.tendIntervalMs": "section.set.field.registrar.tendIntervalMs",
  "registrar.mintBatch": "section.set.field.registrar.mintBatch",
  "registrar.mintDelayMinMs": "section.set.field.registrar.mintDelayMinMs",
  "registrar.mintDelayMaxMs": "section.set.field.registrar.mintDelayMaxMs",
  "registrar.codeTimeoutMs": "section.set.field.registrar.codeTimeoutMs",
  "registrar.maxDomainAttempts": "section.set.field.registrar.maxDomainAttempts",
  "registrar.tokenName": "section.set.field.registrar.tokenName",
  "registrar.agnesPlatformUrl": "section.set.field.registrar.agnesPlatformUrl",
  "registrar.moemail.baseUrl": "section.set.field.channel.moemailBaseUrl",
  "registrar.moemail.apiKey": "section.set.field.channel.moemailApiKey",
  "registrar.yyds.baseUrl": "section.set.field.channel.yydsBaseUrl",
  "registrar.yyds.apiKey": "section.set.field.channel.yydsApiKey",
};

/**
 * 逐字段错误码 → 文案键。
 *
 * 🔴 **面板靠 `code` 选五语言文案，绝不解析 `message`**（后端那句是中文的、
 * 是给日志看的）。
 * ⚠️ 认不出来的 code 走 `section.set.err.unknown`，**把原始 code 一起画出来**
 * —— 「面板还不认识这条」比一句含糊的「保存失败」有用得多。
 *
 * ⚠️⚠️ **这张表刻意只收今天真的发得出来的那 13 条**，
 * 而 `src/core/config_errors.rs::ConfigErrorCode::ALL` 有 22 条 ——
 * 少的那 9 条（`not_a_boolean` / `too_long` / `not_a_channel` / `channel_required` /
 * `delay_min_gt_max` / `channel_credentials_missing` / `legacy_*` 三条）
 * **今天没有任何路径能产出**：它们的字段全在注册机子树里，而那棵树不存在。
 * 🔴 **提前把它们翻译进字典，正是 `FIELDS` 那段注释点名的漂移起点**
 * 「面板字典里有一条永远选不到的文案」—— 一条没人看得到的文案既不会被审核、
 * 也不会被发现写错。注册机落地那天它们会以
 * 「面板还不认识这条错误码：channel_required」的形态**自己喊出来**，那是想要的信号。
 */
const ERR_KEY = {
  unknown_field: "section.set.err.unknownField",
  locked_by_env: "section.set.err.lockedByEnv",
  not_an_integer: "section.set.err.notAnInteger",
  below_min: "section.set.err.belowMin",
  not_a_string: "section.set.err.notAString",
  empty: "section.set.err.empty",
  not_a_url: "section.set.err.notAUrl",
  gateway_token_required: "section.set.err.gatewayTokenRequired",
  whitespace_padded: "section.set.err.whitespacePadded",
  not_sendable: "section.set.err.notSendable",
  too_short: "section.set.err.tooShort",
  same_as_admin_token: "section.set.err.sameAsAdminToken",
  config_unloadable: "section.set.err.configUnloadable",
  // 📜 这几条曾经被刻意留空，理由是「它们的字段全在注册机子树里，而那棵树不存在」。
  // 子树进了配置面（`FIELDS` 27 格），**它们现在真的发得出来** ⇒ 翻译补上。
  // ⚠️ `channel_required` / `legacy_channel_env` **仍然没有产生点**，
  // 如实不收 —— 收了就是那段注释点名的「漂移起点」。
  not_a_boolean: "section.set.err.notABoolean",
  too_long: "section.set.err.tooLong",
  not_a_channel: "section.set.err.notAChannel",
  delay_min_gt_max: "section.set.err.delayMinGtMax",
  channel_credentials_missing: "section.set.err.channelCredentialsMissing",
  legacy_channel_key: "section.set.err.legacyChannelKey",
  legacy_fallback_ignored: "section.set.err.legacyFallbackIgnored",
};

/**
 * 集成示例的三种语言。**只有形态差别，请求本身是同一条。**
 *
 * ⚠️⚠️ **文案键一律写成字面量，不许用模板串拼**：`src/webui/mod.rs` 的
 * `referenced_keys()` 扫的是**双引号字面量**，拼出来的 key 它一个都看不见 ⇒
 * 那几条词条会被「字典里每个 key 都被引用过」那一格判成孤儿而当场红。
 */
const SAMPLE_LANGS = [
  { id: "curl", labelKey: "section.set.lang.curl" },
  { id: "python", labelKey: "section.set.lang.python" },
  { id: "node", labelKey: "section.set.lang.node" },
];

/* ============================================================
   状态
   ============================================================ */

const state = {
  /** `GET /admin/api/config` 的整包。`null` = 还没拉到 / 上一次失败。 */
  config: null,
  /** 拉取失败时那句话的 key。 */
  failedKey: null,
  /** 用户改过的那些格子：`path → 原始输入串`。**只有它们进 patch。** */
  dirty: new Map(),
  /** 上一次保存回读出来真的变了的字段（高亮用）。 */
  changed: [],
  /** 逐字段错误：`path → {code, params}`。`""` 这个 path 装的是整体性错误。 */
  errors: new Map(),
  /** 集成示例卡。`null` = 还没拉到目录（那张卡就不画，**不画一张空的**）。 */
  catalog: null,
  sampleProtocol: null,
  sampleLang: "curl",
  busy: false,
};

let ctl = null;
const nodes = {};

function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }

function langOf() { return document.documentElement.lang || "zh-CN"; }

function fmtSeconds(ms) {
  return t("section.set.seconds", { n: String(Math.round(ms / 100) / 10) });
}

/* ============================================================
   骨架
   ============================================================ */

function build(node) {
  clear(node);

  const head = el("div", { class: "section-header" });
  head.appendChild(elI18n("h2", "section.set.title", { class: "section-title" }));
  const actions = el("div", { class: "section-actions" });
  nodes.reloadBtn = elI18n("button", "section.set.reload", { type: "button", class: "btn btn-sm btn-outline" });
  nodes.reloadBtn.addEventListener("click", () => load());
  actions.appendChild(nodes.reloadBtn);
  head.appendChild(actions);
  node.appendChild(head);

  nodes.banners = el("div");
  node.appendChild(nodes.banners);

  nodes.grid = el("div", { class: "settings-grid" });
  node.appendChild(nodes.grid);

  // ⚠️ **保存条有自己的容器**，不是每次 `render()` 现插一个 ——
  //    `insertBefore` 版本每重画一次就多一条保存条（改一格字段就多一条），
  //    而多出来的那几条各自绑着旧的 `nodes.saveBtn`，点哪一条都可能是上一轮的状态。
  nodes.barHost = el("div");
  node.appendChild(nodes.barHost);

  // 集成示例与危险区都是**整行**的卡，不进两列栅格。
  nodes.examples = el("div");
  node.appendChild(nodes.examples);

  nodes.danger = el("div");
  node.appendChild(nodes.danger);
}

/* ============================================================
   取数
   ============================================================ */

async function load() {
  if (ctl) ctl.abort();
  ctl = new AbortController();
  const mine = ctl;
  state.dirty.clear();
  state.errors.clear();
  try {
    const body = await api.get("/config", { signal: mine.signal });
    if (mine !== ctl) return;
    state.config = body;
    state.failedKey = null;
  } catch (e) {
    if (mine !== ctl) return;
    if (e && e.status === 401) return;
    state.config = null;
    state.failedKey = e && typeof e.status === "number" && e.status > 0
      ? "section.set.loadFailedHttp"
      : "section.set.loadFailedNetwork";
  }
  render();
  // 集成示例的目录**单独拉、失败不影响设置页主体**：它是一张说明卡，
  // 不是这个板块存在的理由。⚠️ 刻意不 `await` 进上面那条路径。
  loadCatalog();
}

async function loadCatalog() {
  if (state.catalog) { renderExamples(); return; }
  const mine = ctl;
  try {
    const body = await api.get("/models", { signal: mine ? mine.signal : undefined });
    if (mine !== ctl) return;
    state.catalog = body;
    if (!state.sampleProtocol && Array.isArray(body.protocols) && body.protocols.length > 0) {
      state.sampleProtocol = body.protocols[0].id;
    }
  } catch (e) {
    // 🔴 **不弹 toast、不画一张空卡**：拉不到目录只是少一张说明卡。
    if (mine !== ctl) return;
    state.catalog = null;
  }
  renderExamples();
}

/* ============================================================
   渲染 —— 横幅
   ============================================================ */

function errText(e) {
  const key = ERR_KEY[e.code];
  if (!key) return t("section.set.err.unknown", { code: String(e.code) });
  return t(key, e.params || {});
}

/** 一条逐字段错误画成一行：`字段名：这句话`。字段为空时只画那句话。 */
function errLine(e) {
  const label = e.field && FIELD_LABEL[e.field] ? t(FIELD_LABEL[e.field]) : e.field;
  return label ? `${label}：${errText(e)}` : errText(e);
}

function renderBanners() {
  const host = nodes.banners;
  clear(host);
  if (state.failedKey) {
    const b = el("div", { class: "banner banner-danger" });
    b.appendChild(elI18n("span", state.failedKey));
    host.appendChild(b);
    return;
  }
  const c = state.config;
  if (!c) return;

  // 🔴 **`fields === null` = 诊断视图**：存储里那份读得出来、却构造不出一份合法
  //    配置。后端刻意**降级成诊断视图而不是 500** —— 那是运维唯一的出路
  //    （在它之前，配置一旦装载不起来，「把口令重新填回去」这条自救路径自己也走不通）。
  if (c.fields === null || c.fields === undefined) {
    const b = el("div", { class: "banner banner-danger" });
    b.appendChild(elI18n("span", "section.set.diagnostic"));
    host.appendChild(b);
  } else if (c.configDegraded) {
    // 降级 ≠ 装不起来：有字段回落到了默认值，网关照常跑。
    const b = el("div", { class: "banner banner-warn" });
    b.appendChild(elI18n("span", "section.set.degraded"));
    host.appendChild(b);
  }

  for (const e of c.loadBlocked || []) {
    const b = el("div", { class: "banner banner-danger" });
    b.appendChild(el("span", null, errLine(e)));
    host.appendChild(b);
  }
  // ⚠️ **`loadNotices` 今天恒为空**（它唯一的一族属于注册机子树），但字段是对外
  //    契约的一格 —— 🔴 **别把「今天没有」读成「这里没有东西要说」**，照画。
  for (const e of c.loadNotices || []) {
    const b = el("div", { class: "banner banner-info" });
    b.appendChild(el("span", null, errLine(e)));
    host.appendChild(b);
  }
}

/* ============================================================
   渲染 —— 一个字段
   ============================================================ */

/**
 * 三档状态的左色条：**锁定 / 变化 / 非法**。
 * 🔴 **颜色 + 非颜色双信号**（WCAG 1.4.1）：`.is-locked` 另配一个 🔒 前缀与
 * `disabled`，`.is-invalid` 另配一行 `.form-error` 文案，`.is-changed` 另配一句
 * 「刚刚变了」。只留颜色就违反 1.4.1，`state_is_not_color_only` 那格钉的是同一条线。
 */
function fieldNode(path, quad, editable) {
  const locked = quad.lockedBy !== null && quad.lockedBy !== undefined;
  // 「能不能改」= 后端说它可编辑 **且** 没被 env 锁住。两条缺一不可：
  // `editable` 排掉 `ReadOnly`，`lockedBy` 排掉被环境变量钉死的。
  const canEdit = editable && !locked;
  const err = state.errors.get(path);

  const wrap = el("div", { class: "form-group field-state" });
  if (locked) wrap.classList.add("is-locked");
  if (err) wrap.classList.add("is-invalid");
  if (state.changed.includes(path)) wrap.classList.add("is-changed");

  const inputId = `set-${path}`;
  const label = el("label", { class: "form-label", for: inputId });
  if (locked) label.appendChild(el("span", { "aria-hidden": "true" }, "🔒 "));
  label.appendChild(el("span", null, FIELD_LABEL[path] ? t(FIELD_LABEL[path]) : path));
  wrap.appendChild(label);

  // ⚠️ **输入控件的类型从 `effective` 的 JS 类型推**，不在面板里再抄一份
  //    `src/admin/config.rs::FIELDS` 的 `Spec`（纪律 ① 的同一条线：第二份表必漂）。
  const eff = quad.effective;
  const isBool = typeof eff === "boolean";
  const isNum = typeof eff === "number";

  if (!canEdit) {
    // 只读呈现：**画生效值**，不画一个改不动的输入框。
    const rv = el("div", { class: "readonly-value" });
    rv.appendChild(el("code", { class: "rv-text", id: inputId },
      eff === null || eff === undefined ? "—" : String(eff)));
    wrap.appendChild(rv);
  } else if (isBool) {
    // 🔴 **开关就画成开关。** 画成文本框的话，运维要手打 `true` 这个词，
    //    而打错一个字母拿到的是一条 `not_a_boolean` —— 一个本来不该存在的错误。
    // ⚠️ `.form-check` 是**容器**类（样式挂在 `.form-check input` 上），
    //    直接扣在 input 上拿不到尺寸与配色。
    const box = el("div", { class: "form-check" });
    const input = el("input", { id: inputId, type: "checkbox" });
    input.checked = state.dirty.has(path) ? state.dirty.get(path) === "true" : eff === true;
    input.addEventListener("change", () => {
      state.dirty.set(path, input.checked ? "true" : "false");
      state.changed = state.changed.filter((p) => p !== path);
      wrap.classList.remove("is-changed");
      paintSaveBar();
    });
    box.appendChild(input);
    wrap.appendChild(box);
  } else {
    const input = el("input", {
      id: inputId,
      class: "form-control",
      type: isNum ? "number" : "text",
      value: state.dirty.has(path) ? state.dirty.get(path) : String(eff ?? ""),
    });
    input.addEventListener("input", () => {
      state.dirty.set(path, input.value);
      // 改过之后那条「刚刚变了」的高亮就不再成立了 —— 它说的是**上一次回读**。
      state.changed = state.changed.filter((p) => p !== path);
      wrap.classList.remove("is-changed");
      paintSaveBar();
    });
    wrap.appendChild(input);
  }

  // 出处（provenance）。🔴 **锁定的那一格必须说清是被哪个环境变量锁的** ——
  // 否则运维会一直改存储、一直没效果，然后去 compose 里 grep 一个搜不到的名字。
  const hint = el("div", { class: "form-hint" });
  if (locked) {
    hint.appendChild(el("span", null, t("section.set.lockedBy", { by: String(quad.lockedBy) })));
  } else if (!editable) {
    // 🔴 **判据是「后端说它不可编辑」，不是「它是个布尔」。**
    //    早一版写的是 `isBool` —— 那时候仓里唯一的两个布尔恰好都是只读的
    //    （`degraded` / `usageStatsEnabled`），于是两条判据看起来等价。
    //    `registrar.enabled` 进来之后它当场变成一句假话：一个真能改的开关，
    //    底下印着「这一格是装载的产物，不是旋钮」。
    hint.appendChild(elI18n("span", "section.set.readOnlyField"));
  } else {
    const stored = quad.stored === null || quad.stored === undefined
      ? t("section.set.storedNone")
      : String(quad.stored);
    hint.appendChild(el("span", null, t("section.set.storedIs", { v: stored })));
  }
  wrap.appendChild(hint);

  if (err) {
    // `.form-error` 是那条**非颜色**线索：一句写出来的话，不是一根红边。
    wrap.appendChild(el("div", { class: "form-error" }, errText(err)));
  }
  return wrap;
}

/* ============================================================
   渲染 —— 凭据卡（纪律 ②）
   ============================================================ */

function credentialCard(path, cred) {
  const card = el("div", { class: "card" });
  const header = el("div", { class: "card-header" });
  header.appendChild(el("h3", null, FIELD_LABEL[path] ? t(FIELD_LABEL[path]) : path));
  header.appendChild(el("span", {
    class: cred.configured ? "badge badge-success" : "badge badge-muted",
  }, t(cred.configured ? "section.set.secretConfigured" : "section.set.secretMissing")));
  card.appendChild(header);

  card.appendChild(elI18n("p", "section.set.secretDesc", { class: "settings-card-desc" }));

  const locked = cred.lockedBy !== null && cred.lockedBy !== undefined;

  // 🔴 **只画末 4 位的提示，永不画明文**（后端 `GET` 根本不发明文）。
  //    ⚠️ 短于 5 位的口令后端一律不给 `hint` —— 对一把 4 位的口令，
  //    末 4 位就是全部，那不叫提示，那叫 reveal。
  const rv = el("div", { class: "readonly-value field-state" });
  if (locked) rv.classList.add("is-locked");
  rv.appendChild(el("code", { class: "rv-text" },
    cred.configured ? (cred.hint ? `••••${cred.hint}` : t("section.set.secretNoHint")) : "—"));
  card.appendChild(rv);

  const group = el("div", { class: "form-group" });
  const inputId = `set-secret-${path}`;
  group.appendChild(el("label", { class: "form-label", for: inputId },
    t(locked ? "section.set.secretLockedLabel" : "section.set.secretNewLabel")));
  const input = el("input", {
    id: inputId, class: "form-control", type: "password", autocomplete: "new-password",
  });
  input.disabled = locked;
  input.addEventListener("input", () => {
    // ⚠️ **空串一律不进 patch**（纪律 ②：空串 = 不改，不是清空）。
    if (input.value === "") state.dirty.delete(path);
    else state.dirty.set(path, input.value);
    paintSaveBar();
  });
  group.appendChild(input);
  const hint = el("div", { class: "form-hint" });
  hint.appendChild(el("span", null, locked
    ? t("section.set.lockedBy", { by: String(cred.lockedBy) })
    : t("section.set.secretHint")));
  group.appendChild(hint);
  const err = state.errors.get(path);
  if (err) group.appendChild(el("div", { class: "form-error" }, errText(err)));
  card.appendChild(group);

  // 清空。🔴 **这是清空凭据的唯一入口**，而且后果要在**二次确认框里**就说清。
  const clearBtn = elI18n("button", "section.set.secretClear", { type: "button", class: "btn btn-sm btn-danger" });
  clearBtn.addEventListener("click", () => confirmClear(path, cred));
  card.appendChild(clearBtn);
  return card;
}

function confirmClear(path, cred) {
  const body = el("div");
  body.appendChild(elI18n("p", "section.set.clearConfirm"));
  // ⚠️⚠️ **两档必须分开说**：清掉网关口令是「下一次冷启动会失败」，
  //    而清掉一把子系统凭据只是那个子系统停摆 —— 拿砖机的措辞去说一件不砖机的事，
  //    会让运维在一次小事故上按大事故处置。
  //    判据是「清完之后环境变量里还有没有」，那正是 `lockedBy` 这一格。
  const stillFromEnv = cred.lockedBy !== null && cred.lockedBy !== undefined;
  body.appendChild(elI18n("p", stillFromEnv ? "section.set.clearStillEnv" : "section.set.clearFatal",
    { class: stillFromEnv ? "text-muted" : "text-danger" }));
  openModal("section.set.clearTitle", body, [
    { labelKey: "common.cancel" },
    { labelKey: "section.set.secretClear", danger: true, onClick: () => { doClear(path); } },
  ]);
}

async function doClear(path) {
  if (state.busy) return;
  state.busy = true;
  try {
    const body = await api.post("/config/secrets/clear", { path });
    // 🔴 **回执里就带着回读出来的 `fields` / `credentials`**，直接拿它重画。
    absorb(body);
    // 两句话分开说：清掉了什么、清完之后这份配置还装不装得起来。
    if (body.gatewayTokenMissing) {
      toast(t("section.set.clearedFatal"), "err", { sticky: true });
    } else if (body.stillConfigured) {
      toast(t("section.set.clearedStillEnv"), "warn", { sticky: true });
    } else {
      toast(t("section.set.cleared"), "ok");
    }
  } catch (e) {
    if (!(e && e.status === 401)) showErrors(e);
  } finally {
    state.busy = false;
    render();
  }
}

/* ============================================================
   保存 / 干跑
   ============================================================ */

/**
 * 把脏格子拼成 patch。
 *
 * **扁平点分路径，不是嵌套对象**：「缺席 = 不改」这条语义只有扁平形态表达得清楚
 * ——嵌套形态下 `{a:{b:1}}` 到底是「只改 b」还是「把整个 a 换掉」取决于合并规则
 * 写在哪一层，而两种读法都有人会按。
 *
 * ⚠️ **数字格子送数字、不送串**：后端 `Spec::Int` 判的是 `as_i64`，
 * 送 `"30000"` 会当场拿一条 `not_an_integer`。
 */
function buildPatch() {
  const c = state.config;
  const fields = (c && c.fields) || {};
  const patch = {};
  for (const [path, raw] of state.dirty) {
    if (raw === "") continue; // 凭据的空串 = 不改；公开字段的空串在后端是 `empty`
    const quad = fields[path];
    // ⚠️ **布尔格子送真布尔**，同「数字格子送数字」那条：
    //    后端 `Spec::Bool` 判的是 `is_boolean()`，送 `"true"` 会当场拿一条
    //    `not_a_boolean` —— 而那正是本层要替运维挡掉的。
    if (quad && typeof quad.effective === "boolean") {
      patch[path] = raw === "true";
    } else if (quad && typeof quad.effective === "number") {
      const n = Number(raw);
      // 🔴 **认不出来的一律原样送过去**，让后端那份唯一的校验器去拒 ——
      //    在这里先拦一道就是把校验实现成了两份，而两份迟早分叉。
      patch[path] = Number.isFinite(n) && `${n}` === raw.trim() ? n : raw;
    } else {
      patch[path] = raw;
    }
  }
  return patch;
}

/** 400 的响应体里 `errors` 在**顶层**。把它摊进 `state.errors`。 */
function showErrors(e) {
  state.errors.clear();
  const list = e && e.body && Array.isArray(e.body.errors) ? e.body.errors : null;
  if (!list || list.length === 0) {
    toast(t(e && typeof e.status === "number" && e.status > 0
      ? "section.set.saveFailedHttp" : "section.set.saveFailedNetwork"), "err", { sticky: true });
    return;
  }
  for (const item of list) state.errors.set(String(item.field || ""), item);
  toast(t("section.set.saveRejected", { n: String(list.length) }), "err", { sticky: true });
}

async function save() {
  if (state.busy) return;
  const patch = buildPatch();
  if (Object.keys(patch).length === 0) { toast(t("section.set.nothingToSave"), "info"); return; }
  state.busy = true;
  paintSaveBar();
  try {
    // 🔴 **这一行之前不许出现任何「已保存」的迹象。**
    const body = await api.put("/config", { patch });
    absorb(body);
    // 🔴 **说的是「回读到这几个字段真的变了」，不是「已保存并生效」。**
    //    ⚠️ `changed` 是按**回读出来的生效值**比的，不是按请求体比的 ——
    //    一个被 env 锁着的字段即便被写进去，生效值也纹丝不动。
    const n = (body.changed || []).length;
    const secrets = (body.credentialsChanged || []).length;
    toast(n === 0 && secrets === 0
      ? t("section.set.savedNoChange")
      : t("section.set.saved", { n: String(n + secrets) }), "ok");
    // 传播窗口单独说一句：**其余实例最多这么久之后才看得见**。
    const ms = body.propagation ? body.propagation.visibilityUpperBoundMs : null;
    if (typeof ms === "number" && ms > 0) toast(t("section.set.propagation", { s: fmtSeconds(ms) }), "info");
  } catch (e) {
    if (!(e && e.status === 401)) showErrors(e);
  } finally {
    state.busy = false;
    render();
  }
}

/**
 * 干跑。
 *
 * ⚠️⚠️ 它跑的是与 `PUT` **同一个**校验器、上下文也逐字段一样 ——
 * 另写一份「轻量版校验」就等于让干跑与真跑给出不同答案，而实测过的分叉方向是
 * 「干跑放行、真跑拒绝」：运维读到的是「面板刚说没问题」。
 * 🔴 **它一次存储写都不产生。**
 */
async function dryRun() {
  if (state.busy) return;
  const patch = buildPatch();
  if (Object.keys(patch).length === 0) { toast(t("section.set.nothingToSave"), "info"); return; }
  state.busy = true;
  paintSaveBar();
  try {
    const body = await api.post("/config/validate", { patch });
    state.errors.clear();
    toast(t("section.set.dryRunOk", { n: String((body.changed || []).length) }), "ok");
  } catch (e) {
    if (!(e && e.status === 401)) showErrors(e);
  } finally {
    state.busy = false;
    render();
  }
}

/** 把一份「带回读」的回执并进状态。四条端点（PUT / clear / reset）共用。 */
function absorb(body) {
  if (!state.config) state.config = {};
  const c = state.config;
  c.fields = body.fields;
  c.credentials = body.credentials;
  c.configDegraded = body.configDegraded;
  c.loadBlocked = body.loadBlocked || [];
  if (body.loadNotices) c.loadNotices = body.loadNotices;
  if (body.resetBlocked) c.resetBlocked = body.resetBlocked;
  if (body.propagation) c.propagation = body.propagation;
  state.changed = Array.isArray(body.changed) ? body.changed.slice() : [];
  state.dirty.clear();
  state.errors.clear();
}

/* ============================================================
   危险区
   ============================================================ */

function renderDanger() {
  const host = nodes.danger;
  clear(host);
  const c = state.config;
  if (!c) return;

  const card = el("div", { class: "card danger-zone" });
  const header = el("div", { class: "card-header" });
  header.appendChild(elI18n("h3", "section.set.dangerTitle"));
  card.appendChild(header);
  card.appendChild(elI18n("p", "section.set.dangerDesc", { class: "settings-card-desc" }));

  // 🔴 **按钮 `align-self: flex-start`（`.danger-zone .btn` 已经这么写）** ——
  //    别让销毁凭据的按钮变成横贯整卡的红色大条：agnes 为这条踩过，真机截图上
  //    整页最抢眼的就是三颗销毁按钮。
  const btn = elI18n("button", "section.set.resetBtn", { type: "button", class: "btn btn-danger" });
  btn.addEventListener("click", () => confirmReset());
  card.appendChild(btn);
  host.appendChild(card);
}

function confirmReset() {
  const c = state.config;
  const body = el("div");
  body.appendChild(elI18n("p", "section.set.resetConfirm"));
  // 🔴 **后果在二次确认框里就说清，而那一刻还没发过任何写请求** ——
  //    `resetBlocked` 是 `GET` 上就给的（零额外存储读），正是为了这一刻。
  const blocked = (c && c.resetBlocked) || [];
  if (blocked.length > 0) {
    const box = el("div", { class: "banner banner-danger" });
    const lines = el("div");
    lines.appendChild(elI18n("div", "section.set.resetWillBreak"));
    for (const e of blocked) lines.appendChild(el("div", null, errLine(e)));
    box.appendChild(lines);
    body.appendChild(box);
  } else {
    body.appendChild(elI18n("p", "section.set.resetSafe", { class: "text-muted" }));
  }
  openModal("section.set.resetTitle", body, [
    { labelKey: "common.cancel" },
    { labelKey: "section.set.resetBtn", danger: true, onClick: () => { doReset(); } },
  ]);
}

async function doReset() {
  if (state.busy) return;
  state.busy = true;
  try {
    // 🔴 `confirm: true` 是**必填的，不是装饰**：一条不带请求体就会重置整份配置
    //    的端点，在枚举式鉴权矩阵里会把夹具自己的配置抹掉。
    const body = await api.post("/config/reset", { confirm: true });
    absorb(body);
    const lost = (body.credentialsChanged || []).length;
    toast(lost > 0 ? t("section.set.resetDoneLost", { n: String(lost) }) : t("section.set.resetDone"),
      lost > 0 ? "warn" : "ok", lost > 0 ? { sticky: true } : undefined);
  } catch (e) {
    if (!(e && e.status === 401)) showErrors(e);
  } finally {
    state.busy = false;
    render();
  }
}

/* ============================================================
   集成示例（`.int-card`，照 kiro 的组件）
   ============================================================ */

/** 这条协议上第一个可用的模型 id。拿不到就用一个占位提示，**不编一个模型名**。 */
function firstModelFor(protocolId) {
  const models = (state.catalog && state.catalog.models) || [];
  for (const m of models) {
    if (Array.isArray(m.protocols) && m.protocols.includes(protocolId)) return m.id;
  }
  return null;
}

/**
 * 拼一段示例代码。
 *
 * 🔴 **一个字节的协议知识都不在这里写死**：方法、路径、鉴权头、示例请求体
 * 全部来自 `GET /admin/api/models`（`src/admin/models.rs` 的 `PROTOCOLS` 是
 * 「怎么调这个网关」那份单一真源）。本函数只负责把它们摆成 curl / Python / Node 的形状。
 * ⚠️ 口令一律画成 `$GATEWAY_TOKEN` 这样的占位符 —— **绝不把真口令拼进示例**
 * （它会被复制、被贴进工单、被截图）。
 */
function sampleCode(proto, lang) {
  const model = firstModelFor(proto.id);
  const path = String(proto.pathTemplate || "").split("{model}").join(model || "MODEL");
  const url = `${location.origin}${path}`;
  const header = String(proto.authHeader || "authorization");
  const headerValue = header === "authorization" ? "Bearer $GATEWAY_TOKEN" : "$GATEWAY_TOKEN";
  let bodyText = "{}";
  try {
    const sample = proto.sampleBody && typeof proto.sampleBody === "object" ? { ...proto.sampleBody } : {};
    if (model && "model" in sample) sample.model = model;
    bodyText = JSON.stringify(sample, null, 2);
  } catch (e) { bodyText = "{}"; }

  if (lang === "curl") {
    return [
      `curl -X ${proto.method} '${url}' \\`,
      `  -H '${header}: ${headerValue}' \\`,
      "  -H 'content-type: application/json' \\",
      `  -d '${bodyText.split("\n").join("")}'`,
    ].join("\n");
  }
  if (lang === "python") {
    return [
      "import os, requests",
      "",
      `resp = requests.request(`,
      `    "${proto.method}",`,
      `    "${url}",`,
      `    headers={"${header}": "${headerValue.split("$GATEWAY_TOKEN").join('" + os.environ["GATEWAY_TOKEN"] + "')}",`,
      '            "content-type": "application/json"},',
      `    json=${bodyText},`,
      ")",
      "print(resp.status_code, resp.text)",
    ].join("\n");
  }
  return [
    `const resp = await fetch("${url}", {`,
    `  method: "${proto.method}",`,
    `  headers: {`,
    `    "${header}": \`${headerValue.split("$GATEWAY_TOKEN").join("${process.env.GATEWAY_TOKEN}")}\`,`,
    '    "content-type": "application/json",',
    "  },",
    `  body: JSON.stringify(${bodyText}),`,
    "});",
    "console.log(resp.status, await resp.text());",
  ].join("\n");
}

function renderExamples() {
  const host = nodes.examples;
  if (!host) return;
  clear(host);
  const cat = state.catalog;
  // 🔴 **拉不到目录就不画这张卡**，不画一张空的（06 §3.2 ① 末行那条同源纪律）。
  if (!cat || !Array.isArray(cat.protocols) || cat.protocols.length === 0) return;

  const card = el("div", { class: "card int-card" });
  const header = el("div", { class: "card-header" });
  header.appendChild(elI18n("h3", "section.set.examplesTitle"));
  card.appendChild(header);
  const bodyWrap = el("div", { class: "card-body" });
  bodyWrap.appendChild(elI18n("p", "section.set.examplesDesc", { class: "settings-card-desc" }));

  const protoGroup = el("div", { class: "seg-group", role: "group" });
  protoGroup.setAttribute("aria-label", t("section.set.examplesProtocol"));
  for (const p of cat.protocols) {
    const b = el("button", { type: "button", class: "seg-btn" }, p.label);
    const on = p.id === state.sampleProtocol;
    b.classList.toggle("is-active", on);
    b.setAttribute("aria-pressed", on ? "true" : "false");
    b.addEventListener("click", () => { state.sampleProtocol = p.id; renderExamples(); });
    protoGroup.appendChild(b);
  }
  bodyWrap.appendChild(protoGroup);

  const langGroup = el("div", { class: "seg-group", role: "group" });
  langGroup.setAttribute("aria-label", t("section.set.examplesLang"));
  for (const spec of SAMPLE_LANGS) {
    const b = elI18n("button", spec.labelKey, { type: "button", class: "seg-btn" });
    const on = spec.id === state.sampleLang;
    b.classList.toggle("is-active", on);
    b.setAttribute("aria-pressed", on ? "true" : "false");
    b.addEventListener("click", () => { state.sampleLang = spec.id; renderExamples(); });
    langGroup.appendChild(b);
  }
  bodyWrap.appendChild(langGroup);

  const proto = cat.protocols.find((p) => p.id === state.sampleProtocol) || cat.protocols[0];
  const code = sampleCode(proto, state.sampleLang);
  const wrap = el("div", { class: "int-codewrap" });
  const copyBtn = elI18n("button", "common.copy", { type: "button", class: "btn btn-sm btn-outline int-copy" });
  copyBtn.addEventListener("click", () => copy(code));
  wrap.appendChild(copyBtn);
  const pre = el("pre", { class: "int-code" });
  pre.appendChild(el("code", null, code));
  wrap.appendChild(pre);
  bodyWrap.appendChild(wrap);

  card.appendChild(bodyWrap);
  host.appendChild(card);
}

/* ============================================================
   渲染 —— 主体
   ============================================================ */

function saveBar() {
  const bar = el("div", { class: "card" });
  const row = el("div", { class: "mng-toolbar" });
  nodes.dirtyNote = el("span", { class: "mng-inline-note" });
  row.appendChild(nodes.dirtyNote);
  row.appendChild(el("span", { class: "mng-spacer" }));
  nodes.dryRunBtn = elI18n("button", "section.set.dryRun", { type: "button", class: "btn btn-outline" });
  nodes.dryRunBtn.addEventListener("click", () => dryRun());
  row.appendChild(nodes.dryRunBtn);
  nodes.saveBtn = elI18n("button", "section.set.save", { type: "button", class: "btn btn-primary" });
  nodes.saveBtn.addEventListener("click", () => save());
  row.appendChild(nodes.saveBtn);
  bar.appendChild(row);
  return bar;
}

function paintSaveBar() {
  if (!nodes.saveBtn) return;
  const n = state.dirty.size;
  nodes.dirtyNote.textContent = n === 0
    ? t("section.set.noPendingEdits")
    : t("section.set.pendingEdits", { n: String(n) });
  nodes.saveBtn.disabled = state.busy || n === 0;
  nodes.dryRunBtn.disabled = state.busy || n === 0;
}

function render() {
  renderBanners();
  const host = nodes.grid;
  clear(host);
  clear(nodes.barHost);
  clear(nodes.danger);
  const c = state.config;
  // 读不到配置时**连保存条都不画**：没有一份读出来的现状，就没有「改动」可言。
  if (!c) { renderExamples(); return; }

  const fields = c.fields || {};
  const credentials = c.credentials || {};
  const editable = new Set(c.editable || []);
  const painted = new Set();

  // ── 凭据卡（`secrets` 那一族，一把一张）──────────────────────────────────
  for (const path of c.secrets || []) {
    const cred = credentials[path];
    if (!cred) continue; // 诊断视图下 `credentials` 整块是 null，**不画空壳**
    host.appendChild(credentialCard(path, cred));
    painted.add(path);
  }

  // ── 分组卡 ──────────────────────────────────────────────────────────────
  for (const g of GROUPS) {
    const present = g.fields.filter((p) => Object.hasOwn(fields, p));
    if (present.length === 0) continue;
    const card = el("div", { class: "card" });
    const header = el("div", { class: "card-header" });
    header.appendChild(elI18n("h3", g.titleKey));
    card.appendChild(header);
    const grid = el("div", { class: "field-grid" });
    for (const p of present) {
      grid.appendChild(fieldNode(p, fields[p], editable.has(p)));
      painted.add(p);
    }
    card.appendChild(grid);
    host.appendChild(card);
  }

  // ── 「面板还不认识」的那些字段 ───────────────────────────────────────────
  // 🔴 **照样画出来，不静默丢弃**：静默丢弃会让一个新加的旋钮在面板上消失，
  //    而运维以为这个部署没有它。
  const rest = Object.keys(fields).filter((p) => !painted.has(p));
  if (rest.length > 0) {
    const card = el("div", { class: "card" });
    const header = el("div", { class: "card-header" });
    header.appendChild(elI18n("h3", "section.set.group.other"));
    card.appendChild(header);
    card.appendChild(elI18n("p", "section.set.group.otherDesc", { class: "settings-card-desc" }));
    const grid = el("div", { class: "field-grid" });
    for (const p of rest) grid.appendChild(fieldNode(p, fields[p], editable.has(p)));
    card.appendChild(grid);
    host.appendChild(card);
  }

  // 保存条排在栅格之后、集成示例之前。整行，不进两列。
  nodes.barHost.appendChild(saveBar());
  paintSaveBar();

  renderExamples();
  renderDanger();
}

/* ============================================================
   板块契约
   ============================================================ */

export const settingsSection = {
  init(node) {
    nodes.root = node;
  },
  onShow() {
    if (ctl) { ctl.abort(); ctl = null; }
    // ⚠️ 切语言时框架层会重跑 `onShow()`；重新 `build()` + `load()` 让所有靠 `t()`
    //    拼进 textContent 的串跟着换语言。
    // 🔴 **重进板块丢掉未保存的编辑**：留着它们会让「屏幕上这个值」与「服务器上
    //    那个值」在下一次保存时静默合流 —— 而中间可能已经有别人改过。
    build(nodes.root);
    state.changed = [];
    load();
  },
  onHide() {
    if (ctl) { ctl.abort(); ctl = null; }
  },
};

export default settingsSection;
