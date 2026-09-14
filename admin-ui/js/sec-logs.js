/**
 * 实时日志板块（`logs` ← agnes 的 `events`，裁定 P-6 改名为 kiro 的 `logs`）。
 *
 * 外观**照 kiro**（06 §3.2 ⑦）：`.logs-head` + `.logs-status` 三态胶囊 →
 * `.logs-toolbar`（搜索 / 级别 / 动作）→ `.logs-layout`（**亮色下也是深色面**的
 * `.logs-console` + 右侧 `.logs-detail`）。五档级别配色、2000 行环形缓冲照抄。
 * 数据面**照 agnes**：事件是**持久**的（写存储、带 TTL 有界），只采白名单诊断事件。
 *
 * # 🔴🔴 只能轮询，不能 SSE —— 这条是**结构性**的，别当成待办捡起来
 *
 * `/admin/api/*` 整棵树**只认 `X-Admin-Key` 请求头**（不收 `Authorization`、
 * 不收查询参数口令），而浏览器的 `EventSource` **设不了任何请求头**
 * ⇒ 一条 SSE 端点在面板里**根本没法带着口令连上**。
 * 剩下两条路都被否掉过（`src/admin/events.rs` 文件头逐字记着）：
 *   · 把口令挪进查询串 ⇒ 它会原样进 access log、进浏览器历史、进 Referer
 *     —— 而 kiro 的 `?api_key=` 正是这一条（06 §9 R-2 / 裁定 P-15）；
 *   · 给事件流单开一条免鉴权路径 ⇒ 事件里带着未鉴权请求的路径与上游错误体。
 * ⇒ **面板按固定间隔轮询 `GET /admin/api/events`，用 `after` 游标取增量。**
 * 🔴 **别为了「像 kiro」就把管理口令放进 URL** —— 那会当面推翻本仓自己钉过的纪律
 * （`js/api.js` 文件头那条「口令只走请求头」）。
 *
 * ⚠️ UI 上照旧保留 `connected / reconnecting / disconnected` 三态 ——
 * **轮询同样有这三态**（上一拍成不成功、连续失败几次），不是为了假装有长连接。
 *
 * # 🔴 四条贯穿本文件的纪律
 *
 * ① **`items` 里的字段一律视为完全不可信**：里面会出现上游返回的内容与
 *    **未鉴权请求的路径**（`admin.login_failed` 的 `fields.path` 就是攻击者能写的值）。
 *    后端**刻意不做**转义、不做截断（JSON 的字段边界由语法本身保证），
 *    🔴 **前端一律 `textContent` 渲染，全文件零 `innerHTML`。**
 * ② 🔴 **级别读不出来的那一条画成显式的 `unknown`，不许伪装成 `info`。**
 *    判定在后端（`view.rows[i].level`，`effective_level`），本文件只搬运 ——
 *    绿色的 info 徽章是这一族里最不该给的一个。
 * ③ **`cursor` 是 `null` 时保留上一个 `after`，不要推进。** `null` 的意思是
 *    「这一页是空的」，把它写回去等于把游标清零，下一拍会把整个窗口重放一遍。
 * ④ **板块内不许监听 `langchange`**：框架层会 `apply(document)` 之后重跑 `onShow()`。
 */
import { api } from "./api.js";
import { t } from "./i18n.js";
import { el, elI18n, toast } from "./ui.js";
import { badgeClass } from "./pure/badge-class.mjs";

/* ============================================================
   常量
   ============================================================ */

/**
 * 轮询间隔。
 *
 * ⚠️ **这个数直接决定稳态读吞吐**（`src/admin/events.rs` 文件头算过包线）：
 * 调小它是在拿服务端的存储读换一点点「更实时」。3 秒是 agnes 的现值。
 */
const POLL_MS = 3000;

/** 连续失败时的退避上限。**指数退避，成功即归零。** */
const POLL_MAX_MS = 30000;

/** 连续失败几次之后从「重连中」变成「断开」。 */
const DISCONNECT_AFTER = 3;

/**
 * 控制台里最多留几行。**环形缓冲**，超了从头丢。
 * ⚠️ **这是渲染侧的上限，不是「日志只有这么多」**：整份下载走
 * `GET /admin/api/events/download`，那一条从存储里现取。
 */
const MAX_LINES = 2000;

/**
 * 一次取多少条。后端 `DEFAULT_LIMIT = 200` / `MAX_LIMIT = 500`，
 * ⚠️ **越界一律回落到默认值（200），不是夹到 500** —— 发一个荒唐的 limit
 * 在后端是「回落」不是「纠正」，面板这边别去试探。
 */
const PAGE_LIMIT = 200;

/**
 * 级别筛选档。`all` = 不带 `level` 参数。
 *
 * ⚠️⚠️ **后端的 `LEVELS` 有 4 个而 Rust 的 `LogLevel` 只有 3 个（没有 `debug`）
 * —— 这是刻意的，别「顺手对齐」**：`level` 是**过滤器**，它比的是存储里那个字符串，
 * 而存储里的东西一律不可信（运维手改、旧版本写下的）。收 `debug` ⇒ 过滤结果是空
 * ⇒ 面板如实显示「这一档没有事件」；不收 ⇒ 它落进「不是合法级别 ⇒ 整个忽略」
 * 那一支 ⇒ **用户点了 debug，拿回来的是全部事件**，那比空列表糟得多。
 *
 * ⚠️⚠️ **文案键一律写成字面量，不许用模板串拼**（`` `section.ev.filter.${lv}` ``）：
 * `src/webui/mod.rs` 的 `referenced_keys()` 扫的是**双引号字面量**，拼出来的 key
 * 它一个都看不见 ⇒ 那几条词条会被「字典里每个 key 都被引用过」那一格判成孤儿而红。
 */
const LEVEL_FILTERS = [
  { id: "all", labelKey: "section.ev.filter.all" },
  { id: "debug", labelKey: "section.ev.filter.debug" },
  { id: "info", labelKey: "section.ev.filter.info" },
  { id: "warn", labelKey: "section.ev.filter.warn" },
  { id: "error", labelKey: "section.ev.filter.error" },
];

/**
 * 后端 `view.rows[i].labelKey` 给的是**它自己那套**文案键
 * （`src/core/admin/view/events.rs::event_level_label_key` 回的 `ev.level.*`），
 * 而本仓字典的命名空间登记表（`src/webui/mod.rs` 的 `NAMESPACES`）里**没有 `ev.`
 * 这一条** ⇒ 那几个串直接当 key 用的话，`t()` 会把它们原样画上屏幕。
 *
 * ⚠️ **这张表是权宜之计，不是设计**（与 `js/sec-accounts.js` 里那张 `VIEW_KEY`
 * 同一个形态）：它让同一个决定有了第二份实现。正确的收口是让两边用同一套命名空间，
 * 而 `.rs` 那两份不归本面改 ⇒ **登记为欠账**，见交付说明。
 * 🔴 认不出来时走 `section.ev.level.unknown`（**说「面板还不认识」，不冒充任何一档**）。
 */
const LEVEL_LABEL = {
  "ev.level.debug": "section.ev.level.debug",
  "ev.level.info": "section.ev.level.info",
  "ev.level.warn": "section.ev.level.warn",
  "ev.level.error": "section.ev.level.error",
  "ev.level.unknown": "section.ev.level.unknown",
};

/**
 * 🔴🔴 **分档徽章的类名映射 —— 一处必须点名的跨面不一致。**
 *
 * `src/core/admin/view/events.rs::level_badge_class()` 回的是
 * `badge` / `badge badge-danger` / `badge badge-warn` / `badge badge-ok`，
 * 而 `css/base.css` 里**根本没有 `.badge-ok` 与 `.badge-warn`**
 *（它有的是 `.badge-success` / `.badge-warning`）。
 * ⇒ 直接把 DTO 那个串挂上去的话，「正常」与「警告」两档**在屏幕上是同一个中性灰**，
 * 而 Rust 那格「互不相同」的判据只比较字符串、照样全绿 —— 那正是它自己注释里
 * 警告过的「在字符串上成立、在屏幕上不成立」。
 * ⚠️ 与 `js/sec-accounts.js` / `js/sec-apikeys.js` 是**同一处欠账**，
 * 而那张表现在**全站只有一份**（`js/pure/badge-class.mjs`）：三个板块各抄一张
 * 逐字相同的表时，漏改一份的表现形态恰好就是本注释开头那条「某两档同色」。
 */

/**
 * 控制台每一行的级别配色钩子（`.log-line.lvl-*`）。
 * ⚠️ CSS 里有五档（error / warn / info / debug / trace），而后端只产出四档 +
 * `unknown`。**`unknown` 不给任何一档颜色**（落到 `.log-line` 的默认前景色）——
 * 给一条读不懂的记录配上「看起来正常」的蓝色，正是纪律 ② 要挡的那件事的另一半。
 */
const LINE_LEVELS = new Set(["error", "warn", "info", "debug", "trace"]);

/* ============================================================
   本地化格式（06 §5.3 的 B 档）
   ============================================================ */

function langOf() { return document.documentElement.lang || "zh-CN"; }

/** 一条事件的时刻 → 本地时区的 `HH:MM:SS`。**控制台里不画日期**（太占宽）。 */
function fmtClock(ms) {
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "--:--:--";
  const p = (n) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** 详情栏里那一行完整时刻（带日期、带时区，本地）。 */
function fmtInstant(ms) {
  const d = new Date(ms);
  return Number.isNaN(d.getTime()) ? String(ms) : d.toLocaleString(langOf());
}

function fmtInt(n) { return new Intl.NumberFormat(langOf()).format(n); }

/** 清空一个节点。**不用 `innerHTML = ""`**（纪律 ①）。 */
function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }

/* ============================================================
   状态
   ============================================================ */

const state = {
  /** 环形缓冲。`{ raw, ts, level, labelKey, badgeClass }`，新的在**后**。 */
  lines: [],
  /** 游标。`null` = 还没取过 / 刚被重置。见纪律 ③。 */
  after: null,
  level: "all",
  search: "",
  paused: false,
  autoScroll: true,
  /** 连续失败次数。成功归零。 */
  fails: 0,
  /**
   * 这一轮里有没有**成功完成过一次轮询**。
   *
   * 🔴 **状态胶囊的判据必须是它，不能是「游标非空」** —— 那一版实测出过缺陷：
   * 事件库是空的时候后端如实回 `cursor: null`（「这一页是空的」），于是游标永远
   * 保持 `null`，胶囊就永远停在「连接中…」。**轮询明明每一拍都成功**，而屏幕上
   * 是一个看起来连不上的后台。
   * ⚠️ 一个全新部署**恰好**就是这个状态 —— 也就是这条缺陷最容易被第一次看见的地方。
   */
  polled: false,
  /** 上一次响应里那几个「本实例自述」的数（tooltip 用）。 */
  status: null,
  /** 选中的那一行（详情栏）。存的是 `lines` 里的对象引用，不是下标 —— 环形缓冲会移位。 */
  selected: null,
};

let timer = null;
let ctl = null;
/** 每次 `onShow()` 自增：迟到的响应拿旧号回来时直接丢掉。 */
let epoch = 0;

const nodes = {};

/* ============================================================
   骨架
   ============================================================ */

function build(node) {
  clear(node);

  // ── 顶部：标题 + 连接状态胶囊 ───────────────────────────────────────────
  const head = el("div", { class: "logs-head" });
  head.appendChild(elI18n("h2", "section.ev.title"));
  nodes.status = el("div", { class: "logs-status" });
  nodes.statusDot = el("span", { class: "logs-status-dot", "aria-hidden": "true" });
  nodes.statusText = el("span", null, t("section.ev.status.connecting"));
  nodes.status.appendChild(nodes.statusDot);
  nodes.status.appendChild(nodes.statusText);
  head.appendChild(nodes.status);
  node.appendChild(head);

  // ── 工具条 ──────────────────────────────────────────────────────────────
  const toolbar = el("div", { class: "logs-toolbar" });

  const search = el("div", { class: "logs-search" });
  search.appendChild(el("span", { class: "logs-search-icon", "aria-hidden": "true" }, "🔍"));
  nodes.searchInput = el("input", {
    type: "search", class: "form-control", "data-i18n-ph": "section.ev.searchPlaceholder",
  });
  nodes.searchInput.setAttribute("placeholder", t("section.ev.searchPlaceholder"));
  nodes.searchInput.setAttribute("aria-label", t("section.ev.searchPlaceholder"));
  nodes.searchInput.addEventListener("input", () => {
    // 🔴 **搜索只在本地过滤已经取回来的那些行，不改请求**：把它做成服务端参数
    // 需要一条后端不存在的全文检索，而「搜不到」与「这段时间没有」会混成一句话。
    state.search = nodes.searchInput.value.trim().toLowerCase();
    renderLines();
  });
  search.appendChild(nodes.searchInput);
  toolbar.appendChild(search);

  nodes.levelBtns = el("div", { class: "logs-levels", role: "group" });
  nodes.levelBtns.setAttribute("aria-label", t("section.ev.levelGroup"));
  for (const spec of LEVEL_FILTERS) {
    const b = elI18n("button", spec.labelKey, { type: "button", class: "btn btn-sm btn-outline" });
    b.setAttribute("data-level", spec.id);
    b.addEventListener("click", () => setLevel(spec.id));
    nodes.levelBtns.appendChild(b);
  }
  toolbar.appendChild(nodes.levelBtns);

  const actions = el("div", { class: "logs-actions" });

  nodes.pauseBtn = el("button", { type: "button", class: "btn btn-sm btn-outline" }, t("section.ev.pause"));
  nodes.pauseBtn.addEventListener("click", () => {
    state.paused = !state.paused;
    // ⚠️ **暂停只停「取新的」，不丢已经在屏幕上的那些**，也不动游标 ——
    //    恢复之后从同一个 `after` 继续，中间那段不会缺。
    if (state.paused) stopTimer(); else startTimer(0);
    paintControls();
    paintStatus();
  });
  actions.appendChild(nodes.pauseBtn);

  const clearBtn = elI18n("button", "section.ev.clear", { type: "button", class: "btn btn-sm btn-outline" });
  // ⚠️ **只清屏，不删服务端那份**：事件在本仓是**持久**的（写存储、带 TTL）。
  //    按钮上的提示必须说清这一点，否则运维会以为自己刚刚销毁了审计线索。
  clearBtn.setAttribute("title", t("section.ev.clearHint"));
  clearBtn.addEventListener("click", () => {
    state.lines = [];
    state.selected = null;
    renderLines();
    renderDetail();
  });
  actions.appendChild(clearBtn);

  nodes.downloadBtn = elI18n("button", "section.ev.download", { type: "button", class: "btn btn-sm btn-outline" });
  nodes.downloadBtn.addEventListener("click", () => download());
  actions.appendChild(nodes.downloadBtn);

  const auto = el("label", { class: "form-check" });
  nodes.autoInput = el("input", { type: "checkbox" });
  nodes.autoInput.checked = state.autoScroll;
  nodes.autoInput.addEventListener("change", () => { state.autoScroll = nodes.autoInput.checked; });
  auto.appendChild(nodes.autoInput);
  auto.appendChild(elI18n("span", "section.ev.autoScroll"));
  actions.appendChild(auto);

  toolbar.appendChild(actions);
  node.appendChild(toolbar);

  // ── 诚实横幅（后端字段驱动，见 `should_warn`）───────────────────────────
  nodes.banner = el("div", { class: "banner banner-warn", hidden: true });
  node.appendChild(nodes.banner);

  // ── 控制台 + 详情栏 ─────────────────────────────────────────────────────
  const layout = el("div", { class: "logs-layout" });
  nodes.console = el("div", { class: "logs-console", role: "log", "aria-live": "off" });
  layout.appendChild(nodes.console);

  nodes.detail = el("aside", { class: "logs-detail", hidden: true });
  const dHead = el("div", { class: "logs-detail-head" });
  dHead.appendChild(elI18n("h4", "section.ev.detail"));
  const dClose = el("button", { type: "button", class: "acct-icon-btn is-mini", "aria-label": t("section.ev.detailClose") }, "×");
  dClose.addEventListener("click", () => { state.selected = null; renderLines(); renderDetail(); });
  dHead.appendChild(dClose);
  nodes.detail.appendChild(dHead);
  nodes.detailBody = el("pre", { class: "logs-detail-body" });
  nodes.detail.appendChild(nodes.detailBody);
  layout.appendChild(nodes.detail);

  node.appendChild(layout);

  paintControls();
  paintStatus();
  renderLines();
  renderDetail();
}

function setLevel(lv) {
  if (state.level === lv) return;
  state.level = lv;
  // 🔴 **换档位必须把游标与缓冲一起清掉**：服务端的 `level` 过滤发生在
  //    「归并 → 过滤 → 截断」那一串里，沿用旧 `after` 只会拿到「这个级别、
  //    比旧游标更新」的那几条 —— 屏幕上是一段掐头的历史，而它看起来很正常。
  state.after = null;
  state.lines = [];
  state.selected = null;
  state.fails = 0;
  paintControls();
  renderLines();
  renderDetail();
  if (!state.paused) startTimer(0);
}

function paintControls() {
  for (const b of nodes.levelBtns.children) {
    const on = b.getAttribute("data-level") === state.level;
    b.classList.toggle("active", on);
    b.setAttribute("aria-pressed", on ? "true" : "false");
  }
  nodes.pauseBtn.textContent = t(state.paused ? "section.ev.resume" : "section.ev.pause");
  nodes.pauseBtn.setAttribute("aria-pressed", state.paused ? "true" : "false");
}

/* ============================================================
   连接状态 —— 轮询的三态
   ============================================================ */

function paintStatus() {
  const s = nodes.status;
  if (!s) return;
  let stateName;
  let key;
  if (state.paused) { stateName = ""; key = "section.ev.status.paused"; }
  else if (state.fails === 0 && state.polled) { stateName = "connected"; key = "section.ev.status.connected"; }
  else if (state.fails === 0) { stateName = "connecting"; key = "section.ev.status.connecting"; }
  else if (state.fails < DISCONNECT_AFTER) { stateName = "reconnecting"; key = "section.ev.status.reconnecting"; }
  else { stateName = "disconnected"; key = "section.ev.status.disconnected"; }
  if (stateName) s.setAttribute("data-state", stateName);
  else s.removeAttribute("data-state");
  nodes.statusText.textContent = t(key);

  // ⚠️ **`buffered` / `malformed` 刻意走 tooltip、不进黄条**
  //   （`should_warn` 的文档逐条写了理由：前者是运行中的正常状态，后者在本仓
  //    `src/` 里没有任何路径能产出、**恒为 0**，挂上去只会多一条永远为假的分支）。
  //    🔴 **代价明写**：它们平时不显眼，出事时要运维去看这个 tooltip 才发现。
  const st = state.status;
  s.setAttribute("title", st
    ? t("section.ev.statusHint", {
      shard: st.shardId, buffered: fmtInt(st.buffered), dropped: fmtInt(st.dropped), malformed: fmtInt(st.malformed),
    })
    : t("section.ev.statusHintEmpty"));
}

/** 顶部黄条。🔴 **只看后端那四个字段**，不在这里硬编码一个真假。 */
function paintBanner(body) {
  const b = nodes.banner;
  clear(b);
  if (!body || !body.view || !body.view.warn) { b.hidden = true; return; }
  // 四条各说各的话，**不并成一句「有问题」** —— 它们的处置完全不同。
  if (body.dropped > 0) b.appendChild(elI18n("span", "section.ev.warn.dropped"));
  if (body.budgetExhausted) b.appendChild(elI18n("span", "section.ev.warn.budgetExhausted"));
  if (body.truncated) b.appendChild(elI18n("span", "section.ev.warn.truncated"));
  // 🔴 **`cursorAhead` 的意思是「空结果不代表没有事件，是时钟纠纷」** ——
  //    不说出来的话，一段本该看见的日志会被读成「这段时间很安静」。
  if (body.cursorAhead) b.appendChild(elI18n("span", "section.ev.warn.cursorAhead"));
  b.hidden = false;
}

/* ============================================================
   轮询
   ============================================================ */

function stopTimer() {
  if (timer !== null) { clearTimeout(timer); timer = null; }
  if (ctl) { ctl.abort(); ctl = null; }
}

/** 排下一拍。`delay` 省略时按连续失败次数指数退避。 */
function startTimer(delay) {
  if (timer !== null) { clearTimeout(timer); timer = null; }
  const wait = delay === undefined
    ? Math.min(POLL_MAX_MS, POLL_MS * 2 ** Math.max(0, state.fails - 1))
    : delay;
  timer = setTimeout(() => { timer = null; poll(); }, wait);
}

async function poll() {
  if (state.paused) return;
  const mine = epoch;
  if (ctl) ctl.abort();
  ctl = new AbortController();
  const mineCtl = ctl;

  const params = [`limit=${PAGE_LIMIT}`];
  if (state.after !== null) params.push(`after=${state.after}`);
  if (state.level !== "all") params.push(`level=${encodeURIComponent(state.level)}`);

  try {
    const body = await api.get(`/events?${params.join("&")}`, { signal: mineCtl.signal });
    if (mine !== epoch || mineCtl !== ctl) return;
    state.fails = 0;
    state.polled = true;
    ingest(body);
  } catch (e) {
    if (mine !== epoch || mineCtl !== ctl) return;
    // 401 已经被 `api.js` 送回登录闸了，这里不再重试也不再报错。
    if (e && e.status === 401) { stopTimer(); return; }
    state.fails += 1;
    paintStatus();
  }
  if (!state.paused) startTimer();
}

/** 把一页响应并进环形缓冲。 */
function ingest(body) {
  state.status = {
    shardId: String(body.shardId || "—"),
    buffered: Number(body.buffered) || 0,
    dropped: Number(body.dropped) || 0,
    malformed: Number(body.malformed) || 0,
  };

  // 🔴 纪律 ③：`cursor` 是 `null` ⇒ **保留上一个 `after`**。
  if (typeof body.cursor === "number") state.after = body.cursor;

  const items = Array.isArray(body.items) ? body.items : [];
  const rows = (body.view && Array.isArray(body.view.rows)) ? body.view.rows : [];
  // 后端按 ts **降序**给（`items[0]` 是最新的一条），控制台从上到下是时间正序
  // ⇒ 倒着并进去。⚠️ `rows` 与 `items` **同序同长**，下标必须一起倒。
  for (let i = items.length - 1; i >= 0; i -= 1) {
    const raw = items[i];
    const view = rows[i] || {};
    state.lines.push({
      raw,
      ts: typeof raw.ts === "number" ? raw.ts : 0,
      // 🔴 纪律 ②：级别一律取后端判好的那一格，**不看 `raw.level`**。
      level: typeof view.level === "string" ? view.level : "unknown",
      labelKey: LEVEL_LABEL[view.labelKey] || "section.ev.level.unknown",
      badgeClass: badgeClass(view.badgeClass),
    });
  }
  // 环形缓冲：超了从**头**丢（丢最旧的）。
  if (state.lines.length > MAX_LINES) state.lines.splice(0, state.lines.length - MAX_LINES);

  paintBanner(body);
  paintStatus();
  renderLines();
}

/* ============================================================
   渲染
   ============================================================ */

/** 这一行参不参与搜索匹配。**全部字段拍平成一个串**，含 `fields` 里的值。 */
function haystack(line) {
  const r = line.raw;
  const parts = [String(r.event || ""), String(r.msg || ""), line.level];
  if (r.fields && typeof r.fields === "object") {
    for (const [k, v] of Object.entries(r.fields)) parts.push(k, String(v));
  }
  return parts.join(" ").toLowerCase();
}

function renderLines() {
  const host = nodes.console;
  if (!host) return;
  // 画之前先记住「人有没有把滚动条拖上去过」：自动滚只在贴底时才滚。
  const atBottom = host.scrollHeight - host.scrollTop - host.clientHeight < 24;
  clear(host);

  const visible = state.search
    ? state.lines.filter((l) => haystack(l).includes(state.search))
    : state.lines;

  if (visible.length === 0) {
    host.appendChild(elI18n("div", state.search ? "section.ev.emptySearch" : "section.ev.empty", { class: "logs-empty" }));
    return;
  }

  for (const line of visible) {
    const row = el("div", { class: "log-line" });
    if (LINE_LEVELS.has(line.level)) row.classList.add(`lvl-${line.level}`);
    if (state.selected === line) row.classList.add("selected");
    row.setAttribute("tabindex", "0");
    row.appendChild(el("span", { class: "log-ts" }, fmtClock(line.ts)));
    // ⚠️ 级别那一列画的是**文案**（已翻译），不是原始串：原始串在详情栏里看得到。
    row.appendChild(el("span", { class: "log-level" }, t(line.labelKey)));
    // 🔴 消息一律 textContent（纪律 ①）：`event` 与 `msg` 都可能带着攻击者写进来的内容。
    row.appendChild(el("span", { class: "log-msg" }, `${line.raw.event || "—"} · ${line.raw.msg || ""}`));
    const open = () => { state.selected = state.selected === line ? null : line; renderLines(); renderDetail(); };
    row.addEventListener("click", open);
    row.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); open(); }
    });
    host.appendChild(row);
  }

  if (state.autoScroll && atBottom) host.scrollTop = host.scrollHeight;
}

function renderDetail() {
  if (!nodes.detail) return;
  const line = state.selected;
  if (!line) { nodes.detail.hidden = true; return; }
  nodes.detail.hidden = false;
  clear(nodes.detailBody);

  // 徽章 + 完整时刻。⚠️ 徽章类名过一道 `badgeClass()`（见那段注释）。
  //    入环时已经翻过一次，这里再翻一次是**幂等**的 —— 留着是因为「上屏前一定翻过」
  //    这条不该依赖「另一处记得翻」。
  nodes.detailBody.appendChild(el("span", { class: badgeClass(line.badgeClass) }, t(line.labelKey)));
  nodes.detailBody.appendChild(el("span", null, `  ${fmtInstant(line.ts)}\n\n`));
  // 整条原文。🔴 **`JSON.stringify` 之后走 textContent**，不拼 HTML。
  // ⚠️ `JSON.stringify` 对含循环引用的值会抛 —— `items` 来自 `res.json()`
  //    产不出循环，但这一条同样服务「将来有人把别的东西塞进来」那一刻。
  let text;
  try { text = JSON.stringify(line.raw, null, 2); }
  catch (e) { text = t("section.ev.detailUnserializable"); }
  nodes.detailBody.appendChild(el("span", null, text));
}

/* ============================================================
   下载
   ============================================================ */

/**
 * 整份下载。
 *
 * 🔴 **必须走 `api.raw()` 取 blob，不能把 URL 塞进 `<a href>`**：那条端点同样
 * 只认 `X-Admin-Key` 请求头，而浏览器的导航请求带不上它 —— 直接开链接拿到的是 401。
 * ⚠️ 拿到的是**逐行 `JSON.stringify`**，不是一个 JSON 数组：它是给人在终端里
 * `grep` / 逐行处理用的格式。
 */
async function download() {
  nodes.downloadBtn.disabled = true;
  let url = null;
  try {
    const res = await api.raw("GET", "/events/download");
    if (!res.ok) { toast(t("section.ev.downloadFailed"), "warn", { sticky: true }); return; }
    const blob = await res.blob();
    url = URL.createObjectURL(blob);
    const a = el("a", { href: url, download: "agnes2api-events.txt" });
    document.body.appendChild(a);
    a.click();
    a.remove();
  } catch (e) {
    if (e && e.status === 401) return;
    toast(t("section.ev.downloadFailed"), "warn", { sticky: true });
  } finally {
    // ⚠️ **objectURL 必须回收**：不回收的话每点一次下载都在这个标签页里钉住一份
    //    完整的日志副本，直到刷新。延一拍再撤，给 `click()` 留出取数的时间。
    if (url) setTimeout(() => URL.revokeObjectURL(url), 10000);
    nodes.downloadBtn.disabled = false;
  }
}

/* ============================================================
   板块契约
   ============================================================ */

export const logsSection = {
  init(node) {
    nodes.root = node;
  },
  onShow() {
    epoch += 1;
    stopTimer();
    // ⚠️ 切语言时框架层会重跑 `onShow()`；重新 `build()` 让所有靠 `t()` 拼进
    //    textContent 的串（级别名、状态文案、时刻）跟着换语言。
    build(nodes.root);
    // 🔴 **重进板块从头拉一遍**：上一次离开之后服务端那边多半已经滚过几轮，
    //    沿用旧游标只会拿到一段掐头的历史。
    state.after = null;
    state.lines = [];
    state.selected = null;
    state.fails = 0;
    state.polled = false;
    state.paused = false;
    paintControls();
    renderLines();
    renderDetail();
    poll();
  },
  onHide() {
    // 🔴 切走停轮询、作废在飞请求。`epoch` 自增让迟到的响应认不出自己。
    epoch += 1;
    stopTimer();
  },
};

export default logsSection;
