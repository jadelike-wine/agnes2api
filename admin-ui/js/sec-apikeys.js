/**
 * 「API 管理」板块 —— **我们签发给下游的那一族 `sk-*`** 的签发 / 改名 / 到期 /
 * 停用 / 吊销 / 明文回显。
 *
 * ⚠️⚠️ **它与「账号管理」板块方向相反，别读混。** 那一族是**我们持有的**上游凭据
 * （拿去向 Agnes 证明身份，端点 `/admin/api/keys`）；这一族是**别人拿来向我们证明
 * 身份**的（端点 `/admin/api/apikeys`）。两个名字很像是**已知代价**，缓解手段是
 * 文案与文档，不是去改一条已经发布过的公开路径。
 *
 * ── 🔴 reveal 的**四态**，本板块最要紧的一条 ──────────────────────────────
 *
 * TS 版把这四态混成过一态，代价是运维**对着一把永远取不回的密钥反复点**。四态是：
 *
 * | 档 | 载体 | 含义 | 运维该做什么 |
 * |---|---|---|---|
 * | `ok`          | 200 体 `kind:"ok"`          | 明文在这儿 | 复制走 |
 * | `unavailable` | 200 体 `kind:"unavailable"` | 这把签发于明文落盘之前，服务端**从来就没有过**它的明文 | **永久**如此，重发一把 |
 * | `gone`        | **404**                      | 这条记录已经不在表里了（别处删过） | 刷新列表 |
 * | `failed`      | 其余非 2xx / 传输失败        | 这一次没拿到 | 重试可能有用 |
 *
 * 🔴 **`unavailable` 与 `failed` 不许并档**：前者是一个**永远不会变**的事实。
 * 本板块把这条「永久」真的画了出来 —— 一旦某一把回了 `unavailable`，
 * 它那一行的「显示明文 / 复制」两颗按钮**当场换成一句话**（见 `revealCell()`），
 * 而不是继续摆在那里等人再点一次。**一句 toast 不够**：toast 3 秒就没了，
 * 而那颗按钮会一直在。
 * ⚠️ `gone` 与 `failed` 也不许并档：前者的处置是「刷新一下列表」，本板块据此**自动
 * 重拉一次列表**（那一行本来就该消失）。
 *
 * ── 三条纪律 ──────────────────────────────────────────────────────────────
 * ① 一切来自接口的内容一律 `textContent`（`el()` 走的就是它）。**本板块尤其要紧**：
 *    密钥名称是运维自由输入、又会被投影到屏幕上的字段；
 * ② **判定一律读响应里那一格 `view`**（06 §5.3 的 A 档，判定在
 *    `src/core/admin/view/apikeys.rs`）：徽章类、档位文案、开关文案、
 *    「清理失效」的 N 与它出不出现、空表那句话，一格都不在这里重算；
 * ③ **一切形态分支只读接口返回的字段**（`/capabilities` 的 `apiKeys` 那一块），
 *    不许把上限 / TTL / 「明文能不能再取回」写死在前端。
 *
 * ── 服务连接卡上**刻意没有主口令的掩码**（与 kiro 的一处偏离，理由写清）────────
 * kiro 那张卡上有一行 `masterKey`。本板块**不画它**，两条理由：
 * ① 那份数据只有 `GET /admin/api/config` 给得出来 ⇒ 每打开一次这个板块就多**一次
 *    存储读**，而部署文档的配额账里这个板块的单价写的是「列一次表 = 1 次 get」；
 * ② 同一把凭据在面板上有两个展示入口，迟早会分叉。
 * ⇒ 这里只写清「主口令在设置页」，**不画一个假的掩码、也不画一串圆点占位**。
 */
import { api, ApiError } from "./api.js";
import { t } from "./i18n.js";
import {
  el, elI18n, iconBtn, svgIcon, toast, copy, openModal, confirmModal, revealControls,
} from "./ui.js";
// 🔴 **徽章类名要过这一道翻译**：后端交的 `badge badge-ok` / `badge badge-warn`
//    在 `css/base.css` 里**根本不存在**（那边是 `-success` / `-warning`），原样上屏
//    的话「可用 / 已过期 / 已停用」三档完全同色。理由全文见那个模块的文件头。
import { badgeClass } from "./pure/badge-class.mjs";

/* ============================================================
   图标
   ============================================================ */

const ICON_LINK = "M 10 13 a 5 5 0 0 0 7.5 .5 l 3 -3 a 5 5 0 0 0 -7 -7 l -1.7 1.7 M 14 11 a 5 5 0 0 0 -7.5 -.5 l -3 3 a 5 5 0 0 0 7 7 l 1.7 -1.7";
const ICON_COPY = "M 9 9 h 10 v 10 h -10 z M 5 15 H 4 a 1 1 0 0 1 -1 -1 V 4 a 1 1 0 0 1 1 -1 h 10 a 1 1 0 0 1 1 1 v 1";
const ICON_PENCIL = "M 17 3 a 2.8 2.8 0 1 1 4 4 L 7.5 20.5 L 2 22 l 1.5 -5.5 z";
const ICON_TRASH = "M 3 6 h 18 M 8 6 V 4 h 8 v 2 M 19 6 l -1 14 H 6 L 5 6 M 10 11 v 6 M 14 11 v 6";
const ICON_PLUS = "M 12 5 v 14 M 5 12 h 14";
const ICON_SORT = "M 3 6 h 12 M 3 12 h 8 M 3 18 h 5 M 17 8 v 11 M 17 19 l -3 -3 M 17 19 l 3 -3";
const ICON_SEARCH = "M 11 4 a 7 7 0 1 0 0 14 a 7 7 0 1 0 0 -14 z M 20 20 l -4 -4";

/* ============================================================
   服务端视图 key → 本板块字典 key
   ============================================================ */

/**
 * 🔴 **服务端交出来的判定 key 的别名表**（与 `js/sec-models.js` 同一条理由）。
 *
 * `src/core/admin/view/apikeys.rs` 交出的是 `ak.*` 那一族名字，而词条命名空间登记表
 * （`src/webui/mod.rs` 的 `NAMESPACES`）今天不认这个前缀，那张表住在 `.rs` 里、
 * 本轮不归本文件这一面改。
 * ⇒ 这里只做一次**纯改名**：**判定完全在服务端**，本表把那个标识符翻成本仓字典 key。
 * ⚠️ **认不出来的一律照实画原始 key**（下面每一处的 `undefined` 分支）。
 */
const VIEW_KEY = {
  "ak.bucket.active": "section.ak.bucket.active",
  "ak.bucket.disabled": "section.ak.bucket.disabled",
  "ak.bucket.expired": "section.ak.bucket.expired",
  "ak.action.enable": "section.ak.action.enable",
  "ak.action.disable": "section.ak.action.disable",
  "ak.empty": "section.ak.empty",
};

/** `ui.js` 的 `revealControls` 四态 → 本板块文案。**四句话各说各的，不许并成一句。** */
const REVEAL_MESSAGE = {
  gone: "section.ak.reveal.gone",
  unavailable: "section.ak.reveal.unavailable",
  expired: "section.ak.reveal.expired",
  failed: "section.ak.reveal.failed",
};

/** 管理接口错误码 → 文案。认不出来的落到那句通用的，**不把英文码画给运维**。 */
const ERROR_KEY = {
  apikey_not_found: "section.ak.err.notFound",
  apikeys_unreadable: "section.ak.err.unreadable",
  too_many_apikeys: "section.ak.err.tooMany",
  stale_write: "section.ak.err.stale",
  invalid_apikey_name: "section.ak.err.badName",
  invalid_expires_at: "section.ak.err.badExpiry",
};

const SORTS = ["new", "old", "name"];
const SORT_KEY = { new: "section.ak.sort.new", old: "section.ak.sort.old", name: "section.ak.sort.name" };
/** 到期快捷档（天）。`0` = 不过期。**文案写的是「自签发时刻起 N 天」。** */
const EXPIRY_DAYS = [0, 7, 30, 90];
const EXPIRY_KEY = {
  0: "section.ak.expiry.never",
  7: "section.ak.expiry.d7",
  30: "section.ak.expiry.d30",
  90: "section.ak.expiry.d90",
};

/* ============================================================
   状态
   ============================================================ */

let nodes = null;
const ui = { q: "", sort: "new" };

/** 最近一次列表响应。`null` = 还没读到 / 这一次读失败了（后者另有 `loadError`）。 */
let data = null;
/** 这一次读失败了没有。与 `data === null` **不是**一回事（后者还包含「没读过」）。 */
let loadError = false;
/** 在飞那条列表读的取消闸。离开板块 / 发起下一次时作废上一次。 */
let abort = null;

/**
 * `capabilities` 里那一块。**只拉一次**：几格都是建 app 时定死的部署期常量。
 *
 * 🔴 **`null` 一律读作「还不知道」，不读作「没有」。** `wired` 一开始就是这样写的，
 * 而 `plaintextRetrievable` 原来的初值是 `false` —— 于是 `loadCapabilities()` 的
 * catch 块（它**刻意什么都不做**，注释写着「保持 `wired: null`，不假装它没接」）
 * 对这一格根本不成立：一次读失败会让它**停在 `false`**，
 * 而 `revealCell()` 据此把「显示明文 / 复制」两颗按钮**静默撤掉**。
 * ⇒ 一次网络抖动长得和「这个部署永远取不回明文」一模一样。
 * **一次读取失败不该长得像一条永久结论** —— 与本板块 reveal 四态那条纪律同源。
 */
let cap = { wired: null, max: null, nameMax: null, plaintextRetrievable: null, cacheTtlMs: null };

/** 用量（近 24 小时）那一次的响应与它的世代号。与列表那一条分开作废。 */
let usageData = null;
let usageFailed = false;
let usageSeq = 0;

/**
 * 🔴 **已知「永久取不回明文」的那几把 id。**
 *
 * 它就是四态里 `unavailable` 那一档**在 UI 上真的分得开**的落点：记住之后，
 * 那一行不再画「显示明文 / 复制」，改画一句说明。
 * ⚠️ **只进不出、也不持久化**：它是一条关于「这把密钥签发于什么时候」的事实，
 * 在这一次会话里不会变；刷新页面重新问一次服务端也完全正确。
 */
const permanentlyUnavailable = new Set();

/**
 * 明文只活在这里。🔴 **绝不写回列表数据**：列表随每次写操作整份重建
 * ⇒ 切板块、刷新之后自动回到掩码态。塞进列表数据会让它跟着进下一次渲染、
 * 进任何对列表做的序列化。
 */
const revealState = (() => {
  const secrets = new Map();
  const shown = new Set();
  return {
    secretOf: (id) => (secrets.has(id) ? secrets.get(id) : null),
    remember: (id, s) => { secrets.set(id, s); },
    isShown: (id) => shown.has(id),
    show: (id) => { shown.add(id); },
    hide: (id) => { shown.delete(id); },
    forget: () => { secrets.clear(); shown.clear(); },
  };
})();

/* ============================================================
   小工具（**本地的格式化，不造第二份判定**）
   ============================================================ */

function asObject(v) {
  return v !== null && typeof v === "object" && !Array.isArray(v) ? v : null;
}

/** 一根 EM DASH = **「我们没有这个数」**，与真实的 0 是两句话。 */
const DASH = "—";

function two(n) { return n < 10 ? `0${n}` : String(n); }

/**
 * 一个时刻画成「本机时区的 YYYY-MM-DD HH:MM」。
 * ⚠️ **不是 UTC**：这一栏是给人读的，而运维读的是自己机器上的钟。
 * 读不出来一律 EM DASH，**绝不画一个 1970**。
 */
function fmtInstant(ms) {
  if (typeof ms !== "number" || !Number.isFinite(ms)) return DASH;
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return DASH;
  return `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}`;
}

/** 一段毫秒画成「N 秒 / N 分钟」。**这个数从后端来（`cacheTtlMs`），不写死。** */
function fmtDuration(ms) {
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) return DASH;
  const sec = Math.ceil(ms / 1000);
  if (sec < 60) return t("section.ak.dur.seconds", { n: sec });
  return t("section.ak.dur.minutes", { n: Math.ceil(sec / 60) });
}

/** 一次管理接口错误翻成一句话。**不把英文码画给运维。** */
function errorMessage(e) {
  const body = e instanceof ApiError ? asObject(e.body) : null;
  const err = body === null ? null : asObject(body.error);
  const code = err !== null && typeof err.code === "string" ? err.code : null;
  const key = code === null ? undefined : ERROR_KEY[code];
  if (key !== undefined) return t(key);
  return t("section.ak.writeFailed");
}

/* ============================================================
   窄化 + 本地排序/搜索
   ============================================================ */

/** 列表响应里那一格已经判好的视图，按 id 建索引。**不靠下标对齐。** */
function viewRows() {
  const d = asObject(data);
  const v = d === null ? null : asObject(d.view);
  const out = new Map();
  if (v === null || !Array.isArray(v.rows)) return out;
  for (const item of v.rows) {
    const r = asObject(item);
    if (r !== null && typeof r.id === "string") out.set(r.id, r);
  }
  return out;
}

function items() {
  const d = asObject(data);
  if (d === null || !Array.isArray(d.keys)) return [];
  return d.keys.filter((k) => asObject(k) !== null && typeof k.id === "string");
}

function version() {
  const d = asObject(data);
  return d !== null && typeof d.version === "number" ? d.version : null;
}

/**
 * 列表这一刻该说哪句话。**四档互斥。**
 * 🔴 `unreadable`（表坏了、全部下游客户端正在 401）与「一把都没有」**绝不合并**：
 * 后者会让运维往完全相反的方向查。这一档由服务端的 `view.state` 独占。
 */
function listState() {
  if (loadError) return "error";
  const d = asObject(data);
  if (d === null) return "error";
  const v = asObject(d.view);
  if (v !== null && v.state === "unreadable") return "unreadable";
  return "ok";
}

function visibleItems() {
  const q = ui.q.trim().toLowerCase();
  let rows = items();
  if (q !== "") {
    rows = rows.filter((k) => {
      const name = String(k.name ?? "").toLowerCase();
      const hint = String(k.hint ?? "").toLowerCase();
      return name.includes(q) || hint.includes(q) || String(k.seq ?? "").includes(q);
    });
  }
  const sorted = rows.slice();
  if (ui.sort === "name") sorted.sort((a, b) => String(a.name ?? "").localeCompare(String(b.name ?? "")));
  else if (ui.sort === "old") sorted.sort((a, b) => Number(a.createdAt ?? 0) - Number(b.createdAt ?? 0));
  else sorted.sort((a, b) => Number(b.createdAt ?? 0) - Number(a.createdAt ?? 0));
  return sorted;
}

/**
 * 这把密钥的用量那一行该说什么。
 * 🔴 **判定整块读服务端的 `view.apiKeyUsageKind`**（`src/core/admin/view/usage.rs`），
 * **不在这里从 `tier` / `shards` / `total` 反推** —— 反推就是把那套五态判定复制了第二份，
 * 而分叉的那一边正好会在「统计关着」时画出一个假的 0。
 */
function usageOf(id) {
  if (usageFailed) return { kind: "unknown", requests: 0 };
  const u = asObject(usageData);
  const v = u === null ? null : asObject(u.view);
  const kind = v !== null && typeof v.apiKeyUsageKind === "string" ? v.apiKeyUsageKind : "unknown";
  if (kind !== "value") return { kind, requests: 0 };
  const by = asObject(u.byApiKey);
  const bucket = by === null ? null : asObject(by[id]);
  // 缺席**就是真的 0 次**（这一档服务端已经判过「读成功了」）。
  return { kind: "value", requests: bucket !== null && typeof bucket.requests === "number" ? bucket.requests : 0 };
}

/* ============================================================
   卡片
   ============================================================ */

/** 服务连接卡。见文件头那段偏离说明。 */
function serviceCard() {
  const card = el("div", { class: "card svc-card" });
  const head = el("div", { class: "svc-card-head" });
  head.appendChild(svgIcon(ICON_LINK, 16));
  head.appendChild(elI18n("span", "section.ak.svc.title"));
  card.appendChild(head);

  const row = el("div", { class: "svc-row" });
  const left = el("div", { class: "svc-row-left" });
  left.appendChild(elI18n("div", "section.ak.svc.baseUrl", { class: "svc-row-label" }));
  // `location.origin` 是**运行期**的源：写死一条 URL 就是在面板上承诺一个可能不对的地址。
  left.appendChild(el("code", { class: "svc-row-val" }, nodes.origin));
  row.appendChild(left);
  row.appendChild(iconBtn(ICON_COPY, "common.copy", () => { void copy(nodes.origin); }));
  card.appendChild(row);

  card.appendChild(elI18n("p", "section.ak.svc.master", { class: "mng-inline-note" }));
  return card;
}

/**
 * 五张状态卡。
 *
 * ⚠️ **kiro 的第 3 张是「待激活」，本仓没有那个概念**（惰性激活本期不做），
 * **画一张恒为 0 的卡就是撒谎** ⇒ 那一格换成「上限」（`max`，来自响应）。
 * 🔴 **不是把格子空着**：`.key-stat-grid` 是固定五列，留一个洞比换一张真卡难看得多，
 * 而「这个部署最多能签几把」恰好是运维在这张页面上真的会问的一个数。
 */
function statGrid() {
  const grid = el("div", { class: "key-stat-grid" });
  const d = asObject(data);
  const counts = d === null ? null : asObject(d.counts);
  const num = (k) => (counts !== null && typeof counts[k] === "number" ? String(counts[k]) : DASH);
  const max = d !== null && typeof d.max === "number" ? String(d.max) : (cap.max === null ? DASH : String(cap.max));
  const cards = [
    ["section.ak.stat.total", num("all"), ""],
    ["section.ak.stat.active", num("active"), " ok"],
    ["section.ak.stat.disabled", num("disabled"), " bad"],
    ["section.ak.stat.expired", num("expired"), " warn"],
    ["section.ak.stat.max", max, " muted"],
  ];
  for (const [key, value, kind] of cards) {
    const c = el("div", { class: `card key-stat${kind}` });
    c.appendChild(elI18n("div", key, { class: "key-stat-label" }));
    c.appendChild(el("div", { class: "key-stat-num" }, value));
    grid.appendChild(c);
  }
  return grid;
}

function toolbar() {
  const head = el("div", { class: "section-header" });
  head.appendChild(elI18n("h2", "section.ak.listTitle", { class: "section-title" }));
  const acts = el("div", { class: "section-actions" });

  const sortWrap = el("div", { class: "key-sort" });
  sortWrap.appendChild(svgIcon(ICON_SORT, 16));
  for (const s of SORTS) {
    const on = ui.sort === s;
    // `.btn-primary` 是实心主色 + 白字（`--primary-strong`），`aria-pressed` 是读屏
    // 那条腿 —— 选中态不许只靠颜色。
    const b = elI18n("button", SORT_KEY[s], {
      type: "button",
      class: `btn btn-sm ${on ? "btn-primary" : "btn-outline"}`,
      "aria-pressed": on ? "true" : "false",
    });
    b.addEventListener("click", () => { ui.sort = s; render(); });
    sortWrap.appendChild(b);
  }
  acts.appendChild(sortWrap);

  const issue = el("button", { type: "button", class: "btn btn-sm btn-primary" });
  issue.appendChild(svgIcon(ICON_PLUS, 15));
  issue.appendChild(elI18n("span", "section.ak.issue"));
  // 版本号还不知道时写操作整个禁掉：不带版本号的写会被后端拒，
  // 而那时面板给出的错误对运维毫无意义。
  issue.disabled = listState() !== "ok" || cap.wired === false || version() === null;
  issue.addEventListener("click", () => openIssueDialog());
  acts.appendChild(issue);

  // 「清理失效（N）」：🔴 **N 与「这颗按钮出不出现」都读服务端那一格**
  //（`view.purgeCount` / `view.purgeVisible`），不在这里数一遍。
  //  N = 0 时整颗按钮不画 —— 一颗点了什么都不会发生的按钮比没有更糟。
  const d = asObject(data);
  const v = d === null ? null : asObject(d.view);
  const purgeVisible = v !== null && v.purgeVisible === true && listState() === "ok" && version() !== null;
  if (purgeVisible) {
    const n = typeof v.purgeCount === "number" ? v.purgeCount : 0;
    const purge = el("button", { type: "button", class: "btn btn-sm btn-outline" });
    purge.appendChild(svgIcon(ICON_TRASH, 15));
    purge.appendChild(el("span", null, t("section.ak.purge", { n })));
    purge.addEventListener("click", () => openPurgeDialog(n));
    acts.appendChild(purge);
  }

  const reload = elI18n("button", "section.ak.refresh", { type: "button", class: "btn btn-sm btn-outline" });
  reload.addEventListener("click", () => { void load(); });
  acts.appendChild(reload);

  head.appendChild(acts);
  return head;
}

function searchBox() {
  const wrap = el("div", { class: "key-search-wrap" });
  wrap.appendChild(svgIcon(ICON_SEARCH, 16));
  const input = el("input", {
    type: "text",
    class: "form-control key-search",
    "data-i18n-ph": "section.ak.searchPh",
  });
  input.setAttribute("placeholder", t("section.ak.searchPh"));
  input.value = ui.q;
  input.addEventListener("input", () => { ui.q = input.value; render(); });
  wrap.appendChild(input);
  return wrap;
}

/**
 * 「掩码 + 显示明文 + 复制」那一组。
 *
 * 🔴 **这里是四态真的分得开的那个落点**（见文件头）：已知 `unavailable` 的那一把
 * 不再画那两颗按钮，改画掩码 + 一句**永久**说明。
 *
 * 🔴🔴 **「取不回」与「还不知道能不能取回」是两件事，这个函数分三支。**
 * 上一版的判据是 `if (!cap.plaintextRetrievable || …)` —— 一个把
 * `false`（部署真的不留明文）与 `null`（`/capabilities` 这一次没读出来）**并成一档**
 * 的写法，而后者压根不是一条结论。两处后果各说一次：
 *   · `null` 被当成 `false` ⇒ 一次读失败让两颗按钮**静默消失**，
 *     运维看到的是一行没有任何操作、也没有任何解释的掩码；
 *   · `false` 那一支**一个字都不说**（那句说明原来只挂在 `permanentlyUnavailable`
 *     那个条件上）⇒ 部署真的不留明文时，按钮同样是无声地没了。
 * ⇒ 现在：`true` 画按钮；`false` 画按钮位的那句**永久**说明；
 *   `null` **照旧画按钮** —— 不知道的时候不许替服务端下结论，让人点一下，
 *   端点自己会用那四态里的一档把真相说清（每一档都有各自的 sticky toast）。
 */
function revealCell(k) {
  const masked = String(k.masked ?? DASH);
  // ⚠️ `=== false` 是刻意的：`null`（还不知道）绝不能落进这一支。
  const noPlaintext = cap.plaintextRetrievable === false || permanentlyUnavailable.has(k.id);
  if (noPlaintext) {
    const wrap = el("span", { class: "reveal" });
    wrap.appendChild(el("code", { class: "reveal-text" }, masked));
    // ⚠️ **一句 toast 不够**：toast 3 秒就没了，而那两颗消失的按钮会一直不在。
    //    两种「永久取不回」（这个部署从不留明文 / 这一把签发于明文落盘之前）
    //    对运维的下一步是同一条：别再点了，重发一把。共用这一句。
    wrap.appendChild(elI18n("span", "section.ak.reveal.permanent", { class: "cell-sub" }));
    return wrap;
  }
  return revealControls({
    id: k.id,
    masked,
    state: revealState,
    fetchSecret: revealSecret,
    messageKeyOf: (kind) => REVEAL_MESSAGE[kind] ?? REVEAL_MESSAGE.failed,
  });
}

/**
 * 取一次明文，**把服务端那四态翻成 `js/ui.js` 那套状态机认得的形状**。
 *
 * ⚠️ `ui.js` 的 `revealErrorKind()` 是按 **HTTP 状态码**分档的（404 ⇒ `gone`、
 * 403/501 ⇒ `unavailable`、401 ⇒ `expired`、其余 ⇒ `failed`），而本族端点把
 * 「永久取不回」放在 **200 的响应体**里（`kind:"unavailable"`）。
 * ⇒ 这里合成一个 403 把它送进 `unavailable` 那一档。
 * 🔴 **这不是「拿状态码撒谎」，是一次翻译**：判定仍然完全来自服务端那一格 `kind`，
 * 本函数一个字节都不自己判。**别把它改成「非 ok 一律抛」** —— 那正好把
 * `unavailable` 与 `failed` 并成一档，也就是这一整段要防的那个缺陷。
 */
async function revealSecret(id) {
  let body = null;
  try {
    body = await api.get(`/apikeys/${encodeURIComponent(id)}/reveal`);
  } catch (e) {
    // 404 ⇒ 这一行在别处被删了。**顺手重拉一次列表**：那一行本来就该消失，
    // 而「刷新一下」正是这一档唯一正确的处置。
    if (e instanceof ApiError && e.status === 404) setTimeout(() => { void load(); }, 0);
    throw e;
  }
  const b = asObject(body);
  const kind = b !== null && typeof b.kind === "string" ? b.kind : null;
  if (kind === "ok" && typeof b.secret === "string" && b.secret !== "") return b.secret;
  if (kind === "unavailable") {
    permanentlyUnavailable.add(id);
    // 记住之后重画一次 —— 那一行的两颗按钮当场换成一句话。
    setTimeout(render, 0);
    throw new ApiError(403, "apikey_plaintext_unavailable", b);
  }
  // 读回来了但形状不认：那是**这一次**的事，重试可能有用 ⇒ `failed` 那一档。
  throw new ApiError(500, "apikey_reveal_unrecognised", b);
}

function keyCard(k, row) {
  const bucketKey = row === undefined ? undefined : VIEW_KEY[row.bucketLabelKey];
  const card = el("div", { class: "key-card" });
  // `.is-dim` 是「这把现在用不了」的那条**非颜色**线索（整卡降透明度）。
  if (row !== undefined && (row.bucketLabelKey === "ak.bucket.disabled" || row.bucketLabelKey === "ak.bucket.expired")) {
    card.classList.add("is-dim");
  }

  const main = el("div", { class: "key-card-main" });
  const info = el("div", { class: "key-card-info" });

  const line1 = el("div", { class: "key-line1" });
  // ⚠️ `seq` **随删除而变，不是身份**：要指认一条记录只能用 `id`。它在这里只是行号。
  line1.appendChild(el("code", { class: "key-serial" }, `#${String(k.seq ?? DASH)}`));
  line1.appendChild(el("span", { class: "key-name" }, String(k.name ?? "")));
  // 🔴 徽章的类与文案**都**来自服务端那一格，本文件不认识「停用优先于过期」那条规则。
  // ⚠️ 类名过一道 `badgeClass()`（见 import 处）：**判定仍然全在服务端**，
  //    这一道只把 DTO 的类名翻成这份 CSS 里真的存在的那个。
  line1.appendChild(el("span", { class: badgeClass(row === undefined ? null : row.badgeClass) },
    bucketKey === undefined ? String(row === undefined ? DASH : row.bucketLabelKey) : t(bucketKey)));
  info.appendChild(line1);

  const line2 = el("div", { class: "key-line2" });
  line2.appendChild(revealCell(k));
  const created = el("span", null);
  created.appendChild(elI18n("span", "section.ak.meta.created", { class: "meta-k" }));
  created.appendChild(el("span", { class: "key-mono" }, ` ${fmtInstant(k.createdAt)}`));
  line2.appendChild(created);
  const exp = el("span", { class: "key-limit" });
  if (k.expiresAt === null || k.expiresAt === undefined) {
    exp.appendChild(elI18n("span", "section.ak.meta.expiresNever"));
  } else {
    exp.appendChild(elI18n("span", "section.ak.meta.expiresAt", { class: "meta-k" }));
    exp.appendChild(el("span", { class: "key-mono" }, ` ${fmtInstant(k.expiresAt)}`));
  }
  line2.appendChild(exp);
  info.appendChild(line2);

  // ── 用量那一行 ──────────────────────────────────────────────────────────
  // ⚠️⚠️ **四档各有各的一句话，其中三档里一个数字都没有**：
  //    `off` = 统计根本没开、`unknown` = 这一块读不出来、`no-shards` = 开着但这段区间
  //    一条分片都还没落盘。三档下画 `0` 都是把别的事说成「没人用」。
  const line3 = el("div", { class: "key-line3" });
  const u = usageOf(k.id);
  const usage = el("span", { class: "key-usg-item" });
  if (u.kind === "value") {
    usage.appendChild(el("span", null, t("section.ak.usage.value", { n: u.requests })));
    usage.setAttribute("title", t("section.ak.usage.tip"));
  } else if (u.kind === "off") {
    usage.appendChild(elI18n("span", "section.ak.usage.off", { class: "key-usg-io" }));
  } else if (u.kind === "no-shards") {
    usage.appendChild(elI18n("span", "section.ak.usage.notLanded", { class: "key-usg-io" }));
    usage.setAttribute("title", t("section.ak.usage.notLandedTip"));
  } else {
    usage.appendChild(el("span", { class: "key-usg-io" }, `${DASH} ${t("section.ak.usage.unknown")}`));
  }
  line3.appendChild(usage);
  info.appendChild(line3);

  main.appendChild(info);

  // ── 行内动作簇 ──────────────────────────────────────────────────────────
  const act = el("div", { class: "key-card-actions" });
  act.appendChild(iconBtn(ICON_COPY, "section.ak.action.copyInfo", () => {
    // **只复制「地址 + 掩码」，绝不复制明文**：一次「复制信息」是给人贴进工单的，
    // 而明文进工单是不可逆的泄漏。要明文请走上面那颗专门的按钮（它会留痕）。
    void copy(t("section.ak.copyBlock", { url: nodes.origin, key: String(k.masked ?? "") }));
  }));

  const disabled = k.disabled === true;
  const toggleKey = row === undefined ? undefined : VIEW_KEY[row.toggleLabelKey];
  const sw = el("button", {
    type: "button",
    class: disabled ? "acct-switch" : "acct-switch is-on",
    role: "switch",
    "aria-checked": disabled ? "false" : "true",
  });
  const swLabel = toggleKey === undefined ? "" : t(toggleKey);
  sw.setAttribute("title", swLabel);
  sw.setAttribute("aria-label", swLabel);
  sw.appendChild(el("span", { class: "acct-switch-thumb" }));
  sw.addEventListener("click", () => toggleDisabled(k, !disabled));
  act.appendChild(sw);

  act.appendChild(iconBtn(ICON_PENCIL, "section.ak.action.edit", () => openEditDialog(k)));
  act.appendChild(iconBtn(ICON_TRASH, "section.ak.action.delete", () => confirmDelete(k), "is-danger"));
  main.appendChild(act);

  card.appendChild(main);
  return card;
}

/* ============================================================
   渲染
   ============================================================ */

function render() {
  if (nodes === null) return;
  const host = nodes.body;
  host.textContent = "";
  host.appendChild(serviceCard());
  host.appendChild(statGrid());
  host.appendChild(toolbar());

  if (cap.wired === false) {
    // 「这个部署没接上存储」与「一把都没签发过」在面板上长得一模一样，而后者是假话。
    host.appendChild(elI18n("div", "section.ak.notWired", { class: "empty-state" }));
    return;
  }

  const st = listState();
  if (st === "error") {
    const banner = el("div", { class: "banner banner-danger", role: "status" });
    banner.appendChild(elI18n("span", "section.ak.loadFailed"));
    const retry = elI18n("button", "section.ak.refresh", { type: "button", class: "btn btn-sm btn-outline" });
    retry.addEventListener("click", () => { void load(); });
    banner.appendChild(retry);
    host.appendChild(banner);
    return;
  }
  if (st === "unreadable") {
    // ⚠️ **这一支绝不能与「一把都没有」合并**：表坏掉时全部下游客户端正在 401，
    //    而一句「你还没签发过密钥」会让运维往完全相反的方向查。
    const banner = el("div", { class: "banner banner-danger", role: "status" });
    banner.appendChild(elI18n("span", "section.ak.unreadable"));
    host.appendChild(banner);
    host.appendChild(elI18n("p", "section.ak.unreadableHelp", { class: "mng-inline-note" }));
    return;
  }

  host.appendChild(searchBox());

  // ── 用量这一次没读到：**本板块自己给出重试入口** ────────────────────────
  // 没有这一条时 `usageFailed` 是个**进得去出不来**的状态：每张卡画一根破折号，
  // 而板块里没有任何一颗按钮会再拉一次用量 ⇒ 运维唯一的出路是切走再切回来。
  // ⚠️ 它是**黄条**不是红条：列表、签发、停用、删除全都照常可用，坏掉的只是卡片上
  //    那一行附加信息。用红条会把「少一行数字」说成「这个板块出事了」。
  if (usageFailed) {
    const warn = el("div", { class: "banner banner-warn" });
    warn.appendChild(elI18n("span", "section.ak.usageFailed"));
    const again = elI18n("button", "section.ak.refresh", { type: "button", class: "btn btn-sm btn-outline" });
    again.addEventListener("click", () => { void loadUsage(); });
    warn.appendChild(again);
    host.appendChild(warn);
  }

  const all = items();
  const visible = visibleItems();
  const list = el("div", { class: "key-list" });
  if (all.length === 0) {
    // 🔴 整张表空时那句话来自服务端（`view.emptyKey`）。
    const d = asObject(data);
    const v = d === null ? null : asObject(d.view);
    const key = v !== null && typeof v.emptyKey === "string" ? VIEW_KEY[v.emptyKey] : undefined;
    list.appendChild(elI18n("div", key === undefined ? "section.ak.empty" : key, { class: "empty-state" }));
  } else if (visible.length === 0) {
    // ⚠️「一把都没有」与「搜索没命中」是两句话。后者服务端答不了（搜索是客户端做的）。
    list.appendChild(elI18n("div", "section.ak.emptySearch", { class: "empty-state" }));
  } else {
    const rows = viewRows();
    const group = el("div", { class: "key-cards" });
    for (const k of visible) group.appendChild(keyCard(k, rows.get(k.id)));
    list.appendChild(group);
  }
  host.appendChild(list);
}

/* ============================================================
   网络
   ============================================================ */

async function loadCapabilities() {
  try {
    const body = await api.get("/capabilities");
    const b = asObject(body);
    const a = b === null ? null : asObject(b.apiKeys);
    if (a === null) return;
    cap = {
      wired: a.wired === true,
      max: typeof a.max === "number" ? a.max : null,
      nameMax: typeof a.nameMax === "number" ? a.nameMax : null,
      // 🔴 **契约不是猜测**：这一格由后端算，面板据它决定那两颗按钮出不出现。
      //    在前端写死就会在哪天有人改了后端存法时变成假话。
      plaintextRetrievable: a.plaintextRetrievable === true,
      cacheTtlMs: typeof a.cacheTtlMs === "number" ? a.cacheTtlMs : null,
    };
  } catch (e) {
    // 🔴 读不出来时**整块保持初值的 `null`**（「还不知道」），一格都不假装。
    //    `wired: null` ⇒ 不假装它没接；`plaintextRetrievable: null` ⇒ 不假装
    //    明文取不回（那会让两颗按钮静默消失，见 `revealCell()` 上面那段）。
    //    ⚠️ 这个 catch **就该是空的** —— 它的全部内容是「什么都别写回去」。
  }
}

async function load() {
  if (abort !== null) abort.abort();
  abort = new AbortController();
  const ctl = abort;
  try {
    const body = await api.get("/apikeys", { signal: ctl.signal });
    if (ctl !== abort) return;
    data = body;
    loadError = false;
  } catch (e) {
    if (ctl !== abort) return;
    // **不把已经画好的列表抹掉**：一次读失败对「表里有什么」什么新东西都没说。
    loadError = true;
  }
  render();
}

/**
 * 拉一次近 24 小时的用量汇总。**每张卡那一行的唯一数据源。**
 *
 * ⚠️ **它不轮询、不跟着搜索框重拉**：进板块拉一次，此外只有黄条上那颗按钮。
 * ⚠️ **写操作收尾刻意不重拉它**：签发 / 改名 / 停用 / 删除**都不会改变「已经发生过的
 * 请求数」**，跟着重拉一遍只会给每一次写操作平白加几次存储读，换不回任何会变的数字。
 * **失败不抹掉已经画好的数字**：一次读失败对「这些密钥被用了多少次」什么都没说。
 */
async function loadUsage() {
  const mine = ++usageSeq;
  const to = Date.now();
  const from = to - 24 * 3600 * 1000;
  try {
    const body = await api.get(`/usage?from=${from}&to=${to}`);
    if (mine !== usageSeq) return;
    usageData = body;
    usageFailed = false;
  } catch (e) {
    if (mine !== usageSeq) return;
    usageFailed = true;
  }
  render();
}

/** 一次写操作的收尾：成功就重新拉一次（**版本号必须刷新**），失败就把码翻成一句话。 */
async function afterWrite(promise, okKey) {
  try {
    const out = await promise;
    toast(t(okKey), "ok");
    // 明文缓存整份作废：改名/停用之后那几把的 id 没变，但列表节点会整份重建，
    // 而留着一份「上一屏显示过的明文」只会让它多活一次渲染。
    revealState.forget();
    await load();
    return out;
  } catch (e) {
    toast(errorMessage(e), "err", { sticky: true });
    // 409（stale_write / 表坏掉）两档都要求运维重新看一眼当前真值。
    if (e instanceof ApiError && e.status === 409) await load();
    return null;
  }
}

/** 「停用之后最多还能再用多久」。🔴 **这个数从后端来（`cacheTtlMs`），不写死。** */
function revokeDelayText() {
  return t("section.ak.revokeDelay", { delay: fmtDuration(cap.cacheTtlMs) });
}

/* ============================================================
   对话框
   ============================================================ */

/** 名称 + 到期那一组表单控件。签发与编辑共用一份 —— 两处各写一份迟早会分叉。 */
function keyForm(initialName, initialExpiresAt) {
  const form = el("div", { class: "key-form" });

  const nameGroup = el("div", { class: "form-group" });
  const nameLabel = el("label", { class: "form-label" });
  nameLabel.appendChild(el("span", { class: "req-star" }, "*"));
  nameLabel.appendChild(elI18n("span", "section.ak.form.name"));
  nameGroup.appendChild(nameLabel);
  const name = el("input", { type: "text", class: "form-control", "data-i18n-ph": "section.ak.form.namePh" });
  name.setAttribute("placeholder", t("section.ak.form.namePh"));
  if (cap.nameMax !== null) name.setAttribute("maxlength", String(cap.nameMax));
  name.value = initialName;
  nameGroup.appendChild(name);
  form.appendChild(nameGroup);

  const expGroup = el("div", { class: "form-group" });
  expGroup.appendChild(elI18n("label", "section.ak.form.expiry", { class: "form-label" }));
  const chips = el("div", { class: "chip-row" });
  // `null` = 自定义日期那一档。初始值：已有到期时刻 ⇒ 自定义；否则不过期。
  let chosen = initialExpiresAt === null ? 0 : null;
  const chipNodes = new Map();
  const custom = el("input", { type: "date", class: "form-control dur-num" });
  if (initialExpiresAt !== null) {
    const d = new Date(initialExpiresAt);
    if (!Number.isNaN(d.getTime())) {
      custom.value = `${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())}`;
    }
  }
  const paint = () => {
    for (const [d, node] of chipNodes) node.classList.toggle("is-active", chosen === d);
    customChip.classList.toggle("is-active", chosen === null);
    custom.style.display = chosen === null ? "" : "none";
  };
  for (const d of EXPIRY_DAYS) {
    const chip = elI18n("button", EXPIRY_KEY[d], { type: "button", class: "chip-btn" });
    chip.addEventListener("click", () => { chosen = d; paint(); });
    chipNodes.set(d, chip);
    chips.appendChild(chip);
  }
  const customChip = elI18n("button", "section.ak.expiry.custom", { type: "button", class: "chip-btn" });
  customChip.addEventListener("click", () => { chosen = null; paint(); });
  chips.appendChild(customChip);
  expGroup.appendChild(chips);
  expGroup.appendChild(custom);
  form.appendChild(expGroup);

  const err = el("div", { class: "form-error", role: "alert" });
  form.appendChild(err);
  paint();

  /**
   * 读一次表单。**校验在这里做一半**：后端只在收到请求时才看得到这些值，
   * 而一次注定被拒的往返会在服务端留下一条无意义的失败记录。
   * ⚠️ 另一半（长度上界、时刻必须在未来）**后端照样自己判**，这里不是唯一那道闸。
   */
  const read = () => {
    err.textContent = "";
    const v = name.value.trim();
    if (v === "") { err.textContent = t("section.ak.err.nameEmpty"); return null; }
    if (cap.nameMax !== null && v.length > cap.nameMax) {
      err.textContent = t("section.ak.err.nameTooLong", { max: cap.nameMax });
      return null;
    }
    let expiresAt = null;
    if (chosen === null) {
      if (custom.value === "") { err.textContent = t("section.ak.err.expiryBad"); return null; }
      const ms = Date.parse(`${custom.value}T23:59:59Z`);
      if (Number.isNaN(ms)) { err.textContent = t("section.ak.err.expiryBad"); return null; }
      if (ms <= Date.now()) { err.textContent = t("section.ak.err.expiryPast"); return null; }
      expiresAt = ms;
    } else if (chosen > 0) {
      expiresAt = Date.now() + chosen * 24 * 3600 * 1000;
    }
    return { name: v, expiresAt };
  };
  return { form, read, err };
}

/** 签发对话框。到期那几个快捷档写的是「**自签发时刻起** N 天」。 */
function openIssueDialog() {
  const f = keyForm("", null);
  const body = el("div");
  body.appendChild(elI18n("p", "section.ak.issueDesc", { class: "modal-desc" }));
  body.appendChild(f.form);
  body.appendChild(elI18n("p", "section.ak.form.onceOnly", { class: "mng-inline-note" }));

  openModal("section.ak.issueTitle", body, [
    { labelKey: "common.cancel" },
    {
      labelKey: "section.ak.issueSubmit",
      // 🔴 `keepOpen`：校验失败 / 网络失败时弹窗**不许先关掉** —— 关了之后用户
      //    填进去的内容就全丢了，屏幕上只剩一句 toast。
      keepOpen: true,
      onClick: (close) => {
        const payload = f.read();
        if (payload === null) return;
        void afterWrite(api.post("/apikeys", payload), "section.ak.issued").then((out) => {
          if (out === null) return;
          close();
          showSecret(out);
        });
      },
    },
  ]);
}

/**
 * 签发之后那一次明文。
 *
 * ⚠️ 这个部署**能不能再取回明文**由 `capabilities.apiKeys.plaintextRetrievable` 说，
 * 面板据它决定要不要画那句「关掉就再也看不到了」，**不在前端写死**。
 */
function showSecret(out) {
  const o = asObject(out);
  const secret = o !== null && typeof o.secret === "string" ? o.secret : null;
  const body = el("div");
  body.appendChild(elI18n("p", "section.ak.secret.intro", { class: "modal-desc" }));
  const box = el("div", { class: "created-key-box" });
  box.appendChild(elI18n("div", "section.ak.secret.label", { class: "form-label" }));
  box.appendChild(el("div", { class: "cka-key" }, secret === null ? DASH : secret));
  const btn = el("button", { type: "button", class: "btn btn-primary btn-sm" });
  btn.appendChild(svgIcon(ICON_COPY, 15));
  btn.appendChild(elI18n("span", "section.ak.secret.copy"));
  // **只复制密钥本身**，不连同周边说明一起塞进剪贴板。
  btn.addEventListener("click", () => { if (secret !== null) void copy(secret); });
  box.appendChild(btn);
  body.appendChild(box);
  // ⚠️⚠️ **这一处与 `revealCell()` 的判据刻意相反，两边各自倒向安全的那一侧。**
  //    这里是 `!cap.plaintextRetrievable` ⇒ `null`（还不知道）**照样警告**：
  //    「还不知道能不能再取回」时叫人现在就复制走，代价是多复制一次；
  //    反过来（不知道就不警告）的代价是关掉这一屏之后明文可能真的没了。
  //    而 `revealCell()` 那边 `null` 倒向「把按钮留着」—— 同一条原则的两个方向：
  //    **不确定时，不做那个不可挽回的选择。**
  if (!cap.plaintextRetrievable) {
    body.appendChild(elI18n("p", "section.ak.secret.onceOnly", { class: "banner banner-warn" }));
  }
  openModal("section.ak.secret.title", body, [{ labelKey: "common.close" }]);
}

function openEditDialog(k) {
  const current = k.expiresAt === null || k.expiresAt === undefined ? null : Number(k.expiresAt);
  const f = keyForm(String(k.name ?? ""), current);
  const body = el("div");
  body.appendChild(elI18n("p", "section.ak.editDesc", { class: "modal-desc" }));
  body.appendChild(f.form);

  openModal("section.ak.editTitle", body, [
    { labelKey: "common.cancel" },
    {
      labelKey: "common.confirm",
      keepOpen: true,
      onClick: (close) => {
        const payload = f.read();
        if (payload === null) return;
        const v = version();
        if (v === null) { f.err.textContent = t("section.ak.staleView"); return; }
        void afterWrite(
          api.patch(`/apikeys/${encodeURIComponent(k.id)}`, { version: v, name: payload.name, expiresAt: payload.expiresAt }),
          "section.ak.saved",
        ).then((out) => { if (out !== null) close(); });
      },
    },
  ]);
}

function toggleDisabled(k, next) {
  const v = version();
  if (v === null) { toast(t("section.ak.staleView"), "warn"); return; }
  void afterWrite(
    api.patch(`/apikeys/${encodeURIComponent(k.id)}`, { version: v, disabled: next }),
    next ? "section.ak.disabled" : "section.ak.enabled",
  ).then((out) => {
    // 🔴 **停用成功要把「多久之后才在别处真的失效」说出来，而且要给具体的数字。**
    //    这是安全相关的：运维会以为停用是即时的，而中间还隔着一层进程内缓存。
    if (out !== null && next) toast(revokeDelayText(), "warn", { sticky: true });
  });
}

function confirmDelete(k) {
  // ⚠️ 标题与正文都走字典；密钥名称**不拼进那句话**（它是运维自由输入的字段，
  //    而 `confirmModal` 的正文走的是不带参数的 `t()`）。
  confirmModal("section.ak.deleteTitle", "section.ak.deleteConfirm", () => {
    const v = version();
    if (v === null) { toast(t("section.ak.staleView"), "warn"); return; }
    void afterWrite(
      // ⚠️ 版本号走**查询参数**而不是 `DELETE` 的请求体：带体的 DELETE 在规范上合法，
      //    但中间代理与部分客户端会把它丢掉 —— 一条**被静默丢掉的并发凭证**的后果是
      //    「运维以为自己带了那道保险，其实没带」。
      api.del(`/apikeys/${encodeURIComponent(k.id)}?version=${v}`),
      "section.ak.deleted",
    ).then((out) => { if (out !== null) toast(revokeDelayText(), "warn", { sticky: true }); });
  });
}

/** 清理失效：把这一次会删掉哪几把**逐条列出来**，不只给一个数字。 */
function openPurgeDialog(n) {
  const rows = viewRows();
  const body = el("div");
  body.appendChild(el("p", { class: "modal-desc" }, t("section.ak.purgeDesc", { n })));
  const list = el("div", { class: "purge-list" });
  for (const k of items()) {
    const row = rows.get(k.id);
    if (row === undefined) continue;
    if (row.bucketLabelKey !== "ak.bucket.disabled" && row.bucketLabelKey !== "ak.bucket.expired") continue;
    const item = el("div", { class: "purge-item" });
    const left = el("span");
    left.appendChild(el("code", { class: "key-serial" }, `#${String(k.seq ?? DASH)}`));
    left.appendChild(el("span", null, ` ${String(k.name ?? "")}`));
    item.appendChild(left);
    const key = VIEW_KEY[row.bucketLabelKey];
    item.appendChild(el("span", { class: badgeClass(row.badgeClass) },
      key === undefined ? String(row.bucketLabelKey) : t(key)));
    list.appendChild(item);
  }
  body.appendChild(list);

  openModal("section.ak.purgeTitle", body, [
    { labelKey: "common.cancel" },
    {
      labelKey: "section.ak.purgeConfirm",
      danger: true,
      onClick: () => {
        const v = version();
        if (v === null) { toast(t("section.ak.staleView"), "warn"); return; }
        void afterWrite(api.post("/apikeys/purge", { version: v }), "section.ak.purged");
      },
    },
  ]);
}

/* ============================================================
   板块契约
   ============================================================ */

export const apikeysSection = {
  init(host) {
    host.textContent = "";
    const body = el("div");
    host.appendChild(body);
    // `location.origin` 只读一次：它在一次会话里不会变，而每一次 render 都要写它。
    nodes = { body, origin: location.origin };
  },

  onShow() {
    render();
    void (async () => {
      if (cap.wired === null) await loadCapabilities();
      await load();
      // ⚠️ **排在列表之后、且不并发发**：这一行是卡片上的附加信息，列表拉不出来时
      //    卡片压根不画，先把它抢在前面只会白付一次请求。
      await loadUsage();
    })();
  },

  onHide() {
    if (abort !== null) { abort.abort(); abort = null; }
    // 用量那一条在飞的同样作废：它回来时会 `render()`，而那时板块已经切走了。
    usageSeq++;
    // 🔴 **明文不跨板块存活**：切走就忘掉，回来时是掩码态。
    revealState.forget();
  },
};
