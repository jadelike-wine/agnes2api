/**
 * 账号管理板块（← agnes 的 `keys`，裁定 P-6 改名为 kiro 的 `accounts`）。
 *
 * 说的是**「我们拿去向 Agnes 上游证明身份」的那一族凭据**，不是我们签发给别人的
 * `sk-`（那一族在 API 管理板块）。两者方向相反、名字很像，是已知代价。
 *
 * # 本板块最大的一次视觉改动：**表格 → 行卡**（06 §3.2 ②）
 *
 * agnes 原来是一张分页表格，kiro 是「一行一张 `.account-row` 卡」——
 * 卡片 hover 抬起 4px、边框换 `--primary-20`、右列一簇图标动作 + 一颗 44×24 的
 * `.acct-switch`。用户那句「太丑了」最直接的落点就是这里。
 * **一个新类名都没发明**，全部取自 `css/sections-manage.css` 里已经搬好的那套。
 *
 * # 🔴 三条 kiro 已经踩过并修好的坑，这里逐条照抄护栏（06 §3.2 ② 末）
 *
 * ① **自动刷新只重拉列表**，绝不顺带重跑任何逐行的探测（验活按钮一次只打一把，
 *    而且是人点出来的）——按 key 数量真打上游是自毁按钮；
 * ② **列表重载要顶掉在飞的那一次**（`generation` + `AbortController`），
 *    否则一次慢响应会在后一次画完之后把旧数据糊回去；
 * ③ **`onHide()` 作废一切在飞请求与定时器**。
 *
 * # 🔴 按钮的可用性判据**一律来自后端 DTO**，这里一个 `if` 都不重写
 *
 * 列表响应里的 `view.rows[]` 已经给了 `canDelete` / `canClearCooldown` /
 * `canUnevict` / `canClearStrikes` / `toggleLabelKey` / `badgeClass` /
 * `bucketLabelKey` / `cooldownRemainingMs`，而它们与写端点的前置校验是
 * **同一份实现**（`src/core/admin/view/keys.rs`）。
 * ⚠️ TS 侧为「各写一份」栽过：`canClearCooldown` 曾经拿 `bucket` 当判据，于是一把
 * 「已停用且仍在冷却」的 key 的清冷却按钮**永远禁用**，而后端照样接受 ——
 * **按钮说「不能」，后端说「能」。**
 * ⚠️⚠️ **`view.rows` 与 `items` 靠 `id` 对齐，不靠下标信任**：DTO 特意逐条带了
 * `id` 就是为了让面板能自查这件事。
 *
 * # 🔴 `note` 是唯一整串来自运维自由输入的字段
 *
 * `src/core/keys/view.rs` 逐字写着：后端刻意不转义（JSON 的字段边界由语法本身
 * 保证），**所以「必须 `textContent`」这句话要在渲染这一侧被执行**。
 * 本文件全程只用 `el()` / `elI18n()`（内部都是 `textContent`），零 `innerHTML`。
 *
 * # ⚠️ 与 06 §3.2 ② 的三处出入，逐条给理由
 *
 * · **统计行的字段换了**：§3.2 ② 列的是「优先级 / 权重 / …」，而本仓的 `KeyView`
 *   里**没有优先级与权重**（`GatewayConfig` 里也没有这两个概念）。换成真有的
 *   `请求 / 成功 / 失败 / 客户端错误 / 连续失败 / 冷却剩余`。
 * · **四张汇总卡是 `总数 / 可用 / 冷却中 / 已停用`，不是「停用+剔除」合并那一格**：
 *   合并卡**点下去无法映射到一个筛选档**（后端的 `bucket` 是四选一的闭集），
 *   而那四张卡旁边那几个数的含义正是「切过去能看到几条」——
 *   一张点了会给出与卡面数字对不上的列表的卡，比少一张卡坏。
 *   「已剔除」那一档放在第四张卡的 `.acct-stat-sub` 上如实报数，并由工具条那个
 *   档位下拉负责筛（五个档一个不少）。
 * · **「立即补池」不在这里**：agnes 那边它与注册机板块共用同一个「明示消耗」的
 *   确认弹窗（`confirmAndTend`）。本轮注册机板块还不存在（`sec-registrar.js`
 *   没有这个文件，而顶层 `import` 一个不存在的模块会让**整个板块加载失败**），
 *   在这里另写一份确认弹窗就是把那个消耗数字抄成第二份。⇒ 添加菜单里的
 *   【自动注册】那一项只做「跳去注册机板块」。
 *
 * # ⚠️ 板块内不许监听 `langchange`
 *
 * 框架层（`js/app.js`）会 `apply(document)` 之后重跑一次 `onShow()`。
 */
import { api } from "./api.js";
import { t } from "./i18n.js";
import {
  el, elI18n, svgIcon, iconBtn, toast, openModal, confirmModal, revealControls,
} from "./ui.js";
import { badgeClass } from "./pure/badge-class.mjs";

/* ============================================================
   0. 常量与登记表
   ============================================================ */

/** 每页条数。**与后端的 `DEFAULT_SIZE` 同值**，但后端会自己夹逼，这里只是初值。 */
const PAGE_SIZE = 20;

/** 搜索防抖。每敲一个字符打一次接口的话，大池子下每次都要重投影 + 序列化整池。 */
const SEARCH_DEBOUNCE_MS = 250;

/** 新鲜度那一行自己重画的节拍（只重画文字，不发请求）。 */
const FRESHNESS_TICK_MS = 5000;

/** 备注长度上限。**与后端的 `MAX_NOTE_LENGTH` 同值**（超了后端会 400）。 */
const NOTE_MAX_LENGTH = 200;

/**
 * 档位闭集。🔴 **顺序即优先级**，与 `src/core/keys/view.rs` 的 `BUCKETS` 逐条对应
 *（`disabled > evicted > cooling > fresh`）。`all` 是「不筛」，不是第五档。
 * ⚠️ 这五条 i18n key **各写一次字面量**（`bucket_label_key()` 在 Rust 侧也是这么
 * 写的，理由相同）：拼出来的 key 与写死的 key 在行为上可以逐字节相同，
 * 而 i18n 门禁只认字面量。
 */
const BUCKET_OPTIONS = [
  { value: "all", labelKey: "keys.bucket.all" },
  { value: "fresh", labelKey: "keys.bucket.fresh" },
  { value: "cooling", labelKey: "keys.bucket.cooling" },
  { value: "evicted", labelKey: "keys.bucket.evicted" },
  { value: "disabled", labelKey: "keys.bucket.disabled" },
];

/** 导入时显式选择上游；一批新账号使用同一个选项。 */
const UPSTREAM_OPTIONS = [
  { value: "international", labelKey: "keys.upstream.international", domain: "apihub.agnes-ai.com" },
  { value: "internationalBackup", labelKey: "keys.upstream.internationalBackup", domain: "apihub.agnes-ai.cn" },
  { value: "china", labelKey: "keys.upstream.china", domain: "api.agnes-ai.cn" },
];

function upstreamText(upstream) {
  const option = UPSTREAM_OPTIONS.find((o) => o.value === upstream);
  return option ? `${t(option.labelKey)} · ${option.domain}` : "—";
}

/** 自动刷新档位（秒）。`0` = 关。 */
const AUTO_SECONDS = [0, 10, 30, 60];

/**
 * 🔴 **后端 DTO 吐出来的 i18n key → 本仓词条表里的 key。**
 *
 * 形态与 `js/sec-apikeys.js` 的 `VIEW_KEY` / `js/sec-models.js` 的 `MODALITY_KEY`
 * 逐条对齐（那两个板块面对的是同一件事）。它做两件事：
 *
 * ① **让门禁看得见**。这几条这个文件并不「写着用」—— 它们经
 *    `view.rows[].bucketLabelKey` / `toggleLabelKey` / `view.listMessageKey`
 *    以**变量**形式进 `t()`，而 i18n 门禁扫的是**字面量**。不在这里各写一次的话，
 *    它们会落进「字典里没人用的 key」并把判据打红，而顺着报文去
 *    「清理未被引用的 key」删掉的就是**面板上正在显示的活文案**。
 * ② 🔴 **当一道闸**（见 `serverKey()`）。后端将来多一档而面板还不认识时，
 *    屏幕上会出现一句「面板还不认识这一档」，而不是一个裸的 `keys.bucket.xxx`
 *    —— 后者正是人工冒烟真抓出来过的那类已上线缺陷。
 *
 * ⚠️ 左边那一列的真源在 Rust（`src/core/admin/view/keys.rs` 的
 * `bucket_label_key()` / `toggle_disable_label_key()` / `list_message_key()`），
 * **改了那边要改这边**。今天两列逐字相同（本板块的词条命名空间恰好就叫 `keys.`），
 * 右边那一列将来若随命名空间调整而改名，**只改这张表**，别去动下面的渲染代码。
 */
const VIEW_KEY = {
  "keys.bucket.all": "keys.bucket.all",
  "keys.bucket.fresh": "keys.bucket.fresh",
  "keys.bucket.cooling": "keys.bucket.cooling",
  "keys.bucket.evicted": "keys.bucket.evicted",
  "keys.bucket.disabled": "keys.bucket.disabled",
  "keys.action.enable": "keys.action.enable",
  "keys.action.disable": "keys.action.disable",
  "keys.empty": "keys.empty",
  "keys.noMatch": "keys.noMatch",
};

/** 认得就用，认不得就说「面板还不认识」。**绝不把一个裸 key 画上屏幕。** */
function serverKey(key) {
  return VIEW_KEY[key] || "keys.serverLabelUnknown";
}

/**
 * 🔴🔴 **分档徽章的类名映射 —— 一处必须点名的跨面不一致。**
 *
 * `src/core/admin/view/keys.rs` 的 `badge_class()` 回的是
 * `badge` / `badge badge-danger` / `badge badge-warn` / `badge badge-ok`，
 * 而 `css/base.css` 里**根本没有 `.badge-ok` 与 `.badge-warn`**
 *（它有的是 `.badge-success` / `.badge-warning`）。
 * ⇒ 直接把 DTO 那个串挂上去的话，「健康」与「冷却中」两档**在屏幕上是同一个
 * 中性灰**，而 Rust 那格「四档互不相同」的判据只比较字符串、照样全绿 ——
 * 那正是它自己注释里警告过的
 * 「在字符串上成立、在屏幕上不成立」。
 *
 * ⚠️ **这张表是权宜之计，不是设计**（正确的收口见那个模块的文件头），但它**只有
 * 一份**：原来本文件与 `js/sec-logs.js` 各抄了一张逐字相同的表，第三个板块
 *（`js/sec-apikeys.js`）要用时就该收了 —— 三份权宜之计里漏改一份，
 * 表现形态恰好就是本注释开头那条「某两档同色」。
 * ⇒ 那张表现在住在 `js/pure/badge-class.mjs`，本文件只是它的调用方之一。
 */

/** 四张汇总卡。`bucket` = 点一下切到哪个筛选档。 */
const STAT_CARDS = [
  { id: "all", bucket: "all", labelKey: "keys.card.total", tone: "" },
  { id: "fresh", bucket: "fresh", labelKey: "keys.card.fresh", tone: "ok" },
  { id: "cooling", bucket: "cooling", labelKey: "keys.card.cooling", tone: "warn" },
  { id: "disabled", bucket: "disabled", labelKey: "keys.card.disabled", tone: "" },
];

/** 批量条那三颗。🔴 **没有第四个**：`src/admin/keys.rs` 的 `BULK_OPS` 就这三条。 */
const BULK_OPS = [
  { op: "disable", labelKey: "keys.bulk.disable", danger: false },
  { op: "clearCooldown", labelKey: "keys.bulk.clearCooldown", danger: false },
  { op: "delete", labelKey: "keys.bulk.delete", danger: true },
];

/* ── 图标（`js/ui.js` 的 `svgIcon()` 是全站唯一造 SVG 的地方）────────────── */

const ICON_PULSE = "M 3 12 h 4 l 2.5 -7 l 5 14 l 2.5 -7 h 4";
const ICON_SNOW = "M 12 3 v 18 M 4.5 7.5 l 15 9 M 19.5 7.5 l -15 9";
const ICON_SHIELD = "M 12 3 l 8 3 v 6 c 0 4.5 -3.2 7.8 -8 9 c -4.8 -1.2 -8 -4.5 -8 -9 V 6 z";
const ICON_UNDO = "M 3 10 h 10 a 5 5 0 1 1 -5 5 M 3 10 l 4 -4 M 3 10 l 4 4";
const ICON_NOTE = "M 4 4 h 12 l 4 4 v 12 h -16 z M 8 10 h 8 M 8 14 h 8";
const ICON_TRASH = "M 4 7 h 16 M 9 7 V 4 h 6 v 3 M 6 7 l 1 13 h 10 l 1 -13 M 10 11 v 6 M 14 11 v 6";
const ICON_PLUS = "M 12 5 v 14 M 5 12 h 14";

/* ============================================================
   1. 状态（🔴 C 档：浏览器交互状态机，06 §5.3 —— **本轮无判据执行器**）
   ============================================================ */

const state = { bucket: "all", q: "", page: 1, autoSec: 0 };

/** 勾选集合。**只存 id**，列表重载之后要按新的 id 集合剪枝。 */
const selected = new Set();

/**
 * 验活状态表，键是 key id。
 * ⚠️ **它的有界性全靠「键只可能来自真实存在过的 key id」** —— 所以
 * `verifyOne()` 里先用现有状态查可用性、查完再建那一项，别反过来。
 */
const verifyState = new Map();

/**
 * 明文回显的状态（U-7 / `js/ui.js` 的 `revealControls`）。
 * 🔴 **明文只活在这里，绝不写回列表数据** —— 列表随刷新整份重建，
 * 于是切板块、刷新之后自动回到掩码态。
 */
const revealState = {
  secrets: new Map(),
  shown: new Set(),
  secretOf(id) { return this.secrets.has(id) ? this.secrets.get(id) : null; },
  remember(id, secret) { this.secrets.set(id, secret); },
  isShown(id) { return this.shown.has(id); },
  show(id) { this.shown.add(id); },
  hide(id) { this.shown.delete(id); },
  reset() { this.secrets.clear(); this.shown.clear(); },
};

let data = null;
let loadFailed = false;
let lastOkAt = 0;

/** 两个只拉一次的旋钮。**拿不到时各自有一条诚实的退路**，见 `loadKnobs()`。 */
let freshnessMs = null;
let probeMinIntervalMs = null;

let generation = 0;
let abort = null;
let searchTimer = null;
let autoTimer = null;
let freshTimer = null;

/* ============================================================
   2. 本地化格式（06 §5.3 的 B 档）
   ⚠️ 已登记的欠账：`js/pure/format.mjs` 本轮还不存在，所以这几个小函数暂住在
   板块文件里，`js/sec-dashboard.js` 里有形近的另一份。
   ============================================================ */

function lang() { return document.documentElement.lang || "zh-CN"; }

function fmtInt(n) { return new Intl.NumberFormat(lang()).format(n); }

function fmtInstant(ms) { return new Date(ms).toLocaleString(lang()); }

/** 一段毫秒 → 「N 天 N 小时」/「N 小时 N 分」/「N 分 N 秒」。 */
function fmtDuration(ms) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return t("keys.durationDays", { d, h });
  if (h > 0) return t("keys.durationHours", { h, m });
  return t("keys.durationMinutes", { m, s: s % 60 });
}

/** 「没读出来」那一格（EM DASH，U-4）。**与「读成功但没有样本」不是同义词。** */
function unknownSpan() {
  return el("span", { class: "usage-unknown", title: t("keys.unknownHint") }, "—");
}

/** 「读成功了，但这一段真的没有样本」（EN DASH）。 */
function noneSpan() {
  return el("span", { class: "usage-none", title: t("keys.noneHint") }, "–");
}

/* ============================================================
   3. 骨架
   ============================================================ */

const nodes = {
  root: null,
  cards: {},
  cardSubs: {},
  search: null,
  bucketSel: null,
  autoSel: null,
  refreshBtn: null,
  addMenu: null,
  addWrap: null,
  freshness: null,
  batchHost: null,
  listHost: null,
  pagerHost: null,
};

function statCard(spec) {
  const card = el("div", {
    class: `card acct-stat-card is-clickable ${spec.tone}`.trim(),
    role: "button",
    tabindex: "0",
  });
  card.appendChild(elI18n("div", spec.labelKey, { class: "acct-stat-label" }));
  const num = el("div", { class: "acct-stat-num" }, "—");
  nodes.cards[spec.id] = num;
  card.appendChild(num);
  const sub = el("div", { class: "acct-stat-sub" });
  nodes.cardSubs[spec.id] = sub;
  card.appendChild(sub);
  const go = () => { state.bucket = spec.bucket; state.page = 1; syncControls(); load(); };
  card.addEventListener("click", go);
  // 卡片是 `<div role="button">`，键盘要自己接 —— 原生按钮的那一半不会白送。
  card.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); go(); }
  });
  return card;
}

/**
 * 「添加账号」分组下拉（`.add-menu`，形态照 kiro，内容换成本仓真做得到的两项）。
 * ⚠️ 06 §9.6 登记过「照抄 kiro 的弹窗会做出三个点不动的按钮」—— 本条就是那个
 * 问题的处置：**只放真的点得动的**。
 */
function buildAddMenu() {
  const wrap = el("div", { class: "add-menu-wrap" });
  const btn = el("button", { type: "button", class: "btn btn-primary btn-sm", "aria-haspopup": "true", "aria-expanded": "false" });
  btn.appendChild(svgIcon(ICON_PLUS, 16));
  btn.appendChild(elI18n("span", "keys.add"));
  const menu = el("div", { class: "add-menu" });

  menu.appendChild(elI18n("div", "keys.add.manualGroup", { class: "add-menu-label" }));
  const imp = el("button", { type: "button" });
  imp.appendChild(elI18n("span", "keys.add.import"));
  imp.appendChild(elI18n("span", "keys.add.importSub", { class: "sub" }));
  imp.addEventListener("click", () => { closeAddMenu(); openImport(); });
  menu.appendChild(imp);

  menu.appendChild(elI18n("div", "keys.add.autoGroup", { class: "add-menu-label" }));
  const go = el("button", { type: "button" });
  go.appendChild(elI18n("span", "keys.add.goRegistrar"));
  go.appendChild(elI18n("span", "keys.add.goRegistrarSub", { class: "sub" }));
  go.addEventListener("click", () => {
    closeAddMenu();
    // 走导航按钮自己的 click，**不另造一条路由**：`js/app.js` 的 `showSection()`
    // 没有对外导出，而那几颗按钮上的 `data-section` 是 `index.html` 与
    // `src/webui/mod.rs` 三方对齐钉住的公开契约。
    const nav = document.querySelector('.nav-item[data-section="registrar"]');
    if (nav) nav.click();
  });
  menu.appendChild(go);

  btn.addEventListener("click", (ev) => {
    ev.stopPropagation();
    const open = menu.classList.toggle("open");
    btn.setAttribute("aria-expanded", open ? "true" : "false");
  });
  wrap.appendChild(btn);
  wrap.appendChild(menu);
  nodes.addMenu = menu;
  nodes.addWrap = btn;
  return wrap;
}

function closeAddMenu() {
  if (nodes.addMenu) nodes.addMenu.classList.remove("open");
  if (nodes.addWrap) nodes.addWrap.setAttribute("aria-expanded", "false");
}

function build(root) {
  root.textContent = "";

  const header = el("div", { class: "section-header" });
  header.appendChild(elI18n("h2", "keys.title", { class: "section-title" }));
  const actions = el("div", { class: "section-actions" });
  actions.appendChild(buildAddMenu());
  header.appendChild(actions);
  root.appendChild(header);

  const grid = el("div", { class: "accounts-stat-grid" });
  for (const spec of STAT_CARDS) grid.appendChild(statCard(spec));
  root.appendChild(grid);

  const bar = el("div", { class: "mng-toolbar" });
  const search = el("input", {
    type: "search", class: "form-control mng-search",
    "data-i18n-ph": "keys.search",
    placeholder: t("keys.search"),
    "aria-label": t("keys.search"),
  });
  search.value = state.q;
  search.addEventListener("input", () => {
    state.q = search.value;
    state.page = 1;
    if (searchTimer !== null) clearTimeout(searchTimer);
    searchTimer = setTimeout(() => { searchTimer = null; load(); }, SEARCH_DEBOUNCE_MS);
  });
  nodes.search = search;
  bar.appendChild(search);

  const bucketSel = el("select", { class: "form-control", "aria-label": t("keys.filterTitle"), "data-i18n-title": "keys.filterTitle" });
  for (const opt of BUCKET_OPTIONS) {
    bucketSel.appendChild(elI18n("option", opt.labelKey, { value: opt.value }));
  }
  bucketSel.value = state.bucket;
  bucketSel.addEventListener("change", () => {
    state.bucket = bucketSel.value;
    state.page = 1;
    load();
  });
  nodes.bucketSel = bucketSel;
  bar.appendChild(bucketSel);

  const autoSel = el("select", { class: "form-control", "aria-label": t("keys.autoTitle"), "data-i18n-title": "keys.autoTitle" });
  for (const sec of AUTO_SECONDS) {
    const o = el("option", { value: String(sec) }, sec === 0 ? t("keys.auto.off") : t("keys.auto.sec", { sec }));
    autoSel.appendChild(o);
  }
  autoSel.value = String(state.autoSec);
  autoSel.addEventListener("change", () => {
    state.autoSec = Number(autoSel.value) || 0;
    restartAuto();
  });
  nodes.autoSel = autoSel;
  bar.appendChild(autoSel);

  bar.appendChild(el("div", { class: "mng-spacer" }));
  const refresh = elI18n("button", "keys.refresh", { type: "button", class: "btn btn-outline btn-sm" });
  refresh.addEventListener("click", () => { load(); });
  nodes.refreshBtn = refresh;
  bar.appendChild(refresh);
  root.appendChild(bar);

  nodes.freshness = el("p", { class: "mng-inline-note", id: "accountsFreshness" });
  root.appendChild(nodes.freshness);

  nodes.batchHost = el("div");
  root.appendChild(nodes.batchHost);
  nodes.listHost = el("div", { class: "accounts-list" });
  root.appendChild(nodes.listHost);
  nodes.pagerHost = el("div");
  root.appendChild(nodes.pagerHost);
}

/** 控件回填（汇总卡点过之后要让下拉跟着走）。 */
function syncControls() {
  if (nodes.bucketSel) nodes.bucketSel.value = state.bucket;
  if (nodes.search) nodes.search.value = state.q;
  if (nodes.autoSel) nodes.autoSel.value = String(state.autoSec);
}

/* ============================================================
   4. 一行的渲染
   ============================================================ */

/** 统计行里的一格。 */
function statCell(labelKey, node, tone) {
  const wrap = el("span", { class: `account-stat ${tone || ""}`.trim() });
  wrap.appendChild(elI18n("span", labelKey, { class: "stat-k" }));
  const n = el("span", { class: "stat-n" });
  n.appendChild(node);
  wrap.appendChild(n);
  return wrap;
}

function numNode(n) { return document.createTextNode(fmtInt(n)); }

/**
 * 验活那一颗 + 它的结果。
 *
 * ⚠️ **它与右边那一簇不是同一类东西**：那几颗都写存储，它一个字段都不写
 *（`src/admin/keys.rs` 的约束：验活是只读探针）。它是这一行里唯一一个
 * **按下去会真的打一次上游**的按钮 ⇒ 排在动作簇最前面，别混进写动作中间。
 *
 * ⚠️⚠️ **`disabled` / `evicted` 的行不禁用它** —— 恰恰相反，「它是不是真的死了」
 * 正是运维最想验的那一把。
 *
 * ⚠️ **结果元素只承载一个 i18n key，永远不承载响应体里的任何值**：
 * 后端刻意只回状态码、一个字节的上游正文都不回，前端这一侧不能把它又漏出去。
 */
function verifyControls(view) {
  const wrap = el("span", { class: "account-stat" });
  const s = verifyState.get(view.id);
  const reason = verifyDisabledReason(view.id, s, Date.now());
  const btn = iconBtn(ICON_PULSE, verifyTitleKey(reason), () => { verifyOne(view); });
  btn.disabled = reason !== null;
  wrap.appendChild(btn);
  if (s && s.inFlight) {
    wrap.appendChild(elI18n("span", "keys.verify.running", { class: "cell-sub" }));
  } else if (s && typeof s.code === "string") {
    wrap.appendChild(elI18n("span", verifyResultLabelKey(s.code), { class: "cell-sub" }));
    // 只读探针那句话与结果**一起**出现：运维恰恰是在看到「验活通过」的那一刻
    // 以为 strikes 已经被清掉了。
    wrap.appendChild(elI18n("span", "keys.verify.readOnlyNote", { class: "cell-sub" }));
  }
  return wrap;
}

/**
 * 一张账号行卡。`view` 是 `items[i]`，`rv` 是 `view.rows` 里**按 id 找到**的那一条。
 * 🔴 `rv` 为空时**整簇动作按钮禁用**，不猜：那说明两边对不齐，而猜出来的
 * 可用性正是「按钮说能、后端说不能」那一类缺陷的来源。
 */
function accountRow(item, rv, generatedAt) {
  const row = el("div", { class: "account-row", "data-key-id": item.id });
  if (item.disabled) row.classList.add("is-disabled");
  if (selected.has(item.id)) row.classList.add("is-selected");

  const pick = el("label", { class: "account-row-select" });
  const box = el("input", { type: "checkbox", "aria-label": t("keys.selectRow") });
  box.checked = selected.has(item.id);
  box.addEventListener("change", () => {
    if (box.checked) selected.add(item.id); else selected.delete(item.id);
    render();
  });
  pick.appendChild(box);
  row.appendChild(pick);

  const info = el("div", { class: "account-row-info" });

  const idline = el("div", { class: "account-row-idline" });
  idline.appendChild(el("span", { class: "account-row-id" }, `#${item.seq}`));
  // 明文回显（U-7，kiro 没有）。🔴 掩码来自后端，**列表响应里永远没有明文**。
  idline.appendChild(revealControls({
    id: item.id,
    masked: item.masked,
    state: revealState,
    fetchSecret: fetchSecret,
    messageKeyOf: revealMessageKey,
  }));
  // 分档徽章：类名与文案 key **都由后端给**（四档必须互不相同，而那条
  // 「互不相同」是在 Rust 那一侧有判据钉着的）。类名过一道 `badgeClass()`，
  // 理由见上面那段。
  if (rv) {
    idline.appendChild(elI18n("span", serverKey(rv.bucketLabelKey), {
      class: badgeClass(rv.badgeClass),
    }));
  }
  info.appendChild(idline);

  const meta = el("div", { class: "account-row-meta" });
  meta.appendChild(el("span", null, `${t("keys.upstream.label")}: ${upstreamText(item.upstream)}`));
  meta.appendChild(el("span", null, t("keys.meta.added", { when: fmtInstant(item.addedAt) })));
  meta.appendChild(el(
    "span", null,
    item.lastUsedAt === null || item.lastUsedAt === undefined
      ? t("keys.meta.neverUsed")
      : t("keys.meta.lastUsed", { when: fmtInstant(item.lastUsedAt) }),
  ));
  // 🔴 `note` 整串来自运维自由输入 ⇒ `el()` 的 textContent 是它唯一合法的落点。
  if (item.note) {
    const noteSpan = el("span", { class: "cell-sub" });
    noteSpan.appendChild(elI18n("span", "keys.meta.note", { class: "meta-k" }));
    noteSpan.appendChild(el("span", null, item.note));
    meta.appendChild(noteSpan);
  }
  // 冷却 / 剔除的**原因**同样来自上游或后端的自由文本，同一条纪律。
  if (item.cooldownReason) {
    meta.appendChild(el("span", null, t("keys.reason.cooldown", { reason: item.cooldownReason })));
  }
  if (item.evictedReason) {
    meta.appendChild(el("span", null, t("keys.reason.evicted", { reason: item.evictedReason })));
  }
  info.appendChild(meta);

  const stats = el("div", { class: "account-row-stats" });
  const s = item.stats;
  stats.appendChild(verifyControls(item));
  stats.appendChild(statCell("keys.stat.requests", numNode(s.requests), ""));
  stats.appendChild(statCell("keys.stat.success", numNode(s.success), "ok"));
  stats.appendChild(statCell("keys.stat.failed", numNode(s.failed), s.failed > 0 ? "bad" : "muted"));
  stats.appendChild(statCell("keys.stat.clientErrors", numNode(s.clientErrors), "info"));
  stats.appendChild(statCell("keys.stat.strikes", numNode(item.strikes), item.strikes > 0 ? "warn" : "muted"));
  // 冷却剩余：🔴 **只有 `cooling` 档才有这个数**，其余一律 `null`。
  // ⚠️ 它是后端用**自己的时钟**算好的（`generatedAt` 与 `cooldownUntil` 来自同一次
  //    `now_ms()`）—— 拿浏览器时钟去减服务端的时刻，时钟有偏差时会算出负数。
  //    这里只在本地扣掉「从那一刻起又过了多久」。
  // ⚠️ `null` 画的是 **EN DASH**（`.usage-none`）不是 EM DASH：这一格**读成功了**，
  //    结论就是「这一行不在冷却」。EM DASH 的意思是「没读出来」，两者不许混
  //    —— 那正是 U-4 要分开的那两件事。
  const left = rv && rv.cooldownRemainingMs !== null && rv.cooldownRemainingMs !== undefined
    ? rv.cooldownRemainingMs - (Date.now() - generatedAt)
    : null;
  stats.appendChild(statCell(
    "keys.stat.cooldown",
    left === null ? noneSpan() : document.createTextNode(fmtDuration(left)),
    "warn",
  ));
  info.appendChild(stats);
  row.appendChild(info);

  row.appendChild(rowActions(item, rv));
  return row;
}

/** 右列动作簇：一颗 44×24 开关 + 五颗图标按钮。 */
function rowActions(item, rv) {
  const box = el("div", { class: "account-row-actions" });

  const toggle = el("button", {
    class: `acct-switch${item.disabled ? "" : " is-on"}`,
    type: "button",
    role: "switch",
    "aria-checked": item.disabled ? "false" : "true",
  });
  const toggleKey = rv ? serverKey(rv.toggleLabelKey) : "keys.action.disable";
  toggle.setAttribute("title", t(toggleKey));
  toggle.setAttribute("aria-label", t(toggleKey));
  toggle.appendChild(el("span", { class: "acct-switch-thumb", "aria-hidden": "true" }));
  toggle.disabled = !rv;
  toggle.addEventListener("click", () => { patchAction(item.id, { disabled: !item.disabled }); });
  box.appendChild(toggle);

  const clearCooldown = iconBtn(ICON_SNOW, "keys.action.clearCooldown", () => {
    patchAction(item.id, { clearCooldown: true });
  });
  clearCooldown.disabled = !rv || !rv.canClearCooldown;
  box.appendChild(clearCooldown);

  const clearStrikes = iconBtn(ICON_SHIELD, "keys.action.clearStrikes", () => {
    // 🔴 **要确认，但理由与删除不同**：它不是不可撤销，是**语义容易与相邻的
    //    「清冷却」混淆** —— 清冷却只让这把 key 现在能用，它离下一次被剔除仍然
    //    只差一次失败；清连续失败才会把那个计数真正清零。
    confirmModal("keys.clearStrikesConfirmTitle", "keys.clearStrikesConfirmMsg", () => {
      patchAction(item.id, { clearStrikes: true });
    });
  });
  clearStrikes.disabled = !rv || !rv.canClearStrikes;
  box.appendChild(clearStrikes);

  const unevict = iconBtn(ICON_UNDO, "keys.action.unevict", () => {
    patchAction(item.id, { unevict: true });
  });
  unevict.disabled = !rv || !rv.canUnevict;
  box.appendChild(unevict);

  const note = iconBtn(ICON_NOTE, "keys.action.note", () => { editNote(item); });
  box.appendChild(note);

  const del = iconBtn(ICON_TRASH, "keys.action.delete", () => { deleteOne(item); }, "is-danger");
  del.disabled = !rv || !rv.canDelete;
  box.appendChild(del);

  return box;
}

/* ============================================================
   5. 整块渲染
   ============================================================ */

function render() {
  if (!nodes.root) return;
  paintCards();
  paintFreshness();
  paintBatchBar();
  paintList();
  paintPager();
}

function paintCards() {
  const c = data && data.counts;
  for (const spec of STAT_CARDS) {
    const num = nodes.cards[spec.id];
    const sub = nodes.cardSubs[spec.id];
    num.textContent = "";
    sub.textContent = "";
    if (!c) { num.appendChild(unknownSpan()); continue; }
    num.appendChild(document.createTextNode(fmtInt(c[spec.id])));
    // 第四张卡把「已剔除」如实报在副行上：它是第五个档，而栅格只有四格，
    // **合并成一个数会让这张卡点下去给出与卡面对不上的列表**。
    if (spec.id === "disabled") sub.textContent = t("keys.card.evictedSub", { n: c.evicted });
  }
}

function paintFreshness() {
  const line = nodes.freshness;
  line.textContent = "";
  line.classList.remove("is-stale");
  if (lastOkAt === 0) {
    line.appendChild(elI18n("span", "keys.freshnessNever"));
    line.classList.add("is-stale");
    return;
  }
  const age = Date.now() - lastOkAt;
  line.appendChild(el("span", null, t("keys.freshnessAge", { dur: fmtDuration(age) })));
  if (freshnessMs !== null) {
    line.appendChild(el("span", null, t("keys.freshnessBound", { sec: Math.max(0, Math.round(freshnessMs / 1000)) })));
  }
  // 陈旧快照不该冒充实时数据：上一次读失败了、或者屏幕上这份已经超过两拍，
  // 就把这一行转成警示色。
  const beat = state.autoSec > 0 ? state.autoSec * 1000 : (freshnessMs || 0);
  if (loadFailed || (beat > 0 && age > 2 * beat)) line.classList.add("is-stale");
}

function paintBatchBar() {
  const host = nodes.batchHost;
  host.textContent = "";
  if (selected.size === 0) return;
  const bar = el("div", { class: "accounts-batchbar" });
  bar.appendChild(el("span", { class: "bb-count" }, t("keys.bulk.count", { n: selected.size })));
  bar.appendChild(el("span", { class: "bb-spacer" }));
  for (const spec of BULK_OPS) {
    const b = elI18n("button", spec.labelKey, {
      type: "button",
      class: `btn btn-sm ${spec.danger ? "btn-danger" : "btn-outline"}`,
    });
    b.addEventListener("click", () => { runBulkWithConfirm(spec.op); });
    bar.appendChild(b);
  }
  const clear = elI18n("button", "keys.bulk.clear", { type: "button", class: "btn btn-sm btn-outline" });
  clear.addEventListener("click", () => { selected.clear(); render(); });
  bar.appendChild(clear);
  host.appendChild(bar);
}

function paintList() {
  const host = nodes.listHost;
  host.textContent = "";

  if (loadFailed) {
    // 🔴 读失败在这条端点上走的是 **500** ⇒ 后端根本产不出一份「200 但读失败了」
    //    的响应体。那一档由这里接住，**不许画成一个空池子**。
    host.appendChild(elI18n("div", "keys.loadFailed", { class: "empty-state text-danger" }));
    return;
  }
  if (!data) {
    const ph = el("div", { class: "stats-placeholder" });
    ph.appendChild(el("span", { class: "stats-spinner", "aria-hidden": "true" }));
    ph.appendChild(elI18n("span", "keys.loading"));
    host.appendChild(ph);
    return;
  }
  // 🔴 「池子里还没有 key」与「筛不出来」**必须分开**：前者是「去导入」，
  //    后者是「把筛选条件放宽」，指向完全相反的动作。判定在后端
  //    （`view.listMessageKey`），这里只照着画。
  const msg = data.view.listMessageKey;
  if (msg) {
    host.appendChild(elI18n("div", serverKey(msg), { class: "empty-state" }));
    return;
  }

  // ⚠️ 逐条**按 id** 去 `view.rows` 里找，不靠下标信任。
  const byId = new Map();
  for (const r of data.view.rows) byId.set(r.id, r);
  const head = el("div", { class: "mng-toolbar" });
  const selectAll = el("input", { type: "checkbox", "aria-label": t("keys.selectAll") });
  const pageIds = data.items.map((i) => i.id);
  selectAll.checked = pageIds.length > 0 && pageIds.every((id) => selected.has(id));
  selectAll.addEventListener("change", () => {
    for (const id of pageIds) {
      if (selectAll.checked) selected.add(id); else selected.delete(id);
    }
    render();
  });
  const lbl = el("label", { class: "account-row-select" });
  lbl.appendChild(selectAll);
  head.appendChild(lbl);
  head.appendChild(elI18n("span", "keys.selectAll", { class: "cell-sub" }));
  host.appendChild(head);

  for (const item of data.items) {
    host.appendChild(accountRow(item, byId.get(item.id) || null, data.generatedAt));
  }
}

function paintPager() {
  const host = nodes.pagerHost;
  host.textContent = "";
  if (!data || loadFailed) return;
  const p = data.view.pager;
  const bar = el("div", { class: "accounts-pager" });
  const prev = elI18n("button", "keys.pager.prev", { type: "button", class: "btn btn-outline btn-sm" });
  prev.disabled = p.prevDisabled;
  prev.addEventListener("click", () => { state.page = Math.max(1, state.page - 1); load(); });
  bar.appendChild(prev);
  const ind = el("span", { class: "accounts-page-indicator" });
  // 🔴 `info` 为 `null` ⇒ **把「第 1/2 页 · 共 3 条」整条抹掉**（空列表时必须一并
  //    复位，否则屏幕上挂着一份已经不存在的数据的页码）。
  if (p.info) ind.textContent = t("keys.pager.info", { page: p.info.page, pages: p.info.pages, total: p.info.total });
  bar.appendChild(ind);
  const next = elI18n("button", "keys.pager.next", { type: "button", class: "btn btn-outline btn-sm" });
  next.disabled = p.nextDisabled;
  next.addEventListener("click", () => { state.page += 1; load(); });
  bar.appendChild(next);
  host.appendChild(bar);
}

/* ============================================================
   6. 取数
   ============================================================ */

function keysQuery() {
  const qs = new URLSearchParams();
  if (state.bucket !== "all") qs.set("bucket", state.bucket);
  if (state.q !== "") qs.set("q", state.q);
  qs.set("page", String(state.page));
  qs.set("size", String(PAGE_SIZE));
  return qs.toString();
}

/**
 * 两个只拉一次的旋钮。**各自有一条诚实的退路，都不写死常量。**
 *
 * · `freshness.poolVisibilityUpperBoundMs` —— 拿不到就不显示那半句话
 *   （不是显示一个猜的秒数）；
 * · 🔴 `probes.minIntervalMs` —— **面板不许把这个数写死**。TS 版那边有一个
 *   `VERIFY_MIN_INTERVAL_MS = 3_000` 常量靠一格 parity 判据与后端对表活着，
 *   06 §5.3 的 A 档把它收成一处：**Rust 是唯一真源，面板读这一格。**
 *   拿不到就是 `null` ⇒ 前端这道闸**整个不设**，让后端的 429 说话
 *   （前端这一半本来就只是「别把一次注定被拒的请求发出去」的优化）。
 */
async function loadKnobs() {
  if (freshnessMs === null) {
    try {
      const body = await api.get("/overview");
      freshnessMs = body.freshness.poolVisibilityUpperBoundMs;
    } catch (e) { /* 拿不到就少说半句话，不猜 */ }
  }
  if (probeMinIntervalMs === null) {
    try {
      const body = await api.get("/capabilities");
      probeMinIntervalMs = body.probes.minIntervalMs;
    } catch (e) { /* 见上：这道闸不设，后端那道还在 */ }
  }
}

async function load() {
  const gen = ++generation;
  if (abort) abort.abort();
  abort = new AbortController();
  if (nodes.refreshBtn) nodes.refreshBtn.disabled = true;
  try {
    const body = await api.get(`/keys?${keysQuery()}`, { signal: abort.signal });
    if (gen !== generation) return;
    data = body;
    loadFailed = false;
    lastOkAt = Date.now();
    // 勾选集合按新的 id 集合剪枝：**别让一次删除在批量条上留下幽灵计数**。
    const alive = new Set(body.items.map((i) => i.id));
    for (const id of [...selected]) if (!alive.has(id)) selected.delete(id);
  } catch (e) {
    if (gen !== generation) return;
    // 401 已经由 `js/api.js` 把人送回登录闸了，这里只负责别把旧数据留在屏幕上。
    data = null;
    loadFailed = true;
  } finally {
    if (gen === generation) {
      if (nodes.refreshBtn) nodes.refreshBtn.disabled = false;
      render();
    }
  }
}

function restartAuto() {
  if (autoTimer !== null) { clearInterval(autoTimer); autoTimer = null; }
  // ⚠️ **自动刷新只重拉列表**，绝不顺带跑任何逐行探测（那是按 key 数量真打上游）。
  if (state.autoSec > 0) autoTimer = setInterval(() => { load(); }, state.autoSec * 1000);
}

/* ============================================================
   7. 写操作
   ============================================================ */

/** 后端信封里那句人话。**取不到就说一句我们自己的**，不把异常对象印上屏幕。 */
function errorText(e) {
  const body = e && typeof e.body === "object" && e.body !== null ? e.body : null;
  if (body && body.error && body.error.code === "invalid_upstream") return t("keys.upstream.invalid");
  const msg = body && body.error && typeof body.error.message === "string" ? body.error.message : "";
  return msg === "" ? t("keys.actionFailedGeneric") : t("keys.actionFailed", { message: msg });
}

function patchAction(id, body) {
  api.patch(`/keys/${encodeURIComponent(id)}`, body)
    .then(() => { toast(t("keys.actionOk"), "ok"); })
    .catch((e) => { toast(errorText(e), "err"); })
    // 成功与失败都重拉：失败那一次同样要把屏幕恢复成**服务端此刻的真相**。
    .then(() => load());
}

function deleteOne(item) {
  const run = () => {
    api.del(`/keys/${encodeURIComponent(item.id)}`)
      .then(() => { toast(t("keys.actionOk"), "ok"); })
      .catch((e) => {
        // 409 + `must_disable_first` 是一句**可以照做的**指引，与别的失败不同。
        const body = e && typeof e.body === "object" && e.body !== null ? e.body : null;
        if (e && e.status === 409 && body && body.reason === "must_disable_first") {
          toast(t("keys.mustDisableFirst"), "warn");
          return;
        }
        toast(errorText(e), "err");
      })
      .then(() => load());
  };
  // 🔴 **只有删除需要「不可撤销」这条理由的确认**：停用 / 清冷却 / 解除剔除
  //    随时能再点一次撤回，给它们弹窗只会让人对真正危险的那一个变得麻木。
  confirmModal("keys.deleteConfirmTitle", "keys.deleteConfirmMsg", run);
}

function editNote(item) {
  const body = el("div");
  const group = el("div", { class: "form-group" });
  group.appendChild(elI18n("label", "keys.note.label", { class: "form-label", for: "acctNoteInput" }));
  const ta = el("textarea", {
    id: "acctNoteInput", class: "form-control", rows: "3", maxlength: String(NOTE_MAX_LENGTH),
  });
  ta.value = item.note || "";
  group.appendChild(ta);
  group.appendChild(el("div", { class: "form-hint" }, t("keys.note.hint", { max: NOTE_MAX_LENGTH })));
  body.appendChild(group);
  openModal("keys.note.title", body, [
    { labelKey: "common.cancel" },
    {
      labelKey: "keys.note.save",
      // ⚠️ `keepOpen` + 自己关：网络失败时弹窗还在，运维输的字不会凭空消失。
      keepOpen: true,
      onClick: (close) => {
        const text = ta.value.trim();
        // 空串 = **清掉备注**（后端用 `null` 表达「清」，`undefined` 表达「不动它」）。
        api.patch(`/keys/${encodeURIComponent(item.id)}`, { note: text === "" ? null : text })
          .then(() => { toast(t("keys.actionOk"), "ok"); close(); load(); })
          .catch((e) => { toast(errorText(e), "err"); });
      },
    },
  ]);
}

/**
 * 批量结果的呈现。
 * 🔴 **逐项结果不是装饰**：批量删除里混着一把没停用的 key 时，整批 409 会让运维
 * 不知道哪几把成功了；而只回一句「成功」则会把那一把的失败吞掉。
 * ⚠️ **有失败就走 `sticky`**：那类信息藏在 HTTP 200 的响应体里，
 * 3 秒一闪而过等于把一条诚实信号做成了几乎看不见的信号。
 */
function summarize(results) {
  const list = Array.isArray(results) ? results : [];
  let ok = 0; let mustDisableFirst = 0; let notFound = 0; let otherFailed = 0;
  for (const r of list) {
    if (!r || typeof r !== "object") continue;
    if (r.ok === true) { ok += 1; continue; }
    if (r.reason === "must_disable_first") mustDisableFirst += 1;
    else if (r.reason === "not_found") notFound += 1;
    else otherFailed += 1;
  }
  const failed = mustDisableFirst + notFound + otherFailed;
  return { total: list.length, ok, failed, mustDisableFirst, notFound, otherFailed };
}

function runBulk(op, ids) {
  api.post("/keys/bulk", { op, ids })
    .then((body) => {
      const s = summarize(body && body.results);
      // ⚠️ 每个候选 key 都是数组的独立元素、都带尾逗号 —— i18n 门禁的第 ⑥ 条
      //    判的是「带占位符的 key 字面量后面紧跟的是不是逗号」。
      const parts = [
        s.failed > 0 && "keys.bulk.partial",
        s.failed === 0 && "keys.bulk.allOk",
        "keys.bulk.countsSuffix",
        s.mustDisableFirst > 0 && "keys.bulk.mustDisableFirstSuffix",
        s.notFound > 0 && "keys.bulk.notFoundSuffix",
      ].filter((k) => typeof k === "string");
      toast(parts.map((k) => t(k, s)).join(""), s.failed > 0 ? "warn" : "ok",
        s.failed > 0 ? { sticky: true } : undefined);
    })
    .catch((e) => { toast(errorText(e), "err", { sticky: true }); })
    .then(() => { selected.clear(); load(); });
}

function runBulkWithConfirm(op) {
  const ids = [...selected];
  // 点击时校验参数非空。**这不是一条业务判据**，是事件处理器的常规早退。
  if (ids.length === 0) return;
  if (op !== "delete") { runBulk(op, ids); return; }
  const msg = el("p", null, t("keys.bulk.confirmDelete", { count: ids.length }));
  openModal("keys.bulk.confirmTitle", msg, [
    { labelKey: "common.cancel" },
    { labelKey: "common.confirm", danger: true, onClick: () => { runBulk(op, ids); } },
  ]);
}

/**
 * 粘贴导入。
 * ⚠️⚠️ **`keepOpen: true`**：`openModal` 无条件先关再跑的话，校验失败 / 网络失败时
 * 弹窗已经关了，粘进去的 200 行只剩一句 toast，其余全丢。
 * ⚠️ **原样按行发，不 trim、不过滤空行**：后端把空行整条跳过，但非法项的位置仍按
 * **原始下标**算 —— 前端先过滤的话，报出来的行号与运维在文本框里数到的对不上。
 */
function openImport() {
  const body = el("div");
  const upstreamGroup = el("div", { class: "form-group" });
  upstreamGroup.appendChild(elI18n("label", "keys.upstream.label", { class: "form-label", for: "acctImportUpstream" }));
  const upstream = el("select", { id: "acctImportUpstream", class: "form-control" });
  for (const option of UPSTREAM_OPTIONS) {
    upstream.appendChild(el("option", { value: option.value }, upstreamText(option.value)));
  }
  upstream.value = "international";
  upstreamGroup.appendChild(upstream);
  upstreamGroup.appendChild(elI18n("div", "keys.upstream.importHint", { class: "form-hint" }));
  body.appendChild(upstreamGroup);
  const group = el("div", { class: "form-group" });
  group.appendChild(elI18n("label", "keys.import.label", { class: "form-label", for: "acctImportInput" }));
  const ta = el("textarea", {
    id: "acctImportInput", class: "form-control", rows: "8",
    "data-i18n-ph": "keys.import.placeholder",
    placeholder: t("keys.import.placeholder"),
  });
  group.appendChild(ta);
  body.appendChild(group);

  const check = el("label", { class: "form-check" });
  const reset = el("input", { type: "checkbox" });
  check.appendChild(reset);
  check.appendChild(elI18n("span", "keys.import.reset"));
  body.appendChild(check);
  // 🔴 **知情重置**：勾上之后「重新导入」就是一条能绕过 `evicted` 的路径，
  //    后端会为此单打一条 warn 事件。这句话必须在勾选框旁边看得见。
  body.appendChild(elI18n("div", "keys.import.resetHint", { class: "form-hint" }));

  openModal("keys.import.title", body, [
    { labelKey: "common.cancel" },
    {
      labelKey: "keys.import.submit",
      keepOpen: true,
      onClick: (close) => {
        const lines = ta.value === "" ? [] : ta.value.split(/\r\n|\r|\n/);
        if (!lines.some((l) => l.trim() !== "")) { toast(t("keys.import.empty"), "warn"); return; }
        api.post("/keys", { keys: lines, resetExisting: reset.checked, upstream: upstream.value })
          .then((res) => {
            const counts = {
              added: (res.added || []).length,
              duplicated: (res.duplicated || []).length,
              invalid: (res.invalid || []).length,
              // 🔴 **`reset` 原样取自响应字段，不许用 `duplicated.length` 代替**：
              //    本批刚新建的那把即使被粘了两遍也谈不上被重置。
              reset: res.reset || 0,
            };
            const parts = [
              "keys.import.done",
              counts.reset > 0 && "keys.import.resetSuffix",
            ].filter((k) => typeof k === "string");
            toast(parts.map((k) => t(k, counts)).join(""),
              counts.invalid > 0 ? "warn" : "ok",
              counts.invalid > 0 ? { sticky: true } : undefined);
            close();
            load();
          })
          .catch((e) => { toast(errorText(e), "err"); });
      },
    },
  ], { large: true });
}

/* ============================================================
   8. 验活（🔴 这一族的判定今天住在前端，见文件尾的欠账说明）
   ============================================================ */

/**
 * 验活按钮的可用性。**三种可区分的 disable 理由**，不并成一句「不可用」：
 * 「上一次还在飞」与「刚探过」在屏幕上长得一模一样，而处置不同（等它 / 稍后再来）。
 * ⚠️ **`in_flight` 必须排在 `cooling_down` 前面**：在飞期间 `lastAt` 恒等于
 * 「刚刚」，两条判据**同时成立**。
 */
function verifyDisabledReason(id, s, now) {
  if (typeof id !== "string" || id === "") return "no_key";
  if (!s) return null;
  if (s.inFlight === true) return "in_flight";
  if (probeMinIntervalMs !== null && typeof s.lastAt === "number"
    && now - s.lastAt < probeMinIntervalMs) return "cooling_down";
  return null;
}

function verifyTitleKey(reason) {
  if (reason === "in_flight") return "keys.verify.disabledInFlight";
  if (reason === "cooling_down") return "keys.verify.disabledCoolingDown";
  if (reason === "no_key") return "keys.verify.disabledNoKey";
  // 可用时那一句说的是**按下去会发生什么**：它会真的向上游发一次请求。
  return "keys.verify.hintEnabled";
}

/**
 * **200 响应体** → 文案 code。
 * ⚠️ **只吃 200 的响应体，非 2xx 一律不走这里**（那一半在 `verifyTransportCode`）：
 * 探测闸占用返回的 429 落进这里会被说成「上游在限流，等一会儿就好」——
 * 而**一次上游请求都没发出去**。
 * ⚠️ `reason` 非空时 `status` 恒为 `null`（后端 catch 分支），所以先判 `reason`。
 * ⚠️⚠️ **这张 reason 表必须显式，不许写成「不认识就当网络错误」**：后端今天只产出
 * `timeout` / `network_error` 两种，而加一种是一行 diff。落进 `network_error`
 * 的后果是面板对运维说「连不上上游」，而真相可能完全不是那回事。
 */
function verifyResultCode(resp) {
  if (!resp || typeof resp !== "object") return "network_error";
  if (typeof resp.reason === "string") {
    if (resp.reason === "timeout") return "timeout";
    if (resp.reason === "network_error") return "network_error";
    return "unknown_reason";
  }
  const s = resp.status;
  if (typeof s !== "number") return "network_error";
  if (s >= 200 && s < 300) return "ok";
  // 401/403 说的是「这把 key 失效了」（换 key），429 说的是「上游在限流」（等一会儿）
  // —— 合成一条就是让运维去做一件没用的事。
  if (s === 401 || s === 403) return "unauthorized";
  if (s === 429) return "rate_limited";
  return "upstream_error";
}

/**
 * **管理层传输错误** → 文案 code。与上面严格分开：这一族说的全是
 * 「这次探测根本没发出去」，与上游状态无关。
 * ⚠️⚠️ **判据是顶层 `reason`，不是状态码、更不是那句中文 `message`**：
 * 同一个 429 下护栏产出**两种**拒绝（`probe_in_flight` 等它回来 /
 * `probe_cooldown` 稍后再试，而后者**不是这把 key 的故障**）。
 * 解析中文 message 会在切到别的语言时整个失效（后端只有中文一种）。
 */
function verifyTransportCode(err) {
  const body = err && typeof err.body === "object" && err.body !== null ? err.body : null;
  const reason = body === null ? undefined : body.reason;
  if (reason === "probe_in_flight") return "probe_in_flight";
  if (reason === "probe_cooldown") return "probe_cooldown";
  const s = err && typeof err.status === "number" ? err.status : 0;
  if (s === 404) return "key_not_found";
  if (s === 401) return "unauthorized_admin";
  return "transport_error";
}

/**
 * code → i18n key。**十二条各写一次字面量，一个都不许拼** ——
 * 拼出来的 key 与写死的 key 在行为上可以逐字节相同，而 i18n 门禁只认字面量：
 * 拼的话这十二个**正在用**的 key 会落进「未被引用」并把判据打红，
 * 而顺着报文去「清理未被引用的 key」删掉的就是活文案。
 * ⚠️ 默认支刻意是「上游回了个我们没细分的状态码」—— 这一族里最不武断的一句。
 */
function verifyResultLabelKey(code) {
  if (code === "ok") return "keys.verify.ok";
  if (code === "unauthorized") return "keys.verify.unauthorized";
  if (code === "rate_limited") return "keys.verify.rateLimited";
  if (code === "timeout") return "keys.verify.timeout";
  if (code === "network_error") return "keys.verify.networkError";
  if (code === "unknown_reason") return "keys.verify.unknownReason";
  if (code === "probe_in_flight") return "keys.verify.probeInFlight";
  if (code === "probe_cooldown") return "keys.verify.probeCooldown";
  if (code === "key_not_found") return "keys.verify.keyNotFound";
  if (code === "unauthorized_admin") return "keys.verify.unauthorizedAdmin";
  if (code === "transport_error") return "keys.verify.transportError";
  return "keys.verify.upstreamError";
}

/**
 * 跑一次验活。
 *
 * ⚠️⚠️ **自己的 `AbortController`，绝不复用模块级那个 `abort`。** 那一个是列表
 * 加载的取消闸，`load()` 每次都会先 abort 再重新赋值 —— 复用的话，任何一次
 * 自动刷新都会把在飞的验活作废，而**结果永远回不来、按钮却已经变回可点**。
 *
 * ⚠️ **不调 `load()`。** 它是只读探针：后端写零个存储字段，前端这一侧也不许
 * 借着「顺手刷新一下」把这一行的 strikes 与分档徽章换掉 —— 那会让运维以为是
 * 验活改的。
 */
function verifyOne(item) {
  const id = typeof item.id === "string" ? item.id : "";
  // **拿现有状态查，查完再建**：否则一行点不动的 key 也会在那张表里凭空落一项，
  // 而那张表的有界性全靠「键只可能来自真实存在过的 key id」。
  if (verifyDisabledReason(id, verifyState.get(id), Date.now()) !== null) return;
  const s = { inFlight: true, lastAt: Date.now(), code: null, ctl: new AbortController(), timer: null };
  const prev = verifyState.get(id);
  if (prev && prev.timer !== null) clearTimeout(prev.timer);
  verifyState.set(id, s);
  render();

  api.post(`/keys/${encodeURIComponent(id)}/verify`, undefined, { signal: s.ctl.signal })
    .then((body) => { if (!s.ctl.signal.aborted) s.code = verifyResultCode(body); })
    .catch((e) => {
      if (s.ctl.signal.aborted) return;
      s.code = verifyTransportCode(e);
    })
    .then(() => {
      // ⚠️ **收尾整段都在这一判里面**：`verifyState.get(id) !== s` 只有一种成因 ——
      //    `onHide()` 已经把这一次作废掉了。此时装定时器与重渲都是错的：
      //    那会给一个已经切走的板块留一个孤儿定时器 + 一次不可见的重绘。
      if (verifyState.get(id) !== s) return;
      s.inFlight = false;
      s.ctl = null;
      // 冷却到点之后要有一次重渲，否则那颗按钮会一直显示成「刚探过」——
      // 自动刷新默认是关着的，没有别的东西会把它重画。
      if (probeMinIntervalMs !== null) {
        s.timer = setTimeout(() => { s.timer = null; render(); }, probeMinIntervalMs);
      }
      render();
    });
}

/* ============================================================
   9. 明文回显
   ============================================================ */

/**
 * 取一把上游 key 的明文。`js/ui.js` 的 `revealControls` 要求**成功时直接返回
 * 明文字符串**，取不到就抛。
 *
 * 🔴 **端点直接给 `kind`，这里不从字段组合里反推**（06 §5.3 的 A 档）。
 * 这一族的 `kind` 恒是 `ok`（上游 key 本来就以明文存，要拿去打上游），
 * 但**照样要判**：写死的话，一把被手工改坏成空串的记录会被当成明文交出去
 * —— 后端那一侧同样没有写死，走的是同一条状态机。
 */
async function fetchSecret(id) {
  const body = await api.get(`/keys/${encodeURIComponent(id)}/reveal`);
  if (!body || body.kind !== "ok" || typeof body.key !== "string" || body.key === "") {
    // 抛一个**不带 status** 的错 ⇒ `js/ui.js` 的 `revealErrorKind()` 落到
    // `failed`（「这一次没拿到」），而不是冒充 404 / 403 那两档确定性的结论。
    throw new Error("reveal_not_ok");
  }
  return body.key;
}

/**
 * 四态各说各的话，**不并成一句「取不到」**：
 *   · `gone`        404 —— 这一行在别的标签页被删掉了（轮询表上的常态路径），去刷新；
 *   · `unavailable` 403 / 501 —— 这个部署关掉了明文回显；
 *   · `expired`     401 —— 会话失效（`js/api.js` 已经把人送回登录闸了）；
 *   · `failed`      其余（含网络失败）—— 重试可能就好。
 */
function revealMessageKey(kind) {
  if (kind === "gone") return "keys.reveal.gone";
  if (kind === "unavailable") return "keys.reveal.unavailable";
  if (kind === "expired") return "common.sessionExpired";
  return "keys.reveal.failed";
}

/* ============================================================
   10. 板块契约
   ============================================================ */

/** 关掉在飞的一切 + 所有定时器。`onHide()` 与「整块重建」共用它。 */
function stopAll() {
  generation += 1;
  if (abort) { abort.abort(); abort = null; }
  if (searchTimer !== null) { clearTimeout(searchTimer); searchTimer = null; }
  if (autoTimer !== null) { clearInterval(autoTimer); autoTimer = null; }
  if (freshTimer !== null) { clearInterval(freshTimer); freshTimer = null; }
  for (const s of verifyState.values()) {
    if (s.ctl) s.ctl.abort();
    if (s.timer !== null) clearTimeout(s.timer);
  }
  verifyState.clear();
  closeAddMenu();
}

export const accountsSection = {
  init(node) {
    nodes.root = node;
    build(node);
    // 点别处关掉添加菜单（与顶栏那个语言菜单同一个手感）。
    document.addEventListener("click", closeAddMenu);
    document.addEventListener("keydown", (ev) => { if (ev.key === "Escape") closeAddMenu(); });
  },
  onShow() {
    // 🔴 **先收干净再开**：`onShow()` 会被重复调用（切语言整页重绘、在同一个
    //    板块上再点一次导航），不先停一次的话每来一轮就多留一组定时器。
    stopAll();
    // ⚠️ 切语言时框架层会重跑 `onShow()` —— 整页重绘走的就是这条路（板块自己
    //    不许监听 `langchange`）。重新 `build()` 能让所有靠 `t()` 拼进 textContent
    //    的串（时刻、时长、计数）跟着换语言。
    build(nodes.root);
    syncControls();
    // 🔴 明文态不跨一次进出板块存活：重进板块一律回到掩码。
    revealState.reset();
    render();
    restartAuto();
    freshTimer = setInterval(paintFreshness, FRESHNESS_TICK_MS);
    loadKnobs().then(() => { if (nodes.root) paintFreshness(); });
    load();
  },
  onHide() {
    // 🔴 切走作废一切在飞请求与定时器（06 §3.2 ② 的第 ③ 条护栏）。
    stopAll();
  },
};

export default accountsSection;
