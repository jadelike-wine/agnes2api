/**
 * 「支持模型」板块 —— **三张卡，回答三个不同的问题，谁也不替代谁**：
 *
 * ① **模型目录**（`GET /admin/api/models`，零存储读、零出站）——「**本网关**支持哪些
 *    模型、拿什么协议/端点去调」；
 * ② **上游模型**（`GET /admin/api/upstream/models`，**真打一次上游**）——「**这个账号**
 *    此刻有哪些模型」，外加与目录的**双向差集**；
 * ③ **连通性测试**（`POST /admin/api/models/{id}/test`，**逐个真发一次最小对话**）——
 *    「**这一刻**这个模型通不通、多快」。
 *
 * ⚠️ 三者不可互相替代：② 那份清单**没有协议归属、没有端点**，拿它替掉 ① 会让集成
 * 示例、调试台、这张表一起失去所有可照抄的调用方式；而一个模型出现在 ② 里、与它在
 * ③ 里真的能出话，同样是两件事（前者是账号的权限表，后者是这一刻的链路）。
 *
 * ── 🔴 本板块最要紧的一条：TS 版那个「全部测一遍只测得到第一个」的缺陷 ─────────
 *
 * 后端那把护栏（`src/admin/models.rs` 的 `MODEL_TEST_KIND`）是**常量 kind**，
 * 整轮测试共用同一个最小间隔 —— 那是扛上游边缘限流的闸，**不是**「整轮会被自动
 * 摊开」。零间隔的串行循环撞上它的结果是：整轮不到一秒跑完，N 行里 N-1 行回
 * `probe_cooldown`，也就是把**我们自己的节流**当成一张连通性矩阵交给运维，
 * 而那些行盖住的可能是真的不通的模型。
 *
 * ⇒ **摊开一轮必须由发起方限速**。本文件的做法：
 *   · 间隔值**从 `GET /admin/api/capabilities` 的 `probes.minIntervalMs` 取**，
 *     🔴 **绝不在前端写一个 3000**（那就是第二份真源，后端改了这边不会有任何信号）；
 *   · **读不到那个数就不让整轮跑**（按钮禁用 + 说清为什么），而不是拿一个猜的数去跑
 *     —— 猜小了就是把缺陷原样复活，猜大了就是凭空拖长每一轮；
 *   · 即便如此仍可能有行落进 `probe_cooldown`（别的标签页 / 一条 curl 循环同样在打），
 *     那时表下面会出现一条**看得见的**说明：**这几行是被我们自己的护栏挡下的，
 *     不是模型不通**。少了这一句，那几行与「不通」在屏幕上不可区分。
 *
 * ── 三条纪律 ──────────────────────────────────────────────────────────────
 * ① 一切来自接口的内容一律 `textContent`（`el()` 走的就是它）；
 * ② **判定一律读响应里那一格 `view`**（06 §5.3 的 A 档，判定在
 *    `src/core/admin/view/models.rs`），不在这里把 `protocols.includes(...)` 再算一遍；
 * ③ **一切形态分支只读接口返回的字段**，不嗅探运行时、不写死协议 id 与端点路径。
 *
 * ⚠️ **板块内不许监听 `langchange`**：框架层 `apply(document)` 之后会重跑 `onShow()`。
 */
import { api, ApiError } from "./api.js";
import { t } from "./i18n.js";
import { el, elI18n, svgIcon } from "./ui.js";
// 🔴 **徽章类名要过这一道翻译**，和 `sec-apikeys.js` / `sec-accounts.js` / `sec-logs.js`
//    同源。今天 `src/core/admin/view/models.rs` 恰好发的是 `badge badge-success` /
//    `badge badge-off`，这两个类 `css/base.css` 里真有 ⇒ 原样上屏也能看；
//    但那是**巧合不是保证**：另外两个发射器（`keys::badge_class` / `apikeys::ak_badge_class`）
//    发的 `badge-ok` / `badge-warn` 在 CSS 里根本不存在，全靠这张别名表翻。
//    哪天 models.rs 也改口发 `badge-ok`，这三档就在屏幕上塌成同一片裸 `.badge` 灰，
//    而 Rust 那几格「N 档互不相同」的判据只比字符串、照样全绿。
//    ⇒ **不赌那个巧合**：和另外三个板块走同一道。理由全文见那个模块的文件头。
import { badgeClass } from "./pure/badge-class.mjs";

/* ============================================================
   图标（`js/ui.js` 的 `svgIcon()` 是全站唯一造 SVG 的地方）
   ============================================================ */

const ICON_CLOUD = "M 18 10 h -1.26 A 8 8 0 1 0 9 20 h 9 a 5 5 0 0 0 0 -10 z";
const ICON_BOLT = "M 13 2 L 3 14 h 8 l -1 8 l 10 -12 h -8 z";
const ICON_REFRESH = "M 23 4 v 6 h -6 M 1 20 v -6 h 6 M 20.5 9 a 9 9 0 0 0 -14.9 -3.4 L 1 10 M 23 14 l -4.6 4.4 A 9 9 0 0 1 3.5 15";

/* ============================================================
   服务端视图 key → 本板块字典 key
   ============================================================ */

/**
 * 🔴 **服务端交出来的判定 key 的别名表。**
 *
 * `src/core/admin/view/models.rs` 的 `modality_label_key()` 交出的是
 * `models.modality.*` 那一族名字，而本仓面板的词条命名空间登记表
 * （`src/webui/mod.rs` 的 `NAMESPACES`）今天只认六个前缀，`models.` 不在其中，
 * 而那张表住在 `.rs` 里、本轮不归本文件这一面改。
 *
 * ⇒ 这里做一次**纯改名**：**判定仍然完全在服务端**（哪个形态对应哪一档由它决定），
 * 本表只把那个标识符翻成本仓字典里的 key。
 * ⚠️ **认不出来的一律照实画原始 key**（下面 `modalityCell` 的 `else` 支），
 * 绝不冒充任何一档已知形态 —— 这与服务端 `null` 那一档是同一条纪律。
 */
const MODALITY_KEY = {
  "models.modality.chat": "section.md.modality.chat",
  "models.modality.image": "section.md.modality.image",
  "models.modality.video": "section.md.modality.video",
};

/** 上游那条端点的 `reason` → 文案。**认不出来落到最后那一档**，不假装知道。 */
const UP_REASON_KEY = {
  no_key: "section.md.up.reason.noKey",
  upstream_error: "section.md.up.reason.upstreamError",
  body_incomplete: "section.md.up.reason.bodyIncomplete",
  bad_payload: "section.md.up.reason.badPayload",
  timeout: "section.md.up.reason.timeout",
  network_error: "section.md.up.reason.networkError",
  probe_in_flight: "section.md.up.reason.probeInFlight",
  probe_cooldown: "section.md.up.reason.probeCooldown",
};

/** 连通性测试那条端点的 `reason` → 文案。 */
const TEST_REASON_KEY = {
  no_key: "section.md.test.reason.noKey",
  upstream_error: "section.md.test.reason.upstreamError",
  timeout: "section.md.test.reason.timeout",
  network_error: "section.md.test.reason.networkError",
  probe_in_flight: "section.md.test.reason.probeInFlight",
  probe_cooldown: "section.md.test.reason.probeCooldown",
  modality_not_testable: "section.md.test.reason.notTestable",
};

/**
 * 「这几行是被我们自己的护栏挡下的」那句话该不该出现。
 * 🔴 **判据是 `reason`，不是「失败了几行」**：一行 `upstream_error` 是真的不通。
 */
const SELF_THROTTLED = ["probe_in_flight", "probe_cooldown"];

/* ============================================================
   状态
   ============================================================ */

let nodes = null;

/** 目录。`null` = 还没读到 / 读不出来（两者在渲染上同一档，都还不知道）。 */
let catalog = null;
/** 当前协议筛选；`""` = 「全部」。**档位来自响应，不是本地枚举。** */
let filter = "";
/** 有一条目录读在飞没有。**隐式入口（`onShow`）在飞时只重画、不再发第二条。** */
let inFlight = false;
/** 在飞那条目录读的世代号（被抢占的那条不许把结果写回来）。 */
let seq = 0;
let abort = null;

/**
 * 上游那张卡的状态机。**四档，恒有一档。**
 * ⚠️ `idle`（还没问过上游）与 `ok` 且清单为空（问过了、上游一个都没回）**不共用一档**：
 * 合并的话，一张什么都没查过的卡会对运维说一句关于上游的事实。
 */
let up = { state: "idle", reason: null, status: null, models: null };

/**
 * 连通性测试那张卡的状态。
 * `rows === null` = 这一次进面板以来**还没测过**，与「测过了、一行都没通」是两句话。
 */
let test = { rows: null, running: false, waiting: false };

/**
 * 后端那把探测护栏的最小间隔（毫秒）。`null` = **还没读到**。
 * 🔴 见文件头：它是整轮节奏的唯一真源，读不到就不让整轮跑。
 */
let probeMinIntervalMs = null;
/** capabilities 这一次读失败了没有（与「还没读」分开，前者要给一颗重试）。 */
let capFailed = false;

function sleep(ms) {
  return new Promise((resolve) => { setTimeout(resolve, ms); });
}

/* ============================================================
   响应窄化（**读不出来一律 `null`，不是空数组**）
   ============================================================ */

function asObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v) ? v : null;
}

/**
 * 把目录响应窄化成本板块要的形状。
 *
 * ⚠️ **`view.rows` 与 `models` 同序同长是服务端承诺的**，这里仍然按 `id` 建索引再取
 * —— 靠下标对齐等于把一条「今天成立」的性质当成不变量用，而它失效时不会有任何信号。
 */
function narrowCatalog(body) {
  const b = asObject(body);
  if (b === null || !Array.isArray(b.protocols) || !Array.isArray(b.models)) return null;
  const view = asObject(b.view);
  if (view === null || !Array.isArray(view.rows)) return null;

  const protocols = [];
  for (const item of b.protocols) {
    const p = asObject(item);
    if (p === null || typeof p.id !== "string" || typeof p.label !== "string") return null;
    protocols.push({ id: p.id, label: p.label });
  }

  const rowsById = new Map();
  for (const item of view.rows) {
    const r = asObject(item);
    if (r === null || typeof r.id !== "string" || !Array.isArray(r.protocols)) return null;
    rowsById.set(r.id, r);
  }

  const models = [];
  for (const item of b.models) {
    const m = asObject(item);
    if (m === null || typeof m.id !== "string" || typeof m.modality !== "string") return null;
    if (!Array.isArray(m.protocols) || !Array.isArray(m.endpoints)) return null;
    const row = rowsById.get(m.id);
    if (row === undefined) return null;
    const endpoints = [];
    for (const e of m.endpoints) {
      const ep = asObject(e);
      if (ep === null || typeof ep.method !== "string" || typeof ep.path !== "string") return null;
      endpoints.push({ method: ep.method, path: ep.path });
    }
    const badges = [];
    for (const b2 of row.protocols) {
      const bb = asObject(b2);
      if (bb === null || typeof bb.id !== "string" || typeof bb.label !== "string") return null;
      if (typeof bb.badgeClass !== "string") return null;
      // 🔴 翻译**落在读出来的那一刻**，不是落在上屏的那一刻：这样 `cls` 从诞生起
      //    就已经是「这份 CSS 里真的存在」的那一串，中间再转手几次都不会漏。
      //    （`typeof` 那一格是窄化契约 —— 读不出来整份判 `null`，不退化成裸 `.badge`。）
      badges.push({
        id: bb.id,
        label: bb.label,
        available: bb.available === true,
        cls: badgeClass(bb.badgeClass),
      });
    }
    models.push({
      id: m.id,
      modality: m.modality,
      // 🔴 判定来自服务端，本文件一格都不重算。
      modalityLabelKey: typeof row.modalityLabelKey === "string" ? row.modalityLabelKey : null,
      protocols: m.protocols.filter((x) => typeof x === "string"),
      endpoints,
      badges,
    });
  }
  return { protocols, models };
}

/* ============================================================
   卡 ① 模型目录
   ============================================================ */

function cardShell(titleKey, actionNodes) {
  const card = el("div", { class: "card" });
  const head = el("div", { class: "card-header" });
  head.appendChild(elI18n("h3", titleKey));
  if (actionNodes && actionNodes.length > 0) {
    const acts = el("div", { class: "table-actions" });
    for (const n of actionNodes) acts.appendChild(n);
    head.appendChild(acts);
  }
  card.appendChild(head);
  const body = el("div");
  card.appendChild(body);
  return { card, body };
}

/**
 * 按协议筛选的分段控件。**档位由响应里的 `protocols[]` 生成**，本文件不认识任何协议 id。
 * 「全部」那一档的值是空串 —— 它不是一个协议，所以它不从响应里来。
 *
 * ⚠️ `.seg-btn.is-active` 除了实心底还带 `font-weight:700`（`.seg-btn` 本身是 500），
 * 那是 WCAG 1.4.1 要的那条**非颜色**线索；`aria-pressed` 是读屏那条腿，两条都要。
 */
function filterBar() {
  const wrap = el("div", { class: "seg-group" });
  const mk = (id, label, i18nKey) => {
    const on = filter === id;
    const b = el("button", {
      type: "button",
      class: on ? "seg-btn is-active" : "seg-btn",
      "aria-pressed": on ? "true" : "false",
      "data-protocol": id,
    }, i18nKey === null ? label : t(i18nKey));
    b.addEventListener("click", () => { if (filter !== id) { filter = id; render(); } });
    return b;
  };
  wrap.appendChild(mk("", "", "section.md.catalog.filterAll"));
  // 展示名走响应里的 `label`（协议的专名，**刻意不进 i18n**：翻译一个专名只会制造歧义）。
  for (const p of catalog.protocols) wrap.appendChild(mk(p.id, p.label, null));
  return wrap;
}

/**
 * 一行的协议可用性矩阵。🔴 **四个格子恒在**，不可用的那几格只是换个类。
 * 把不可用的过滤掉之后，一个只在一条协议上可用的模型那一行会**近乎是空的**
 * —— 与「读不出来」长得一模一样。
 */
function badgeCell(m) {
  const td = el("td");
  for (const b of m.badges) {
    const tip = b.available ? t("section.md.badge.yes") : t("section.md.badge.no");
    // `textContent` 是协议的**专名**不是状态名 ⇒ 状态必须另有线索：
    // `.badge-off` 自带一条非颜色声明（删除线），`title` 是鼠标/读屏那条腿。
    // ⚠️ `b.cls` 在 `narrowCatalog()` 里就已经过了 `badgeClass()`，这里不再翻第二遍。
    td.appendChild(el("span", {
      class: b.cls,
      "data-protocol": b.id,
      "data-available": b.available ? "yes" : "no",
      title: `${b.label} — ${tip}`,
    }, b.label));
  }
  return td;
}

/** 端点那一列。**原样搬运响应里的 `method` / `path`，一个字符都不在前端拼。** */
function endpointCell(m) {
  const td = el("td");
  // 视频模型那一行有两条（建任务 + 轮询），**两条都要画** ——
  // 只画一条就是把两段式教成一段式，运维照着写出来的客户端永远拿不到结果。
  for (const e of m.endpoints) td.appendChild(el("div", { class: "kv-mono" }, `${e.method} ${e.path}`));
  return td;
}

/** 形态那一格。**表外的形态照实显示原值**，不冒充任何一档已知形态。 */
function modalityCell(m) {
  const td = el("td");
  const key = m.modalityLabelKey === null ? undefined : MODALITY_KEY[m.modalityLabelKey];
  if (key === undefined) td.appendChild(el("code", { class: "kv-mono" }, m.modality));
  else td.appendChild(elI18n("span", key));
  return td;
}

function catalogCard() {
  const { card, body } = cardShell("section.md.catalog.title", catalog === null ? [] : [filterBar()]);

  if (catalog === null) {
    // 🔴 **这一档绝不能退化成一张空表**：空表读作「这个网关一个模型都没有」，
    // 而事实是我们**不知道**它认得哪些。⇒ 一条红条 + 一颗「再读一次」+ 一根 EM DASH。
    const banner = el("div", { class: "banner banner-danger", role: "status" });
    banner.appendChild(elI18n("span", "section.md.loadFailed"));
    const retry = elI18n("button", "section.md.retry", { type: "button", class: "btn btn-sm btn-outline" });
    // **`true` = 抢占**：这是用户明确表达的意图，它该赢；否则一条挂住的读会让这颗
    // 按钮变成一颗按了没反应的死按钮。
    retry.addEventListener("click", () => { load(true); });
    banner.appendChild(retry);
    body.appendChild(banner);
    const dash = el("p", { class: "empty-state", title: t("section.md.unknown") }, "—");
    body.appendChild(dash);
    return card;
  }

  const rows = filter === "" ? catalog.models : catalog.models.filter((m) => m.protocols.includes(filter));
  const scroll = el("div", { class: "models-table-scroll" });
  const table = el("table", { class: "models-table" });
  const thead = el("thead");
  const hr = el("tr");
  for (const k of ["section.md.col.id", "section.md.col.modality", "section.md.col.protocols", "section.md.col.endpoints"]) {
    hr.appendChild(elI18n("th", k));
  }
  thead.appendChild(hr);
  table.appendChild(thead);
  const tbody = el("tbody");
  if (rows.length === 0) {
    // ⚠️「这条协议上一个模型都没有」与「这个网关一个模型都没有」是两句话。
    const tr = el("tr", { class: "models-state-row" });
    const td = el("td", { colspan: "4" });
    td.appendChild(elI18n("div", filter === "" ? "section.md.empty" : "section.md.filterEmpty", { class: "empty-state" }));
    tr.appendChild(td);
    tbody.appendChild(tr);
  }
  for (const m of rows) {
    const tr = el("tr", { class: "model-row", "data-model": m.id });
    const idTd = el("td", { class: "model-cell-id" });
    idTd.appendChild(el("code", null, m.id));
    tr.appendChild(idTd);
    tr.appendChild(modalityCell(m));
    tr.appendChild(badgeCell(m));
    tr.appendChild(endpointCell(m));
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  scroll.appendChild(table);
  body.appendChild(scroll);
  return card;
}

/* ============================================================
   卡 ② 上游模型
   ============================================================ */

function idChips(labelKey, ids, group) {
  const wrap = el("div", { "data-group": group });
  // ⚠️ 小标题带 `{n}` ⇒ **必须走 `t(key, params)`**；`elI18n` 内部调的是不带参数的
  //    `t()`，用它会让屏幕上出现一个裸的占位符。
  wrap.appendChild(el("div", { class: "key-sub-h" }, t(labelKey, { n: ids.length })));
  const grid = el("div", { class: "model-chip-grid" });
  for (const id of ids) grid.appendChild(el("span", { class: "model-chip", "data-up-id": id }, id));
  wrap.appendChild(grid);
  return wrap;
}

/**
 * 「去问一次上游」。**没有自动加载，只有一颗按钮**：这条读会拿池里的一把 key 去
 * **真打一次上游**，挂在 `onShow()` 上就是「每切一次板块打一次外网」。
 * 面板这一侧的护栏是「在飞时早退 + 按钮 disabled」，后端那把 `ProbeGuard` 是另一半。
 */
function upstreamCard() {
  const btn = el("button", { type: "button", class: "btn btn-sm btn-outline" });
  btn.appendChild(svgIcon(ICON_CLOUD, 15));
  btn.appendChild(elI18n("span", "section.md.up.run"));
  btn.disabled = up.state === "loading";
  btn.addEventListener("click", () => { loadUpstream(); });

  const { card, body } = cardShell("section.md.up.title", [btn]);
  body.appendChild(elI18n("p", "section.md.up.desc", { class: "mng-inline-note" }));

  if (up.state === "loading") {
    const p = el("div", { class: "empty-state" });
    p.appendChild(el("span", { class: "loading-spinner" }));
    p.appendChild(elI18n("span", "section.md.up.loading"));
    body.appendChild(p);
    return card;
  }

  if (up.state === "idle") {
    // 一根 EM DASH + **一句看得见的话**。「还没查过」最容易被误读成「上游没有模型」，
    // 而把那句话塞进 `title` 在触屏上根本出不来。
    const p = el("div", { class: "empty-state" });
    p.appendChild(el("span", { class: "text-muted" }, "— "));
    p.appendChild(elI18n("span", "section.md.up.idle"));
    body.appendChild(p);
    return card;
  }

  if (up.state === "error") {
    const banner = el("div", { class: "banner banner-danger", role: "status" });
    const key = UP_REASON_KEY[up.reason];
    banner.appendChild(el("span", { "data-up": "msg" },
      key === undefined ? t("section.md.up.reason.other") : t(key)));
    // 状态码**另起一句**，不拼进上面那句：「上游回了几」只在真收到过响应头时有意义。
    if (up.status !== null) {
      banner.appendChild(el("span", { class: "tag", "data-up": "status" },
        t("section.md.up.status", { status: up.status })));
    }
    body.appendChild(banner);
    return card;
  }

  const v = up.models;
  if (v.truncated) {
    // ⚠️ 条数取的是「这次拿到手的那份」的长度，**不在前端写一个上限常量**：
    //    上限是后端的，抄一份就会漂。
    const warn = el("div", { class: "banner banner-warn" });
    warn.appendChild(el("span", null, t("section.md.up.truncated", { n: v.ids.length })));
    body.appendChild(warn);
  }
  body.appendChild(idChips("section.md.up.listLabel", v.ids, "ids"));
  if (v.onlyUpstream.length === 0 && v.onlyCatalog.length === 0) {
    body.appendChild(elI18n("p", "section.md.up.diffNone", { class: "mng-inline-note" }));
  } else {
    if (v.onlyUpstream.length > 0) body.appendChild(idChips("section.md.up.onlyUpstream", v.onlyUpstream, "onlyUpstream"));
    if (v.onlyCatalog.length > 0) body.appendChild(idChips("section.md.up.onlyCatalog", v.onlyCatalog, "onlyCatalog"));
  }
  return card;
}

/**
 * 向上游问一次。**只有那颗按钮会调它**，没有任何隐式入口。
 *
 * ⚠️ **两类失败落在两条路上**：200 的响应体自带 `reason`（那是上游/护栏的事实），
 * 非 2xx 由 `js/api.js` 抛 `ApiError`（那是管理接口这一段的事实）。混成一条的后果是
 * 护栏的 429 会被说成「上游出错了」—— 而那一次**一个出站请求都没发生过**。
 */
function loadUpstream() {
  if (up.state === "loading") return;
  up = { state: "loading", reason: null, status: null, models: null };
  render();
  api.get("/upstream/models")
    .then((body) => {
      const b = asObject(body);
      const models = b === null ? null : asObject(b.models);
      if (b !== null && b.ok === true && models !== null && Array.isArray(models.ids)) {
        up = {
          state: "ok",
          reason: null,
          status: null,
          models: {
            ids: models.ids.filter((x) => typeof x === "string"),
            truncated: models.truncated === true,
            onlyUpstream: Array.isArray(models.onlyUpstream) ? models.onlyUpstream.filter((x) => typeof x === "string") : [],
            onlyCatalog: Array.isArray(models.onlyCatalog) ? models.onlyCatalog.filter((x) => typeof x === "string") : [],
          },
        };
        return;
      }
      up = {
        state: "error",
        reason: b !== null && typeof b.reason === "string" ? b.reason : null,
        status: b !== null && typeof b.status === "number" ? b.status : null,
        models: null,
      };
    })
    .catch((e) => {
      // 429 那一档的 `reason` 在响应体里（`js/api.js` 把它原样挂在 `body` 上）。
      const b = e instanceof ApiError ? asObject(e.body) : null;
      up = {
        state: "error",
        reason: b !== null && typeof b.reason === "string" ? b.reason : null,
        status: null,
        models: null,
      };
    })
    .then(() => { render(); });
}

/* ============================================================
   卡 ③ 连通性测试
   ============================================================ */

/** 目录里哪些模型能进这一轮。🔴 **只有对话模型**，判据是服务端给的形态标识。 */
function testableModels() {
  if (catalog === null) return [];
  // 图片模型测一次 = **真生成一张图**；视频模型测一次 = **建一个任务 + 反复轮询**。
  // 后端对这两档一律 400 `modality_not_testable`（那才是硬边界），面板这一侧同样筛，
  // 但**两处不是一条判据的两半**：这里筛掉是为了不发那几次注定失败的请求。
  return catalog.models.filter((m) => m.modality === "chat").map((m) => m.id);
}

function testCard() {
  const ids = testableModels();
  const done = test.rows === null ? 0 : test.rows.filter((r) => r.state === "done").length;
  const total = test.rows === null ? ids.length : test.rows.length;

  const btn = el("button", { type: "button", class: "btn btn-sm btn-primary" });
  btn.appendChild(svgIcon(ICON_BOLT, 15));
  const label = el("span");
  if (!test.running) label.textContent = t(test.rows === null ? "section.md.test.run" : "section.md.test.rerun");
  else if (test.waiting) label.textContent = t("section.md.test.waiting", { done, total });
  else label.textContent = t("section.md.test.progress", { done, total });
  btn.appendChild(label);
  btn.disabled = test.running || catalog === null || ids.length === 0 || probeMinIntervalMs === null;
  btn.addEventListener("click", () => { runTests(); });

  const { card, body } = cardShell("section.md.test.title", [btn]);
  body.appendChild(elI18n("p", "section.md.test.desc", { class: "mng-inline-note" }));

  if (catalog === null) {
    const p = el("div", { class: "empty-state" });
    p.appendChild(el("span", { class: "text-muted" }, "— "));
    p.appendChild(elI18n("span", "section.md.unknown"));
    body.appendChild(p);
    return card;
  }
  if (ids.length === 0) {
    // ⚠️「目录里一个模型都没有」与「目录里没有能这么测的模型」是两句话。
    body.appendChild(elI18n("div", "section.md.test.empty", { class: "empty-state" }));
    return card;
  }

  // 🔴 读不到最小间隔 ⇒ **不让整轮跑**，并说清为什么（见文件头）。
  if (probeMinIntervalMs === null) {
    const warn = el("div", { class: "banner banner-warn" });
    warn.appendChild(elI18n("span", "section.md.test.pacingUnknown"));
    if (capFailed) {
      const again = elI18n("button", "section.md.retry", { type: "button", class: "btn btn-sm btn-outline" });
      again.addEventListener("click", () => { void loadCapabilities().then(render); });
      warn.appendChild(again);
    }
    body.appendChild(warn);
    return card;
  }

  // 整轮**至少**要多少秒，先把话说在前面：一轮几十秒，事先不说的话运维会在第一段
  // 间隔里就以为它卡住了。⚠️ 这个数由**服务端那个间隔**乘出来，不是一个前端常量。
  const etaSec = Math.ceil((probeMinIntervalMs * Math.max(0, ids.length - 1)) / 1000);
  if (etaSec > 0) {
    body.appendChild(el("p", { class: "mng-inline-note", "data-test": "eta" },
      t("section.md.test.eta", { n: ids.length, sec: etaSec })));
  }

  if (test.rows === null) {
    const p = el("div", { class: "empty-state" });
    p.appendChild(el("span", { class: "text-muted" }, "— "));
    p.appendChild(elI18n("span", "section.md.test.idle"));
    body.appendChild(p);
    return card;
  }

  // 进度条：几十秒里运维得看得见它在动。
  const meter = el("div", { class: "meter" });
  const bar = el("span");
  bar.style.width = `${total === 0 ? 0 : Math.round((done / total) * 100)}%`;
  meter.appendChild(bar);
  body.appendChild(meter);

  // ⚠️ **这张表刻意也用 `.models-table` 而不是 `.data-table`**：数值右对齐与
  //    `tabular-nums` 那两条（`.ta-right` / `.tabular`）在本仓的 CSS 里是**挂在
  //    `.models-table` 下面的后代选择器**，换成 `.data-table` 之后它们一格都不生效
  //    —— 延迟那一列会变成左对齐、非等宽数字，逐行比较时完全对不齐。
  const wrap = el("div", { class: "models-table-scroll" });
  const table = el("table", { class: "models-table" });
  const thead = el("thead");
  const hr = el("tr");
  for (const k of ["section.md.test.col.model", "section.md.test.col.result", "section.md.test.col.latency"]) {
    hr.appendChild(elI18n("th", k));
  }
  thead.appendChild(hr);
  table.appendChild(thead);
  const tbody = el("tbody");
  let throttled = 0;
  for (const row of test.rows) {
    const tr = el("tr", { "data-model": row.id, "data-state": row.state });
    tr.appendChild(el("td", { class: "kv-mono" }, row.id));

    const cell = el("td");
    if (row.state === "pending") {
      cell.appendChild(elI18n("span", "section.md.test.state.pending", { class: "badge badge-muted" }));
    } else if (row.state === "active") {
      cell.appendChild(elI18n("span", "section.md.test.state.active", { class: "badge badge-info" }));
    } else if (row.ok) {
      cell.appendChild(elI18n("span", "section.md.test.reason.ok", { class: "badge badge-success" }));
    } else {
      const key = TEST_REASON_KEY[row.reason];
      if (SELF_THROTTLED.includes(row.reason)) throttled++;
      cell.appendChild(el("span", { class: "badge badge-danger" },
        key === undefined ? t("section.md.test.reason.other") : t(key)));
    }
    if (row.status !== null) {
      cell.appendChild(el("span", { class: "tag" }, t("section.md.test.status", { status: row.status })));
    }
    tr.appendChild(cell);

    // ⚠️ **绝不伪造 0**：0 毫秒是一句关于链路的话，「没有这个数」是关于我们自己的话。
    tr.appendChild(el("td", { class: "tabular ta-right" }, row.latencyMs === null ? "—" : String(row.latencyMs)));
    tbody.appendChild(tr);
  }
  table.appendChild(tbody);
  wrap.appendChild(table);
  body.appendChild(wrap);

  // 🔴 **这句话是这张表的诚实性本身**：被自己那把护栏挡下的行与「模型不通」在屏幕上
  //    长得一模一样，不说出来的话运维会把我们自己的节流读成一张连通性矩阵。
  if (throttled > 0) {
    const note = el("div", { class: "banner banner-warn" });
    note.appendChild(el("span", null, t("section.md.test.throttledNote", { n: throttled })));
    body.appendChild(note);
  }
  return card;
}

/**
 * 跑一轮逐模型测试。**只有那颗按钮会调它。**
 *
 * 🔴 **串行 + 两条之间真的隔满后端那个最小间隔**，缺一不可（见文件头）。
 * 🔴 **每一步都 `render()` 一次**，不许等整轮跑完再一次性渲染 —— 一轮几十秒，
 *    中途不重画与一个挂死的面板不可区分。
 *
 * ⚠️ **代价如实登记：这一轮没有中途退出的条件。** 会话在半路失效时剩下那几条照样
 * 会逐个发出去、各拿一个 401、各画成一行失败；切走板块同理（这一轮不会因为切板块
 * 而停）。**今天仍不加那条早退**：它要一张「哪几种 reason 该中止整轮」的表，
 * 而那张表一旦写歪，就会把一条**该继续**的失败也当成整轮的终止条件，
 * 静默少测掉后面全部模型 —— 那比多打几次注定失败的往返坏得多。
 */
async function runTests() {
  if (test.running) return;
  if (catalog === null) return;
  if (probeMinIntervalMs === null) return;
  const ids = testableModels();
  if (ids.length === 0) return;

  test = {
    rows: ids.map((id) => ({ id, state: "pending", ok: false, reason: null, status: null, latencyMs: null })),
    running: true,
    waiting: false,
  };
  render();

  /** 上一条**落定**的时刻（拿到响应或拿到错误都算）；这一轮第一条是 `null`。 */
  let prevSettledAt = null;
  for (const id of ids) {
    // 参照点取「上一条落定」而不是「上一条发起」：后端那把闸量的是两次**发起**之间的
    // 距离，而一次慢响应已经替我们把一部分间隔等掉了 —— 取发起时刻会把整轮拖长而
    // 换不回任何东西；取落定时刻是**偏保守**的那一侧（只会等得更久，不会更短）。
    const wait = prevSettledAt === null ? 0 : probeMinIntervalMs - (Date.now() - prevSettledAt);
    if (wait > 0) {
      // ⚠️⚠️ **这几秒必须换一句话。** 那期间一条请求都没在飞、一行状态都不会变，
      //    还写着「正在逐个测：1/6」是**假的**，而运维读到的是一个停住不动的进度
      //    —— 与挂死不可区分。
      test = { rows: test.rows, running: true, waiting: true };
      render();
      await sleep(wait);
    }
    test = {
      rows: test.rows.map((r) => (r.id === id ? { ...r, state: "active" } : r)),
      running: true,
      waiting: false,
    };
    render();

    let ok = false;
    let reason = null;
    let status = null;
    let latencyMs = null;
    try {
      // ⚠️ **`encodeURIComponent`**：模型 id 经过一次网络往返才到这里，即便今天它由
      //    本仓的目录写死，也不该被原样拼进 URL。
      const resp = await api.post(`/models/${encodeURIComponent(id)}/test`);
      const r = asObject(resp);
      ok = r !== null && r.ok === true;
      reason = r !== null && typeof r.reason === "string" ? r.reason : null;
      status = r !== null && typeof r.status === "number" ? r.status : null;
      latencyMs = r !== null && typeof r.latencyMs === "number" ? r.latencyMs : null;
    } catch (e) {
      const b = e instanceof ApiError ? asObject(e.body) : null;
      reason = b !== null && typeof b.reason === "string" ? b.reason : null;
    }
    // ⚠️ **成功与失败共用同一个参照点，别只在成功那一支记**：一次传输层失败同样在
    //    后端那把护栏上占过一次。
    prevSettledAt = Date.now();
    test = {
      rows: test.rows.map((r) => (r.id === id ? { id, state: "done", ok, reason, status, latencyMs } : r)),
      running: true,
      waiting: false,
    };
    render();
  }
  test = { rows: test.rows, running: false, waiting: false };
  render();
}

/* ============================================================
   网络
   ============================================================ */

/**
 * 读一次能力面。**本板块只要 `probes.minIntervalMs` 那一格**（整轮节奏的真源）。
 * 读不到就把 `capFailed` 立起来 —— 「还没读」与「读了没读到」要给出不同的出路。
 */
async function loadCapabilities() {
  try {
    const body = await api.get("/capabilities");
    const b = asObject(body);
    const probes = b === null ? null : asObject(b.probes);
    const ms = probes === null ? null : probes.minIntervalMs;
    if (typeof ms === "number" && ms >= 0) { probeMinIntervalMs = ms; capFailed = false; return; }
    capFailed = true;
  } catch (e) {
    // 🔴 **不要在这里塞一个默认值**：一个猜出来的间隔就是第二份真源，
    //    猜小了等于把「整轮只测得到第一个」原样复活，而不会有任何东西会红。
    capFailed = true;
  }
}

/**
 * 拉一次协议目录。**零存储读**，但仍然是一次网络往返 —— 成功读过一次就不再读
 * （目录是静态的，重读一遍只换来一次「这次可能失败」的机会）。
 *
 * ⚠️ **两个不同的失败必须落到同一档**：HTTP 失败（`api.get` 抛）与「响应读得回来但
 * 形状不对」（窄化交出 `null`）在面板上都是「我们不知道」。少了后一半的话，一份被
 * 中间件改过形状的响应会让面板画出一张**结构自洽而内容缺斤少两**的表。
 */
async function load(preempt) {
  // ⚠️⚠️ **隐式入口在飞时不发第二条，但必须把当前状态画出来。** 裸 `return` 的话，
  //    一条永不落地的读会让此后每一次 `onShow()` 都什么都不画 ⇒ 只剩标题的空板块，
  //    **连自救入口都没有**。
  if (inFlight && preempt !== true) { render(); return; }
  if (abort !== null) abort.abort();
  abort = new AbortController();
  const signal = abort.signal;
  const mine = ++seq;
  inFlight = true;
  try {
    const body = await api.get("/models", { signal });
    if (mine !== seq) return;
    catalog = narrowCatalog(body);
  } catch (e) {
    // ⚠️ 一条**晚到的失败**不许把一份刚读成功的目录抹掉 —— 世代号就是挡它的那一道。
    if (mine !== seq) return;
    catalog = null;
  } finally {
    if (mine === seq) { inFlight = false; abort = null; }
  }
  render();
}

/* ============================================================
   渲染
   ============================================================ */

/**
 * 整个板块重画一遍。**每次都清空重建**：藏起来的表格仍然在无障碍树里、仍然会被
 * 复制粘贴带走，而它显示的可能是一份读不出来的数据。
 */
function render() {
  if (nodes === null) return;
  const host = nodes.body;
  host.textContent = "";
  host.appendChild(catalogCard());
  // ⚠️ **目录读不出来时这两张卡照画。** 三条读没有任何依赖关系，把它们塞进
  //    `catalog !== null` 那一支的后果是：一次目录读失败会连带把两个完全能用的
  //    功能藏起来，而屏幕上不会有任何东西说它们去哪了。
  host.appendChild(upstreamCard());
  host.appendChild(testCard());
}

export const modelsSection = {
  init(section) {
    section.textContent = "";
    const head = el("div", { class: "section-header" });
    head.appendChild(elI18n("h2", "section.md.title", { class: "section-title" }));
    const acts = el("div", { class: "section-actions" });
    const refresh = el("button", { type: "button", class: "btn btn-sm btn-outline" });
    refresh.appendChild(svgIcon(ICON_REFRESH, 15));
    refresh.appendChild(elI18n("span", "section.md.retry"));
    refresh.addEventListener("click", () => { load(true); });
    acts.appendChild(refresh);
    head.appendChild(acts);
    section.appendChild(head);
    section.appendChild(elI18n("p", "section.md.desc", { class: "mng-inline-note" }));
    const body = el("div");
    section.appendChild(body);
    nodes = { body };
  },

  onShow() {
    // 切语言时框架层会重跑 `onShow()`，那时走的也是这一支。
    if (probeMinIntervalMs === null && !capFailed) void loadCapabilities().then(render);
    if (catalog !== null) { render(); return; }
    void load(false);
  },

  onHide() {
    // 在飞的目录读作废：回来时它会 `render()`，而那时板块已经切走了。
    if (abort !== null) { abort.abort(); abort = null; }
    seq++;
    inFlight = false;
  },
};
