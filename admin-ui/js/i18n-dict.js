/**
 * 五语言词条表。**key-major**（一个 key 下挂五种语言），不是 lang-major。
 * 理由：key-major 下「加了键只填一种语言」在 diff 里一眼可见；lang-major 下它散在
 * 五个相距几百行的地方，评审基本看不出来。
 *
 * 🔴🔴 **`export const I18N =` 与末尾那个 `;` 之间必须是一整块合法 JSON**
 *（裁定 P-10）：键全带双引号、**无尾逗号、无注释、无单引号、无模板串**。
 * 浏览器侧照旧 `import { I18N } from "./i18n-dict.js"`，**零变化**；
 * 而 Rust 侧 `include_str!` 掐掉头尾之后能 `serde_json` **严格解析** ——
 * 于是 agnes 那 10 条 node 门禁里的 8 条进了 `cargo test`
 *（齐全性 / 占位符集合五语言一致 / 未引用 key 是硬错 / 面板里的 key 必须在字典里 /
 *  带占位符的 key 不许当裸标签 / 未翻译泄漏 / 偏好词前缀表 + 反向控制 / 严格可解析）。
 * ⇒ **别往下面那一块里加注释或尾逗号，那会让整组判据当场红。**
 *
 * ⚠️ **「未被引用的 key 是硬错，不是警告」**：一个没人用的词条既不会被看见、也不会
 * 被翻译审核，它只会在下一次改名时被漏掉，然后以「面板上出现一个裸 key」的形态浮上来。
 *
 * ⚠️ 语言菜单里那五个**母语名**（简体中文 / English / 日本語 / 한국어 …）
 * **刻意不在这里**，它们是 `js/i18n.js` 的 `LANG_LABELS`：一个看不懂当前界面语言的人
 * 要靠那四个字找到自己那一项，把它翻译掉恰好毁掉它唯一的用处。
 *
 * ⚠️ **命名空间是三族，不是一族**（这一点曾经真的坑过人，见下一段）：
 *   · 壳层 —— `nav.` / `header.` / `login.` / `common.` / `reveal.`；
 *   · 仪表盘与账号管理 —— `dash.` / `keys.`（两个板块先落地，各自起了自己的前缀）；
 *   · 其余七个板块 —— `section.<板块简称>.`（`ak` / `ev` / `md` / `pg` / `reg` /
 *     `set` / `usg`）。
 * 三族没有优劣之分，也**不必强行统一**：判据不看前缀长什么样，只看
 * 「面板里出现的 key 是不是都在这里」与「这里的 key 是不是都有人用」。
 *
 * 🔴🔴 **已了结的一处事故，写在这里是为了别再犯第二次。**
 * `dash.*`（43 条）与 `keys.*`（115 条）曾经**整整两族 158 条一条都不在这张表里**：
 * 仪表盘整屏印的是 `dash.title` / `dash.refresh` 这样的裸 key，而 `cargo test` 全绿。
 * 成因不在这个文件，在判据那一侧：`src/webui/mod.rs` 原来按一张**命名空间白名单**
 * 决定「哪些字符串算 i18n key」，而这两个前缀没人记得往表里加 ⇒ 扫描根本看不见它们
 * ⇒ 两条双向门禁对它们同时失效。**白名单这个形态本身就是缺陷之源：忘了加不会红。**
 * 处置已落地：扫描改成按 key 形态认，「不是词条」的那几条反过来登记。
 * ⇒ 今天在这里**新开一个前缀不需要去改任何一张表**；漏了词条会当场红。
 */
export const I18N =
{
  "nav.dashboard": {
    "zh-CN": "仪表盘",
    "zh-TW": "儀表板",
    "en": "Dashboard",
    "ja": "ダッシュボード",
    "ko": "대시보드"
  },
  "nav.accounts": {
    "zh-CN": "账号管理",
    "zh-TW": "帳號管理",
    "en": "Accounts",
    "ja": "アカウント管理",
    "ko": "계정 관리"
  },
  "nav.models": {
    "zh-CN": "支持模型",
    "zh-TW": "支援模型",
    "en": "Models",
    "ja": "対応モデル",
    "ko": "지원 모델"
  },
  "nav.apikeys": {
    "zh-CN": "API 管理",
    "zh-TW": "API 管理",
    "en": "API Keys",
    "ja": "API キー管理",
    "ko": "API 키 관리"
  },
  "nav.playground": {
    "zh-CN": "调试台",
    "zh-TW": "除錯台",
    "en": "Playground",
    "ja": "デバッグ台",
    "ko": "디버그 콘솔"
  },
  "nav.usage": {
    "zh-CN": "使用统计",
    "zh-TW": "使用統計",
    "en": "Usage",
    "ja": "利用統計",
    "ko": "사용 통계"
  },
  "nav.logs": {
    "zh-CN": "实时日志",
    "zh-TW": "即時日誌",
    "en": "Live Logs",
    "ja": "リアルタイムログ",
    "ko": "실시간 로그"
  },
  "nav.registrar": {
    "zh-CN": "注册机",
    "zh-TW": "註冊機",
    "en": "Registrar",
    "ja": "自動登録",
    "ko": "자동 등록기"
  },
  "nav.settings": {
    "zh-CN": "设置",
    "zh-TW": "設定",
    "en": "Settings",
    "ja": "設定",
    "ko": "설정"
  },
  "nav.logout": {
    "zh-CN": "退出登录",
    "zh-TW": "登出",
    "en": "Sign out",
    "ja": "ログアウト",
    "ko": "로그아웃"
  },
  "nav.menu": {
    "zh-CN": "菜单",
    "zh-TW": "選單",
    "en": "Menu",
    "ja": "メニュー",
    "ko": "메뉴"
  },
  "header.statusHint": {
    "zh-CN": "最近一次探测的结果，点一下重新探",
    "zh-TW": "最近一次探測的結果，點一下重新探",
    "en": "Result of the last probe — click to probe again",
    "ja": "直近の確認結果です。クリックで再確認します",
    "ko": "마지막 확인 결과입니다. 클릭하면 다시 확인합니다"
  },
  "header.statusUnknown": {
    "zh-CN": "状态未知",
    "zh-TW": "狀態未知",
    "en": "Status unknown",
    "ja": "状態不明",
    "ko": "상태 알 수 없음"
  },
  "header.statusRunning": {
    "zh-CN": "运行中",
    "zh-TW": "運行中",
    "en": "Running",
    "ja": "稼働中",
    "ko": "실행 중"
  },
  "header.statusDegraded": {
    "zh-CN": "降级运行",
    "zh-TW": "降級運行",
    "en": "Degraded",
    "ja": "機能低下",
    "ko": "성능 저하"
  },
  "header.statusOffline": {
    "zh-CN": "探测失败",
    "zh-TW": "探測失敗",
    "en": "Probe failed",
    "ja": "確認できません",
    "ko": "확인 실패"
  },
  "header.repo": {
    "zh-CN": "打开代码仓库",
    "zh-TW": "開啟程式碼倉庫",
    "en": "Open the source repository",
    "ja": "ソースリポジトリを開く",
    "ko": "소스 저장소 열기"
  },
  "header.language": {
    "zh-CN": "切换界面语言",
    "zh-TW": "切換介面語言",
    "en": "Change language",
    "ja": "表示言語を切り替える",
    "ko": "표시 언어 변경"
  },
  "header.theme": {
    "zh-CN": "切换深色 / 浅色",
    "zh-TW": "切換深色 / 淺色",
    "en": "Toggle dark / light",
    "ja": "ダーク / ライトを切り替える",
    "ko": "다크 / 라이트 전환"
  },
  "login.title": {
    "zh-CN": "请输入管理口令",
    "zh-TW": "請輸入管理口令",
    "en": "Enter the admin token",
    "ja": "管理トークンを入力してください",
    "ko": "관리 토큰을 입력하세요"
  },
  "login.label": {
    "zh-CN": "管理口令（ADMIN_TOKEN）",
    "zh-TW": "管理口令（ADMIN_TOKEN）",
    "en": "Admin token (ADMIN_TOKEN)",
    "ja": "管理トークン（ADMIN_TOKEN）",
    "ko": "관리 토큰(ADMIN_TOKEN)"
  },
  "login.keyPlaceholder": {
    "zh-CN": "粘贴部署时配置的口令",
    "zh-TW": "貼上部署時配置的口令",
    "en": "Paste the token configured at deploy time",
    "ja": "デプロイ時に設定したトークンを貼り付けます",
    "ko": "배포 시 설정한 토큰을 붙여 넣으세요"
  },
  "login.submit": {
    "zh-CN": "进入",
    "zh-TW": "進入",
    "en": "Enter",
    "ja": "入る",
    "ko": "들어가기"
  },
  "login.hint": {
    "zh-CN": "口令来自部署时配置的环境变量 ADMIN_TOKEN",
    "zh-TW": "口令來自部署時配置的環境變數 ADMIN_TOKEN",
    "en": "The token comes from the ADMIN_TOKEN environment variable",
    "ja": "トークンは環境変数 ADMIN_TOKEN から来ます",
    "ko": "토큰은 환경 변수 ADMIN_TOKEN 에서 옵니다"
  },
  "login.empty": {
    "zh-CN": "口令不能为空",
    "zh-TW": "口令不能為空",
    "en": "The token cannot be empty",
    "ja": "トークンを空にはできません",
    "ko": "토큰은 비워 둘 수 없습니다"
  },
  "login.badShape": {
    "zh-CN": "口令里有送不出去的字符（只允许可打印 ASCII）",
    "zh-TW": "口令裡有送不出去的字元（只允許可列印 ASCII）",
    "en": "The token contains characters that cannot be sent (printable ASCII only)",
    "ja": "送信できない文字が含まれています（印字可能な ASCII のみ）",
    "ko": "보낼 수 없는 문자가 있습니다(인쇄 가능한 ASCII 만 허용)"
  },
  "login.invalid": {
    "zh-CN": "口令不对",
    "zh-TW": "口令不對",
    "en": "Wrong token",
    "ja": "トークンが違います",
    "ko": "토큰이 올바르지 않습니다"
  },
  "login.httpError": {
    "zh-CN": "服务端返回了 {status}，这一次没能登录",
    "zh-TW": "伺服器回傳了 {status}，這一次沒能登入",
    "en": "The server answered {status}; this attempt did not sign in",
    "ja": "サーバーが {status} を返しました。今回はログインできていません",
    "ko": "서버가 {status} 을(를) 반환했습니다. 이번에는 로그인되지 않았습니다"
  },
  "login.network": {
    "zh-CN": "连不上服务端",
    "zh-TW": "連不上伺服器",
    "en": "Cannot reach the server",
    "ja": "サーバーに接続できません",
    "ko": "서버에 연결할 수 없습니다"
  },
  "common.sessionExpired": {
    "zh-CN": "会话已到期，请重新输入口令",
    "zh-TW": "工作階段已到期，請重新輸入口令",
    "en": "The session has expired; please enter the token again",
    "ja": "セッションが期限切れです。トークンを入力し直してください",
    "ko": "세션이 만료되었습니다. 토큰을 다시 입력하세요"
  },
  "common.cancel": {
    "zh-CN": "取消",
    "zh-TW": "取消",
    "en": "Cancel",
    "ja": "キャンセル",
    "ko": "취소"
  },
  "common.confirm": {
    "zh-CN": "确认",
    "zh-TW": "確認",
    "en": "Confirm",
    "ja": "確認する",
    "ko": "확인"
  },
  "common.close": {
    "zh-CN": "关闭",
    "zh-TW": "關閉",
    "en": "Close",
    "ja": "閉じる",
    "ko": "닫기"
  },
  "common.dismiss": {
    "zh-CN": "知道了",
    "zh-TW": "知道了",
    "en": "Dismiss",
    "ja": "了解",
    "ko": "확인했습니다"
  },
  "common.copy": {
    "zh-CN": "复制",
    "zh-TW": "複製",
    "en": "Copy",
    "ja": "コピー",
    "ko": "복사"
  },
  "common.copied": {
    "zh-CN": "已复制到剪贴板",
    "zh-TW": "已複製到剪貼簿",
    "en": "Copied to the clipboard",
    "ja": "クリップボードにコピーしました",
    "ko": "클립보드에 복사했습니다"
  },
  "common.copyFailed": {
    "zh-CN": "复制没成功，剪贴板在非 TLS 页面上不可用",
    "zh-TW": "複製沒成功，剪貼簿在非 TLS 頁面上不可用",
    "en": "Copy failed — the clipboard is unavailable on a non-TLS page",
    "ja": "コピーできませんでした。TLS でないページではクリップボードを使えません",
    "ko": "복사하지 못했습니다. TLS 가 아닌 페이지에서는 클립보드를 쓸 수 없습니다"
  },
  "reveal.show": {
    "zh-CN": "显示明文",
    "zh-TW": "顯示明文",
    "en": "Reveal",
    "ja": "平文を表示",
    "ko": "평문 보기"
  },
  "reveal.hide": {
    "zh-CN": "收起明文",
    "zh-TW": "收起明文",
    "en": "Hide",
    "ja": "平文を隠す",
    "ko": "평문 숨기기"
  },
  "dash.approxHint": {
    "zh-CN": "近似值：池级累计在并发下会少计，最后一段也可能还没落盘。",
    "zh-TW": "近似值：池級累計在並發下會少計，最後一段也可能還沒落盤。",
    "en": "Approximate: the pool-wide totals undercount under concurrency, and the latest slice may not be persisted yet.",
    "ja": "概算値：プール全体の累計は同時実行下で少なく数えられ、直近の分はまだ保存されていない可能性があります。",
    "ko": "근사값: 풀 전체 누적은 동시 실행에서 적게 집계되며, 가장 최근 구간은 아직 저장되지 않았을 수 있습니다."
  },
  "dash.cardModels": {
    "zh-CN": "可用模型",
    "zh-TW": "可用模型",
    "en": "Models available",
    "ja": "利用できるモデル",
    "ko": "사용 가능한 모델"
  },
  "dash.cardPool": {
    "zh-CN": "账号池（可用 / 总数）",
    "zh-TW": "帳號池（可用 / 總數）",
    "en": "Account pool (usable / total)",
    "ja": "アカウントプール（利用可能 / 総数）",
    "ko": "계정 풀(사용 가능 / 전체)"
  },
  "dash.cardRequests": {
    "zh-CN": "今日请求",
    "zh-TW": "今日請求",
    "en": "Requests today",
    "ja": "本日のリクエスト",
    "ko": "오늘 요청"
  },
  "dash.cardSuccessRate": {
    "zh-CN": "今日成功率",
    "zh-TW": "今日成功率",
    "en": "Success rate today",
    "ja": "本日の成功率",
    "ko": "오늘 성공률"
  },
  "dash.cardTokens": {
    "zh-CN": "今日 token",
    "zh-TW": "今日 token",
    "en": "Tokens today",
    "ja": "本日のトークン",
    "ko": "오늘 토큰"
  },
  "dash.degraded": {
    "zh-CN": "这一次配置装载发生了降级：有字段没读出来或回落到了默认值，你在设置页保存过的值可能没有生效。",
    "zh-TW": "這一次設定載入發生了降級：有欄位沒讀出來或回落到了預設值，你在設定頁儲存過的值可能沒有生效。",
    "en": "This config load was degraded: some fields could not be read or fell back to defaults, so values you saved under Settings may not be in effect.",
    "ja": "今回の設定読み込みは劣化しています：読み取れなかった項目や既定値に戻った項目があり、設定ページで保存した値が反映されていない可能性があります。",
    "ko": "이번 설정 로드는 저하되었습니다: 읽지 못했거나 기본값으로 되돌아간 항목이 있어 설정 페이지에서 저장한 값이 적용되지 않았을 수 있습니다."
  },
  "dash.docs": {
    "zh-CN": "部署文档",
    "zh-TW": "部署文件",
    "en": "Deployment docs",
    "ja": "デプロイ手順",
    "ko": "배포 문서"
  },
  "dash.docsDesc": {
    "zh-CN": "怎么部署、怎么配置，都写在仓库的 docs 里",
    "zh-TW": "怎麼部署、怎麼設定，都寫在倉庫的 docs 裡",
    "en": "How to deploy and how to configure it are both in docs/ in the repository",
    "ja": "デプロイ方法と設定方法はリポジトリの docs にあります",
    "ko": "배포 방법과 설정 방법은 저장소의 docs 에 있습니다"
  },
  "dash.docsHint": {
    "zh-CN": "docs/ 目录",
    "zh-TW": "docs/ 目錄",
    "en": "docs/ folder",
    "ja": "docs/ ディレクトリ",
    "ko": "docs/ 디렉터리"
  },
  "dash.durationDays": {
    "zh-CN": "{d} 天 {h} 小时",
    "zh-TW": "{d} 天 {h} 小時",
    "en": "{d}d {h}h",
    "ja": "{d}日{h}時間",
    "ko": "{d}일 {h}시간"
  },
  "dash.durationHours": {
    "zh-CN": "{h} 小时 {m} 分",
    "zh-TW": "{h} 小時 {m} 分",
    "en": "{h}h {m}m",
    "ja": "{h}時間{m}分",
    "ko": "{h}시간 {m}분"
  },
  "dash.durationMinutes": {
    "zh-CN": "{m} 分 {s} 秒",
    "zh-TW": "{m} 分 {s} 秒",
    "en": "{m}m {s}s",
    "ja": "{m}分{s}秒",
    "ko": "{m}분 {s}초"
  },
  "dash.envLocked": {
    "zh-CN": "被环境变量锁住的字段",
    "zh-TW": "被環境變數鎖住的欄位",
    "en": "Fields locked by the environment",
    "ja": "環境変数で固定された項目",
    "ko": "환경 변수로 고정된 항목"
  },
  "dash.envLockedNone": {
    "zh-CN": "没有字段被环境变量锁住，设置页改什么都会生效。",
    "zh-TW": "沒有欄位被環境變數鎖住，設定頁改什麼都會生效。",
    "en": "No field is locked by an environment variable — everything you change under Settings takes effect.",
    "ja": "環境変数で固定されている項目はありません。設定ページでの変更はすべて反映されます。",
    "ko": "환경 변수로 고정된 항목이 없습니다. 설정 페이지에서 바꾼 내용은 모두 적용됩니다."
  },
  "dash.envLockedValue": {
    "zh-CN": "{n} 个字段 · 去设置页",
    "zh-TW": "{n} 個欄位 · 去設定頁",
    "en": "{n} field(s) · open Settings",
    "ja": "{n} 項目 · 設定ページへ",
    "ko": "{n}개 항목 · 설정 페이지로"
  },
  "dash.freshConfig": {
    "zh-CN": "配置保存之后，最多还要 {sec} 秒才会在每个实例上生效 —— 不是「立即生效」。",
    "zh-TW": "設定儲存之後，最多還要 {sec} 秒才會在每個實例上生效 —— 不是「立即生效」。",
    "en": "After a config is saved it takes up to {sec} s to take effect on every instance — not “immediately”.",
    "ja": "設定を保存してから、すべてのインスタンスに反映されるまで最大 {sec} 秒かかります。「即時反映」ではありません。",
    "ko": "설정을 저장한 뒤 모든 인스턴스에 반영되기까지 최대 {sec}초 걸립니다 — “즉시 반영”이 아닙니다."
  },
  "dash.freshPool": {
    "zh-CN": "别的实例判定的冷却 / 剔除，这里最多晚 {sec} 秒才看得到。",
    "zh-TW": "別的實例判定的冷卻 / 剔除，這裡最多晚 {sec} 秒才看得到。",
    "en": "Cooldowns and evictions decided by another instance show up here up to {sec} s late.",
    "ja": "他のインスタンスが判定したクールダウン／除外は、ここに最大 {sec} 秒遅れて現れます。",
    "ko": "다른 인스턴스가 판정한 쿨다운/제외는 여기에 최대 {sec}초 늦게 나타납니다."
  },
  "dash.incompleteHint": {
    "zh-CN": "这段数据不完整：有分片没读到，这个数偏小。",
    "zh-TW": "這段資料不完整：有分片沒讀到，這個數偏小。",
    "en": "This window is incomplete: some shards were not read, so the number is an undercount.",
    "ja": "この期間のデータは不完全です：読み取れなかった断片があり、この数値は少なめに出ています。",
    "ko": "이 구간의 데이터는 불완전합니다: 읽지 못한 조각이 있어 값이 실제보다 적습니다."
  },
  "dash.memory": {
    "zh-CN": "常驻内存",
    "zh-TW": "常駐記憶體",
    "en": "Resident memory",
    "ja": "常駐メモリ",
    "ko": "상주 메모리"
  },
  "dash.noneHint": {
    "zh-CN": "这一次读成功了，只是这一段没有样本。",
    "zh-TW": "這一次讀成功了，只是這一段沒有樣本。",
    "en": "The read succeeded; this window simply has no sample.",
    "ja": "読み取りは成功しましたが、この期間にはサンプルがありません。",
    "ko": "읽기는 성공했지만 이 구간에는 표본이 없습니다."
  },
  "dash.pid": {
    "zh-CN": "进程号",
    "zh-TW": "行程號",
    "en": "Process id",
    "ja": "プロセス ID",
    "ko": "프로세스 ID"
  },
  "dash.poolShape": {
    "zh-CN": "池子形态",
    "zh-TW": "池子形態",
    "en": "Pool breakdown",
    "ja": "プールの内訳",
    "ko": "풀 구성"
  },
  "dash.poolShapeValue": {
    "zh-CN": "冷却 {cooling} · 剔除 {evicted} · 停用 {disabled}",
    "zh-TW": "冷卻 {cooling} · 剔除 {evicted} · 停用 {disabled}",
    "en": "{cooling} cooling · {evicted} evicted · {disabled} disabled",
    "ja": "クールダウン {cooling} · 除外 {evicted} · 無効 {disabled}",
    "ko": "쿨다운 {cooling} · 제외 {evicted} · 정지 {disabled}"
  },
  "dash.poolStats": {
    "zh-CN": "池级累计",
    "zh-TW": "池級累計",
    "en": "Pool totals",
    "ja": "プール累計",
    "ko": "풀 누적"
  },
  "dash.poolStatsValue": {
    "zh-CN": "请求 {requests} · 失败 {failed}",
    "zh-TW": "請求 {requests} · 失敗 {failed}",
    "en": "{requests} requests · {failed} failed",
    "ja": "リクエスト {requests} · 失敗 {failed}",
    "ko": "요청 {requests} · 실패 {failed}"
  },
  "dash.refresh": {
    "zh-CN": "刷新",
    "zh-TW": "重新整理",
    "en": "Refresh",
    "ja": "更新",
    "ko": "새로 고침"
  },
  "dash.registrar": {
    "zh-CN": "注册机",
    "zh-TW": "註冊機",
    "en": "Registrar",
    "ja": "自動登録",
    "ko": "자동 등록기"
  },
  "dash.registrarBlocked": {
    "zh-CN": "开着，但这一轮没跑起来",
    "zh-TW": "開著，但這一輪沒跑起來",
    "en": "On, but it never started this round",
    "ja": "オンですが、今回は起動していません",
    "ko": "켜져 있지만 이번에는 시작되지 않았습니다"
  },
  "dash.registrarOff": {
    "zh-CN": "已关闭",
    "zh-TW": "已關閉",
    "en": "Off",
    "ja": "無効",
    "ko": "꺼짐"
  },
  "dash.registrarOn": {
    "zh-CN": "运行中",
    "zh-TW": "運行中",
    "en": "Running",
    "ja": "稼働中",
    "ko": "실행 중"
  },
  "dash.repo": {
    "zh-CN": "代码仓库",
    "zh-TW": "程式碼倉庫",
    "en": "Source repository",
    "ja": "ソースリポジトリ",
    "ko": "소스 저장소"
  },
  "dash.repoDesc": {
    "zh-CN": "源码、问题与发版都在 GitHub 上",
    "zh-TW": "原始碼、問題與發版都在 GitHub 上",
    "en": "Source, issues and releases all live on GitHub",
    "ja": "ソース・課題・リリースはすべて GitHub にあります",
    "ko": "소스와 이슈, 릴리스는 모두 GitHub 에 있습니다"
  },
  "dash.repoHint": {
    "zh-CN": "GitHub 仓库",
    "zh-TW": "GitHub 倉庫",
    "en": "GitHub repo",
    "ja": "GitHub リポジトリ",
    "ko": "GitHub 저장소"
  },
  "dash.runtime": {
    "zh-CN": "运行时",
    "zh-TW": "執行時",
    "en": "Runtime",
    "ja": "ランタイム",
    "ko": "런타임"
  },
  "dash.serverTime": {
    "zh-CN": "服务器时间",
    "zh-TW": "伺服器時間",
    "en": "Server time",
    "ja": "サーバー時刻",
    "ko": "서버 시각"
  },
  "dash.storage": {
    "zh-CN": "存储后端",
    "zh-TW": "儲存後端",
    "en": "Storage backend",
    "ja": "ストレージバックエンド",
    "ko": "스토리지 백엔드"
  },
  "dash.storageReadonly": {
    "zh-CN": "只读",
    "zh-TW": "唯讀",
    "en": "read-only",
    "ja": "読み取り専用",
    "ko": "읽기 전용"
  },
  "dash.storageWritable": {
    "zh-CN": "可写",
    "zh-TW": "可寫",
    "en": "writable",
    "ja": "書き込み可能",
    "ko": "쓰기 가능"
  },
  "dash.systemInfo": {
    "zh-CN": "系统信息",
    "zh-TW": "系統資訊",
    "en": "System info",
    "ja": "システム情報",
    "ko": "시스템 정보"
  },
  "dash.title": {
    "zh-CN": "仪表盘",
    "zh-TW": "儀表板",
    "en": "Dashboard",
    "ja": "ダッシュボード",
    "ko": "대시보드"
  },
  "dash.unknownHint": {
    "zh-CN": "这一格压根没读出来 —— 这是「不知道」，不是 0。",
    "zh-TW": "這一格壓根沒讀出來 —— 這是「不知道」，不是 0。",
    "en": "This value could not be read at all — it means “unknown”, not zero.",
    "ja": "この値はまったく読み取れませんでした —— 「不明」であって 0 ではありません。",
    "ko": "이 값은 전혀 읽지 못했습니다 — “알 수 없음”이지 0이 아닙니다."
  },
  "dash.uptime": {
    "zh-CN": "进程已运行",
    "zh-TW": "行程已運行",
    "en": "Process uptime",
    "ja": "プロセス稼働時間",
    "ko": "프로세스 가동 시간"
  },
  "dash.usageOffHint": {
    "zh-CN": "这个部署没在记时间序列统计，去设置页打开之后才会有数。",
    "zh-TW": "這個部署沒在記時間序列統計，去設定頁打開之後才會有數。",
    "en": "This deployment is not recording time-series usage; turn it on under Settings and numbers will appear.",
    "ja": "このデプロイは時系列の使用量を記録していません。設定ページで有効にすると数値が出ます。",
    "ko": "이 배포는 시계열 사용량을 기록하지 않습니다. 설정 페이지에서 켜야 수치가 나옵니다."
  },
  "dash.version": {
    "zh-CN": "版本号",
    "zh-TW": "版本號",
    "en": "Version",
    "ja": "バージョン",
    "ko": "버전"
  },
  "keys.action.clearCooldown": {
    "zh-CN": "清冷却",
    "zh-TW": "清冷卻",
    "en": "Clear cooldown",
    "ja": "クールダウン解除",
    "ko": "쿨다운 해제"
  },
  "keys.action.clearStrikes": {
    "zh-CN": "清连续失败",
    "zh-TW": "清連續失敗",
    "en": "Clear strikes",
    "ja": "連続失敗をクリア",
    "ko": "연속 실패 초기화"
  },
  "keys.action.delete": {
    "zh-CN": "删除这把",
    "zh-TW": "刪除這把",
    "en": "Delete this account",
    "ja": "このアカウントを削除",
    "ko": "이 계정 삭제"
  },
  "keys.action.disable": {
    "zh-CN": "停用",
    "zh-TW": "停用",
    "en": "Disable",
    "ja": "無効化",
    "ko": "정지"
  },
  "keys.action.enable": {
    "zh-CN": "启用",
    "zh-TW": "啟用",
    "en": "Enable",
    "ja": "有効化",
    "ko": "활성화"
  },
  "keys.action.note": {
    "zh-CN": "备注",
    "zh-TW": "備註",
    "en": "Note",
    "ja": "備考",
    "ko": "비고"
  },
  "keys.action.unevict": {
    "zh-CN": "解除剔除",
    "zh-TW": "解除剔除",
    "en": "Unevict",
    "ja": "除外解除",
    "ko": "제외 해제"
  },
  "keys.actionFailed": {
    "zh-CN": "操作失败：{message}",
    "zh-TW": "操作失敗：{message}",
    "en": "The action failed: {message}",
    "ja": "操作に失敗しました：{message}",
    "ko": "작업에 실패했습니다: {message}"
  },
  "keys.actionFailedGeneric": {
    "zh-CN": "操作失败，过一会儿再试一次。",
    "zh-TW": "操作失敗，過一會兒再試一次。",
    "en": "The action failed; try again in a moment.",
    "ja": "操作に失敗しました。少し待ってから再試行してください。",
    "ko": "작업에 실패했습니다. 잠시 후 다시 시도하세요."
  },
  "keys.actionOk": {
    "zh-CN": "操作成功",
    "zh-TW": "操作成功",
    "en": "Done",
    "ja": "操作が完了しました",
    "ko": "작업 완료"
  },
  "keys.add": {
    "zh-CN": "添加账号",
    "zh-TW": "新增帳號",
    "en": "Add account",
    "ja": "アカウントを追加",
    "ko": "계정 추가"
  },
  "keys.add.autoGroup": {
    "zh-CN": "自动注册",
    "zh-TW": "自動註冊",
    "en": "Auto-register",
    "ja": "自動登録",
    "ko": "자동 등록"
  },
  "keys.add.goRegistrar": {
    "zh-CN": "去注册机板块",
    "zh-TW": "去註冊機板塊",
    "en": "Open the Registrar section",
    "ja": "自動登録の画面へ",
    "ko": "자동 등록기 화면으로"
  },
  "keys.add.goRegistrarSub": {
    "zh-CN": "「立即补池」与通道设置都在那里",
    "zh-TW": "「立即補池」與通道設定都在那裡",
    "en": "Replenish-now and the channel settings both live there",
    "ja": "「今すぐ補充」とチャネル設定はそちらにあります",
    "ko": "‘즉시 보충’과 채널 설정은 거기에 있습니다"
  },
  "keys.add.import": {
    "zh-CN": "粘贴导入",
    "zh-TW": "貼上匯入",
    "en": "Paste and import",
    "ja": "貼り付けてインポート",
    "ko": "붙여넣어 가져오기"
  },
  "keys.add.importSub": {
    "zh-CN": "每行一把，支持多行粘贴",
    "zh-TW": "每行一把，支援多行貼上",
    "en": "One per line; multi-line paste works",
    "ja": "1 行に 1 つ。複数行の貼り付けに対応",
    "ko": "한 줄에 하나. 여러 줄 붙여넣기 가능"
  },
  "keys.add.manualGroup": {
    "zh-CN": "手动",
    "zh-TW": "手動",
    "en": "Manual",
    "ja": "手動",
    "ko": "수동"
  },
  "keys.auto.off": {
    "zh-CN": "不自动刷新",
    "zh-TW": "不自動重新整理",
    "en": "Auto refresh off",
    "ja": "自動更新オフ",
    "ko": "자동 새로고침 끔"
  },
  "keys.auto.sec": {
    "zh-CN": "每 {sec} 秒刷新",
    "zh-TW": "每 {sec} 秒重新整理",
    "en": "Refresh every {sec} s",
    "ja": "{sec} 秒ごとに更新",
    "ko": "{sec}초마다 새로 고침"
  },
  "keys.autoTitle": {
    "zh-CN": "自动刷新间隔",
    "zh-TW": "自動重新整理間隔",
    "en": "Auto refresh interval",
    "ja": "自動更新の間隔",
    "ko": "자동 새로고침 간격"
  },
  "keys.bucket.all": {
    "zh-CN": "全部",
    "zh-TW": "全部",
    "en": "All",
    "ja": "すべて",
    "ko": "전체"
  },
  "keys.bucket.cooling": {
    "zh-CN": "冷却中",
    "zh-TW": "冷卻中",
    "en": "Cooling down",
    "ja": "クールダウン中",
    "ko": "쿨다운 중"
  },
  "keys.bucket.disabled": {
    "zh-CN": "已停用",
    "zh-TW": "已停用",
    "en": "Disabled",
    "ja": "無効化済み",
    "ko": "비활성화됨"
  },
  "keys.bucket.evicted": {
    "zh-CN": "已剔除",
    "zh-TW": "已剔除",
    "en": "Evicted",
    "ja": "除外済み",
    "ko": "제외됨"
  },
  "keys.bucket.fresh": {
    "zh-CN": "可用",
    "zh-TW": "可用",
    "en": "Available",
    "ja": "利用可能",
    "ko": "사용 가능"
  },
  "keys.bulk.allOk": {
    "zh-CN": "批量操作全部成功。",
    "zh-TW": "批次操作全部成功。",
    "en": "The bulk action fully succeeded.",
    "ja": "一括操作はすべて成功しました。",
    "ko": "일괄 작업이 모두 성공했습니다."
  },
  "keys.bulk.clear": {
    "zh-CN": "取消选择",
    "zh-TW": "取消選擇",
    "en": "Clear selection",
    "ja": "選択を解除",
    "ko": "선택 해제"
  },
  "keys.bulk.clearCooldown": {
    "zh-CN": "批量清冷却",
    "zh-TW": "批量清冷卻",
    "en": "Bulk clear cooldown",
    "ja": "一括クールダウン解除",
    "ko": "일괄 쿨다운 해제"
  },
  "keys.bulk.confirmDelete": {
    "zh-CN": "确定删掉选中的 {count} 把？这一步不可撤销，而且没停用的那几把后端会直接拒。",
    "zh-TW": "確定刪掉選中的 {count} 把？這一步不可復原，而且沒停用的那幾把後端會直接拒。",
    "en": "Delete the {count} selected account(s)? This cannot be undone, and the backend will refuse any of them that are not disabled yet.",
    "ja": "選択した {count} 件を削除しますか？この操作は取り消せず、無効化していないものはバックエンドが拒否します。",
    "ko": "선택한 {count}개를 삭제할까요? 이 작업은 되돌릴 수 없고, 아직 정지하지 않은 항목은 백엔드가 거부합니다."
  },
  "keys.bulk.confirmTitle": {
    "zh-CN": "确认批量删除",
    "zh-TW": "確認批次刪除",
    "en": "Confirm bulk delete",
    "ja": "一括削除の確認",
    "ko": "일괄 삭제 확인"
  },
  "keys.bulk.count": {
    "zh-CN": "已选中 {n} 把",
    "zh-TW": "已選中 {n} 把",
    "en": "{n} selected",
    "ja": "{n} 件を選択中",
    "ko": "{n}개 선택됨"
  },
  "keys.bulk.countsSuffix": {
    "zh-CN": "（{ok}/{total} 把成功）",
    "zh-TW": "（{ok}/{total} 把成功）",
    "en": " ({ok}/{total} succeeded)",
    "ja": "（{ok}/{total} 件が成功）",
    "ko": " ({ok}/{total}개 성공)"
  },
  "keys.bulk.delete": {
    "zh-CN": "批量删除",
    "zh-TW": "批量刪除",
    "en": "Bulk delete",
    "ja": "一括削除",
    "ko": "일괄 삭제"
  },
  "keys.bulk.disable": {
    "zh-CN": "批量停用",
    "zh-TW": "批量停用",
    "en": "Bulk disable",
    "ja": "一括無効化",
    "ko": "일괄 정지"
  },
  "keys.bulk.mustDisableFirstSuffix": {
    "zh-CN": "；其中 {mustDisableFirst} 把还没停用，删除前得先停用。",
    "zh-TW": "；其中 {mustDisableFirst} 把還沒停用，刪除前得先停用。",
    "en": " {mustDisableFirst} of them are not disabled yet and must be disabled before deletion.",
    "ja": "；うち {mustDisableFirst} 件は無効化されていないため、削除の前に無効化が必要です。",
    "ko": " 그중 {mustDisableFirst}개는 아직 정지되지 않아 삭제 전에 먼저 정지해야 합니다."
  },
  "keys.bulk.notFoundSuffix": {
    "zh-CN": "；另有 {notFound} 把已经不在池子里了。",
    "zh-TW": "；另有 {notFound} 把已經不在池子裡了。",
    "en": " Another {notFound} are no longer in the pool.",
    "ja": "；ほかに {notFound} 件はすでにプールにありません。",
    "ko": " 다른 {notFound}개는 이미 풀에 없습니다."
  },
  "keys.bulk.partial": {
    "zh-CN": "批量操作跑完了，但不是每一把都成功。",
    "zh-TW": "批次操作跑完了，但不是每一把都成功。",
    "en": "The bulk action finished, but not every item succeeded.",
    "ja": "一括操作は完了しましたが、すべてが成功したわけではありません。",
    "ko": "일괄 작업은 끝났지만 전부 성공하지는 않았습니다."
  },
  "keys.card.cooling": {
    "zh-CN": "冷却中",
    "zh-TW": "冷卻中",
    "en": "Cooling down",
    "ja": "クールダウン中",
    "ko": "쿨다운 중"
  },
  "keys.card.disabled": {
    "zh-CN": "已停用",
    "zh-TW": "已停用",
    "en": "Disabled",
    "ja": "無効化済み",
    "ko": "비활성화됨"
  },
  "keys.card.evictedSub": {
    "zh-CN": "另有 {n} 把已剔除",
    "zh-TW": "另有 {n} 把已剔除",
    "en": "{n} more are evicted",
    "ja": "ほかに {n} 件が除外済み",
    "ko": "그 밖에 {n}개가 제외됨"
  },
  "keys.card.fresh": {
    "zh-CN": "可用",
    "zh-TW": "可用",
    "en": "Available",
    "ja": "利用可能",
    "ko": "사용 가능"
  },
  "keys.card.total": {
    "zh-CN": "总数",
    "zh-TW": "總數",
    "en": "Total",
    "ja": "総数",
    "ko": "전체"
  },
  "keys.clearStrikesConfirmMsg": {
    "zh-CN": "这与「清冷却」不是一回事：清冷却只让这把现在能用，它离下一次被剔除仍然只差一次失败；清连续失败才会把那个计数真正清零。",
    "zh-TW": "這與「清冷卻」不是一回事：清冷卻只讓這把現在能用，它離下一次被剔除仍然只差一次失敗；清連續失敗才會把那個計數真正清零。",
    "en": "This is not the same as clearing the cooldown: that only makes the account usable right now, and it is still one failure away from being evicted. Clearing the strikes is what actually resets that counter.",
    "ja": "「クールダウン解除」とは別のことです：クールダウン解除は今すぐ使えるようにするだけで、あと 1 回失敗すれば除外されます。連続失敗をクリアして初めてそのカウンターがゼロに戻ります。",
    "ko": "‘쿨다운 해제’와는 다른 일입니다: 쿨다운 해제는 지금 당장 쓸 수 있게 할 뿐, 한 번만 더 실패하면 제외됩니다. 연속 실패를 지워야 그 카운터가 실제로 0이 됩니다."
  },
  "keys.clearStrikesConfirmTitle": {
    "zh-CN": "清空连续失败计数？",
    "zh-TW": "清空連續失敗計數？",
    "en": "Clear the strike count?",
    "ja": "連続失敗回数をクリアしますか？",
    "ko": "연속 실패 횟수를 지울까요?"
  },
  "keys.deleteConfirmMsg": {
    "zh-CN": "这一步不可撤销。删之前先确认这把已经停用或已被剔除 —— 没停用的后端会直接拒。",
    "zh-TW": "這一步不可復原。刪之前先確認這把已經停用或已被剔除 —— 沒停用的後端會直接拒。",
    "en": "This cannot be undone. Make sure the account is already disabled or evicted — the backend refuses to delete one that is still live.",
    "ja": "この操作は取り消せません。削除する前に、無効化済みまたは除外済みであることを確認してください。有効なままのものはバックエンドが拒否します。",
    "ko": "이 작업은 되돌릴 수 없습니다. 삭제하기 전에 이미 정지되었거나 제외되었는지 확인하세요 — 아직 살아 있는 항목은 백엔드가 거부합니다."
  },
  "keys.deleteConfirmTitle": {
    "zh-CN": "删除这个账号？",
    "zh-TW": "刪除這個帳號？",
    "en": "Delete this account?",
    "ja": "このアカウントを削除しますか？",
    "ko": "이 계정을 삭제할까요?"
  },
  "keys.durationDays": {
    "zh-CN": "{d} 天 {h} 小时",
    "zh-TW": "{d} 天 {h} 小時",
    "en": "{d}d {h}h",
    "ja": "{d}日{h}時間",
    "ko": "{d}일 {h}시간"
  },
  "keys.durationHours": {
    "zh-CN": "{h} 小时 {m} 分",
    "zh-TW": "{h} 小時 {m} 分",
    "en": "{h}h {m}m",
    "ja": "{h}時間{m}分",
    "ko": "{h}시간 {m}분"
  },
  "keys.durationMinutes": {
    "zh-CN": "{m} 分 {s} 秒",
    "zh-TW": "{m} 分 {s} 秒",
    "en": "{m}m {s}s",
    "ja": "{m}分{s}秒",
    "ko": "{m}분 {s}초"
  },
  "keys.empty": {
    "zh-CN": "池子里还没有账号，先去导入一批。",
    "zh-TW": "池子裡還沒有帳號，先去匯入一批。",
    "en": "There is no account in the pool yet — import a batch first.",
    "ja": "プールにはまだアカウントがありません。まずまとめてインポートしてください。",
    "ko": "풀에 아직 계정이 없습니다. 먼저 한 묶음 가져오세요."
  },
  "keys.filterTitle": {
    "zh-CN": "按档位筛选",
    "zh-TW": "依檔位篩選",
    "en": "Filter by bucket",
    "ja": "状態で絞り込み",
    "ko": "상태로 필터"
  },
  "keys.freshnessAge": {
    "zh-CN": "这一屏的数据是 {dur} 前拉的",
    "zh-TW": "這一屏的資料是 {dur} 前拉的",
    "en": "This view was loaded {dur} ago",
    "ja": "この画面のデータは {dur} 前に取得したものです",
    "ko": "이 화면의 데이터는 {dur} 전에 가져온 것입니다"
  },
  "keys.freshnessBound": {
    "zh-CN": "；别的实例判定的冷却 / 剔除最多要 {sec} 秒才会出现在这里。",
    "zh-TW": "；別的實例判定的冷卻 / 剔除最多要 {sec} 秒才會出現在這裡。",
    "en": "; cooldowns and evictions decided by another instance take up to {sec} s to show up here.",
    "ja": "；他のインスタンスが判定したクールダウン／除外がここに出るまで最大 {sec} 秒かかります。",
    "ko": "; 다른 인스턴스가 판정한 쿨다운/제외가 여기에 나타나기까지 최대 {sec}초 걸립니다."
  },
  "keys.freshnessNever": {
    "zh-CN": "这一屏还没有成功拉到过数据",
    "zh-TW": "這一屏還沒有成功拉到過資料",
    "en": "This view has not loaded any data yet",
    "ja": "この画面はまだ一度もデータを取得できていません",
    "ko": "이 화면은 아직 데이터를 가져온 적이 없습니다"
  },
  "keys.import.done": {
    "zh-CN": "已添加 {added} 把，{duplicated} 把重复被跳过，{invalid} 行不是合法的 key。",
    "zh-TW": "已新增 {added} 把，{duplicated} 把重複被跳過，{invalid} 行不是合法的 key。",
    "en": "Added {added}, skipped {duplicated} duplicate(s), {invalid} line(s) are not valid keys.",
    "ja": "{added} 件を追加し、{duplicated} 件の重複をスキップしました。{invalid} 行は有効な key ではありません。",
    "ko": "{added}개를 추가하고 중복 {duplicated}개를 건너뛰었습니다. {invalid}줄은 올바른 key가 아닙니다."
  },
  "keys.import.empty": {
    "zh-CN": "至少粘一把 key 进去。",
    "zh-TW": "至少貼一把 key 進去。",
    "en": "Paste at least one key.",
    "ja": "key を少なくとも 1 つ貼り付けてください。",
    "ko": "key를 적어도 하나는 붙여넣으세요."
  },
  "keys.import.label": {
    "zh-CN": "每行一把 key",
    "zh-TW": "每行一把 key",
    "en": "One key per line",
    "ja": "1 行に 1 つの key",
    "ko": "한 줄에 key 하나"
  },
  "keys.import.placeholder": {
    "zh-CN": "每行一把 key，支持多行粘贴；空行会被跳过",
    "zh-TW": "每行一把 key，支援多行貼上；空行會被跳過",
    "en": "One key per line; multi-line paste works and blank lines are skipped",
    "ja": "1 行に 1 つの key。複数行の貼り付けに対応し、空行はスキップされます",
    "ko": "한 줄에 key 하나. 여러 줄 붙여넣기가 되고 빈 줄은 건너뜁니다"
  },
  "keys.import.reset": {
    "zh-CN": "对已存在的账号也重置冷却 / 连续失败 / 剔除状态",
    "zh-TW": "對已存在的帳號也重設冷卻 / 連續失敗 / 剔除狀態",
    "en": "Also reset cooldown, strikes and eviction for accounts that already exist",
    "ja": "既存のアカウントもクールダウン・連続失敗・除外状態をリセットする",
    "ko": "이미 있는 계정도 쿨다운·연속 실패·제외 상태를 재설정"
  },
  "keys.import.resetHint": {
    "zh-CN": "勾上之后，重新导入就成了一条能绕过「已剔除」的路径，后端会为此单打一条 warn 事件。",
    "zh-TW": "勾上之後，重新匯入就成了一條能繞過「已剔除」的路徑，後端會為此單打一條 warn 事件。",
    "en": "With this ticked, re-importing becomes a way to bypass eviction, and the backend records a separate warn event for it.",
    "ja": "これを有効にすると、再インポートが「除外済み」を回避する経路になり、バックエンドはそのために warn イベントを 1 件記録します。",
    "ko": "이 항목을 켜면 다시 가져오기가 ‘제외됨’을 우회하는 경로가 되며, 백엔드는 이를 위해 warn 이벤트를 따로 기록합니다."
  },
  "keys.import.resetSuffix": {
    "zh-CN": "其中 {reset} 把被重置了状态。",
    "zh-TW": "其中 {reset} 把被重設了狀態。",
    "en": " {reset} of them had their state reset.",
    "ja": "うち {reset} 件は状態をリセットしました。",
    "ko": " 그중 {reset}개는 상태를 재설정했습니다."
  },
  "keys.import.submit": {
    "zh-CN": "导入",
    "zh-TW": "匯入",
    "en": "Import",
    "ja": "インポート",
    "ko": "가져오기"
  },
  "keys.import.title": {
    "zh-CN": "粘贴导入账号",
    "zh-TW": "貼上匯入帳號",
    "en": "Paste and import accounts",
    "ja": "アカウントを貼り付けてインポート",
    "ko": "계정 붙여넣어 가져오기"
  },
  "keys.loadFailed": {
    "zh-CN": "这一次没读到账号池 —— 这不是「池子空了」。",
    "zh-TW": "這一次沒讀到帳號池 —— 這不是「池子空了」。",
    "en": "The account pool could not be read this time — that is not the same as an empty pool.",
    "ja": "今回はアカウントプールを読み取れませんでした。「プールが空」とは別のことです。",
    "ko": "이번에는 계정 풀을 읽지 못했습니다 — ‘풀이 비었다’와는 다른 일입니다."
  },
  "keys.loading": {
    "zh-CN": "正在读取…",
    "zh-TW": "正在讀取…",
    "en": "Loading…",
    "ja": "読み込み中…",
    "ko": "불러오는 중…"
  },
  "keys.meta.added": {
    "zh-CN": "加入于 {when}",
    "zh-TW": "加入於 {when}",
    "en": "Added {when}",
    "ja": "追加 {when}",
    "ko": "추가 {when}"
  },
  "keys.meta.lastUsed": {
    "zh-CN": "最后使用 {when}",
    "zh-TW": "最後使用 {when}",
    "en": "Last used {when}",
    "ja": "最終使用 {when}",
    "ko": "마지막 사용 {when}"
  },
  "keys.meta.neverUsed": {
    "zh-CN": "还没用过",
    "zh-TW": "還沒用過",
    "en": "Never used",
    "ja": "未使用",
    "ko": "사용한 적 없음"
  },
  "keys.meta.note": {
    "zh-CN": "备注：",
    "zh-TW": "備註：",
    "en": "Note: ",
    "ja": "備考：",
    "ko": "비고: "
  },
  "keys.mustDisableFirst": {
    "zh-CN": "先停用这把再删（删除不可撤销，而停用随时能撤回）",
    "zh-TW": "先停用這把再刪（刪除不可復原，而停用隨時能撤回）",
    "en": "Disable this account before deleting it — deletion is permanent, disabling can be undone anytime",
    "ja": "削除する前に無効化してください（削除は取り消せませんが、無効化はいつでも戻せます）",
    "ko": "삭제하기 전에 먼저 정지하세요 — 삭제는 되돌릴 수 없지만 정지는 언제든 되돌릴 수 있습니다"
  },
  "keys.noMatch": {
    "zh-CN": "没有符合当前筛选条件的账号。",
    "zh-TW": "沒有符合目前篩選條件的帳號。",
    "en": "No account matches the current filter.",
    "ja": "現在の絞り込み条件に一致するアカウントはありません。",
    "ko": "현재 필터 조건에 맞는 계정이 없습니다."
  },
  "keys.noneHint": {
    "zh-CN": "这一次读成功了，这一行就是不在冷却。",
    "zh-TW": "這一次讀成功了，這一行就是不在冷卻。",
    "en": "The read succeeded — this row simply is not cooling down.",
    "ja": "読み取りは成功しています。この行はクールダウン中ではありません。",
    "ko": "읽기는 성공했습니다 — 이 행은 쿨다운 중이 아닙니다."
  },
  "keys.note.hint": {
    "zh-CN": "最多 {max} 个字符；清空表示删掉备注。",
    "zh-TW": "最多 {max} 個字元；清空表示刪掉備註。",
    "en": "At most {max} characters; leaving it empty clears the note.",
    "ja": "最大 {max} 文字。空にすると備考を削除します。",
    "ko": "최대 {max}자. 비우면 비고를 지웁니다."
  },
  "keys.note.label": {
    "zh-CN": "备注内容",
    "zh-TW": "備註內容",
    "en": "Note text",
    "ja": "備考の内容",
    "ko": "비고 내용"
  },
  "keys.note.save": {
    "zh-CN": "保存",
    "zh-TW": "儲存",
    "en": "Save",
    "ja": "保存する",
    "ko": "저장"
  },
  "keys.note.title": {
    "zh-CN": "编辑备注",
    "zh-TW": "編輯備註",
    "en": "Edit note",
    "ja": "備考を編集",
    "ko": "비고 편집"
  },
  "keys.pager.info": {
    "zh-CN": "第 {page}/{pages} 页 · 共 {total} 条",
    "zh-TW": "第 {page}/{pages} 頁 · 共 {total} 筆",
    "en": "Page {page} of {pages} · {total} in total",
    "ja": "{pages} ページ中 {page} ページ目 · 全 {total} 件",
    "ko": "{pages}페이지 중 {page}페이지 · 총 {total}개"
  },
  "keys.pager.next": {
    "zh-CN": "下一页",
    "zh-TW": "下一頁",
    "en": "Next",
    "ja": "次へ",
    "ko": "다음"
  },
  "keys.pager.prev": {
    "zh-CN": "上一页",
    "zh-TW": "上一頁",
    "en": "Previous",
    "ja": "前へ",
    "ko": "이전"
  },
  "keys.reason.cooldown": {
    "zh-CN": "冷却原因：{reason}",
    "zh-TW": "冷卻原因：{reason}",
    "en": "Cooling because: {reason}",
    "ja": "クールダウンの理由：{reason}",
    "ko": "쿨다운 사유: {reason}"
  },
  "keys.reason.evicted": {
    "zh-CN": "剔除原因：{reason}",
    "zh-TW": "剔除原因：{reason}",
    "en": "Evicted because: {reason}",
    "ja": "除外の理由：{reason}",
    "ko": "제외 사유: {reason}"
  },
  "keys.refresh": {
    "zh-CN": "刷新",
    "zh-TW": "重新整理",
    "en": "Refresh",
    "ja": "更新",
    "ko": "새로 고침"
  },
  "keys.reveal.failed": {
    "zh-CN": "这一次没取到明文，重试可能有用。",
    "zh-TW": "這一次沒取到明文，重試可能有用。",
    "en": "The plaintext did not come back this time; retrying may help.",
    "ja": "今回は平文を取得できませんでした。再試行が有効かもしれません。",
    "ko": "이번에는 평문을 받지 못했습니다. 다시 시도하면 될 수 있습니다."
  },
  "keys.reveal.gone": {
    "zh-CN": "这一行已经不在池子里了 —— 刷新一次列表。",
    "zh-TW": "這一行已經不在池子裡了 —— 重新整理一次列表。",
    "en": "This row is no longer in the pool — refresh the list.",
    "ja": "この行はすでにプールにありません。一覧を再読み込みしてください。",
    "ko": "이 행은 더 이상 풀에 없습니다 — 목록을 새로 고치세요."
  },
  "keys.reveal.unavailable": {
    "zh-CN": "这个部署关掉了明文回显。",
    "zh-TW": "這個部署關掉了明文回顯。",
    "en": "This deployment has plaintext reveal turned off.",
    "ja": "このデプロイでは平文の表示が無効になっています。",
    "ko": "이 배포는 평문 표시를 꺼 두었습니다."
  },
  "keys.search": {
    "zh-CN": "搜索 id 或掩码",
    "zh-TW": "搜尋 id 或遮罩",
    "en": "Search by id or masked key",
    "ja": "id またはマスクで検索",
    "ko": "id 또는 마스크로 검색"
  },
  "keys.selectAll": {
    "zh-CN": "全选本页",
    "zh-TW": "全選本頁",
    "en": "Select all on this page",
    "ja": "このページをすべて選択",
    "ko": "이 페이지 전체 선택"
  },
  "keys.selectRow": {
    "zh-CN": "选择这一行",
    "zh-TW": "選擇這一行",
    "en": "Select this row",
    "ja": "この行を選択",
    "ko": "이 행 선택"
  },
  "keys.serverLabelUnknown": {
    "zh-CN": "后端给了一个这个面板还不认识的标签，请核对两边的版本。",
    "zh-TW": "後端給了一個這個面板還不認識的標籤，請核對兩邊的版本。",
    "en": "The backend sent a label this panel does not recognise yet — check that both sides are on the same version.",
    "ja": "この画面がまだ認識できないラベルがバックエンドから返りました。両者のバージョンを確認してください。",
    "ko": "이 콘솔이 아직 알지 못하는 라벨을 백엔드가 보냈습니다. 양쪽 버전을 확인하세요."
  },
  "keys.stat.clientErrors": {
    "zh-CN": "客户端 4xx",
    "zh-TW": "用戶端 4xx",
    "en": "Client 4xx",
    "ja": "クライアント 4xx",
    "ko": "클라이언트 4xx"
  },
  "keys.stat.cooldown": {
    "zh-CN": "冷却剩余",
    "zh-TW": "冷卻剩餘",
    "en": "Cooldown left",
    "ja": "クールダウン残り",
    "ko": "쿨다운 잔여"
  },
  "keys.stat.failed": {
    "zh-CN": "失败",
    "zh-TW": "失敗",
    "en": "Failed",
    "ja": "失敗",
    "ko": "실패"
  },
  "keys.stat.requests": {
    "zh-CN": "请求",
    "zh-TW": "請求",
    "en": "Requests",
    "ja": "リクエスト",
    "ko": "요청"
  },
  "keys.stat.strikes": {
    "zh-CN": "连续失败",
    "zh-TW": "連續失敗",
    "en": "Strikes",
    "ja": "連続失敗",
    "ko": "연속 실패"
  },
  "keys.stat.success": {
    "zh-CN": "成功",
    "zh-TW": "成功",
    "en": "Success",
    "ja": "成功件数",
    "ko": "성공"
  },
  "keys.title": {
    "zh-CN": "账号池",
    "zh-TW": "帳號池",
    "en": "Account pool",
    "ja": "アカウントプール",
    "ko": "계정 풀"
  },
  "keys.unknownHint": {
    "zh-CN": "这一格压根没读出来 —— 这是「不知道」，不是 0。",
    "zh-TW": "這一格壓根沒讀出來 —— 這是「不知道」，不是 0。",
    "en": "This value could not be read at all — it means “unknown”, not zero.",
    "ja": "この値はまったく読み取れませんでした —— 「不明」であって 0 ではありません。",
    "ko": "이 값은 전혀 읽지 못했습니다 — “알 수 없음”이지 0이 아닙니다."
  },
  "keys.verify.disabledCoolingDown": {
    "zh-CN": "刚探过，隔一小段时间再试。",
    "zh-TW": "剛探過，隔一小段時間再試。",
    "en": "Just probed; try again in a moment.",
    "ja": "たった今確認したばかりです。少し待ってから再試行してください。",
    "ko": "방금 탐지했습니다. 잠시 후 다시 시도하세요."
  },
  "keys.verify.disabledInFlight": {
    "zh-CN": "上一次探测还在飞，等它回来。",
    "zh-TW": "上一次探測還在飛，等它回來。",
    "en": "The previous probe is still in flight; wait for it to come back.",
    "ja": "前回のプローブがまだ実行中です。戻るまでお待ちください。",
    "ko": "이전 탐지가 아직 진행 중입니다. 돌아올 때까지 기다리세요."
  },
  "keys.verify.disabledNoKey": {
    "zh-CN": "这一行没有可用的账号标识，点不了。",
    "zh-TW": "這一行沒有可用的帳號識別碼，點不了。",
    "en": "This row has no usable account id, so the button does nothing.",
    "ja": "この行には利用できるアカウント識別子がないため、押せません。",
    "ko": "이 행에는 쓸 수 있는 계정 식별자가 없어 누를 수 없습니다."
  },
  "keys.verify.hintEnabled": {
    "zh-CN": "按一下会真的向上游发一次请求。",
    "zh-TW": "按一下會真的向上游發一次請求。",
    "en": "Each click really does send one request upstream.",
    "ja": "クリックするたびに上流へ実際にリクエストを 1 回送ります。",
    "ko": "누를 때마다 업스트림으로 실제 요청을 한 번 보냅니다."
  },
  "keys.verify.keyNotFound": {
    "zh-CN": "这个账号已经不在池子里了，刷新一下列表。",
    "zh-TW": "這個帳號已經不在池子裡了，重新整理一下列表。",
    "en": "This account is no longer in the pool — refresh the list.",
    "ja": "このアカウントはすでにプールにありません。一覧を再読み込みしてください。",
    "ko": "이 계정은 더 이상 풀에 없습니다. 목록을 새로 고치세요."
  },
  "keys.verify.networkError": {
    "zh-CN": "连不上上游：这次探测没有拿到任何响应。",
    "zh-TW": "連不上上游：這次探測沒有拿到任何回應。",
    "en": "Cannot reach upstream: this probe got no response at all.",
    "ja": "上流に到達できません：今回のプローブは応答をまったく受け取れませんでした。",
    "ko": "업스트림에 연결할 수 없습니다: 이번 탐지는 어떤 응답도 받지 못했습니다."
  },
  "keys.verify.ok": {
    "zh-CN": "验活通过：上游接受了这个账号。",
    "zh-TW": "驗活通過：上游接受了這個帳號。",
    "en": "Verified: upstream accepted this account.",
    "ja": "確認できました：上流はこのアカウントを受け付けました。",
    "ko": "확인 완료: 업스트림이 이 계정을 받아들였습니다."
  },
  "keys.verify.probeCooldown": {
    "zh-CN": "两次探测之间至少要隔一小段时间，稍后再试。这不是这个账号的故障。",
    "zh-TW": "兩次探測之間至少要隔一小段時間，稍後再試。這不是這個帳號的故障。",
    "en": "There is a minimum interval between two probes; retry shortly. This is not a fault of the account.",
    "ja": "プローブとプローブの間には最小間隔があります。少し待ってから再試行してください。アカウントの障害ではありません。",
    "ko": "탐지와 탐지 사이에는 최소 간격이 있습니다. 잠시 후 다시 시도하세요. 계정의 고장이 아닙니다."
  },
  "keys.verify.probeInFlight": {
    "zh-CN": "上一次探测还没回来，等它结束再试 —— 这颗按钮每按一次都会真的打一次上游。",
    "zh-TW": "上一次探測還沒回來，等它結束再試 —— 這顆按鈕每按一次都會真的打一次上游。",
    "en": "The previous probe has not returned yet; wait for it to finish before retrying — each click really does hit upstream.",
    "ja": "前回のプローブがまだ返っていません。終わってから再試行してください。このボタンは押すたびに上流へ実際に送ります。",
    "ko": "이전 탐지가 아직 돌아오지 않았습니다. 끝난 뒤에 다시 시도하세요 — 이 버튼은 누를 때마다 실제로 업스트림을 호출합니다."
  },
  "keys.verify.rateLimited": {
    "zh-CN": "上游正在限流（429）：这个账号本身没问题，等一会儿再用。",
    "zh-TW": "上游正在限流（429）：這個帳號本身沒問題，等一會兒再用。",
    "en": "Upstream is rate limiting (429): the account itself is fine — wait a while.",
    "ja": "上流がレート制限中です（429）：アカウント自体に問題はありません。しばらく待ってください。",
    "ko": "업스트림이 속도를 제한하고 있습니다(429): 계정 자체는 문제없으니 잠시 기다리세요."
  },
  "keys.verify.readOnlyNote": {
    "zh-CN": "这是一次人造探测，不改这个账号的任何状态：连续失败、冷却、用量统计都不会动。",
    "zh-TW": "這是一次人造探測，不改這個帳號的任何狀態：連續失敗、冷卻、用量統計都不會動。",
    "en": "This is a synthetic probe. It changes none of the account's state — strikes, cooldown and usage stats all stay untouched.",
    "ja": "これは人工的なプローブです。このアカウントの状態は変わりません（連続失敗・クールダウン・利用統計はいずれも動きません）。",
    "ko": "이것은 인위적인 탐지입니다. 이 계정의 상태는 바뀌지 않습니다 — 연속 실패, 쿨다운, 사용량 통계 모두 그대로입니다."
  },
  "keys.verify.running": {
    "zh-CN": "正在探测…",
    "zh-TW": "正在探測…",
    "en": "Probing…",
    "ja": "確認中…",
    "ko": "확인 중…"
  },
  "keys.verify.timeout": {
    "zh-CN": "探测超时：上游在超时档内没有返回响应头。",
    "zh-TW": "探測逾時：上游在逾時檔內沒有返回回應標頭。",
    "en": "Probe timed out: upstream did not return response headers within the timeout budget.",
    "ja": "プローブがタイムアウトしました：上流はタイムアウト時間内にレスポンスヘッダーを返しませんでした。",
    "ko": "탐지 시간 초과: 업스트림이 제한 시간 안에 응답 헤더를 반환하지 않았습니다."
  },
  "keys.verify.transportError": {
    "zh-CN": "这次探测没有发出去：请求本身失败了，与上游状态无关。",
    "zh-TW": "這次探測沒有發出去：請求本身失敗了，與上游狀態無關。",
    "en": "This probe never left the panel: the request itself failed, which says nothing about upstream.",
    "ja": "今回のプローブは送信されていません：リクエスト自体が失敗しており、上流の状態とは無関係です。",
    "ko": "이번 탐지는 전송되지 않았습니다: 요청 자체가 실패한 것이며 업스트림 상태와는 무관합니다."
  },
  "keys.verify.unauthorized": {
    "zh-CN": "上游拒绝了这个账号（401/403）：它已经失效，换一个。",
    "zh-TW": "上游拒絕了這個帳號（401/403）：它已經失效，換一個。",
    "en": "Upstream rejected this account (401/403): it is no longer valid — replace it.",
    "ja": "上流がこのアカウントを拒否しました（401/403）：すでに無効です。差し替えてください。",
    "ko": "업스트림이 이 계정을 거부했습니다(401/403): 이미 무효이므로 교체하세요."
  },
  "keys.verify.unauthorizedAdmin": {
    "zh-CN": "管理会话已失效，请重新输入管理口令。",
    "zh-TW": "管理工作階段已失效，請重新輸入管理口令。",
    "en": "The admin session has expired; enter the admin token again.",
    "ja": "管理セッションの有効期限が切れました。管理トークンを入力し直してください。",
    "ko": "관리 세션이 만료되었습니다. 관리 토큰을 다시 입력하세요."
  },
  "keys.verify.unknownReason": {
    "zh-CN": "后端给出了这个面板还不认识的一种探测结果，请核对两边的版本。",
    "zh-TW": "後端給出了這個面板還不認識的一種探測結果，請核對兩邊的版本。",
    "en": "The backend reported a probe outcome this panel does not recognise yet — check that both sides are on the same version.",
    "ja": "この画面がまだ認識できない種類の結果がバックエンドから返りました。両者のバージョンを確認してください。",
    "ko": "이 콘솔이 아직 인식하지 못하는 종류의 결과를 백엔드가 반환했습니다. 양쪽 버전을 확인하세요."
  },
  "keys.verify.upstreamError": {
    "zh-CN": "上游回了一个我们没细分的错误状态码：这次探测没能确认这个账号是好是坏。",
    "zh-TW": "上游回了一個我們沒細分的錯誤狀態碼：這次探測沒能確認這個帳號是好是壞。",
    "en": "Upstream returned an error status we do not classify further: this probe could not tell whether the account is good.",
    "ja": "上流が細分していないエラーステータスを返しました：このアカウントが有効かどうかは今回の確認では判断できません。",
    "ko": "업스트림이 세분화하지 않은 오류 상태 코드를 반환했습니다: 이번 탐지로는 이 계정이 정상인지 판단할 수 없습니다."
  },
  "section.notWired": {
    "zh-CN": "这个板块还没接线（P2b 分期交付，见 docs/porting/06-panel-spec.md §9 R-8）",
    "zh-TW": "這個板塊還沒接線（P2b 分期交付，見 docs/porting/06-panel-spec.md §9 R-8）",
    "en": "This section is not wired yet (staged delivery, see docs/porting/06-panel-spec.md 9 R-8)",
    "ja": "このセクションはまだ接続されていません（段階的な提供、docs/porting/06-panel-spec.md 9 R-8 を参照）",
    "ko": "이 섹션은 아직 연결되지 않았습니다(단계적 제공, docs/porting/06-panel-spec.md 9 R-8 참조)"
  },
  "section.ak.action.copyInfo": {
    "zh-CN": "复制地址与掩码",
    "zh-TW": "複製位址與遮罩",
    "en": "Copy endpoint and masked key",
    "ja": "接続先とマスク済みキーをコピー",
    "ko": "주소와 마스킹된 키 복사"
  },
  "section.ak.action.delete": {
    "zh-CN": "吊销这把",
    "zh-TW": "吊銷這把",
    "en": "Revoke this key",
    "ja": "このキーを失効させる",
    "ko": "이 키 폐기"
  },
  "section.ak.action.disable": {
    "zh-CN": "停用这把",
    "zh-TW": "停用這把",
    "en": "Disable this key",
    "ja": "このキーを無効にする",
    "ko": "이 키 비활성화"
  },
  "section.ak.action.edit": {
    "zh-CN": "改名 / 改到期",
    "zh-TW": "改名 / 改到期",
    "en": "Rename or change expiry",
    "ja": "名前・有効期限を変更",
    "ko": "이름·만료 변경"
  },
  "section.ak.action.enable": {
    "zh-CN": "启用这把",
    "zh-TW": "啟用這把",
    "en": "Enable this key",
    "ja": "このキーを有効にする",
    "ko": "이 키 활성화"
  },
  "section.ak.bucket.active": {
    "zh-CN": "可用",
    "zh-TW": "可用",
    "en": "Active",
    "ja": "有効",
    "ko": "사용 가능"
  },
  "section.ak.bucket.disabled": {
    "zh-CN": "已停用",
    "zh-TW": "已停用",
    "en": "Disabled",
    "ja": "無効化",
    "ko": "비활성"
  },
  "section.ak.bucket.expired": {
    "zh-CN": "已过期",
    "zh-TW": "已過期",
    "en": "Expired",
    "ja": "期限切れ",
    "ko": "만료됨"
  },
  "section.ak.copyBlock": {
    "zh-CN": "网关地址：{url}\n密钥（掩码）：{key}",
    "zh-TW": "閘道位址：{url}\n金鑰（遮罩）：{key}",
    "en": "Gateway: {url}\nKey (masked): {key}",
    "ja": "ゲートウェイ：{url}\nキー（マスク）：{key}",
    "ko": "게이트웨이: {url}\n키(마스킹): {key}"
  },
  "section.ak.deleteConfirm": {
    "zh-CN": "吊销之后拿着它的客户端会开始被拒。这一步不可撤销。",
    "zh-TW": "吊銷之後拿著它的客戶端會開始被拒。這一步不可撤銷。",
    "en": "Once revoked, clients holding it start getting rejected. This cannot be undone.",
    "ja": "失効させると、それを持つクライアントは拒否され始めます。取り消せません。",
    "ko": "폐기하면 이 키를 가진 클라이언트가 거부되기 시작합니다. 되돌릴 수 없습니다."
  },
  "section.ak.deleteTitle": {
    "zh-CN": "吊销这把密钥",
    "zh-TW": "吊銷這把金鑰",
    "en": "Revoke this key",
    "ja": "このキーを失効させる",
    "ko": "이 키 폐기"
  },
  "section.ak.deleted": {
    "zh-CN": "已吊销",
    "zh-TW": "已吊銷",
    "en": "Key revoked",
    "ja": "キーを失効させました",
    "ko": "키를 폐기했습니다"
  },
  "section.ak.disabled": {
    "zh-CN": "已停用",
    "zh-TW": "已停用",
    "en": "Key disabled",
    "ja": "キーを無効化しました",
    "ko": "키를 비활성화했습니다"
  },
  "section.ak.dur.minutes": {
    "zh-CN": "{n} 分钟",
    "zh-TW": "{n} 分鐘",
    "en": "{n} min",
    "ja": "{n} 分間",
    "ko": "{n}분"
  },
  "section.ak.dur.seconds": {
    "zh-CN": "{n} 秒",
    "zh-TW": "{n} 秒",
    "en": "{n} s",
    "ja": "{n} 秒間",
    "ko": "{n}초"
  },
  "section.ak.editDesc": {
    "zh-CN": "改名与改到期都不会换掉密钥本身，下游客户端不必改配置。",
    "zh-TW": "改名與改到期都不會換掉金鑰本身，下游客戶端不必改設定。",
    "en": "Neither renaming nor changing the expiry replaces the key itself; downstream clients need no config change.",
    "ja": "名前や有効期限を変えてもキー自体は変わりません。下流のクライアントは設定を変える必要がありません。",
    "ko": "이름이나 만료를 바꿔도 키 자체는 바뀌지 않습니다. 다운스트림 클라이언트는 설정을 바꿀 필요가 없습니다."
  },
  "section.ak.editTitle": {
    "zh-CN": "改名 / 改到期",
    "zh-TW": "改名 / 改到期",
    "en": "Rename or change expiry",
    "ja": "名前・有効期限の変更",
    "ko": "이름·만료 변경"
  },
  "section.ak.empty": {
    "zh-CN": "还没有签发过任何密钥。",
    "zh-TW": "還沒有簽發過任何金鑰。",
    "en": "No key has been issued yet.",
    "ja": "まだキーを発行していません。",
    "ko": "아직 발급한 키가 없습니다."
  },
  "section.ak.emptySearch": {
    "zh-CN": "没有匹配的密钥。",
    "zh-TW": "沒有符合的金鑰。",
    "en": "No key matches that search.",
    "ja": "検索条件に一致するキーはありません。",
    "ko": "검색 조건에 맞는 키가 없습니다."
  },
  "section.ak.enabled": {
    "zh-CN": "已启用",
    "zh-TW": "已啟用",
    "en": "Key enabled",
    "ja": "キーを有効化しました",
    "ko": "키를 활성화했습니다"
  },
  "section.ak.err.badExpiry": {
    "zh-CN": "这个到期时刻后端不收。",
    "zh-TW": "這個到期時刻後端不收。",
    "en": "The backend refuses that expiry.",
    "ja": "その有効期限はバックエンドが受け付けません。",
    "ko": "백엔드가 그 만료 시각을 받지 않습니다."
  },
  "section.ak.err.badName": {
    "zh-CN": "这个名称后端不收。",
    "zh-TW": "這個名稱後端不收。",
    "en": "The backend refuses that name.",
    "ja": "その名前はバックエンドが受け付けません。",
    "ko": "백엔드가 그 이름을 받지 않습니다."
  },
  "section.ak.err.expiryBad": {
    "zh-CN": "这个日期读不出来。",
    "zh-TW": "這個日期讀不出來。",
    "en": "That date could not be read.",
    "ja": "その日付を解釈できません。",
    "ko": "그 날짜를 해석할 수 없습니다."
  },
  "section.ak.err.expiryPast": {
    "zh-CN": "到期时刻必须在将来。",
    "zh-TW": "到期時刻必須在將來。",
    "en": "The expiry must be in the future.",
    "ja": "有効期限は未来である必要があります。",
    "ko": "만료 시각은 미래여야 합니다."
  },
  "section.ak.err.nameEmpty": {
    "zh-CN": "名称不能为空。",
    "zh-TW": "名稱不能為空。",
    "en": "The name cannot be empty.",
    "ja": "名前は空にできません。",
    "ko": "이름은 비워 둘 수 없습니다."
  },
  "section.ak.err.nameTooLong": {
    "zh-CN": "名称最多 {max} 个字符。",
    "zh-TW": "名稱最多 {max} 個字元。",
    "en": "The name may be at most {max} characters.",
    "ja": "名前は最大 {max} 文字です。",
    "ko": "이름은 최대 {max}자입니다."
  },
  "section.ak.err.notFound": {
    "zh-CN": "没有这把密钥 —— 它可能在别处被删掉了。",
    "zh-TW": "沒有這把金鑰 —— 它可能在別處被刪掉了。",
    "en": "No such key — it may have been deleted elsewhere.",
    "ja": "そのキーはありません。別の場所で削除された可能性があります。",
    "ko": "그런 키가 없습니다 — 다른 곳에서 삭제되었을 수 있습니다."
  },
  "section.ak.err.stale": {
    "zh-CN": "有人在你之前改过这张表，刷新一次再来。",
    "zh-TW": "有人在你之前改過這張表，重新整理一次再來。",
    "en": "Someone changed this table before you; refresh and try again.",
    "ja": "先に誰かがこの一覧を変更しました。再読み込みしてからやり直してください。",
    "ko": "누군가 먼저 이 목록을 변경했습니다. 새로 고친 뒤 다시 시도하세요."
  },
  "section.ak.err.tooMany": {
    "zh-CN": "已经到上限了，先清理几把再签发。",
    "zh-TW": "已經到上限了，先清理幾把再簽發。",
    "en": "The cap is reached; purge a few before issuing more.",
    "ja": "上限に達しています。いくつか整理してから発行してください。",
    "ko": "상한에 도달했습니다. 몇 개를 정리한 뒤 발급하세요."
  },
  "section.ak.err.unreadable": {
    "zh-CN": "存储里那张表结构不认，本次写入已被拒绝。",
    "zh-TW": "儲存裡那張表結構不認，本次寫入已被拒絕。",
    "en": "The table in storage has an unrecognised shape; this write was refused.",
    "ja": "ストレージ上の一覧の形式が認識できないため、この書き込みは拒否されました。",
    "ko": "저장소의 목록 형식을 인식할 수 없어 이번 쓰기는 거부되었습니다."
  },
  "section.ak.expiry.custom": {
    "zh-CN": "指定日期",
    "zh-TW": "指定日期",
    "en": "Pick a date",
    "ja": "日付を指定",
    "ko": "날짜 지정"
  },
  "section.ak.expiry.d30": {
    "zh-CN": "自签发起 30 天",
    "zh-TW": "自簽發起 30 天",
    "en": "30 days from issue",
    "ja": "発行から 30 日",
    "ko": "발급 후 30일"
  },
  "section.ak.expiry.d7": {
    "zh-CN": "自签发起 7 天",
    "zh-TW": "自簽發起 7 天",
    "en": "7 days from issue",
    "ja": "発行から 7 日",
    "ko": "발급 후 7일"
  },
  "section.ak.expiry.d90": {
    "zh-CN": "自签发起 90 天",
    "zh-TW": "自簽發起 90 天",
    "en": "90 days from issue",
    "ja": "発行から 90 日",
    "ko": "발급 후 90일"
  },
  "section.ak.expiry.never": {
    "zh-CN": "不过期",
    "zh-TW": "不過期",
    "en": "Never",
    "ja": "無期限",
    "ko": "없음"
  },
  "section.ak.form.expiry": {
    "zh-CN": "到期",
    "zh-TW": "到期",
    "en": "Expiry",
    "ja": "有効期限",
    "ko": "만료"
  },
  "section.ak.form.name": {
    "zh-CN": "名称",
    "zh-TW": "名稱",
    "en": "Name",
    "ja": "名前",
    "ko": "이름"
  },
  "section.ak.form.namePh": {
    "zh-CN": "给这把密钥起个认得出来的名字",
    "zh-TW": "給這把金鑰起個認得出來的名字",
    "en": "A name you will recognise later",
    "ja": "後から分かる名前を付けてください",
    "ko": "나중에 알아볼 수 있는 이름"
  },
  "section.ak.form.onceOnly": {
    "zh-CN": "签发是追加操作，不会动到已经发出去的任何一把。",
    "zh-TW": "簽發是追加操作，不會動到已經發出去的任何一把。",
    "en": "Issuing only appends; it touches none of the keys already handed out.",
    "ja": "発行は追加のみで、すでに配ったキーには影響しません。",
    "ko": "발급은 추가만 하며 이미 배포한 키에는 영향을 주지 않습니다."
  },
  "section.ak.issue": {
    "zh-CN": "签发一把",
    "zh-TW": "簽發一把",
    "en": "Issue a key",
    "ja": "キーを発行",
    "ko": "키 발급"
  },
  "section.ak.issueDesc": {
    "zh-CN": "明文只在下一屏出现一次，之后能不能再取回由这个部署的存法决定。",
    "zh-TW": "明文只在下一屏出現一次，之後能不能再取回由這個部署的存法決定。",
    "en": "The plaintext appears once on the next screen; whether it can be fetched again depends on how this deployment stores it.",
    "ja": "平文は次の画面で一度だけ表示されます。後から取り出せるかはこのデプロイの保存方法次第です。",
    "ko": "평문은 다음 화면에 한 번만 표시됩니다. 이후 다시 가져올 수 있는지는 이 배포의 저장 방식에 달려 있습니다."
  },
  "section.ak.issueSubmit": {
    "zh-CN": "签发",
    "zh-TW": "簽發",
    "en": "Issue",
    "ja": "発行する",
    "ko": "발급"
  },
  "section.ak.issueTitle": {
    "zh-CN": "签发一把新密钥",
    "zh-TW": "簽發一把新金鑰",
    "en": "Issue a new key",
    "ja": "新しいキーを発行",
    "ko": "새 키 발급"
  },
  "section.ak.issued": {
    "zh-CN": "已签发",
    "zh-TW": "已簽發",
    "en": "Key issued",
    "ja": "キーを発行しました",
    "ko": "키를 발급했습니다"
  },
  "section.ak.listTitle": {
    "zh-CN": "API 密钥",
    "zh-TW": "API 金鑰",
    "en": "API keys",
    "ja": "API キー",
    "ko": "API 키"
  },
  "section.ak.loadFailed": {
    "zh-CN": "这一次没读到密钥表。",
    "zh-TW": "這一次沒讀到金鑰表。",
    "en": "The key table could not be read this time.",
    "ja": "今回はキー一覧を読み取れませんでした。",
    "ko": "이번에는 키 목록을 읽지 못했습니다."
  },
  "section.ak.meta.created": {
    "zh-CN": "签发于",
    "zh-TW": "簽發於",
    "en": "Issued",
    "ja": "発行",
    "ko": "발급"
  },
  "section.ak.meta.expiresAt": {
    "zh-CN": "到期",
    "zh-TW": "到期",
    "en": "Expires",
    "ja": "有効期限",
    "ko": "만료"
  },
  "section.ak.meta.expiresNever": {
    "zh-CN": "不过期",
    "zh-TW": "不過期",
    "en": "Never expires",
    "ja": "無期限",
    "ko": "만료 없음"
  },
  "section.ak.notWired": {
    "zh-CN": "这个部署没有接上对外 API 密钥的存储 —— 这与「一把都没签发过」不是一回事。",
    "zh-TW": "這個部署沒有接上對外 API 金鑰的儲存 —— 這與「一把都沒簽發過」不是一回事。",
    "en": "This deployment has no storage wired for client API keys — that is not the same as “no key has ever been issued”.",
    "ja": "このデプロイにはクライアント API キー用のストレージが接続されていません。「1 本も発行していない」とは別のことです。",
    "ko": "이 배포에는 클라이언트 API 키 저장소가 연결되어 있지 않습니다 — “한 번도 발급하지 않았다”와는 다릅니다."
  },
  "section.ak.purge": {
    "zh-CN": "清理失效（{n}）",
    "zh-TW": "清理失效（{n}）",
    "en": "Purge unusable ({n})",
    "ja": "無効を整理（{n}）",
    "ko": "무효 정리({n})"
  },
  "section.ak.purgeConfirm": {
    "zh-CN": "全部删掉",
    "zh-TW": "全部刪掉",
    "en": "Delete them all",
    "ja": "すべて削除",
    "ko": "모두 삭제"
  },
  "section.ak.purgeDesc": {
    "zh-CN": "这一次会删掉下面这 {n} 把（已停用或已过期）。",
    "zh-TW": "這一次會刪掉下面這 {n} 把（已停用或已過期）。",
    "en": "This will delete the {n} keys listed below (disabled or expired).",
    "ja": "以下の {n} 本（無効化済みまたは期限切れ）を削除します。",
    "ko": "아래 {n}개(비활성 또는 만료)를 삭제합니다."
  },
  "section.ak.purgeTitle": {
    "zh-CN": "清理失效密钥",
    "zh-TW": "清理失效金鑰",
    "en": "Purge unusable keys",
    "ja": "無効なキーを整理",
    "ko": "무효 키 정리"
  },
  "section.ak.purged": {
    "zh-CN": "已清理",
    "zh-TW": "已清理",
    "en": "Purged",
    "ja": "整理しました",
    "ko": "정리했습니다"
  },
  "section.ak.refresh": {
    "zh-CN": "刷新",
    "zh-TW": "重新整理",
    "en": "Refresh",
    "ja": "更新",
    "ko": "새로 고침"
  },
  "section.ak.reveal.expired": {
    "zh-CN": "管理会话已失效，请重新输入口令。",
    "zh-TW": "管理工作階段已失效，請重新輸入口令。",
    "en": "The admin session has expired; enter the token again.",
    "ja": "管理セッションが失効しました。トークンを入力し直してください。",
    "ko": "관리 세션이 만료되었습니다. 토큰을 다시 입력하세요."
  },
  "section.ak.reveal.failed": {
    "zh-CN": "这一次没取到明文，重试可能有用。",
    "zh-TW": "這一次沒取到明文，重試可能有用。",
    "en": "The plaintext did not come back this time; retrying may help.",
    "ja": "今回は平文を取得できませんでした。再試行が有効かもしれません。",
    "ko": "이번에는 평문을 받지 못했습니다. 재시도가 도움이 될 수 있습니다."
  },
  "section.ak.reveal.gone": {
    "zh-CN": "这条记录已经不在表里了 —— 列表正在重新拉一次。",
    "zh-TW": "這條記錄已經不在表裡了 —— 列表正在重新拉一次。",
    "en": "That record is no longer in the table — the list is being reloaded.",
    "ja": "そのレコードはもう一覧にありません。リストを再取得しています。",
    "ko": "해당 레코드가 더 이상 목록에 없습니다 — 목록을 다시 불러오는 중입니다."
  },
  "section.ak.reveal.permanent": {
    "zh-CN": "明文永久取不回",
    "zh-TW": "明文永久取不回",
    "en": "plaintext is permanently unrecoverable",
    "ja": "平文は永久に取り出せません",
    "ko": "평문은 영구히 복구할 수 없습니다"
  },
  "section.ak.reveal.unavailable": {
    "zh-CN": "这把签发于明文落盘之前，服务端从来就没有过它的明文 —— 永久取不回，重发一把。",
    "zh-TW": "這把簽發於明文落盤之前，伺服器從來就沒有過它的明文 —— 永久取不回，重發一把。",
    "en": "This key was issued before plaintext was persisted, so the server never had it — permanently unrecoverable, issue a new one.",
    "ja": "このキーは平文を保存する前に発行されたため、サーバーは元から平文を持っていません。永久に取り出せないので新しく発行してください。",
    "ko": "이 키는 평문을 저장하기 전에 발급되어 서버가 애초에 평문을 가진 적이 없습니다 — 영구히 복구 불가, 새로 발급하세요."
  },
  "section.ak.revokeDelay": {
    "zh-CN": "停用/吊销最多还要 {delay} 才会在每个实例上生效 —— 这段时间里它仍然能用。",
    "zh-TW": "停用/吊銷最多還要 {delay} 才會在每個實例上生效 —— 這段時間裡它仍然能用。",
    "en": "It can take up to {delay} for this to take effect on every instance — until then the key still works.",
    "ja": "すべてのインスタンスで反映されるまで最大 {delay} かかります。その間このキーはまだ使えます。",
    "ko": "모든 인스턴스에 반영되기까지 최대 {delay} 걸립니다 — 그동안 이 키는 여전히 동작합니다."
  },
  "section.ak.saved": {
    "zh-CN": "已保存",
    "zh-TW": "已儲存",
    "en": "Saved",
    "ja": "保存しました",
    "ko": "저장했습니다"
  },
  "section.ak.searchPh": {
    "zh-CN": "按名称或末四位搜索",
    "zh-TW": "按名稱或末四位搜尋",
    "en": "Search by name or last four",
    "ja": "名前または末尾 4 桁で検索",
    "ko": "이름 또는 마지막 네 자리로 검색"
  },
  "section.ak.secret.copy": {
    "zh-CN": "只复制密钥",
    "zh-TW": "只複製金鑰",
    "en": "Copy the key only",
    "ja": "キーだけをコピー",
    "ko": "키만 복사"
  },
  "section.ak.secret.intro": {
    "zh-CN": "把它交给下游客户端。它现在就已经能用了。",
    "zh-TW": "把它交給下游客戶端。它現在就已經能用了。",
    "en": "Hand this to the downstream client. It already works.",
    "ja": "これを下流のクライアントに渡してください。すでに使えます。",
    "ko": "이것을 다운스트림 클라이언트에 전달하세요. 이미 사용할 수 있습니다."
  },
  "section.ak.secret.label": {
    "zh-CN": "密钥",
    "zh-TW": "金鑰",
    "en": "Key",
    "ja": "キー",
    "ko": "키"
  },
  "section.ak.secret.onceOnly": {
    "zh-CN": "这个部署不保存明文：关掉这一屏就再也看不到了。",
    "zh-TW": "這個部署不保存明文：關掉這一屏就再也看不到了。",
    "en": "This deployment does not keep the plaintext: close this screen and it is gone for good.",
    "ja": "このデプロイは平文を保存しません。この画面を閉じると二度と見られません。",
    "ko": "이 배포는 평문을 보관하지 않습니다. 이 화면을 닫으면 다시 볼 수 없습니다."
  },
  "section.ak.secret.title": {
    "zh-CN": "这把密钥的明文",
    "zh-TW": "這把金鑰的明文",
    "en": "The key in plaintext",
    "ja": "キーの平文",
    "ko": "키 평문"
  },
  "section.ak.sort.name": {
    "zh-CN": "按名称",
    "zh-TW": "按名稱",
    "en": "By name",
    "ja": "名前順",
    "ko": "이름순"
  },
  "section.ak.sort.new": {
    "zh-CN": "最新在前",
    "zh-TW": "最新在前",
    "en": "Newest first",
    "ja": "新しい順",
    "ko": "최신순"
  },
  "section.ak.sort.old": {
    "zh-CN": "最早在前",
    "zh-TW": "最早在前",
    "en": "Oldest first",
    "ja": "古い順",
    "ko": "오래된 순"
  },
  "section.ak.staleView": {
    "zh-CN": "这一屏的版本号已经过期，先刷新一次再写。",
    "zh-TW": "這一屏的版本號已經過期，先重新整理一次再寫。",
    "en": "This view's version is stale — refresh before writing.",
    "ja": "この画面のバージョンは古くなっています。書き込む前に再読み込みしてください。",
    "ko": "이 화면의 버전이 오래되었습니다. 쓰기 전에 새로 고치세요."
  },
  "section.ak.stat.active": {
    "zh-CN": "可用",
    "zh-TW": "可用",
    "en": "Active",
    "ja": "有効",
    "ko": "사용 가능"
  },
  "section.ak.stat.disabled": {
    "zh-CN": "已停用",
    "zh-TW": "已停用",
    "en": "Disabled",
    "ja": "無効化",
    "ko": "비활성"
  },
  "section.ak.stat.expired": {
    "zh-CN": "已过期",
    "zh-TW": "已過期",
    "en": "Expired",
    "ja": "期限切れ",
    "ko": "만료됨"
  },
  "section.ak.stat.max": {
    "zh-CN": "上限",
    "zh-TW": "上限",
    "en": "Cap",
    "ja": "上限数",
    "ko": "상한"
  },
  "section.ak.stat.total": {
    "zh-CN": "总数",
    "zh-TW": "總數",
    "en": "Total",
    "ja": "合計",
    "ko": "전체"
  },
  "section.ak.svc.baseUrl": {
    "zh-CN": "网关地址",
    "zh-TW": "閘道位址",
    "en": "Gateway base URL",
    "ja": "ゲートウェイのベース URL",
    "ko": "게이트웨이 주소"
  },
  "section.ak.svc.master": {
    "zh-CN": "这一族密钥是发给下游客户端的；管理口令是另一把，它在「设置」里轮换。",
    "zh-TW": "這一族金鑰是發給下游客戶端的；管理口令是另一把，它在「設定」裡輪換。",
    "en": "These keys are handed to downstream clients. The admin token is a different key and is rotated under Settings.",
    "ja": "これらのキーは下流のクライアントに配るものです。管理トークンは別の鍵で、「設定」でローテーションします。",
    "ko": "이 키들은 다운스트림 클라이언트에게 주는 것입니다. 관리 토큰은 별개의 키이며 ‘설정’에서 교체합니다."
  },
  "section.ak.svc.title": {
    "zh-CN": "服务连接",
    "zh-TW": "服務連線",
    "en": "Service endpoint",
    "ja": "サービス接続先",
    "ko": "서비스 연결"
  },
  "section.ak.unreadable": {
    "zh-CN": "存储里那张密钥表结构不认 —— 此刻全部下游客户端都在被拒。",
    "zh-TW": "儲存裡那張金鑰表結構不認 —— 此刻全部下游客戶端都在被拒。",
    "en": "The key table in storage has a shape we do not recognise — every downstream client is being rejected right now.",
    "ja": "ストレージ上のキー一覧の形式が認識できません。今この瞬間、下流のクライアントはすべて拒否されています。",
    "ko": "저장소의 키 목록 형식을 인식할 수 없습니다 — 지금 모든 다운스트림 클라이언트가 거부되고 있습니다."
  },
  "section.ak.unreadableHelp": {
    "zh-CN": "面板已经拒绝写入，原字节原地不动，人工捞回随时可做。",
    "zh-TW": "面板已經拒絕寫入，原位元組原地不動，人工撈回隨時可做。",
    "en": "The panel refuses to write, so the original bytes are untouched and can still be recovered by hand.",
    "ja": "パネルは書き込みを拒否しており、元のバイト列はそのまま残っているため手作業で復旧できます。",
    "ko": "패널이 쓰기를 거부했으므로 원래 바이트는 그대로 남아 있어 수동 복구가 가능합니다."
  },
  "section.ak.usage.notLanded": {
    "zh-CN": "这段区间还没有分片落盘",
    "zh-TW": "這段區間還沒有分片落盤",
    "en": "No shard has been persisted for this window yet",
    "ja": "この期間の断片はまだ保存されていません",
    "ko": "이 구간의 조각이 아직 저장되지 않았습니다"
  },
  "section.ak.usage.notLandedTip": {
    "zh-CN": "统计是开着的，只是这段时间的分片还没写下去 —— 这不是「没人用」。",
    "zh-TW": "統計是開著的，只是這段時間的分片還沒寫下去 —— 這不是「沒人用」。",
    "en": "Usage recording is on; the shard for this window simply has not been written yet — this is not “nobody used it”.",
    "ja": "使用量の記録は有効ですが、この期間の断片がまだ書き込まれていません。「誰も使っていない」ではありません。",
    "ko": "사용량 기록은 켜져 있고 이 구간의 조각이 아직 기록되지 않았을 뿐입니다 — “아무도 쓰지 않았다”가 아닙니다."
  },
  "section.ak.usage.off": {
    "zh-CN": "这个部署没在记账",
    "zh-TW": "這個部署沒在記帳",
    "en": "This deployment is not recording usage",
    "ja": "このデプロイは使用量を記録していません",
    "ko": "이 배포는 사용량을 기록하지 않습니다"
  },
  "section.ak.usage.tip": {
    "zh-CN": "这个数是近似值：未落盘的尾巴不算在内。",
    "zh-TW": "這個數是近似值：未落盤的尾巴不算在內。",
    "en": "Approximate: the not-yet-persisted tail is excluded.",
    "ja": "概算です。まだ保存されていない分は含みません。",
    "ko": "근사치입니다. 아직 저장되지 않은 부분은 제외됩니다."
  },
  "section.ak.usage.unknown": {
    "zh-CN": "用量这一块读不出来",
    "zh-TW": "用量這一塊讀不出來",
    "en": "usage could not be read",
    "ja": "使用量を読み取れません",
    "ko": "사용량을 읽지 못했습니다"
  },
  "section.ak.usage.value": {
    "zh-CN": "近 24 小时 {n} 次请求",
    "zh-TW": "近 24 小時 {n} 次請求",
    "en": "{n} requests in the last 24 h",
    "ja": "直近 24 時間で {n} 件のリクエスト",
    "ko": "최근 24시간 동안 요청 {n}건"
  },
  "section.ak.usageFailed": {
    "zh-CN": "用量这一次没读到，卡片上那一行先留空。",
    "zh-TW": "用量這一次沒讀到，卡片上那一行先留空。",
    "en": "Usage could not be read this time; that line on each card stays blank.",
    "ja": "今回は使用量を取得できませんでした。各カードのその行は空のままです。",
    "ko": "이번에는 사용량을 읽지 못했습니다. 각 카드의 해당 줄은 비워 둡니다."
  },
  "section.ak.writeFailed": {
    "zh-CN": "这一次写入没成功。",
    "zh-TW": "這一次寫入沒成功。",
    "en": "The write did not go through.",
    "ja": "今回の書き込みは失敗しました。",
    "ko": "이번 쓰기는 실패했습니다."
  },
  "section.ev.title": {
    "zh-CN": "实时日志",
    "zh-TW": "即時日誌",
    "en": "Live Logs",
    "ja": "リアルタイムログ",
    "ko": "실시간 로그"
  },
  "section.ev.status.connected": {
    "zh-CN": "轮询正常",
    "zh-TW": "輪詢正常",
    "en": "Polling",
    "ja": "ポーリング中",
    "ko": "폴링 중"
  },
  "section.ev.status.connecting": {
    "zh-CN": "连接中…",
    "zh-TW": "連線中…",
    "en": "Connecting…",
    "ja": "接続中…",
    "ko": "연결 중…"
  },
  "section.ev.status.reconnecting": {
    "zh-CN": "重试中…",
    "zh-TW": "重試中…",
    "en": "Retrying…",
    "ja": "再試行中…",
    "ko": "재시도 중…"
  },
  "section.ev.status.disconnected": {
    "zh-CN": "已断开",
    "zh-TW": "已斷線",
    "en": "Disconnected",
    "ja": "切断されました",
    "ko": "연결 끊김"
  },
  "section.ev.status.paused": {
    "zh-CN": "已暂停",
    "zh-TW": "已暫停",
    "en": "Paused",
    "ja": "一時停止中",
    "ko": "일시 중지됨"
  },
  "section.ev.statusHint": {
    "zh-CN": "实例 {shard} · 缓冲区里还有 {buffered} 条没落盘 · 已丢弃 {dropped} 条 · 畸形 {malformed} 条",
    "zh-TW": "實例 {shard} · 緩衝區裡還有 {buffered} 條沒落盤 · 已丟棄 {dropped} 條 · 畸形 {malformed} 條",
    "en": "Shard {shard} · {buffered} buffered (not yet persisted) · {dropped} dropped · {malformed} malformed",
    "ja": "インスタンス {shard} · 未保存 {buffered} 件 · 破棄 {dropped} 件 · 不正 {malformed} 件",
    "ko": "인스턴스 {shard} · 미저장 {buffered}건 · 폐기 {dropped}건 · 비정상 {malformed}건"
  },
  "section.ev.statusHintEmpty": {
    "zh-CN": "还没有取回任何一页",
    "zh-TW": "還沒有取回任何一頁",
    "en": "No page fetched yet",
    "ja": "まだ 1 ページも取得していません",
    "ko": "아직 한 페이지도 가져오지 않았습니다"
  },
  "section.ev.searchPlaceholder": {
    "zh-CN": "搜索事件名、消息与字段",
    "zh-TW": "搜尋事件名、訊息與欄位",
    "en": "Search event, message and fields",
    "ja": "イベント名・メッセージ・フィールドを検索",
    "ko": "이벤트명·메시지·필드 검색"
  },
  "section.ev.levelGroup": {
    "zh-CN": "级别筛选",
    "zh-TW": "等級篩選",
    "en": "Level filter",
    "ja": "レベルの絞り込み",
    "ko": "레벨 필터"
  },
  "section.ev.filter.all": {
    "zh-CN": "全部",
    "zh-TW": "全部",
    "en": "All",
    "ja": "すべて",
    "ko": "전체"
  },
  "section.ev.filter.debug": {
    "zh-CN": "调试",
    "zh-TW": "偵錯",
    "en": "Debug",
    "ja": "デバッグ",
    "ko": "디버그"
  },
  "section.ev.filter.info": {
    "zh-CN": "信息",
    "zh-TW": "資訊",
    "en": "Info",
    "ja": "情報",
    "ko": "정보"
  },
  "section.ev.filter.warn": {
    "zh-CN": "警告",
    "zh-TW": "警告",
    "en": "Warn",
    "ja": "警告レベル",
    "ko": "경고 레벨"
  },
  "section.ev.filter.error": {
    "zh-CN": "错误",
    "zh-TW": "錯誤",
    "en": "Error",
    "ja": "エラーレベル",
    "ko": "오류 레벨"
  },
  "section.ev.level.debug": {
    "zh-CN": "调试",
    "zh-TW": "偵錯",
    "en": "DEBUG",
    "ja": "デバッグ",
    "ko": "디버그"
  },
  "section.ev.level.info": {
    "zh-CN": "信息",
    "zh-TW": "資訊",
    "en": "INFO",
    "ja": "情報",
    "ko": "정보"
  },
  "section.ev.level.warn": {
    "zh-CN": "警告",
    "zh-TW": "警告",
    "en": "WARN",
    "ja": "ワーニング",
    "ko": "경고"
  },
  "section.ev.level.error": {
    "zh-CN": "错误",
    "zh-TW": "錯誤",
    "en": "ERROR",
    "ja": "エラー",
    "ko": "오류"
  },
  "section.ev.level.unknown": {
    "zh-CN": "未知级别",
    "zh-TW": "未知等級",
    "en": "UNKNOWN",
    "ja": "不明なレベル",
    "ko": "알 수 없는 레벨"
  },
  "section.ev.pause": {
    "zh-CN": "暂停",
    "zh-TW": "暫停",
    "en": "Pause",
    "ja": "一時停止",
    "ko": "일시 중지"
  },
  "section.ev.resume": {
    "zh-CN": "继续",
    "zh-TW": "繼續",
    "en": "Resume",
    "ja": "再開",
    "ko": "재개"
  },
  "section.ev.clear": {
    "zh-CN": "清屏",
    "zh-TW": "清畫面",
    "en": "Clear view",
    "ja": "画面をクリア",
    "ko": "화면 지우기"
  },
  "section.ev.clearHint": {
    "zh-CN": "只清掉当前屏幕上的行。服务端那份事件是持久的，不会被删掉。",
    "zh-TW": "只清掉目前畫面上的行。伺服器那份事件是持久的，不會被刪掉。",
    "en": "Clears the on-screen lines only. Events on the server are persisted and stay untouched.",
    "ja": "画面上の行だけを消します。サーバー側のイベントは永続化されており削除されません。",
    "ko": "화면의 줄만 지웁니다. 서버에 저장된 이벤트는 삭제되지 않습니다."
  },
  "section.ev.download": {
    "zh-CN": "下载整份",
    "zh-TW": "下載整份",
    "en": "Download all",
    "ja": "全件ダウンロード",
    "ko": "전체 다운로드"
  },
  "section.ev.downloadFailed": {
    "zh-CN": "下载失败：这一份没有取回来",
    "zh-TW": "下載失敗：這一份沒有取回來",
    "en": "Download failed: the file was not retrieved",
    "ja": "ダウンロードに失敗しました：ファイルを取得できませんでした",
    "ko": "다운로드 실패: 파일을 가져오지 못했습니다"
  },
  "section.ev.autoScroll": {
    "zh-CN": "自动滚动",
    "zh-TW": "自動捲動",
    "en": "Auto-scroll",
    "ja": "自動スクロール",
    "ko": "자동 스크롤"
  },
  "section.ev.empty": {
    "zh-CN": "这一档还没有事件",
    "zh-TW": "這一檔還沒有事件",
    "en": "No events in this level yet",
    "ja": "このレベルのイベントはまだありません",
    "ko": "이 레벨의 이벤트가 아직 없습니다"
  },
  "section.ev.emptySearch": {
    "zh-CN": "没有匹配这个关键词的行",
    "zh-TW": "沒有符合這個關鍵字的行",
    "en": "No line matches this keyword",
    "ja": "このキーワードに一致する行はありません",
    "ko": "이 키워드와 일치하는 줄이 없습니다"
  },
  "section.ev.detail": {
    "zh-CN": "事件详情",
    "zh-TW": "事件詳情",
    "en": "Event detail",
    "ja": "イベントの詳細",
    "ko": "이벤트 상세"
  },
  "section.ev.detailClose": {
    "zh-CN": "关闭详情",
    "zh-TW": "關閉詳情",
    "en": "Close detail",
    "ja": "詳細を閉じる",
    "ko": "상세 닫기"
  },
  "section.ev.detailUnserializable": {
    "zh-CN": "这一条序列化不出来，原文没法显示",
    "zh-TW": "這一條序列化不出來，原文沒法顯示",
    "en": "This entry cannot be serialised; the raw form cannot be shown",
    "ja": "このエントリはシリアライズできず、原文を表示できません",
    "ko": "이 항목은 직렬화할 수 없어 원문을 표시할 수 없습니다"
  },
  "section.ev.warn.dropped": {
    "zh-CN": "有事件被丢弃过：缓冲区满过，这段时间的记录是不完整的。",
    "zh-TW": "有事件被丟棄過：緩衝區滿過，這段時間的記錄是不完整的。",
    "en": "Some events were dropped: the buffer overflowed, so this window is incomplete.",
    "ja": "破棄されたイベントがあります：バッファが溢れたため、この期間の記録は不完全です。",
    "ko": "폐기된 이벤트가 있습니다: 버퍼가 넘쳐 이 구간의 기록은 불완전합니다."
  },
  "section.ev.warn.budgetExhausted": {
    "zh-CN": "落盘预算已耗尽：新事件暂时只在内存里，实例被回收就会丢。",
    "zh-TW": "落盤預算已耗盡：新事件暫時只在記憶體裡，實例被回收就會丟。",
    "en": "The persistence budget is exhausted: new events live in memory only and are lost if this instance is recycled.",
    "ja": "書き込み予算を使い切りました：新しいイベントはメモリ上のみで、インスタンス再起動で失われます。",
    "ko": "저장 예산이 소진되었습니다: 새 이벤트는 메모리에만 있으며 인스턴스가 재활용되면 사라집니다."
  },
  "section.ev.warn.truncated": {
    "zh-CN": "这一页被截断了：还有更旧的事件没显示出来。",
    "zh-TW": "這一頁被截斷了：還有更舊的事件沒顯示出來。",
    "en": "This page was truncated: older events exist but are not shown.",
    "ja": "このページは切り詰められました：表示されていない古いイベントがあります。",
    "ko": "이 페이지는 잘렸습니다: 표시되지 않은 더 오래된 이벤트가 있습니다."
  },
  "section.ev.warn.cursorAhead": {
    "zh-CN": "游标比服务端时钟还新：这一页是空的并不代表没有事件（多副本之间时钟不一致）。",
    "zh-TW": "游標比伺服器時鐘還新：這一頁是空的並不代表沒有事件（多副本之間時鐘不一致）。",
    "en": "The cursor is ahead of the server clock: an empty page here does not mean there are no events (clock skew across replicas).",
    "ja": "カーソルがサーバー時計より進んでいます：ここが空でもイベントが無いとは限りません（レプリカ間の時刻ずれ）。",
    "ko": "커서가 서버 시계보다 앞서 있습니다: 이 페이지가 비어 있다고 이벤트가 없는 것은 아닙니다(복제본 간 시계 차이)."
  },
  "section.md.badge.no": {
    "zh-CN": "这条协议上不可用",
    "zh-TW": "這條協定上不可用",
    "en": "not available on this protocol",
    "ja": "このプロトコルでは利用できません",
    "ko": "이 프로토콜에서는 사용할 수 없음"
  },
  "section.md.badge.yes": {
    "zh-CN": "这条协议上可用",
    "zh-TW": "這條協定上可用",
    "en": "available on this protocol",
    "ja": "このプロトコルで利用可能",
    "ko": "이 프로토콜에서 사용 가능"
  },
  "section.md.catalog.filterAll": {
    "zh-CN": "全部",
    "zh-TW": "全部",
    "en": "All",
    "ja": "すべて",
    "ko": "전체"
  },
  "section.md.catalog.title": {
    "zh-CN": "模型目录",
    "zh-TW": "模型目錄",
    "en": "Model catalog",
    "ja": "モデルカタログ",
    "ko": "모델 카탈로그"
  },
  "section.md.col.endpoints": {
    "zh-CN": "端点",
    "zh-TW": "端點",
    "en": "Endpoints",
    "ja": "エンドポイント",
    "ko": "엔드포인트"
  },
  "section.md.col.id": {
    "zh-CN": "模型",
    "zh-TW": "模型",
    "en": "Model",
    "ja": "モデル",
    "ko": "모델"
  },
  "section.md.col.modality": {
    "zh-CN": "形态",
    "zh-TW": "形態",
    "en": "Kind",
    "ja": "種別",
    "ko": "종류"
  },
  "section.md.col.protocols": {
    "zh-CN": "协议可用性",
    "zh-TW": "協定可用性",
    "en": "Protocol availability",
    "ja": "プロトコル対応",
    "ko": "프로토콜 가용성"
  },
  "section.md.desc": {
    "zh-CN": "三张卡回答三个不同的问题：本网关支持什么、这个账号此刻有什么、这一刻通不通。",
    "zh-TW": "三張卡回答三個不同的問題：本閘道支援什麼、這個帳號此刻有什麼、這一刻通不通。",
    "en": "Three cards, three different questions: what this gateway supports, what the account has right now, and whether it answers right now.",
    "ja": "3 枚のカードが答えるのは別々の問いです：このゲートウェイが対応するもの、このアカウントが今持つもの、今この瞬間つながるかどうか。",
    "ko": "세 카드가 서로 다른 세 질문에 답합니다: 이 게이트웨이가 지원하는 것, 이 계정이 지금 가진 것, 지금 이 순간 연결되는지."
  },
  "section.md.empty": {
    "zh-CN": "这个网关的目录里一个模型都没有。",
    "zh-TW": "這個閘道的目錄裡一個模型都沒有。",
    "en": "This gateway's catalog lists no model at all.",
    "ja": "このゲートウェイのカタログにはモデルが 1 つもありません。",
    "ko": "이 게이트웨이 카탈로그에는 모델이 하나도 없습니다."
  },
  "section.md.filterEmpty": {
    "zh-CN": "这条协议上没有可用的模型。",
    "zh-TW": "這條協定上沒有可用的模型。",
    "en": "No model is available on this protocol.",
    "ja": "このプロトコルで使えるモデルはありません。",
    "ko": "이 프로토콜에서 사용할 수 있는 모델이 없습니다."
  },
  "section.md.loadFailed": {
    "zh-CN": "模型目录这一次没读出来。",
    "zh-TW": "模型目錄這一次沒讀出來。",
    "en": "The model catalog could not be read this time.",
    "ja": "今回はモデルカタログを読み取れませんでした。",
    "ko": "이번에는 모델 카탈로그를 읽지 못했습니다."
  },
  "section.md.modality.chat": {
    "zh-CN": "对话",
    "zh-TW": "對話",
    "en": "Chat",
    "ja": "チャット",
    "ko": "대화"
  },
  "section.md.modality.image": {
    "zh-CN": "图片",
    "zh-TW": "圖片",
    "en": "Image",
    "ja": "画像",
    "ko": "이미지"
  },
  "section.md.modality.video": {
    "zh-CN": "视频",
    "zh-TW": "影片",
    "en": "Video",
    "ja": "動画",
    "ko": "동영상"
  },
  "section.md.retry": {
    "zh-CN": "再读一次",
    "zh-TW": "再讀一次",
    "en": "Read again",
    "ja": "もう一度読む",
    "ko": "다시 읽기"
  },
  "section.md.test.col.latency": {
    "zh-CN": "延迟（ms）",
    "zh-TW": "延遲（ms）",
    "en": "Latency (ms)",
    "ja": "レイテンシ（ms）",
    "ko": "지연(ms)"
  },
  "section.md.test.col.model": {
    "zh-CN": "模型",
    "zh-TW": "模型",
    "en": "Model",
    "ja": "モデル",
    "ko": "모델"
  },
  "section.md.test.col.result": {
    "zh-CN": "结果",
    "zh-TW": "結果",
    "en": "Result",
    "ja": "結果",
    "ko": "결과"
  },
  "section.md.test.desc": {
    "zh-CN": "逐个对话模型真发一次最小请求。图片与视频模型不进这一轮 —— 测它们会真的花掉这个账号的生成额度。",
    "zh-TW": "逐個對話模型真發一次最小請求。圖片與影片模型不進這一輪 —— 測它們會真的花掉這個帳號的生成額度。",
    "en": "Sends one minimal real request per chat model. Image and video models are excluded — testing those really spends the account's generation quota.",
    "ja": "チャットモデルごとに最小のリクエストを実際に 1 回送ります。画像・動画モデルは対象外です。テストすると本当に生成枠を消費するからです。",
    "ko": "채팅 모델마다 최소 요청을 실제로 한 번 보냅니다. 이미지와 동영상 모델은 제외됩니다 — 테스트하면 계정의 생성 할당량을 실제로 씁니다."
  },
  "section.md.test.empty": {
    "zh-CN": "目录里没有能这么测的模型。",
    "zh-TW": "目錄裡沒有能這麼測的模型。",
    "en": "The catalog has no model that can be tested this way.",
    "ja": "この方法でテストできるモデルはカタログにありません。",
    "ko": "이 방식으로 테스트할 수 있는 모델이 카탈로그에 없습니다."
  },
  "section.md.test.eta": {
    "zh-CN": "{n} 个模型，整轮至少要 {sec} 秒 —— 中间那几段是在等后端的最小间隔。",
    "zh-TW": "{n} 個模型，整輪至少要 {sec} 秒 —— 中間那幾段是在等後端的最小間隔。",
    "en": "{n} models; one round takes at least {sec} s — the gaps are the backend's minimum interval.",
    "ja": "{n} 件のモデル、1 周で最低 {sec} 秒かかります。間の時間はバックエンドの最小間隔の待ちです。",
    "ko": "모델 {n}개, 한 바퀴에 최소 {sec}초 걸립니다 — 중간 구간은 백엔드 최소 간격을 기다리는 시간입니다."
  },
  "section.md.test.idle": {
    "zh-CN": "这一次进面板以来还没测过（这与「都不通」不是一回事）。",
    "zh-TW": "這一次進面板以來還沒測過（這與「都不通」不是一回事）。",
    "en": "Nothing has been tested since this panel was opened — that is not the same as “none of them work”.",
    "ja": "この画面を開いてからまだテストしていません（「どれもつながらない」とは別です）。",
    "ko": "이 화면을 연 뒤 아직 테스트하지 않았습니다(“모두 안 된다”와는 다릅니다)."
  },
  "section.md.test.pacingUnknown": {
    "zh-CN": "读不到后端那个最小间隔，整轮的节奏就排不出来 —— 这颗按钮先停用，免得把我们自己的节流当成一张连通性矩阵。",
    "zh-TW": "讀不到後端那個最小間隔，整輪的節奏就排不出來 —— 這顆按鈕先停用，免得把我們自己的節流當成一張連通性矩陣。",
    "en": "Without the backend's minimum interval we cannot pace a round, so the button stays disabled — otherwise our own throttling would be read as a connectivity matrix.",
    "ja": "バックエンドの最小間隔が読めないと 1 周のペースを組めないため、ボタンは無効のままです。さもないと自分たちのスロットルを疎通結果と読み違えます。",
    "ko": "백엔드의 최소 간격을 읽지 못하면 한 바퀴의 속도를 정할 수 없어 버튼을 비활성화합니다. 그렇지 않으면 자체 스로틀을 연결 결과로 오해하게 됩니다."
  },
  "section.md.test.progress": {
    "zh-CN": "正在逐个测：{done}/{total}",
    "zh-TW": "正在逐個測：{done}/{total}",
    "en": "Testing one by one: {done}/{total}",
    "ja": "1 件ずつテスト中：{done}/{total}",
    "ko": "하나씩 테스트 중: {done}/{total}"
  },
  "section.md.test.reason.networkError": {
    "zh-CN": "没连上",
    "zh-TW": "沒連上",
    "en": "Could not connect",
    "ja": "接続できません",
    "ko": "연결하지 못함"
  },
  "section.md.test.reason.noKey": {
    "zh-CN": "没有可用的 key",
    "zh-TW": "沒有可用的 key",
    "en": "No usable key",
    "ja": "使える鍵がありません",
    "ko": "사용 가능한 키 없음"
  },
  "section.md.test.reason.notTestable": {
    "zh-CN": "这个形态不做连通性测试",
    "zh-TW": "這個形態不做連通性測試",
    "en": "This kind is not connectivity-tested",
    "ja": "この種別は疎通テストの対象外",
    "ko": "이 종류는 연결 테스트 대상이 아님"
  },
  "section.md.test.reason.ok": {
    "zh-CN": "通了",
    "zh-TW": "通了",
    "en": "OK",
    "ja": "成功",
    "ko": "성공"
  },
  "section.md.test.reason.other": {
    "zh-CN": "这一次没测到",
    "zh-TW": "這一次沒測到",
    "en": "No result this time",
    "ja": "今回は結果なし",
    "ko": "이번에는 결과 없음"
  },
  "section.md.test.reason.probeCooldown": {
    "zh-CN": "被自己的护栏挡下（还在最小间隔里）",
    "zh-TW": "被自己的護欄擋下（還在最小間隔裡）",
    "en": "Blocked by our own guard (still inside the minimum interval)",
    "ja": "自分たちのガードに阻止（最小間隔の内側）",
    "ko": "자체 가드에 차단됨(최소 간격 이내)"
  },
  "section.md.test.reason.probeInFlight": {
    "zh-CN": "被自己的护栏挡下（上一次还没回来）",
    "zh-TW": "被自己的護欄擋下（上一次還沒回來）",
    "en": "Blocked by our own guard (the previous probe is still open)",
    "ja": "自分たちのガードに阻止（前回がまだ返っていない）",
    "ko": "자체 가드에 차단됨(이전 요청이 아직 안 끝남)"
  },
  "section.md.test.reason.timeout": {
    "zh-CN": "超时",
    "zh-TW": "逾時",
    "en": "Timed out",
    "ja": "タイムアウト",
    "ko": "시간 초과"
  },
  "section.md.test.reason.upstreamError": {
    "zh-CN": "上游没正常回",
    "zh-TW": "上游沒正常回",
    "en": "Upstream did not answer normally",
    "ja": "上流が正常に応答しません",
    "ko": "업스트림이 정상 응답하지 않음"
  },
  "section.md.test.rerun": {
    "zh-CN": "再测一遍",
    "zh-TW": "再測一遍",
    "en": "Test again",
    "ja": "もう一度テスト",
    "ko": "다시 테스트"
  },
  "section.md.test.run": {
    "zh-CN": "全部测一遍",
    "zh-TW": "全部測一遍",
    "en": "Test them all",
    "ja": "すべてテスト",
    "ko": "전체 테스트"
  },
  "section.md.test.state.active": {
    "zh-CN": "正在测",
    "zh-TW": "正在測",
    "en": "Testing",
    "ja": "テスト中",
    "ko": "테스트 중"
  },
  "section.md.test.state.pending": {
    "zh-CN": "待测",
    "zh-TW": "待測",
    "en": "Queued",
    "ja": "待機中",
    "ko": "대기"
  },
  "section.md.test.status": {
    "zh-CN": "HTTP 状态 {status}",
    "zh-TW": "HTTP 狀態 {status}",
    "en": "HTTP {status}",
    "ja": "HTTP ステータス {status}",
    "ko": "HTTP 상태 {status}"
  },
  "section.md.test.throttledNote": {
    "zh-CN": "其中 {n} 行是被我们自己的护栏挡下的，不是模型不通 —— 隔一会儿再单独测那几个。",
    "zh-TW": "其中 {n} 行是被我們自己的護欄擋下的，不是模型不通 —— 隔一會兒再單獨測那幾個。",
    "en": "{n} of these rows were blocked by our own guard, not by the model — retest those separately in a moment.",
    "ja": "そのうち {n} 行は自分たちのガードに阻止されたもので、モデルの不通ではありません。少し置いて個別に再テストしてください。",
    "ko": "그중 {n}행은 모델 문제가 아니라 자체 가드에 막힌 것입니다 — 잠시 후 따로 다시 테스트하세요."
  },
  "section.md.test.title": {
    "zh-CN": "连通性测试",
    "zh-TW": "連通性測試",
    "en": "Connectivity test",
    "ja": "疎通テスト",
    "ko": "연결 테스트"
  },
  "section.md.test.waiting": {
    "zh-CN": "在等节流：{done}/{total}",
    "zh-TW": "在等節流：{done}/{total}",
    "en": "Waiting out the throttle: {done}/{total}",
    "ja": "スロットル待ち：{done}/{total}",
    "ko": "스로틀 대기 중: {done}/{total}"
  },
  "section.md.title": {
    "zh-CN": "支持模型",
    "zh-TW": "支援模型",
    "en": "Supported models",
    "ja": "対応モデル",
    "ko": "지원 모델"
  },
  "section.md.unknown": {
    "zh-CN": "我们不知道这个网关认得哪些模型。",
    "zh-TW": "我們不知道這個閘道認得哪些模型。",
    "en": "We do not know which models this gateway recognises.",
    "ja": "このゲートウェイがどのモデルを認識するか分かりません。",
    "ko": "이 게이트웨이가 어떤 모델을 아는지 알 수 없습니다."
  },
  "section.md.up.desc": {
    "zh-CN": "这颗按钮会拿池里的一把 key 真打一次上游，问它此刻有哪些模型。",
    "zh-TW": "這顆按鈕會拿池裡的一把 key 真打一次上游，問它此刻有哪些模型。",
    "en": "This button really calls upstream with one key from the pool and asks which models it has right now.",
    "ja": "このボタンはプールの鍵を 1 本使って実際に上流へ問い合わせ、今どのモデルがあるかを尋ねます。",
    "ko": "이 버튼은 풀의 키 하나로 실제로 업스트림을 호출해 지금 어떤 모델이 있는지 묻습니다."
  },
  "section.md.up.diffNone": {
    "zh-CN": "两边一条不差。",
    "zh-TW": "兩邊一條不差。",
    "en": "Both sides match exactly.",
    "ja": "両者は完全に一致しています。",
    "ko": "양쪽이 정확히 일치합니다."
  },
  "section.md.up.idle": {
    "zh-CN": "还没问过上游（这与「上游没有模型」不是一回事）。",
    "zh-TW": "還沒問過上游（這與「上游沒有模型」不是一回事）。",
    "en": "We have not asked upstream yet — that is not the same as “upstream has no models”.",
    "ja": "まだ上流に問い合わせていません（「上流にモデルがない」とは別のことです）。",
    "ko": "아직 업스트림에 묻지 않았습니다(“업스트림에 모델이 없다”와는 다릅니다)."
  },
  "section.md.up.listLabel": {
    "zh-CN": "上游这次回的模型（{n}）",
    "zh-TW": "上游這次回的模型（{n}）",
    "en": "Models upstream returned ({n})",
    "ja": "上流が返したモデル（{n}）",
    "ko": "업스트림이 반환한 모델({n})"
  },
  "section.md.up.loading": {
    "zh-CN": "正在问上游……",
    "zh-TW": "正在問上游……",
    "en": "Asking upstream...",
    "ja": "上流に問い合わせ中…",
    "ko": "업스트림에 문의 중…"
  },
  "section.md.up.onlyCatalog": {
    "zh-CN": "目录有、上游这次没回（{n}）",
    "zh-TW": "目錄有、上游這次沒回（{n}）",
    "en": "In the catalog, not returned upstream this time ({n})",
    "ja": "カタログにあり今回上流が返さなかった（{n}）",
    "ko": "카탈로그에 있고 이번에 업스트림이 반환하지 않음({n})"
  },
  "section.md.up.onlyUpstream": {
    "zh-CN": "上游有、目录没有（{n}）",
    "zh-TW": "上游有、目錄沒有（{n}）",
    "en": "Upstream only, not in the catalog ({n})",
    "ja": "上流のみ・カタログにない（{n}）",
    "ko": "업스트림에만 있고 카탈로그에 없음({n})"
  },
  "section.md.up.reason.badPayload": {
    "zh-CN": "正文读全了，但这份响应我们看不懂。",
    "zh-TW": "內文讀全了，但這份回應我們看不懂。",
    "en": "The body arrived in full, but we cannot make sense of it.",
    "ja": "本文は届きましたが、その中身を解釈できません。",
    "ko": "본문은 모두 받았지만 그 내용을 해석할 수 없습니다."
  },
  "section.md.up.reason.bodyIncomplete": {
    "zh-CN": "响应头到手了，正文没读全。",
    "zh-TW": "回應標頭到手了，內文沒讀全。",
    "en": "Headers arrived, the body did not come through in full.",
    "ja": "ヘッダーは届きましたが本文が最後まで届きませんでした。",
    "ko": "헤더는 도착했지만 본문을 끝까지 받지 못했습니다."
  },
  "section.md.up.reason.networkError": {
    "zh-CN": "根本没连上（DNS / TCP / TLS）。",
    "zh-TW": "根本沒連上（DNS / TCP / TLS）。",
    "en": "Never connected at all (DNS / TCP / TLS).",
    "ja": "そもそも接続できませんでした（DNS / TCP / TLS）。",
    "ko": "아예 연결되지 않았습니다(DNS / TCP / TLS)."
  },
  "section.md.up.reason.noKey": {
    "zh-CN": "池子里一把能用的 key 都没有。",
    "zh-TW": "池子裡一把能用的 key 都沒有。",
    "en": "No usable key in the pool.",
    "ja": "プールに使える鍵が 1 本もありません。",
    "ko": "풀에 사용할 수 있는 키가 없습니다."
  },
  "section.md.up.reason.other": {
    "zh-CN": "这一次没问到，重试可能有用。",
    "zh-TW": "這一次沒問到，重試可能有用。",
    "en": "Nothing came back this time; retrying may help.",
    "ja": "今回は結果を得られませんでした。再試行が有効かもしれません。",
    "ko": "이번에는 결과를 받지 못했습니다. 재시도가 도움이 될 수 있습니다."
  },
  "section.md.up.reason.probeCooldown": {
    "zh-CN": "两次探测之间要隔一小段时间，这一次被我们自己的护栏挡下了。",
    "zh-TW": "兩次探測之間要隔一小段時間，這一次被我們自己的護欄擋下了。",
    "en": "Probes need a gap between them; this one was blocked by our own guard.",
    "ja": "プローブの間には間隔が必要です。今回は自分たちのガードに止められました。",
    "ko": "탐지 사이에는 간격이 필요합니다. 이번에는 자체 가드에 막혔습니다."
  },
  "section.md.up.reason.probeInFlight": {
    "zh-CN": "上一次探测还没回来，等它结束再试。",
    "zh-TW": "上一次探測還沒回來，等它結束再試。",
    "en": "The previous probe has not returned yet; wait for it to finish.",
    "ja": "前回のプローブがまだ返っていません。終わってから再試行してください。",
    "ko": "이전 탐지가 아직 끝나지 않았습니다. 끝난 뒤 다시 시도하세요."
  },
  "section.md.up.reason.timeout": {
    "zh-CN": "响应头到达之前就超时了。",
    "zh-TW": "回應標頭到達之前就逾時了。",
    "en": "Timed out before any response header arrived.",
    "ja": "応答ヘッダーが届く前にタイムアウトしました。",
    "ko": "응답 헤더가 도착하기 전에 시간이 초과되었습니다."
  },
  "section.md.up.reason.upstreamError": {
    "zh-CN": "上游回了，但不是 2xx。",
    "zh-TW": "上游回了，但不是 2xx。",
    "en": "Upstream answered, but not with a 2xx.",
    "ja": "上流は応答しましたが 2xx ではありません。",
    "ko": "업스트림이 응답했지만 2xx가 아닙니다."
  },
  "section.md.up.run": {
    "zh-CN": "去问一次上游",
    "zh-TW": "去問一次上游",
    "en": "Ask upstream",
    "ja": "上流に問い合わせる",
    "ko": "업스트림에 문의"
  },
  "section.md.up.status": {
    "zh-CN": "上游回了 {status}",
    "zh-TW": "上游回了 {status}",
    "en": "Upstream answered {status}",
    "ja": "上流の応答は {status}",
    "ko": "업스트림 응답 {status}"
  },
  "section.md.up.title": {
    "zh-CN": "上游模型",
    "zh-TW": "上游模型",
    "en": "Upstream models",
    "ja": "上流のモデル",
    "ko": "업스트림 모델"
  },
  "section.md.up.truncated": {
    "zh-CN": "上游回的清单被截断了，这里只有前 {n} 条。",
    "zh-TW": "上游回的清單被截斷了，這裡只有前 {n} 條。",
    "en": "The upstream list was truncated; only the first {n} entries are shown.",
    "ja": "上流のリストは切り詰められました。ここには先頭 {n} 件だけです。",
    "ko": "업스트림 목록이 잘렸습니다. 여기에는 처음 {n}개만 있습니다."
  },
  "section.pg.cancel": {
    "zh-CN": "取消这一轮",
    "zh-TW": "取消這一輪",
    "en": "Cancel this round",
    "ja": "この回をキャンセル",
    "ko": "이번 회차 취소"
  },
  "section.pg.cfg.modality": {
    "zh-CN": "形态",
    "zh-TW": "形態",
    "en": "Kind",
    "ja": "種別",
    "ko": "종류"
  },
  "section.pg.cfg.model": {
    "zh-CN": "模型",
    "zh-TW": "模型",
    "en": "Model",
    "ja": "モデル",
    "ko": "모델"
  },
  "section.pg.cfg.noModel": {
    "zh-CN": "这个形态下没有可选模型",
    "zh-TW": "這個形態下沒有可選模型",
    "en": "No model available for this kind",
    "ja": "この種別に選べるモデルはありません",
    "ko": "이 종류에 선택할 수 있는 모델이 없습니다"
  },
  "section.pg.cfg.prompt": {
    "zh-CN": "提示词",
    "zh-TW": "提示詞",
    "en": "Prompt",
    "ja": "プロンプト",
    "ko": "프롬프트"
  },
  "section.pg.cfg.promptPh": {
    "zh-CN": "说点什么",
    "zh-TW": "說點什麼",
    "en": "Say something",
    "ja": "何か入力してください",
    "ko": "무언가 입력하세요"
  },
  "section.pg.cfg.protocol": {
    "zh-CN": "协议",
    "zh-TW": "協定",
    "en": "Protocol",
    "ja": "プロトコル",
    "ko": "프로토콜"
  },
  "section.pg.cfg.save": {
    "zh-CN": "存下来",
    "zh-TW": "存起來",
    "en": "Save",
    "ja": "保存",
    "ko": "저장"
  },
  "section.pg.cfg.stream": {
    "zh-CN": "流式返回",
    "zh-TW": "串流回應",
    "en": "Stream the response",
    "ja": "ストリーミングで受け取る",
    "ko": "스트리밍으로 받기"
  },
  "section.pg.cfg.title": {
    "zh-CN": "请求",
    "zh-TW": "請求",
    "en": "Request",
    "ja": "リクエスト",
    "ko": "요청"
  },
  "section.pg.cfg.token": {
    "zh-CN": "网关口令",
    "zh-TW": "閘道口令",
    "en": "Gateway token",
    "ja": "ゲートウェイトークン",
    "ko": "게이트웨이 토큰"
  },
  "section.pg.cfg.tokenPh": {
    "zh-CN": "粘贴发给下游客户端的那把口令",
    "zh-TW": "貼上發給下游客戶端的那把口令",
    "en": "Paste the token you hand to downstream clients",
    "ja": "下流のクライアントに配るトークンを貼り付けてください",
    "ko": "다운스트림 클라이언트에 배포하는 토큰을 붙여 넣으세요"
  },
  "section.pg.clear": {
    "zh-CN": "清空对话",
    "zh-TW": "清空對話",
    "en": "Clear the conversation",
    "ja": "会話をクリア",
    "ko": "대화 비우기"
  },
  "section.pg.desc": {
    "zh-CN": "拿网关自己的对外接口真发一次请求。用的是网关口令，不是管理口令 —— 两把钥匙分开存、分开走。",
    "zh-TW": "拿閘道自己的對外介面真發一次請求。用的是閘道口令，不是管理口令 —— 兩把鑰匙分開存、分開走。",
    "en": "Sends a real request through the gateway's own public API. It uses the gateway token, not the admin token — the two keys are stored and sent separately.",
    "ja": "ゲートウェイ自身の公開 API へ実際にリクエストを送ります。使うのはゲートウェイトークンで、管理トークンではありません。2 つの鍵は別々に保存し、別々に送ります。",
    "ko": "게이트웨이의 공개 API로 실제 요청을 보냅니다. 관리 토큰이 아니라 게이트웨이 토큰을 사용하며, 두 키는 따로 저장하고 따로 전송합니다."
  },
  "section.pg.err.badUrl": {
    "zh-CN": "目录给的地址不是一条能用的 URL。",
    "zh-TW": "目錄給的位址不是一條能用的 URL。",
    "en": "The catalog's address is not a usable URL.",
    "ja": "カタログのアドレスは有効な URL ではありません。",
    "ko": "카탈로그의 주소가 유효한 URL이 아닙니다."
  },
  "section.pg.err.buildFailed": {
    "zh-CN": "按目录拼不出这一次请求 —— 宁可不发，也不发一条与你选的不一样的。",
    "zh-TW": "按目錄拼不出這一次請求 —— 寧可不發，也不發一條與你選的不一樣的。",
    "en": "The request could not be built from the catalog — better to send nothing than to send something different from what you picked.",
    "ja": "カタログからこのリクエストを組み立てられませんでした。選んだ内容と違うものを送るくらいなら送りません。",
    "ko": "카탈로그로 이번 요청을 구성할 수 없습니다 — 선택한 것과 다른 요청을 보내느니 보내지 않습니다."
  },
  "section.pg.err.crossOrigin": {
    "zh-CN": "目录给的地址不在本面板这个源上 —— 一个字节都没发（那把口令是发给每个下游用户的）。",
    "zh-TW": "目錄給的位址不在本面板這個來源上 —— 一個位元組都沒發（那把口令是發給每個下游使用者的）。",
    "en": "The catalog's URL is not on this panel's origin — nothing was sent (that token is the one handed to every downstream user).",
    "ja": "カタログの URL がこのパネルのオリジンと異なります。何も送信していません（そのトークンは全下流ユーザーに配るものです）。",
    "ko": "카탈로그의 URL이 이 패널의 출처가 아닙니다 — 아무것도 보내지 않았습니다(그 토큰은 모든 다운스트림 사용자에게 배포하는 것입니다)."
  },
  "section.pg.err.emptyPrompt": {
    "zh-CN": "先写点提示词。",
    "zh-TW": "先寫點提示詞。",
    "en": "Type a prompt first.",
    "ja": "まずプロンプトを入力してください。",
    "ko": "먼저 프롬프트를 입력하세요."
  },
  "section.pg.err.http": {
    "zh-CN": "网关回了 {status}，下面是它的原话。",
    "zh-TW": "閘道回了 {status}，下面是它的原話。",
    "en": "The gateway answered {status}; its own words are below.",
    "ja": "ゲートウェイの応答は {status} です。以下はその原文です。",
    "ko": "게이트웨이가 {status}로 응답했습니다. 아래는 원문입니다."
  },
  "section.pg.err.noEndpoint": {
    "zh-CN": "目录里没有这个形态要用的端点。",
    "zh-TW": "目錄裡沒有這個形態要用的端點。",
    "en": "The catalog has no endpoint for this kind.",
    "ja": "この種別に必要なエンドポイントがカタログにありません。",
    "ko": "이 종류에 필요한 엔드포인트가 카탈로그에 없습니다."
  },
  "section.pg.err.noTaskId": {
    "zh-CN": "建任务那一步没回任务标识 —— 不去猜一个 id 接着轮询。",
    "zh-TW": "建任務那一步沒回任務識別 —— 不去猜一個 id 接著輪詢。",
    "en": "The create step returned no task id — we will not guess one and keep polling.",
    "ja": "作成ステップがタスク ID を返しませんでした。ID を推測してポーリングを続けることはしません。",
    "ko": "생성 단계가 작업 ID를 반환하지 않았습니다 — ID를 추측해 계속 폴링하지 않습니다."
  },
  "section.pg.err.noToken": {
    "zh-CN": "还没粘网关口令 —— 一个字节都没发出去。",
    "zh-TW": "還沒貼閘道口令 —— 一個位元組都沒發出去。",
    "en": "No gateway token pasted — not a single byte was sent.",
    "ja": "ゲートウェイトークンが貼られていません。1 バイトも送信していません。",
    "ko": "게이트웨이 토큰이 없습니다 — 한 바이트도 보내지 않았습니다."
  },
  "section.pg.err.pollTimeout": {
    "zh-CN": "我们不再等了 —— 任务可能还在上游跑着，这不代表它失败了。",
    "zh-TW": "我們不再等了 —— 任務可能還在上游跑著，這不代表它失敗了。",
    "en": "We stopped waiting — the task may still be running upstream; this does not mean it failed.",
    "ja": "待機を打ち切りました。タスクは上流でまだ実行中かもしれず、失敗を意味しません。",
    "ko": "대기를 중단했습니다 — 작업은 업스트림에서 아직 실행 중일 수 있으며 실패를 뜻하지 않습니다."
  },
  "section.pg.err.stream": {
    "zh-CN": "这条流读到一半断了，上面那半句是真的收到过的。",
    "zh-TW": "這條串流讀到一半斷了，上面那半句是真的收到過的。",
    "en": "The stream broke halfway; the half sentence above really did arrive.",
    "ja": "ストリームが途中で切れました。上の途中までの文は実際に届いたものです。",
    "ko": "스트림이 중간에 끊겼습니다. 위의 반쪽 문장은 실제로 도착한 것입니다."
  },
  "section.pg.err.title": {
    "zh-CN": "这一轮没成",
    "zh-TW": "這一輪沒成",
    "en": "This round did not go through",
    "ja": "この回は失敗しました",
    "ko": "이번 회차는 실패했습니다"
  },
  "section.pg.err.transport": {
    "zh-CN": "这一次连响应都没拿到（断网 / 被取消 / 被重定向拦下）。",
    "zh-TW": "這一次連回應都沒拿到（斷網 / 被取消 / 被重新導向攔下）。",
    "en": "No response came back at all (offline, cancelled, or stopped by a redirect).",
    "ja": "応答をまったく受け取れませんでした（オフライン・キャンセル・リダイレクトによる中止）。",
    "ko": "응답을 전혀 받지 못했습니다(오프라인, 취소, 또는 리디렉션으로 중단)."
  },
  "section.pg.loading": {
    "zh-CN": "正在读协议目录……",
    "zh-TW": "正在讀協定目錄……",
    "en": "Reading the protocol catalog...",
    "ja": "プロトコルカタログを読み込み中…",
    "ko": "프로토콜 카탈로그를 읽는 중…"
  },
  "section.pg.media.none": {
    "zh-CN": "这一轮没有可展示的媒体地址，完整响应体在下面。",
    "zh-TW": "這一輪沒有可展示的媒體位址，完整回應體在下面。",
    "en": "No displayable media URL this round; the full body is below.",
    "ja": "この回に表示できるメディア URL はありません。完全なボディは下にあります。",
    "ko": "이번 회차에는 표시할 미디어 URL이 없습니다. 전체 본문은 아래에 있습니다."
  },
  "section.pg.media.remote": {
    "zh-CN": "远端地址只给链接：面板的 CSP 会拦下跨源图片。",
    "zh-TW": "遠端位址只給連結：面板的 CSP 會擋下跨來源圖片。",
    "en": "Remote URLs are shown as links only: the panel's CSP blocks cross-origin images.",
    "ja": "リモート URL はリンクのみです。パネルの CSP がクロスオリジン画像を遮断します。",
    "ko": "원격 URL은 링크로만 표시합니다: 패널의 CSP가 교차 출처 이미지를 차단합니다."
  },
  "section.pg.media.title": {
    "zh-CN": "媒体结果",
    "zh-TW": "媒體結果",
    "en": "Media result",
    "ja": "メディア結果",
    "ko": "미디어 결과"
  },
  "section.pg.modality.chat": {
    "zh-CN": "对话",
    "zh-TW": "對話",
    "en": "Chat",
    "ja": "チャット",
    "ko": "대화"
  },
  "section.pg.modality.image": {
    "zh-CN": "图片",
    "zh-TW": "圖片",
    "en": "Image",
    "ja": "画像",
    "ko": "이미지"
  },
  "section.pg.modality.video": {
    "zh-CN": "视频",
    "zh-TW": "影片",
    "en": "Video",
    "ja": "動画",
    "ko": "동영상"
  },
  "section.pg.raw.title": {
    "zh-CN": "响应体原文",
    "zh-TW": "回應體原文",
    "en": "Raw response body",
    "ja": "レスポンス本文の原文",
    "ko": "응답 본문 원문"
  },
  "section.pg.resp.chunks": {
    "zh-CN": "收到块数",
    "zh-TW": "收到塊數",
    "en": "Chunks",
    "ja": "受信チャンク",
    "ko": "수신 청크"
  },
  "section.pg.resp.latency": {
    "zh-CN": "延迟",
    "zh-TW": "延遲",
    "en": "Latency",
    "ja": "レイテンシ",
    "ko": "지연"
  },
  "section.pg.resp.malformed": {
    "zh-CN": "读不出来的块",
    "zh-TW": "讀不出來的塊",
    "en": "Unreadable chunks",
    "ja": "解釈できないチャンク",
    "ko": "해석 불가 청크"
  },
  "section.pg.resp.nonStream": {
    "zh-CN": "这一轮是非流式的：完整响应体在下面的原文里。",
    "zh-TW": "這一輪是非串流的：完整回應體在下面的原文裡。",
    "en": "This round was non-streaming: the full body is in the raw text below.",
    "ja": "この回は非ストリーミングです。完全なボディは下の原文にあります。",
    "ko": "이번 회차는 비스트리밍입니다. 전체 본문은 아래 원문에 있습니다."
  },
  "section.pg.resp.placeholder": {
    "zh-CN": "还没发过请求。",
    "zh-TW": "還沒發過請求。",
    "en": "Nothing has been sent yet.",
    "ja": "まだ何も送信していません。",
    "ko": "아직 아무것도 보내지 않았습니다."
  },
  "section.pg.resp.status": {
    "zh-CN": "状态",
    "zh-TW": "狀態",
    "en": "Status",
    "ja": "ステータス",
    "ko": "상태"
  },
  "section.pg.resp.title": {
    "zh-CN": "响应",
    "zh-TW": "回應",
    "en": "Response",
    "ja": "レスポンス",
    "ko": "응답"
  },
  "section.pg.retry": {
    "zh-CN": "再读一次",
    "zh-TW": "再讀一次",
    "en": "Read again",
    "ja": "もう一度読む",
    "ko": "다시 읽기"
  },
  "section.pg.send": {
    "zh-CN": "发出去",
    "zh-TW": "發出去",
    "en": "Send",
    "ja": "送信",
    "ko": "보내기"
  },
  "section.pg.title": {
    "zh-CN": "调试台",
    "zh-TW": "除錯台",
    "en": "Playground",
    "ja": "デバッグ台",
    "ko": "디버그 콘솔"
  },
  "section.pg.token.cleared": {
    "zh-CN": "口令已清掉。",
    "zh-TW": "口令已清掉。",
    "en": "The token has been cleared.",
    "ja": "トークンを削除しました。",
    "ko": "토큰을 지웠습니다."
  },
  "section.pg.token.empty": {
    "zh-CN": "还没粘口令 —— 在粘上之前一个字节都不会发出去。",
    "zh-TW": "還沒貼口令 —— 在貼上之前一個位元組都不會發出去。",
    "en": "No token pasted yet — nothing is sent until you paste one.",
    "ja": "まだトークンが貼られていません。貼るまで 1 バイトも送信しません。",
    "ko": "아직 토큰을 붙여 넣지 않았습니다 — 붙여 넣기 전에는 한 바이트도 보내지 않습니다."
  },
  "section.pg.token.match": {
    "zh-CN": "与设置页记着的那把末位一致。",
    "zh-TW": "與設定頁記著的那把末位一致。",
    "en": "Matches the last characters recorded in Settings.",
    "ja": "設定ページに記録された末尾と一致しています。",
    "ko": "설정 페이지에 기록된 마지막 문자와 일치합니다."
  },
  "section.pg.token.mismatch": {
    "zh-CN": "与设置页记着的那把末位对不上 —— 可能粘错了。",
    "zh-TW": "與設定頁記著的那把末位對不上 —— 可能貼錯了。",
    "en": "Does not match the last characters recorded in Settings — you may have pasted the wrong one.",
    "ja": "設定ページに記録された末尾と一致しません。貼り間違えた可能性があります。",
    "ko": "설정 페이지에 기록된 마지막 문자와 다릅니다 — 잘못 붙여 넣었을 수 있습니다."
  },
  "section.pg.token.note": {
    "zh-CN": "面板拿不到明文网关口令，所以这里没有「一键填入」——那需要在后端开一个明文回显口子。",
    "zh-TW": "面板拿不到明文閘道口令，所以這裡沒有「一鍵填入」——那需要在後端開一個明文回顯口子。",
    "en": "The panel cannot read the gateway token in plaintext, so there is no “fill it in for me” — that would need a plaintext echo on the backend.",
    "ja": "パネルはゲートウェイトークンの平文を取得できないため「自動入力」はありません。実現にはバックエンドに平文を返す口を開ける必要があります。",
    "ko": "패널은 게이트웨이 토큰 평문을 읽을 수 없으므로 ‘자동 입력’이 없습니다 — 그러려면 백엔드에 평문 회신 통로를 열어야 합니다."
  },
  "section.pg.token.saved": {
    "zh-CN": "口令已存在这台浏览器里。",
    "zh-TW": "口令已存在這台瀏覽器裡。",
    "en": "The token is stored in this browser.",
    "ja": "トークンをこのブラウザーに保存しました。",
    "ko": "이 브라우저에 토큰을 저장했습니다."
  },
  "section.pg.token.unknown": {
    "zh-CN": "设置页那份末位读不到，这一次比不了。",
    "zh-TW": "設定頁那份末位讀不到，這一次比不了。",
    "en": "The hint from Settings could not be read, so there is nothing to compare against.",
    "ja": "設定ページのヒントを読み取れないため、照合できません。",
    "ko": "설정 페이지의 힌트를 읽을 수 없어 대조할 수 없습니다."
  },
  "section.pg.turn.assistant": {
    "zh-CN": "模型",
    "zh-TW": "模型",
    "en": "Model",
    "ja": "モデル",
    "ko": "모델"
  },
  "section.pg.turn.user": {
    "zh-CN": "你",
    "zh-TW": "你",
    "en": "You",
    "ja": "あなた",
    "ko": "나"
  },
  "section.pg.turnsTrimmed": {
    "zh-CN": "上下文超过 {max} 条，最早的几条已经丢掉了。",
    "zh-TW": "上下文超過 {max} 條，最早的幾條已經丟掉了。",
    "en": "The context passed {max} turns; the oldest ones were dropped.",
    "ja": "文脈が {max} 件を超えたため、古いものを破棄しました。",
    "ko": "문맥이 {max}개를 넘어 가장 오래된 항목을 버렸습니다."
  },
  "section.pg.unavailable": {
    "zh-CN": "协议目录读不出来，这个板块暂时什么都做不了。",
    "zh-TW": "協定目錄讀不出來，這個板塊暫時什麼都做不了。",
    "en": "The protocol catalog cannot be read, so this section can do nothing for now.",
    "ja": "プロトコルカタログを読み取れないため、この画面では今のところ何もできません。",
    "ko": "프로토콜 카탈로그를 읽을 수 없어 지금은 이 화면에서 아무것도 할 수 없습니다."
  },
  "section.pg.video.polling": {
    "zh-CN": "正在轮询视频任务：第 {n} 次，最多 {max} 次。",
    "zh-TW": "正在輪詢影片任務：第 {n} 次，最多 {max} 次。",
    "en": "Polling the video task: attempt {n} of at most {max}.",
    "ja": "動画タスクをポーリング中：{n} 回目（最大 {max} 回）。",
    "ko": "동영상 작업 폴링 중: {n}번째, 최대 {max}번."
  },
  "section.reg.title": {
    "zh-CN": "注册机",
    "zh-TW": "註冊機",
    "en": "Registrar",
    "ja": "登録マシン",
    "ko": "등록기"
  },
  "section.reg.desc": {
    "zh-CN": "自动铸 key 补池：按设定的目标数量，用邮箱通道注册新账号并取回 key。",
    "zh-TW": "自動鑄 key 補池：依設定的目標數量，用郵箱通道註冊新帳號並取回 key。",
    "en": "Automatic key minting: registers new accounts through a mailbox channel and collects their keys until the pool reaches its target size.",
    "ja": "キーの自動発行：メールチャネルで新しいアカウントを登録し、プールが目標数に達するまでキーを取得します。",
    "ko": "키 자동 발급: 메일 채널로 새 계정을 등록하고 풀이 목표 수량에 도달할 때까지 키를 회수합니다."
  },
  "section.reg.tab.status": {
    "zh-CN": "运行状态",
    "zh-TW": "執行狀態",
    "en": "Status",
    "ja": "稼働状況",
    "ko": "실행 상태"
  },
  "section.reg.tab.settings": {
    "zh-CN": "设置",
    "zh-TW": "設定",
    "en": "Settings",
    "ja": "設定",
    "ko": "설정"
  },
  "section.reg.refresh": {
    "zh-CN": "刷新",
    "zh-TW": "重新整理",
    "en": "Refresh",
    "ja": "更新",
    "ko": "새로 고침"
  },
  "section.reg.retry": {
    "zh-CN": "重试",
    "zh-TW": "重試",
    "en": "Retry",
    "ja": "再試行",
    "ko": "다시 시도"
  },
  "section.reg.loading": {
    "zh-CN": "正在读取注册机状态……",
    "zh-TW": "正在讀取註冊機狀態……",
    "en": "Loading registrar status…",
    "ja": "登録マシンの状態を読み込み中…",
    "ko": "등록기 상태를 불러오는 중…"
  },
  "section.reg.loadFailed": {
    "zh-CN": "读不出注册机状态。这一屏上的每个数字都来自那一次读取，所以下面一格都不画——画出来的只会是上一次的旧值。",
    "zh-TW": "讀不出註冊機狀態。這一屏上的每個數字都來自那一次讀取，所以下面一格都不畫——畫出來的只會是上一次的舊值。",
    "en": "The registrar status could not be read. Every number on this page comes from that one read, so nothing below is drawn: whatever appeared would be last time's stale value.",
    "ja": "登録マシンの状態を読み取れませんでした。この画面の数値はすべてその読み取りに由来するため、以下には何も表示しません（表示すれば前回の古い値になります）。",
    "ko": "등록기 상태를 읽지 못했습니다. 이 화면의 모든 숫자는 그 한 번의 읽기에서 나오므로 아래에는 아무것도 그리지 않습니다. 그렸다면 지난번의 낡은 값일 뿐입니다."
  },
  "section.reg.unknownHint": {
    "zh-CN": "这一格没读出来（不是 0）。",
    "zh-TW": "這一格沒讀出來（不是 0）。",
    "en": "This value could not be read (it is not zero).",
    "ja": "この値は読み取れませんでした（0 ではありません）。",
    "ko": "이 값은 읽지 못했습니다(0이 아닙니다)."
  },
  "section.reg.notWired.title": {
    "zh-CN": "这个部署没有接上注册机。",
    "zh-TW": "這個部署沒有接上註冊機。",
    "en": "This deployment has no registrar wired in.",
    "ja": "このデプロイには登録マシンが接続されていません。",
    "ko": "이 배포에는 등록기가 연결되어 있지 않습니다."
  },
  "section.reg.notWired.body": {
    "zh-CN": "这不是出错，也不是「功能正常」：网关本身一切照常，只是自动补池这一块在这套部署里没有装起来，所以下面那几块没有对象可显示。",
    "zh-TW": "這不是出錯，也不是「功能正常」：閘道本身一切照常，只是自動補池這一塊在這套部署裡沒有裝起來，所以下面那幾塊沒有對象可顯示。",
    "en": "This is neither an error nor a working feature: the gateway itself is fine, but automatic pool replenishment was never assembled in this deployment, so the panels below have nothing to describe.",
    "ja": "これはエラーでも「正常動作」でもありません。ゲートウェイ自体は正常ですが、このデプロイでは自動補充が組み込まれていないため、以下のパネルには表示すべき対象がありません。",
    "ko": "이것은 오류도 아니고 「정상 동작」도 아닙니다. 게이트웨이 자체는 정상이지만 이 배포에는 자동 보충이 조립되어 있지 않아 아래 영역에는 표시할 대상이 없습니다."
  },
  "section.reg.notWired.next": {
    "zh-CN": "因此这里刻意一颗动作按钮都不画：一颗点下去只会得到同一句话的按钮，比没有按钮更容易让人以为补池在跑。",
    "zh-TW": "因此這裡刻意一顆動作按鈕都不畫：一顆點下去只會得到同一句話的按鈕，比沒有按鈕更容易讓人以為補池在跑。",
    "en": "That is why no action button is drawn here on purpose: a button that only ever repeats this same sentence is more likely to suggest that replenishment is running than no button at all.",
    "ja": "そのため、ここには操作ボタンを意図的に一つも描いていません。押しても同じ文言しか返さないボタンは、ボタンが無い場合よりも「補充が動いている」と誤解されやすいからです。",
    "ko": "그래서 여기에는 의도적으로 동작 버튼을 하나도 그리지 않습니다. 눌러도 같은 문장만 돌려주는 버튼은 버튼이 아예 없을 때보다 「보충이 돌고 있다」는 오해를 더 쉽게 부릅니다."
  },
  "section.reg.notWired.recheck": {
    "zh-CN": "重新检查",
    "zh-TW": "重新檢查",
    "en": "Check again",
    "ja": "再確認",
    "ko": "다시 확인"
  },
  "section.reg.notWired.toast": {
    "zh-CN": "这一次什么都没发生：这个部署没有接上注册机。",
    "zh-TW": "這一次什麼都沒發生：這個部署沒有接上註冊機。",
    "en": "Nothing happened this time: this deployment has no registrar wired in.",
    "ja": "今回は何も起きていません。このデプロイには登録マシンが接続されていません。",
    "ko": "이번에는 아무 일도 일어나지 않았습니다. 이 배포에는 등록기가 연결되어 있지 않습니다."
  },
  "section.reg.state.title": {
    "zh-CN": "运行状态",
    "zh-TW": "執行狀態",
    "en": "Runtime state",
    "ja": "稼働状態",
    "ko": "실행 상태"
  },
  "section.reg.state.on": {
    "zh-CN": "已启用",
    "zh-TW": "已啟用",
    "en": "Enabled",
    "ja": "有効",
    "ko": "사용 중"
  },
  "section.reg.state.off": {
    "zh-CN": "已关闭",
    "zh-TW": "已關閉",
    "en": "Disabled",
    "ja": "無効",
    "ko": "꺼짐"
  },
  "section.reg.state.blocked": {
    "zh-CN": "开着，但这份配置装不起来",
    "zh-TW": "開著，但這份設定裝不起來",
    "en": "On, but this configuration will not assemble",
    "ja": "オンですが、この設定では組み立てられません",
    "ko": "켜져 있지만 이 설정으로는 조립되지 않습니다"
  },
  "section.reg.state.unknown": {
    "zh-CN": "状态未知",
    "zh-TW": "狀態未知",
    "en": "State unknown",
    "ja": "状態不明",
    "ko": "상태 불명"
  },
  "section.reg.channel.label": {
    "zh-CN": "本次使用的通道",
    "zh-TW": "本次使用的通道",
    "en": "Channel in use",
    "ja": "使用中のチャネル",
    "ko": "사용 중인 채널"
  },
  "section.reg.channel.none": {
    "zh-CN": "还没有选定通道",
    "zh-TW": "還沒有選定通道",
    "en": "No channel selected yet",
    "ja": "チャネル未選択",
    "ko": "아직 선택된 채널 없음"
  },
  "section.reg.emptyChannel": {
    "zh-CN": "注册机是开着的，但一条邮箱通道都没选中——这种状态下补池一轮都跑不起来。",
    "zh-TW": "註冊機是開著的，但一條郵箱通道都沒選中——這種狀態下補池一輪都跑不起來。",
    "en": "The registrar is on, but no mailbox channel is selected: in this state not a single replenishment round can run.",
    "ja": "登録マシンはオンですが、メールチャネルが一つも選択されていません。この状態では補充は一度も実行できません。",
    "ko": "등록기는 켜져 있지만 메일 채널이 하나도 선택되지 않았습니다. 이 상태에서는 보충이 한 번도 실행되지 않습니다."
  },
  "section.reg.quotaLabel": {
    "zh-CN": "今天还能手动补几次",
    "zh-TW": "今天還能手動補幾次",
    "en": "Manual runs left today",
    "ja": "本日の手動実行の残り",
    "ko": "오늘 남은 수동 실행 횟수"
  },
  "section.reg.quota": {
    "zh-CN": "还剩 {remaining} 次（每天 {perDay} 次）",
    "zh-TW": "還剩 {remaining} 次（每天 {perDay} 次）",
    "en": "{remaining} left (of {perDay} per day)",
    "ja": "残り {remaining} 回（1 日 {perDay} 回）",
    "ko": "{remaining}회 남음(하루 {perDay}회)"
  },
  "section.reg.cooldown": {
    "zh-CN": "手动补池还在冷却里：还要等 {left}，{at} 之后可以再点。",
    "zh-TW": "手動補池還在冷卻裡：還要等 {left}，{at} 之後可以再點。",
    "en": "Manual replenishment is cooling down: {left} to go, available again after {at}.",
    "ja": "手動補充はクールダウン中です：あと {left}、{at} 以降に再度実行できます。",
    "ko": "수동 보충이 쿨다운 중입니다: {left} 남았고 {at} 이후에 다시 실행할 수 있습니다."
  },
  "section.reg.locked": {
    "zh-CN": "另一个副本正攥着补池锁，{at} 之前这一轮不会重复跑。",
    "zh-TW": "另一個副本正攥著補池鎖，{at} 之前這一輪不會重複跑。",
    "en": "Another replica holds the replenishment lock; no duplicate round will start before {at}.",
    "ja": "別のレプリカが補充ロックを保持しています。{at} まで重複した実行は始まりません。",
    "ko": "다른 복제본이 보충 잠금을 쥐고 있습니다. {at} 전까지 중복 실행은 시작되지 않습니다."
  },
  "section.reg.backoff.edge": {
    "zh-CN": "上游边缘层正在限流，还要等 {left}（{at} 之后重试）。换一条邮箱通道逃不掉——限流发生在出口 IP 到上游那条边上。",
    "zh-TW": "上游邊緣層正在限流，還要等 {left}（{at} 之後重試）。換一條郵箱通道逃不掉——限流發生在出口 IP 到上游那條邊上。",
    "en": "The upstream edge layer is rate-limiting us; {left} to go (retry after {at}). Switching mailbox channels will not help: the limit sits on the egress IP to upstream edge.",
    "ja": "上流のエッジ層がレート制限中です。あと {left}（{at} 以降に再試行）。メールチャネルを変えても回避できません。制限は送信元 IP と上流の間にあります。",
    "ko": "상류 엣지 계층이 속도를 제한하고 있습니다. {left} 남음({at} 이후 재시도). 메일 채널을 바꿔도 소용없습니다. 제한은 출구 IP와 상류 사이에 걸려 있습니다."
  },
  "section.reg.backoff.app": {
    "zh-CN": "上游应用层明说了在限流，还要等 {left}（{at} 之后重试）。换一条邮箱通道逃不掉——限流发生在出口 IP 到上游那条边上。",
    "zh-TW": "上游應用層明說了在限流，還要等 {left}（{at} 之後重試）。換一條郵箱通道逃不掉——限流發生在出口 IP 到上游那條邊上。",
    "en": "The upstream application layer explicitly said it is rate-limiting us; {left} to go (retry after {at}). Switching mailbox channels will not help: the limit sits on the egress IP to upstream edge.",
    "ja": "上流のアプリケーション層が明示的にレート制限を通知しました。あと {left}（{at} 以降に再試行）。メールチャネルを変えても回避できません。制限は送信元 IP と上流の間にあります。",
    "ko": "상류 애플리케이션 계층이 속도 제한을 명시했습니다. {left} 남음({at} 이후 재시도). 메일 채널을 바꿔도 소용없습니다. 제한은 출구 IP와 상류 사이에 걸려 있습니다."
  },
  "section.reg.backoff.cluster": {
    "zh-CN": "这一轮的域名成片被判屏蔽而且零产出，看起来像在被限流——但我们的词表一个字都没命中，所以这是形状上的推断，不是上游的声明。还要等 {left}（{at} 之后重试）。换一条邮箱通道逃不掉。",
    "zh-TW": "這一輪的網域成片被判封鎖而且零產出，看起來像在被限流——但我們的詞表一個字都沒命中，所以這是形狀上的推斷，不是上游的聲明。還要等 {left}（{at} 之後重試）。換一條郵箱通道逃不掉。",
    "en": "A whole cluster of domains was judged blocked this round with zero output, which looks like rate limiting. Our word list matched nothing, so this is inferred from the shape, not stated by upstream. {left} to go (retry after {at}). Switching mailbox channels will not help.",
    "ja": "今回はドメインがまとまってブロック判定となり産出ゼロでした。レート制限のように見えますが、当方の語句リストは一つも一致していないため、これは形状からの推測であり上流の宣言ではありません。あと {left}（{at} 以降に再試行）。メールチャネルを変えても回避できません。",
    "ko": "이번 회차에는 도메인이 무더기로 차단 판정되고 산출이 0이었습니다. 속도 제한처럼 보이지만 우리 단어 목록은 하나도 일치하지 않았으므로 이는 형태로부터의 추론이지 상류의 선언이 아닙니다. {left} 남음({at} 이후 재시도). 메일 채널을 바꿔도 소용없습니다."
  },
  "section.reg.pool.title": {
    "zh-CN": "池子",
    "zh-TW": "池子",
    "en": "Pool",
    "ja": "プール",
    "ko": "풀"
  },
  "section.reg.pool.target": {
    "zh-CN": "目标数量",
    "zh-TW": "目標數量",
    "en": "Target size",
    "ja": "目標数",
    "ko": "목표 수량"
  },
  "section.reg.pool.counted": {
    "zh-CN": "占名额数",
    "zh-TW": "佔名額數",
    "en": "Slots taken",
    "ja": "枠の使用数",
    "ko": "차지한 정원 수"
  },
  "section.reg.pool.countedTip": {
    "zh-CN": "停用的与冷却中的都算在里面，它们都打不了上游。",
    "zh-TW": "停用的與冷卻中的都算在裡面，它們都打不了上游。",
    "en": "Disabled and cooling-down keys are counted here too; neither of them can reach upstream.",
    "ja": "無効化中とクールダウン中のキーもここに含まれます。どちらも上流には到達できません。",
    "ko": "비활성화된 키와 쿨다운 중인 키도 여기에 포함됩니다. 둘 다 상류에 도달할 수 없습니다."
  },
  "section.reg.pool.gap": {
    "zh-CN": "还差",
    "zh-TW": "還差",
    "en": "Short by",
    "ja": "不足",
    "ko": "부족분"
  },
  "section.reg.pool.fresh": {
    "zh-CN": "真正可用",
    "zh-TW": "真正可用",
    "en": "Actually usable",
    "ja": "実際に使用可能",
    "ko": "실제 사용 가능"
  },
  "section.reg.channels.title": {
    "zh-CN": "邮箱通道",
    "zh-TW": "郵箱通道",
    "en": "Mailbox channels",
    "ja": "メールチャネル",
    "ko": "메일 채널"
  },
  "section.reg.channels.hint": {
    "zh-CN": "两条通道完全平级，任选其一即可；「测试连接」每点一次都会向对应服务发一次真实请求。",
    "zh-TW": "兩條通道完全平級，任選其一即可；「測試連線」每點一次都會向對應服務發一次真實請求。",
    "en": "The two channels are strictly equal in rank; pick either one. Each click on Test connection sends one real request to that service.",
    "ja": "2 つのチャネルは完全に対等です。どちらを選んでも構いません。「接続テスト」は 1 回押すごとに該当サービスへ実際のリクエストを 1 件送信します。",
    "ko": "두 채널은 완전히 대등합니다. 어느 쪽이든 선택하면 됩니다. 「연결 테스트」는 누를 때마다 해당 서비스로 실제 요청을 한 번 보냅니다."
  },
  "section.reg.channel.inUseLabel": {
    "zh-CN": "本次使用",
    "zh-TW": "本次使用",
    "en": "Used this time",
    "ja": "今回の使用",
    "ko": "이번 사용 여부"
  },
  "section.reg.channel.inUse": {
    "zh-CN": "本次使用这条",
    "zh-TW": "本次使用這條",
    "en": "This one is in use",
    "ja": "今回はこちらを使用",
    "ko": "이번에는 이 채널 사용"
  },
  "section.reg.channel.unused": {
    "zh-CN": "本次没有用到",
    "zh-TW": "本次沒有用到",
    "en": "Not used this time",
    "ja": "今回は未使用",
    "ko": "이번에는 사용하지 않음"
  },
  "section.reg.channel.creds": {
    "zh-CN": "凭据",
    "zh-TW": "憑證",
    "en": "Credentials",
    "ja": "資格情報",
    "ko": "자격 증명"
  },
  "section.reg.channel.credsYes": {
    "zh-CN": "已配置",
    "zh-TW": "已設定",
    "en": "Configured",
    "ja": "設定済み",
    "ko": "설정됨"
  },
  "section.reg.channel.credsNo": {
    "zh-CN": "没配置",
    "zh-TW": "沒設定",
    "en": "Not configured",
    "ja": "未設定",
    "ko": "설정되지 않음"
  },
  "section.reg.channel.fact.moemail": {
    "zh-CN": "自建的临时邮箱服务：收信域名由你自己在它的后台登记，建邮箱不占活跃名额。",
    "zh-TW": "自建的臨時郵箱服務：收信網域由你自己在它的後台登記，建郵箱不佔活躍名額。",
    "en": "A self-hosted temporary mailbox service: you register the receiving domains in its own console, and creating a mailbox takes no active slot.",
    "ja": "自前でホストする一時メールサービスです。受信ドメインは自分でその管理画面に登録し、メールボックスの作成でアクティブ枠を消費しません。",
    "ko": "직접 호스팅하는 임시 메일 서비스입니다. 수신 도메인은 자체 콘솔에서 직접 등록하며, 메일박스를 만들어도 활성 정원을 쓰지 않습니다."
  },
  "section.reg.channel.fact.yyds": {
    "zh-CN": "第三方临时邮箱服务：收信域名由它自己提供，活跃邮箱有名额上限，建一个就占一个。",
    "zh-TW": "第三方臨時郵箱服務：收信網域由它自己提供，活躍郵箱有名額上限，建一個就佔一個。",
    "en": "A third-party temporary mailbox service: it supplies the receiving domains itself, and active mailboxes are capped, so each one you create takes a slot.",
    "ja": "サードパーティの一時メールサービスです。受信ドメインはサービス側が提供し、アクティブなメールボックスには上限があるため、作成するたびに枠を 1 つ消費します。",
    "ko": "서드파티 임시 메일 서비스입니다. 수신 도메인은 서비스가 제공하며 활성 메일박스에 상한이 있어 하나 만들 때마다 정원을 하나 차지합니다."
  },
  "section.reg.channel.test": {
    "zh-CN": "测试连接",
    "zh-TW": "測試連線",
    "en": "Test connection",
    "ja": "接続テスト",
    "ko": "연결 테스트"
  },
  "section.reg.channel.testing": {
    "zh-CN": "正在测试……",
    "zh-TW": "正在測試……",
    "en": "Testing…",
    "ja": "テスト中…",
    "ko": "테스트 중…"
  },
  "section.reg.channel.testOk": {
    "zh-CN": "通了：读到 {domains} 个域名，凭据也验过了（{ms} ms）。",
    "zh-TW": "通了：讀到 {domains} 個網域，憑證也驗過了（{ms} ms）。",
    "en": "Reachable: {domains} domains read and credentials verified ({ms} ms).",
    "ja": "接続成功：{domains} 件のドメインを取得し、資格情報も検証済みです（{ms} ms）。",
    "ko": "연결 성공: 도메인 {domains}개를 읽었고 자격 증명도 검증했습니다({ms} ms)."
  },
  "section.reg.channel.testOkNoDomains": {
    "zh-CN": "接口通了，但一个可用域名都没读到——这条通道现在补不了池（{ms} ms）。",
    "zh-TW": "介面通了，但一個可用網域都沒讀到——這條通道現在補不了池（{ms} ms）。",
    "en": "The API answered, but not a single usable domain came back, so this channel cannot replenish anything right now ({ms} ms).",
    "ja": "API は応答しましたが、利用可能なドメインが一つも返りませんでした。このチャネルでは現在補充できません（{ms} ms）。",
    "ko": "API는 응답했지만 사용 가능한 도메인이 하나도 오지 않았습니다. 이 채널로는 지금 보충할 수 없습니다({ms} ms)."
  },
  "section.reg.channel.testOkUnverified": {
    "zh-CN": "读到 {domains} 个域名，但这一次没有验凭据（{ms} ms）。",
    "zh-TW": "讀到 {domains} 個網域，但這一次沒有驗憑證（{ms} ms）。",
    "en": "{domains} domains were read, but the credentials were not checked this time ({ms} ms).",
    "ja": "{domains} 件のドメインを取得しましたが、今回は資格情報を検証していません（{ms} ms）。",
    "ko": "도메인 {domains}개를 읽었지만 이번에는 자격 증명을 검증하지 않았습니다({ms} ms)."
  },
  "section.reg.channel.testOkDirty": {
    "zh-CN": "通了（{domains} 个域名，{ms} ms），但这次建出来的临时邮箱没能删掉——它正占着一个活跃名额，去事件里查删除失败那条。",
    "zh-TW": "通了（{domains} 個網域，{ms} ms），但這次建出來的臨時郵箱沒能刪掉——它正佔著一個活躍名額，去事件裡查刪除失敗那條。",
    "en": "Reachable ({domains} domains, {ms} ms), but the temporary mailbox created for this test could not be deleted; it is holding an active slot. Look for the deletion-failure event in the log.",
    "ja": "接続成功（ドメイン {domains} 件、{ms} ms）ですが、このテストで作成した一時メールボックスを削除できませんでした。アクティブ枠を 1 つ占有しています。ログで削除失敗のイベントを確認してください。",
    "ko": "연결 성공(도메인 {domains}개, {ms} ms)이지만 이 테스트로 만든 임시 메일박스를 삭제하지 못했습니다. 활성 정원을 하나 차지하고 있습니다. 로그에서 삭제 실패 이벤트를 확인하세요."
  },
  "section.reg.channel.testNotAttempted": {
    "zh-CN": "一次上游请求都没发出去：读本网关自己的配置就失败了（{ms} ms）。要查的是这一侧，不是对面。",
    "zh-TW": "一次上游請求都沒發出去：讀本閘道自己的設定就失敗了（{ms} ms）。要查的是這一側，不是對面。",
    "en": "Not a single upstream request was sent: reading this gateway's own configuration already failed ({ms} ms). Look on this side, not at the other end.",
    "ja": "上流へのリクエストは一度も送信されていません。このゲートウェイ自身の設定の読み取り段階で失敗しました（{ms} ms）。調べるべきはこちら側です。",
    "ko": "상류 요청은 한 번도 보내지 않았습니다. 이 게이트웨이 자체 설정을 읽는 단계에서 이미 실패했습니다({ms} ms). 확인할 곳은 반대편이 아니라 이쪽입니다."
  },
  "section.reg.channel.testRejected": {
    "zh-CN": "上游拒绝了这套凭据（HTTP {status}）。",
    "zh-TW": "上游拒絕了這套憑證（HTTP {status}）。",
    "en": "Upstream rejected these credentials (HTTP {status}).",
    "ja": "上流がこの資格情報を拒否しました（HTTP {status}）。",
    "ko": "상류가 이 자격 증명을 거부했습니다(HTTP {status})."
  },
  "section.reg.channel.testRateLimited": {
    "zh-CN": "上游正在限流这条通道，隔一会儿再试。",
    "zh-TW": "上游正在限流這條通道，隔一會兒再試。",
    "en": "Upstream is rate-limiting this channel; try again in a while.",
    "ja": "上流がこのチャネルをレート制限しています。しばらくしてから再試行してください。",
    "ko": "상류가 이 채널을 속도 제한하고 있습니다. 잠시 후 다시 시도하세요."
  },
  "section.reg.channel.testFailed": {
    "zh-CN": "上游回了 HTTP {status}（{ms} ms）。",
    "zh-TW": "上游回了 HTTP {status}（{ms} ms）。",
    "en": "Upstream answered HTTP {status} ({ms} ms).",
    "ja": "上流が HTTP {status} を返しました（{ms} ms）。",
    "ko": "상류가 HTTP {status}를 반환했습니다({ms} ms)."
  },
  "section.reg.channel.testFailedNoStatus": {
    "zh-CN": "这一次连状态码都没拿到（{ms} ms）。请求可能压根没走通，也可能上游回了 2xx 但正文不是我们能解析的东西（反代或 CDN 的错误页就是这样）——两种都落在这一档里。",
    "zh-TW": "這一次連狀態碼都沒拿到（{ms} ms）。請求可能壓根沒走通，也可能上游回了 2xx 但正文不是我們能解析的東西（反向代理或 CDN 的錯誤頁就是這樣）——兩種都落在這一檔裡。",
    "en": "No status code came back at all this time ({ms} ms). The request may never have gone through, or upstream may have answered 2xx with a body we cannot parse (a reverse-proxy or CDN error page does exactly that). Both land in this bucket.",
    "ja": "今回はステータスコードすら取得できませんでした（{ms} ms）。リクエストが通らなかった可能性も、上流が 2xx を返しつつ解析できない本文を返した可能性（リバースプロキシや CDN のエラーページがこれに当たります）もあり、どちらもこの区分に入ります。",
    "ko": "이번에는 상태 코드조차 받지 못했습니다({ms} ms). 요청이 아예 통하지 않았을 수도 있고, 상류가 2xx를 반환하면서 우리가 해석할 수 없는 본문을 보냈을 수도 있습니다(리버스 프록시나 CDN 오류 페이지가 그렇습니다). 둘 다 이 구간에 들어갑니다."
  },
  "section.reg.channel.testError": {
    "zh-CN": "这一次测试没有结果，原因说不准。",
    "zh-TW": "這一次測試沒有結果，原因說不準。",
    "en": "This test produced no result, and we cannot say why.",
    "ja": "今回のテストは結果が得られず、理由も特定できません。",
    "ko": "이번 테스트는 결과가 없었고 이유도 특정할 수 없습니다."
  },
  "section.reg.domains.title": {
    "zh-CN": "域名台账",
    "zh-TW": "網域台帳",
    "en": "Domain ledger",
    "ja": "ドメイン台帳",
    "ko": "도메인 장부"
  },
  "section.reg.domains.note": {
    "zh-CN": "「判死」是我们按一张启发式词表做的判定，不是上游的声明——它会误判。",
    "zh-TW": "「判死」是我們按一張啟發式詞表做的判定，不是上游的聲明——它會誤判。",
    "en": "Blocked here is our own verdict from a heuristic word list, not a statement by upstream, and it does get things wrong.",
    "ja": "ここでの「ブロック」は当方がヒューリスティックな語句リストで下した判定であり、上流の宣言ではありません。誤判定も起こります。",
    "ko": "여기서 「차단」은 우리가 휴리스틱 단어 목록으로 내린 판정이며 상류의 선언이 아닙니다. 오판도 일어납니다."
  },
  "section.reg.domains.ok": {
    "zh-CN": "还能用",
    "zh-TW": "還能用",
    "en": "Usable",
    "ja": "使用可能",
    "ko": "사용 가능"
  },
  "section.reg.domains.blocked": {
    "zh-CN": "判为屏蔽",
    "zh-TW": "判為封鎖",
    "en": "Judged blocked",
    "ja": "ブロック判定",
    "ko": "차단 판정"
  },
  "section.reg.domains.suspect": {
    "zh-CN": "可疑",
    "zh-TW": "可疑",
    "en": "Suspect",
    "ja": "疑わしい",
    "ko": "의심"
  },
  "section.reg.domains.unknown": {
    "zh-CN": "还没试过",
    "zh-TW": "還沒試過",
    "en": "Never tried",
    "ja": "未試行",
    "ko": "아직 시도 안 함"
  },
  "section.reg.domains.updatedAt": {
    "zh-CN": "台账更新于 {at}",
    "zh-TW": "台帳更新於 {at}",
    "en": "Ledger updated at {at}",
    "ja": "台帳の更新時刻：{at}",
    "ko": "장부 갱신 시각: {at}"
  },
  "section.reg.history.title": {
    "zh-CN": "补池历史",
    "zh-TW": "補池歷史",
    "en": "Replenishment history",
    "ja": "補充の履歴",
    "ko": "보충 이력"
  },
  "section.reg.history.empty": {
    "zh-CN": "这个环里还没有任何一轮记录。",
    "zh-TW": "這個環裡還沒有任何一輪記錄。",
    "en": "This ring buffer holds no round yet.",
    "ja": "このリングバッファにはまだ 1 回分の記録もありません。",
    "ko": "이 링 버퍼에는 아직 한 회차의 기록도 없습니다."
  },
  "section.reg.history.unreadable": {
    "zh-CN": "补池历史整块没读出来——这与「还没跑过」不是一回事。",
    "zh-TW": "補池歷史整塊沒讀出來——這與「還沒跑過」不是一回事。",
    "en": "The whole replenishment history could not be read, which is not the same thing as never having run.",
    "ja": "補充履歴のブロック全体を読み取れませんでした。これは「まだ一度も実行していない」とは別のことです。",
    "ko": "보충 이력 블록 전체를 읽지 못했습니다. 이는 「아직 한 번도 실행하지 않았다」와 다른 이야기입니다."
  },
  "section.reg.history.malformed": {
    "zh-CN": "环里有 {count} 条记录读不出来，它们没有出现在下面的表里。",
    "zh-TW": "環裡有 {count} 條記錄讀不出來，它們沒有出現在下面的表裡。",
    "en": "{count} records in the ring could not be read; they are absent from the table below.",
    "ja": "リング内の {count} 件の記録が読み取れず、下の表には現れていません。",
    "ko": "링 안의 기록 {count}건을 읽지 못했고, 아래 표에는 나타나지 않습니다."
  },
  "section.reg.col.at": {
    "zh-CN": "时刻",
    "zh-TW": "時刻",
    "en": "When",
    "ja": "時刻",
    "ko": "시각"
  },
  "section.reg.col.trigger": {
    "zh-CN": "触发",
    "zh-TW": "觸發",
    "en": "Trigger",
    "ja": "トリガー",
    "ko": "트리거"
  },
  "section.reg.col.channel": {
    "zh-CN": "用的通道",
    "zh-TW": "用的通道",
    "en": "Channel used",
    "ja": "使用チャネル",
    "ko": "사용 채널"
  },
  "section.reg.col.result": {
    "zh-CN": "结果",
    "zh-TW": "結果",
    "en": "Result",
    "ja": "結果",
    "ko": "결과"
  },
  "section.reg.col.duration": {
    "zh-CN": "耗时",
    "zh-TW": "耗時",
    "en": "Duration",
    "ja": "所要時間",
    "ko": "소요 시간"
  },
  "section.reg.col.failures": {
    "zh-CN": "失败归因",
    "zh-TW": "失敗歸因",
    "en": "Failure reasons",
    "ja": "失敗の原因",
    "ko": "실패 원인"
  },
  "section.reg.trigger.manual": {
    "zh-CN": "人点的",
    "zh-TW": "人點的",
    "en": "Manual",
    "ja": "手動",
    "ko": "수동"
  },
  "section.reg.trigger.cron": {
    "zh-CN": "定时",
    "zh-TW": "定時",
    "en": "Scheduled",
    "ja": "定期実行",
    "ko": "정기 실행"
  },
  "section.reg.row.minted": {
    "zh-CN": "铸出 {minted} 把 / 尝试 {attempted} 次",
    "zh-TW": "鑄出 {minted} 把 / 嘗試 {attempted} 次",
    "en": "{minted} minted out of {attempted} attempts",
    "ja": "{attempted} 回の試行で {minted} 本を発行",
    "ko": "{attempted}회 시도 중 {minted}개 발급"
  },
  "section.reg.row.noAttempt": {
    "zh-CN": "这一轮根本没跑起来（池子已满，或还在退避窗口里）",
    "zh-TW": "這一輪根本沒跑起來（池子已滿，或還在退避視窗裡）",
    "en": "This round never got going (pool already full, or still inside a backoff window)",
    "ja": "この回は開始すらしていません（プールが満杯、またはバックオフ期間中）",
    "ko": "이 회차는 시작조차 하지 않았습니다(풀이 가득 찼거나 백오프 구간 중)"
  },
  "section.reg.row.skipped": {
    "zh-CN": "一次上游请求都没发（配置在两步之间被改掉了）",
    "zh-TW": "一次上游請求都沒發（設定在兩步之間被改掉了）",
    "en": "Not one upstream request was sent (the configuration changed between the two steps)",
    "ja": "上流へのリクエストは一度も送信されていません（2 つの手順の間に設定が変更されました）",
    "ko": "상류 요청을 한 번도 보내지 않았습니다(두 단계 사이에 설정이 바뀌었습니다)"
  },
  "section.reg.row.unreadable": {
    "zh-CN": "这一轮的记录读不出来",
    "zh-TW": "這一輪的記錄讀不出來",
    "en": "This round's record could not be read",
    "ja": "この回の記録は読み取れません",
    "ko": "이 회차의 기록은 읽을 수 없습니다"
  },
  "section.reg.fail.domainBlockedAll": {
    "zh-CN": "试过的域名全被判屏蔽",
    "zh-TW": "試過的網域全被判封鎖",
    "en": "Every domain tried was judged blocked",
    "ja": "試したドメインがすべてブロック判定",
    "ko": "시도한 도메인이 모두 차단 판정"
  },
  "section.reg.fail.upstreamError": {
    "zh-CN": "上游报错",
    "zh-TW": "上游報錯",
    "en": "Upstream error",
    "ja": "上流のエラー",
    "ko": "상류 오류"
  },
  "section.reg.fail.codeTimeout": {
    "zh-CN": "等验证码超时",
    "zh-TW": "等驗證碼逾時",
    "en": "Timed out waiting for the code",
    "ja": "認証コードの待機がタイムアウト",
    "ko": "인증 코드 대기 시간 초과"
  },
  "section.reg.fail.registerFailed": {
    "zh-CN": "注册这一步失败",
    "zh-TW": "註冊這一步失敗",
    "en": "The sign-up step failed",
    "ja": "登録のステップが失敗",
    "ko": "가입 단계 실패"
  },
  "section.reg.fail.loginFailed": {
    "zh-CN": "登录这一步失败",
    "zh-TW": "登入這一步失敗",
    "en": "The sign-in step failed",
    "ja": "ログインのステップが失敗",
    "ko": "로그인 단계 실패"
  },
  "section.reg.fail.keyFailed": {
    "zh-CN": "取 key 这一步失败",
    "zh-TW": "取 key 這一步失敗",
    "en": "Fetching the key failed",
    "ja": "キーの取得が失敗",
    "ko": "키 가져오기 실패"
  },
  "section.reg.fail.providerError": {
    "zh-CN": "邮箱服务报错",
    "zh-TW": "郵箱服務報錯",
    "en": "The mailbox service errored",
    "ja": "メールサービスがエラー",
    "ko": "메일 서비스 오류"
  },
  "section.reg.fail.networkError": {
    "zh-CN": "网络断了",
    "zh-TW": "網路斷了",
    "en": "The network broke",
    "ja": "ネットワーク障害",
    "ko": "네트워크 끊김"
  },
  "section.reg.fail.rateLimited": {
    "zh-CN": "被限流",
    "zh-TW": "被限流",
    "en": "Rate limited",
    "ja": "レート制限",
    "ko": "속도 제한"
  },
  "section.reg.fail.providerMissing": {
    "zh-CN": "这条通道没配凭据",
    "zh-TW": "這條通道沒設定憑證",
    "en": "That channel has no credentials configured",
    "ja": "そのチャネルに資格情報が未設定",
    "ko": "해당 채널에 자격 증명이 설정되지 않음"
  },
  "section.reg.fail.roundCrashed": {
    "zh-CN": "整轮抛错",
    "zh-TW": "整輪拋錯",
    "en": "The whole round threw",
    "ja": "回全体が例外で停止",
    "ko": "회차 전체가 예외로 중단"
  },
  "section.reg.fail.keySuspicious": {
    "zh-CN": "铸出来的 key 形状可疑，没有收下",
    "zh-TW": "鑄出來的 key 形狀可疑，沒有收下",
    "en": "The minted key looked wrong and was not accepted",
    "ja": "発行されたキーの形が不審なため受け取りませんでした",
    "ko": "발급된 키의 형태가 수상해 받아들이지 않았습니다"
  },
  "section.reg.fail.upstreamBackoff": {
    "zh-CN": "还在上游退避窗口里",
    "zh-TW": "還在上游退避視窗裡",
    "en": "Still inside the upstream backoff window",
    "ja": "上流のバックオフ期間中",
    "ko": "상류 백오프 구간 중"
  },
  "section.reg.fail.unknownReason": {
    "zh-CN": "这个面板版本不认识的归因：{reason}",
    "zh-TW": "這個面板版本不認識的歸因：{reason}",
    "en": "A reason this panel version does not recognise: {reason}",
    "ja": "このパネルのバージョンが認識できない原因：{reason}",
    "ko": "이 패널 버전이 알지 못하는 원인: {reason}"
  },
  "section.reg.tend.button": {
    "zh-CN": "立即补池",
    "zh-TW": "立即補池",
    "en": "Replenish now",
    "ja": "今すぐ補充",
    "ko": "지금 보충"
  },
  "section.reg.tend.confirmTitle": {
    "zh-CN": "确认补一轮",
    "zh-TW": "確認補一輪",
    "en": "Confirm one round",
    "ja": "1 回分の補充を確認",
    "ko": "한 회차 보충 확인"
  },
  "section.reg.tend.confirmMsg": {
    "zh-CN": "这一次最多铸 {keys} 把 key，并为此建 {mailboxes} 个临时邮箱。",
    "zh-TW": "這一次最多鑄 {keys} 把 key，並為此建 {mailboxes} 個臨時郵箱。",
    "en": "This run will mint at most {keys} keys and create {mailboxes} temporary mailboxes for them.",
    "ja": "今回は最大 {keys} 本のキーを発行し、そのために {mailboxes} 個の一時メールボックスを作成します。",
    "ko": "이번 실행은 최대 {keys}개의 키를 발급하고 이를 위해 임시 메일박스 {mailboxes}개를 만듭니다."
  },
  "section.reg.tend.confirmUnknown": {
    "zh-CN": "这一次会消耗多少说不准：池子那一块没读出来，所以算不出上界。",
    "zh-TW": "這一次會消耗多少說不準：池子那一塊沒讀出來，所以算不出上界。",
    "en": "How much this run will consume cannot be stated: the pool block could not be read, so no upper bound can be computed.",
    "ja": "今回の消費量は特定できません。プールのブロックを読み取れなかったため、上限を算出できません。",
    "ko": "이번 실행의 소모량은 특정할 수 없습니다. 풀 블록을 읽지 못해 상한을 계산할 수 없습니다."
  },
  "section.reg.tend.confirmWait": {
    "zh-CN": "点下去之后这颗按钮会一直挂到整轮跑完，最长约两分钟。",
    "zh-TW": "點下去之後這顆按鈕會一直掛到整輪跑完，最長約兩分鐘。",
    "en": "After you confirm, this action stays pending until the whole round finishes, up to about two minutes.",
    "ja": "確認後、この操作は 1 回分が完了するまで（最長で約 2 分）保留されたままになります。",
    "ko": "확인한 뒤에는 이 작업이 한 회차가 끝날 때까지(최대 약 2분) 대기 상태로 남습니다."
  },
  "section.reg.tend.channelLabel": {
    "zh-CN": "这一次用哪条通道",
    "zh-TW": "這一次用哪條通道",
    "en": "Which channel for this run",
    "ja": "今回使用するチャネル",
    "ko": "이번에 사용할 채널"
  },
  "section.reg.tend.channelAny": {
    "zh-CN": "按设置里选中的那条",
    "zh-TW": "按設定裡選中的那條",
    "en": "Whichever one the settings select",
    "ja": "設定で選択されているものに従う",
    "ko": "설정에서 선택된 채널을 따름"
  },
  "section.reg.tend.skipped": {
    "zh-CN": "这一轮一次上游请求都没发：配置在两步之间被改掉了。",
    "zh-TW": "這一輪一次上游請求都沒發：設定在兩步之間被改掉了。",
    "en": "This round sent no upstream request at all: the configuration changed between the two steps.",
    "ja": "この回は上流へのリクエストを一度も送信していません。2 つの手順の間に設定が変更されました。",
    "ko": "이 회차는 상류 요청을 한 번도 보내지 않았습니다. 두 단계 사이에 설정이 바뀌었습니다."
  },
  "section.reg.tend.crashed": {
    "zh-CN": "这一轮整个抛错了，而且可能已经建出了临时邮箱——去补池历史和事件里核对一下。",
    "zh-TW": "這一輪整個拋錯了，而且可能已經建出了臨時郵箱——去補池歷史和事件裡核對一下。",
    "en": "This round threw outright, and temporary mailboxes may already have been created. Check the replenishment history and the log.",
    "ja": "この回は例外で停止しました。一時メールボックスがすでに作成されている可能性があります。補充履歴とログを確認してください。",
    "ko": "이 회차는 예외로 중단되었습니다. 임시 메일박스가 이미 만들어졌을 수 있습니다. 보충 이력과 로그를 확인하세요."
  },
  "section.reg.tend.unknownOutcome": {
    "zh-CN": "这一轮回来了，但它的结果读不出来——去补池历史里看那一行。",
    "zh-TW": "這一輪回來了，但它的結果讀不出來——去補池歷史裡看那一行。",
    "en": "The round came back, but its outcome could not be read. Look for its row in the replenishment history.",
    "ja": "この回は応答しましたが、結果を読み取れませんでした。補充履歴の該当行を確認してください。",
    "ko": "이 회차는 응답했지만 결과를 읽지 못했습니다. 보충 이력의 해당 행을 확인하세요."
  },
  "section.reg.tend.unknownResult": {
    "zh-CN": "请求断了，结果未知——那一轮可能仍在跑，去补池历史里看。",
    "zh-TW": "請求斷了，結果未知——那一輪可能仍在跑，去補池歷史裡看。",
    "en": "The request broke off and the outcome is unknown; that round may still be running. Check the replenishment history.",
    "ja": "リクエストが切断され、結果は不明です。その回はまだ実行中の可能性があります。補充履歴を確認してください。",
    "ko": "요청이 끊겨 결과를 알 수 없습니다. 그 회차는 아직 실행 중일 수 있습니다. 보충 이력을 확인하세요."
  },
  "section.reg.tend.failed": {
    "zh-CN": "这一次补池被拒绝了，但后端没有说明原因。",
    "zh-TW": "這一次補池被拒絕了，但後端沒有說明原因。",
    "en": "This replenishment was refused, but the backend gave no reason.",
    "ja": "今回の補充は拒否されましたが、バックエンドは理由を示していません。",
    "ko": "이번 보충은 거부되었지만 백엔드가 이유를 밝히지 않았습니다."
  },
  "section.reg.refuse.disabled": {
    "zh-CN": "注册机没有打开，请先在设置里打开它。",
    "zh-TW": "註冊機沒有打開，請先在設定裡打開它。",
    "en": "The registrar is switched off; turn it on in the settings first.",
    "ja": "登録マシンがオフです。まず設定でオンにしてください。",
    "ko": "등록기가 꺼져 있습니다. 먼저 설정에서 켜 주세요."
  },
  "section.reg.refuse.blocked": {
    "zh-CN": "注册机的开关是开的，但这份配置装不起来——要改的是配置本身，不是那个开关。",
    "zh-TW": "註冊機的開關是開的，但這份設定裝不起來——要改的是設定本身，不是那個開關。",
    "en": "The registrar switch is on, but this configuration will not assemble. What needs fixing is the configuration itself, not the switch.",
    "ja": "登録マシンのスイッチはオンですが、この設定では組み立てられません。直すべきはスイッチではなく設定そのものです。",
    "ko": "등록기 스위치는 켜져 있지만 이 설정으로는 조립되지 않습니다. 고쳐야 할 것은 스위치가 아니라 설정 자체입니다."
  },
  "section.reg.refuse.unknownChannel": {
    "zh-CN": "这个通道名后端不认识。",
    "zh-TW": "這個通道名後端不認識。",
    "en": "The backend does not recognise that channel name.",
    "ja": "そのチャネル名をバックエンドが認識できません。",
    "ko": "백엔드가 그 채널 이름을 인식하지 못합니다."
  },
  "section.reg.refuse.channelNotConfigured": {
    "zh-CN": "这条通道还没配凭据。",
    "zh-TW": "這條通道還沒設定憑證。",
    "en": "That channel has no credentials configured yet.",
    "ja": "そのチャネルにはまだ資格情報が設定されていません。",
    "ko": "그 채널에는 아직 자격 증명이 설정되지 않았습니다."
  },
  "section.reg.refuse.notWired": {
    "zh-CN": "这个部署没有接上注册机。",
    "zh-TW": "這個部署沒有接上註冊機。",
    "en": "This deployment has no registrar wired in.",
    "ja": "このデプロイには登録マシンが接続されていません。",
    "ko": "이 배포에는 등록기가 연결되어 있지 않습니다."
  },
  "section.reg.refuse.tendInFlight": {
    "zh-CN": "已经有一轮补池在跑，等它回来再点。",
    "zh-TW": "已經有一輪補池在跑，等它回來再點。",
    "en": "A replenishment round is already running; wait for it to come back.",
    "ja": "すでに補充が 1 回分実行中です。完了を待ってください。",
    "ko": "이미 보충 한 회차가 실행 중입니다. 끝날 때까지 기다려 주세요."
  },
  "section.reg.refuse.locked": {
    "zh-CN": "另一个副本正攥着补池锁，这一轮没有重复跑。",
    "zh-TW": "另一個副本正攥著補池鎖，這一輪沒有重複跑。",
    "en": "Another replica holds the replenishment lock, so this round did not run twice.",
    "ja": "別のレプリカが補充ロックを保持しているため、この回は重複実行されていません。",
    "ko": "다른 복제본이 보충 잠금을 쥐고 있어 이 회차는 중복 실행되지 않았습니다."
  },
  "section.reg.refuse.manualCooldown": {
    "zh-CN": "手动补池还在冷却里，等冷却过去再点。",
    "zh-TW": "手動補池還在冷卻裡，等冷卻過去再點。",
    "en": "Manual replenishment is still cooling down; wait for the cooldown to pass.",
    "ja": "手動補充はまだクールダウン中です。クールダウンが明けるまでお待ちください。",
    "ko": "수동 보충이 아직 쿨다운 중입니다. 쿨다운이 끝날 때까지 기다려 주세요."
  },
  "section.reg.refuse.budgetExhausted": {
    "zh-CN": "今天的手动补池次数已经用完了。",
    "zh-TW": "今天的手動補池次數已經用完了。",
    "en": "Today's manual replenishment allowance is used up.",
    "ja": "本日の手動補充の回数を使い切りました。",
    "ko": "오늘의 수동 보충 횟수를 모두 사용했습니다."
  },
  "section.reg.refuse.probeInFlight": {
    "zh-CN": "同一条通道的上一次测试还没回来，等它回来再点。",
    "zh-TW": "同一條通道的上一次測試還沒回來，等它回來再點。",
    "en": "The previous test on this same channel has not come back yet; wait for it.",
    "ja": "同じチャネルの前回のテストがまだ完了していません。完了を待ってください。",
    "ko": "같은 채널의 이전 테스트가 아직 돌아오지 않았습니다. 완료를 기다려 주세요."
  },
  "section.reg.refuse.probeCooldown": {
    "zh-CN": "刚测过这条通道，隔几秒再来。",
    "zh-TW": "剛測過這條通道，隔幾秒再來。",
    "en": "This channel was just tested; come back in a few seconds.",
    "ja": "このチャネルは先ほどテストしたばかりです。数秒後に再試行してください。",
    "ko": "이 채널은 방금 테스트했습니다. 몇 초 후에 다시 시도하세요."
  },
  "section.reg.settings.absent": {
    "zh-CN": "这套部署里还没有注册机配置这回事。",
    "zh-TW": "這套部署裡還沒有註冊機設定這回事。",
    "en": "There is no such thing as registrar configuration in this deployment yet.",
    "ja": "このデプロイには登録マシンの設定という概念がまだ存在しません。",
    "ko": "이 배포에는 등록기 설정이라는 것이 아직 존재하지 않습니다."
  },
  "section.reg.settings.absentBody": {
    "zh-CN": "所以这里刻意不摆一排能填能存的输入框：那些字段在后端根本没有对应物，填完保存只会得到一份自相矛盾的配置。",
    "zh-TW": "所以這裡刻意不擺一排能填能存的輸入框：那些欄位在後端根本沒有對應物，填完儲存只會得到一份自相矛盾的設定。",
    "en": "That is why no row of fillable, savable inputs is shown here: those fields have no counterpart in the backend, and saving them would only produce a configuration that contradicts itself.",
    "ja": "そのため、入力して保存できるフォームを意図的に置いていません。これらの項目はバックエンドに対応物が無く、保存しても矛盾した設定が出来上がるだけです。",
    "ko": "그래서 입력하고 저장할 수 있는 입력란을 의도적으로 두지 않았습니다. 그 항목들은 백엔드에 대응물이 없어 저장해도 스스로 모순되는 설정만 남습니다."
  },
  "section.reg.durationDays": {
    "zh-CN": "{d} 天 {h} 小时",
    "zh-TW": "{d} 天 {h} 小時",
    "en": "{d}d {h}h",
    "ja": "{d}日{h}時間",
    "ko": "{d}일 {h}시간"
  },
  "section.reg.durationHours": {
    "zh-CN": "{h} 小时 {m} 分",
    "zh-TW": "{h} 小時 {m} 分",
    "en": "{h}h {m}m",
    "ja": "{h}時間{m}分",
    "ko": "{h}시간 {m}분"
  },
  "section.reg.durationMinutes": {
    "zh-CN": "{m} 分 {s} 秒",
    "zh-TW": "{m} 分 {s} 秒",
    "en": "{m}m {s}s",
    "ja": "{m}分{s}秒",
    "ko": "{m}분 {s}초"
  },
  "section.set.title": {
    "zh-CN": "设置",
    "zh-TW": "設定",
    "en": "Settings",
    "ja": "設定画面",
    "ko": "설정 화면"
  },
  "section.set.reload": {
    "zh-CN": "重新读取",
    "zh-TW": "重新讀取",
    "en": "Reload",
    "ja": "再読み込み",
    "ko": "다시 읽기"
  },
  "section.set.loadFailedHttp": {
    "zh-CN": "配置没读出来（接口返回了错误）。在读出来之前，这一页不会显示任何值。",
    "zh-TW": "設定沒讀出來（介面回傳了錯誤）。在讀出來之前，這一頁不會顯示任何值。",
    "en": "The configuration could not be read (the endpoint returned an error). No value is shown until it can be read.",
    "ja": "設定を読み取れませんでした（エンドポイントがエラーを返しました）。読み取れるまで値は表示しません。",
    "ko": "설정을 읽지 못했습니다(엔드포인트가 오류를 반환). 읽을 수 있을 때까지 값을 표시하지 않습니다."
  },
  "section.set.loadFailedNetwork": {
    "zh-CN": "配置没读出来（请求没发出去或没回来）。在读出来之前，这一页不会显示任何值。",
    "zh-TW": "設定沒讀出來（請求沒發出去或沒回來）。在讀出來之前，這一頁不會顯示任何值。",
    "en": "The configuration could not be read (the request never completed). No value is shown until it can be read.",
    "ja": "設定を読み取れませんでした（リクエストが完了しませんでした）。読み取れるまで値は表示しません。",
    "ko": "설정을 읽지 못했습니다(요청이 완료되지 않음). 읽을 수 있을 때까지 값을 표시하지 않습니다."
  },
  "section.set.diagnostic": {
    "zh-CN": "存储里那份配置读得出来，却构造不出一份合法配置。这一页已经降级成诊断视图：先按下面的原因把缺的那几格补回来。",
    "zh-TW": "儲存裡那份設定讀得出來，卻建構不出一份合法設定。這一頁已經降級成診斷檢視：先按下面的原因把缺的那幾格補回來。",
    "en": "The stored configuration can be read but does not produce a valid config. This page has degraded to a diagnostic view — fix the causes listed below first.",
    "ja": "保存された設定は読めますが、有効な設定を構築できません。この画面は診断ビューに縮退しています —— 下の原因から直してください。",
    "ko": "저장된 설정은 읽히지만 유효한 설정을 만들지 못합니다. 이 화면은 진단 뷰로 축소되었습니다 — 아래 원인부터 해결하세요."
  },
  "section.set.degraded": {
    "zh-CN": "有字段回落到了默认值（降级），网关照常在跑。逐格的「存储里是」会告诉你是哪一个。",
    "zh-TW": "有欄位回落到了預設值（降級），閘道照常在跑。逐格的「儲存裡是」會告訴你是哪一個。",
    "en": "Some fields fell back to defaults (degraded); the gateway keeps running. The per-field “stored” line tells you which.",
    "ja": "一部のフィールドが既定値にフォールバックしました（デグレード）。ゲートウェイは稼働中です。各項目の「保存値」を確認してください。",
    "ko": "일부 필드가 기본값으로 폴백했습니다(디그레이드). 게이트웨이는 계속 동작합니다. 각 항목의 “저장된 값”을 확인하세요."
  },
  "section.set.group.upstream": {
    "zh-CN": "上游与超时",
    "zh-TW": "上游與逾時",
    "en": "Upstream & timeouts",
    "ja": "アップストリームとタイムアウト",
    "ko": "업스트림 및 타임아웃"
  },
  "section.set.group.resilience": {
    "zh-CN": "容错与冷却",
    "zh-TW": "容錯與冷卻",
    "en": "Failure handling & cooldown",
    "ja": "障害処理とクールダウン",
    "ko": "장애 처리 및 쿨다운"
  },
  "section.set.group.pool": {
    "zh-CN": "账号池旋钮",
    "zh-TW": "帳號池旋鈕",
    "en": "Account-pool knobs",
    "ja": "アカウントプールの調整",
    "ko": "계정 풀 조절값"
  },
  "section.set.group.readonly": {
    "zh-CN": "只读状态（面板改不了）",
    "zh-TW": "唯讀狀態（面板改不了）",
    "en": "Read-only state (not editable here)",
    "ja": "読み取り専用の状態（ここでは変更できません）",
    "ko": "읽기 전용 상태(여기서는 변경 불가)"
  },
  "section.set.group.registrar": {
    "zh-CN": "注册机：开关与节奏",
    "zh-TW": "註冊機：開關與節奏",
    "en": "Registrar: switch and cadence",
    "ja": "レジストラ：オンオフと間隔",
    "ko": "레지스트라: 켜기와 주기"
  },
  "section.set.group.registrarChannels": {
    "zh-CN": "邮箱通道凭据",
    "zh-TW": "郵箱通道憑據",
    "en": "Mailbox channel credentials",
    "ja": "メールチャネルの認証情報",
    "ko": "메일 채널 자격 증명"
  },
  "section.set.group.registrarMint": {
    "zh-CN": "铸号细节",
    "zh-TW": "鑄號細節",
    "en": "Minting details",
    "ja": "アカウント作成の詳細",
    "ko": "계정 생성 세부값"
  },
  "section.set.field.registrar.enabled": {
    "zh-CN": "打开之后，网关会按下面的目标数量自动注册新账号来补池。关着时它一次上游请求都不发。",
    "zh-TW": "打開之後，網關會按下面的目標數量自動註冊新帳號來補池。關著時它一次上游請求都不發。",
    "en": "When on, the gateway registers new accounts to top the pool up to the target below. While off it makes no upstream calls at all.",
    "ja": "オンにすると、以下の目標数までアカウントを自動作成してプールを補充します。オフの間は上流へのリクエストを一切行いません。",
    "ko": "켜면 아래 목표 개수에 맞춰 계정을 자동 생성해 풀을 채웁니다. 꺼져 있는 동안에는 업스트림 요청을 전혀 보내지 않습니다."
  },
  "section.set.field.registrar.channel": {
    "zh-CN": "这一轮用哪条邮箱通道。两条完全平级，任选其一；选中的那条必须先把凭据填全。",
    "zh-TW": "這一輪用哪條郵箱通道。兩條完全平級，任選其一；選中的那條必須先把憑據填全。",
    "en": "Which mailbox channel a round uses. The two are fully equivalent — pick either; the one you pick must have its credentials filled in first.",
    "ja": "この回で使うメールチャネル。2 つは完全に同格です。選んだ方は先に認証情報を揃えてください。",
    "ko": "한 회차에서 사용할 메일 채널. 두 채널은 완전히 동등합니다. 선택한 쪽은 먼저 자격 증명을 모두 입력해야 합니다."
  },
  "section.set.field.registrar.targetKeys": {
    "zh-CN": "池子要维持的 key 数量。占名额数低于它时才补池 —— 被停用的和冷却中的都算在占名额数里。",
    "zh-TW": "池子要維持的 key 數量。占名額數低於它時才補池 —— 被停用的和冷卻中的都算在占名額數裡。",
    "en": "How many keys the pool should hold. Topping up starts when the counted total falls below it — disabled and cooling keys count toward that total.",
    "ja": "プールに保ちたいキーの本数。カウント数がこれを下回ると補充します。無効化中・クールダウン中のキーもカウントに含まれます。",
    "ko": "풀에 유지할 키 개수. 집계 수가 이 값 아래로 떨어지면 보충합니다. 비활성화·쿨다운 중인 키도 집계에 포함됩니다."
  },
  "section.set.field.registrar.tendIntervalMs": {
    "zh-CN": "两轮自动补池之间隔多久（毫秒）。调小会让上游更容易把出口地址限流。",
    "zh-TW": "兩輪自動補池之間隔多久（毫秒）。調小會讓上游更容易把出口地址限流。",
    "en": "Milliseconds between automatic top-up rounds. Lowering it makes upstream rate-limiting of your egress address more likely.",
    "ja": "自動補充の実行間隔（ミリ秒）。短くするほど、上流に送信元アドレスをレート制限されやすくなります。",
    "ko": "자동 보충 회차 사이의 간격(밀리초). 값을 줄이면 업스트림이 출구 주소를 제한할 가능성이 커집니다."
  },
  "section.set.field.registrar.mintBatch": {
    "zh-CN": "一轮最多铸几把。每铸一把都是一次真实的账号注册，还会占用一个临时邮箱名额。",
    "zh-TW": "一輪最多鑄幾把。每鑄一把都是一次真實的帳號註冊，還會佔用一個臨時郵箱名額。",
    "en": "How many keys one round mints at most. Each one is a real account registration and consumes one temporary-mailbox slot.",
    "ja": "1 回の実行で作成するキーの上限。1 本ごとに実際のアカウント登録が走り、使い捨てメールの枠を 1 つ消費します。",
    "ko": "한 회차에서 생성할 키의 최대 개수. 한 개마다 실제 계정 등록이 일어나고 임시 메일함 한 자리를 차지합니다."
  },
  "section.set.field.registrar.mintDelayMinMs": {
    "zh-CN": "两次铸号之间至少等多久（毫秒）。它必须不大于上界，否则这一轮起不来。",
    "zh-TW": "兩次鑄號之間至少等多久（毫秒）。它必須不大於上界，否則這一輪起不來。",
    "en": "Minimum wait between two mints, in milliseconds. It must not exceed the maximum, or the round will not start.",
    "ja": "作成と作成の間に最低限あける時間（ミリ秒）。上限を超えてはいけません。超えるとその回は開始しません。",
    "ko": "생성과 생성 사이 최소 대기 시간(밀리초). 상한보다 크면 해당 회차가 시작되지 않습니다."
  },
  "section.set.field.registrar.mintDelayMaxMs": {
    "zh-CN": "两次铸号之间最多等多久（毫秒）。真实等待在下界与它之间随机取。",
    "zh-TW": "兩次鑄號之間最多等多久（毫秒）。真實等待在下界與它之間隨機取。",
    "en": "Maximum wait between two mints, in milliseconds. The actual wait is picked at random between the minimum and this.",
    "ja": "作成と作成の間にあける最大時間（ミリ秒）。実際の待ち時間は下限とこの値の間でランダムに決まります。",
    "ko": "생성과 생성 사이 최대 대기 시간(밀리초). 실제 대기 시간은 하한과 이 값 사이에서 무작위로 정해집니다."
  },
  "section.set.field.registrar.codeTimeoutMs": {
    "zh-CN": "等验证码最多等多久（毫秒）。等不到就算这一把失败，账号那边可能已经建出来了。",
    "zh-TW": "等驗證碼最多等多久（毫秒）。等不到就算這一把失敗，帳號那邊可能已經建出來了。",
    "en": "How long to wait for the verification code, in milliseconds. Timing out fails that key — the account may already exist upstream.",
    "ja": "確認コードを待つ上限（ミリ秒）。時間切れはそのキーの失敗として扱われます。アカウントは上流に既に作られている可能性があります。",
    "ko": "인증 코드를 기다리는 최대 시간(밀리초). 시간이 지나면 해당 키는 실패로 처리되며, 계정은 이미 만들어져 있을 수 있습니다."
  },
  "section.set.field.registrar.maxDomainAttempts": {
    "zh-CN": "一把 key 最多换几个收信域名再放弃。调大会让单次尝试的耗时成倍增加。",
    "zh-TW": "一把 key 最多換幾個收信域名再放棄。調大會讓單次嘗試的耗時成倍增加。",
    "en": "How many receiving domains one key may try before giving up. Raising it multiplies how long a single attempt can take.",
    "ja": "1 本のキーが諦めるまでに試す受信ドメインの数。増やすと 1 回の試行にかかる時間が倍々に伸びます。",
    "ko": "키 하나가 포기하기 전까지 시도할 수신 도메인 수. 늘리면 한 번의 시도에 걸리는 시간이 배로 늘어납니다."
  },
  "section.set.field.registrar.tokenName": {
    "zh-CN": "铸出来的 key 在上游后台显示成什么名字。只影响那边的列表，与本网关无关。",
    "zh-TW": "鑄出來的 key 在上游後台顯示成什麼名字。只影響那邊的列表，與本網關無關。",
    "en": "The label minted keys carry in the upstream console. It only affects that listing, not this gateway.",
    "ja": "作成したキーが上流の管理画面に表示される名前。向こうの一覧にだけ影響し、本ゲートウェイには関係しません。",
    "ko": "생성된 키가 업스트림 콘솔에 표시될 이름. 그쪽 목록에만 영향을 주며 이 게이트웨이와는 무관합니다."
  },
  "section.set.field.registrar.agnesPlatformUrl": {
    "zh-CN": "注册与登录打到哪个地址。它与转发用的上游地址是两个，别填混。",
    "zh-TW": "註冊與登入打到哪個地址。它與轉發用的上游地址是兩個，別填混。",
    "en": "Where registration and sign-in are sent. This is a different address from the upstream used for forwarding — do not mix them up.",
    "ja": "登録とログインの送信先アドレス。転送に使う上流アドレスとは別物です。取り違えないでください。",
    "ko": "회원가입과 로그인을 보낼 주소. 전달에 사용하는 업스트림 주소와는 별개이니 혼동하지 마세요."
  },
  "section.set.field.channel.moemailBaseUrl": {
    "zh-CN": "MoeMail 的地址。它是自建服务，每个实例的地址都不同，必须自己填。",
    "zh-TW": "MoeMail 的地址。它是自建服務，每個實例的地址都不同，必須自己填。",
    "en": "The MoeMail address. It is self-hosted, so every instance has its own address and you must supply it.",
    "ja": "MoeMail のアドレス。セルフホスト型なので、インスタンスごとにアドレスが異なり、自分で入力する必要があります。",
    "ko": "MoeMail 주소. 직접 호스팅하는 서비스라 인스턴스마다 주소가 다르므로 직접 입력해야 합니다."
  },
  "section.set.field.channel.moemailApiKey": {
    "zh-CN": "MoeMail 的访问凭据。保存后只显示末 4 位；留空表示不改，清空要用下面的清除按钮。",
    "zh-TW": "MoeMail 的存取憑據。保存後只顯示末 4 位；留空表示不改，清空要用下面的清除按鈕。",
    "en": "The MoeMail access credential. Only the last 4 characters are shown after saving; leaving it blank means no change — use the clear button below to erase it.",
    "ja": "MoeMail のアクセス認証情報。保存後は末尾 4 文字だけ表示されます。空欄は「変更しない」を意味し、消すには下の消去ボタンを使います。",
    "ko": "MoeMail 접근 자격 증명. 저장 후에는 마지막 4자만 표시됩니다. 비워 두면 변경하지 않으며, 지우려면 아래 삭제 버튼을 사용하세요."
  },
  "section.set.field.channel.yydsBaseUrl": {
    "zh-CN": "YYDS 的地址。它是地址固定的公共服务，通常不用改。",
    "zh-TW": "YYDS 的地址。它是地址固定的公共服務，通常不用改。",
    "en": "The YYDS address. It is a public service at a fixed address, so you normally leave it alone.",
    "ja": "YYDS のアドレス。アドレスが固定された公開サービスなので、通常は変更しません。",
    "ko": "YYDS 주소. 주소가 고정된 공개 서비스라 보통은 그대로 두면 됩니다."
  },
  "section.set.field.channel.yydsApiKey": {
    "zh-CN": "YYDS 的访问凭据。保存后只显示末 4 位；留空表示不改，清空要用下面的清除按钮。",
    "zh-TW": "YYDS 的存取憑據。保存後只顯示末 4 位；留空表示不改，清空要用下面的清除按鈕。",
    "en": "The YYDS access credential. Only the last 4 characters are shown after saving; leaving it blank means no change — use the clear button below to erase it.",
    "ja": "YYDS のアクセス認証情報。保存後は末尾 4 文字だけ表示されます。空欄は「変更しない」を意味し、消すには下の消去ボタンを使います。",
    "ko": "YYDS 접근 자격 증명. 저장 후에는 마지막 4자만 표시됩니다. 비워 두면 변경하지 않으며, 지우려면 아래 삭제 버튼을 사용하세요."
  },
  "section.set.err.notABoolean": {
    "zh-CN": "这一格只收开或关两种取值。",
    "zh-TW": "這一格只收開或關兩種取值。",
    "en": "This field only accepts on or off.",
    "ja": "この項目はオンかオフのみを受け付けます。",
    "ko": "이 항목은 켜기 또는 끄기만 받습니다."
  },
  "section.set.err.tooLong": {
    "zh-CN": "太长了，请改短一些。",
    "zh-TW": "太長了，請改短一些。",
    "en": "Too long — please shorten it.",
    "ja": "長すぎます。短くしてください。",
    "ko": "너무 깁니다. 더 짧게 입력하세요."
  },
  "section.set.err.notAChannel": {
    "zh-CN": "不是可用的邮箱通道名。",
    "zh-TW": "不是可用的郵箱通道名。",
    "en": "Not a valid mailbox channel name.",
    "ja": "有効なメールチャネル名ではありません。",
    "ko": "사용할 수 있는 메일 채널 이름이 아닙니다."
  },
  "section.set.err.delayMinGtMax": {
    "zh-CN": "铸号间隔的下界比上界还大，这样注册机起不来。",
    "zh-TW": "鑄號間隔的下界比上界還大，這樣註冊機起不來。",
    "en": "The minimum mint interval is larger than the maximum, so the registrar cannot start.",
    "ja": "作成間隔の下限が上限より大きいため、レジストラを開始できません。",
    "ko": "생성 간격의 하한이 상한보다 커서 레지스트라를 시작할 수 없습니다."
  },
  "section.set.err.channelCredentialsMissing": {
    "zh-CN": "选中的这条通道还没把凭据填全。",
    "zh-TW": "選中的這條通道還沒把憑據填全。",
    "en": "The selected channel is missing part of its credentials.",
    "ja": "選択したチャネルの認証情報が揃っていません。",
    "ko": "선택한 채널의 자격 증명이 아직 모두 입력되지 않았습니다."
  },
  "section.set.err.legacyChannelKey": {
    "zh-CN": "存储里还留着旧的通道键，保存任意一次设置它就会消失。",
    "zh-TW": "儲存裡還留著舊的通道鍵，保存任意一次設定它就會消失。",
    "en": "An older channel key is still in storage; saving settings once will clear it.",
    "ja": "古いチャネル項目がストレージに残っています。設定を一度保存すると消えます。",
    "ko": "이전 채널 항목이 저장소에 남아 있습니다. 설정을 한 번 저장하면 사라집니다."
  },
  "section.set.err.legacyFallbackIgnored": {
    "zh-CN": "存储里那条备用通道已被丢弃：今天是二选一，没有自动降级。",
    "zh-TW": "儲存裡那條備用通道已被丟棄：今天是二選一，沒有自動降級。",
    "en": "The fallback channel in storage was dropped: today it is one-of-two, with no automatic failover.",
    "ja": "ストレージにある予備チャネルは破棄されました。現在は 2 つから 1 つを選ぶ方式で、自動切り替えはありません。",
    "ko": "저장소의 예비 채널은 폐기되었습니다. 현재는 둘 중 하나를 고르는 방식이며 자동 전환은 없습니다."
  },
  "section.set.group.other": {
    "zh-CN": "面板还不认识的字段",
    "zh-TW": "面板還不認識的欄位",
    "en": "Fields this panel does not know yet",
    "ja": "パネルがまだ認識していない項目",
    "ko": "패널이 아직 모르는 항목"
  },
  "section.set.group.otherDesc": {
    "zh-CN": "后端发来了这几格，而这一版面板没有它们的说明。照实画出来，总比让它们在屏幕上消失好。",
    "zh-TW": "後端發來了這幾格，而這一版面板沒有它們的說明。照實畫出來，總比讓它們在畫面上消失好。",
    "en": "The backend sent these fields and this panel build has no description for them. Showing them as-is beats making them disappear.",
    "ja": "バックエンドから届いた項目ですが、このパネルには説明がありません。画面から消すより、そのまま表示します。",
    "ko": "백엔드가 보낸 항목이지만 이 패널에는 설명이 없습니다. 화면에서 사라지게 하는 것보다 그대로 보여줍니다."
  },
  "section.set.field.gatewayToken": {
    "zh-CN": "网关口令（GATEWAY_TOKEN）",
    "zh-TW": "閘道口令（GATEWAY_TOKEN）",
    "en": "Gateway token (GATEWAY_TOKEN)",
    "ja": "ゲートウェイトークン（GATEWAY_TOKEN）",
    "ko": "게이트웨이 토큰(GATEWAY_TOKEN)"
  },
  "section.set.field.agnesBaseUrl": {
    "zh-CN": "上游地址",
    "zh-TW": "上游位址",
    "en": "Upstream base URL",
    "ja": "アップストリームのベース URL",
    "ko": "업스트림 기본 URL"
  },
  "section.set.field.upstreamTimeoutMs": {
    "zh-CN": "上游超时（毫秒）",
    "zh-TW": "上游逾時（毫秒）",
    "en": "Upstream timeout (ms)",
    "ja": "アップストリームのタイムアウト（ミリ秒）",
    "ko": "업스트림 타임아웃(밀리초)"
  },
  "section.set.field.upstreamSyncTimeoutMs": {
    "zh-CN": "非流式请求超时（毫秒）",
    "zh-TW": "非串流請求逾時（毫秒）",
    "en": "Non-streaming request timeout (ms)",
    "ja": "非ストリーミング要求のタイムアウト（ミリ秒）",
    "ko": "비스트리밍 요청 타임아웃(밀리초)"
  },
  "section.set.field.maxStrikes": {
    "zh-CN": "连续失败几次算一次剔除",
    "zh-TW": "連續失敗幾次算一次剔除",
    "en": "Consecutive failures before eviction",
    "ja": "連続失敗が何回で除外するか",
    "ko": "연속 실패 몇 회에 제외할지"
  },
  "section.set.field.cooldownRateLimitMs": {
    "zh-CN": "被限流后冷却多久（毫秒）",
    "zh-TW": "被限流後冷卻多久（毫秒）",
    "en": "Cooldown after a rate limit (ms)",
    "ja": "レート制限後のクールダウン（ミリ秒）",
    "ko": "레이트 리밋 후 쿨다운(밀리초)"
  },
  "section.set.field.cooldownPaymentMs": {
    "zh-CN": "欠费后冷却多久（毫秒）",
    "zh-TW": "欠費後冷卻多久（毫秒）",
    "en": "Cooldown after a payment failure (ms)",
    "ja": "支払いエラー後のクールダウン（ミリ秒）",
    "ko": "결제 실패 후 쿨다운(밀리초)"
  },
  "section.set.field.cooldownStrikeMs": {
    "zh-CN": "一次普通失败后冷却多久（毫秒）",
    "zh-TW": "一次普通失敗後冷卻多久（毫秒）",
    "en": "Cooldown after an ordinary failure (ms)",
    "ja": "通常の失敗後のクールダウン（ミリ秒）",
    "ko": "일반 실패 후 쿨다운(밀리초)"
  },
  "section.set.field.poolCacheTtlMs": {
    "zh-CN": "池子缓存有效期（毫秒，0 = 关掉）",
    "zh-TW": "池子快取有效期（毫秒，0 = 關掉）",
    "en": "Pool cache TTL (ms, 0 disables it)",
    "ja": "プールキャッシュの有効期間（ミリ秒、0 で無効）",
    "ko": "풀 캐시 유효 기간(밀리초, 0이면 비활성)"
  },
  "section.set.field.poolTouchIntervalMs": {
    "zh-CN": "回写「最后使用」的最小间隔（毫秒，0 = 每次都写）",
    "zh-TW": "回寫「最後使用」的最小間隔（毫秒，0 = 每次都寫）",
    "en": "Minimum interval for writing back “last used” (ms, 0 writes every time)",
    "ja": "「最終使用」書き戻しの最小間隔（ミリ秒、0 で毎回書き込み）",
    "ko": "“마지막 사용” 기록 최소 간격(밀리초, 0이면 매번 기록)"
  },
  "section.set.field.degraded": {
    "zh-CN": "配置是否处于降级状态",
    "zh-TW": "設定是否處於降級狀態",
    "en": "Whether the config is in a degraded state",
    "ja": "設定がデグレード状態かどうか",
    "ko": "설정이 디그레이드 상태인지 여부"
  },
  "section.set.field.usageStatsEnabled": {
    "zh-CN": "用量统计（Tier-2）开没开",
    "zh-TW": "用量統計（Tier-2）開沒開",
    "en": "Whether usage statistics (Tier-2) are on",
    "ja": "利用統計（Tier-2）が有効かどうか",
    "ko": "사용 통계(Tier-2) 활성화 여부"
  },
  "section.set.lockedBy": {
    "zh-CN": "被环境变量钉死了（{by}），面板改不了；要改就改部署里那一行。",
    "zh-TW": "被環境變數釘死了（{by}），面板改不了；要改就改部署裡那一行。",
    "en": "Pinned by an environment variable ({by}); this panel cannot change it — edit that line in your deployment.",
    "ja": "環境変数（{by}）で固定されています。パネルからは変更できません —— デプロイ側のその行を編集してください。",
    "ko": "환경 변수({by})로 고정되어 있습니다. 패널에서는 변경할 수 없으니 배포 설정의 해당 줄을 수정하세요."
  },
  "section.set.storedIs": {
    "zh-CN": "存储里是：{v}",
    "zh-TW": "儲存裡是：{v}",
    "en": "Stored value: {v}",
    "ja": "保存値：{v}",
    "ko": "저장된 값: {v}"
  },
  "section.set.storedNone": {
    "zh-CN": "没配（用的是内置默认值）",
    "zh-TW": "沒設（用的是內建預設值）",
    "en": "not set (built-in default in use)",
    "ja": "未設定（組み込みの既定値を使用）",
    "ko": "미설정(내장 기본값 사용)"
  },
  "section.set.readOnlyField": {
    "zh-CN": "这一格是装载的产物，不是旋钮。",
    "zh-TW": "這一格是裝載的產物，不是旋鈕。",
    "en": "This cell is an outcome of loading, not a knob.",
    "ja": "この項目は読み込み結果であり、設定つまみではありません。",
    "ko": "이 항목은 로딩 결과이며 조절 값이 아닙니다."
  },
  "section.set.secretConfigured": {
    "zh-CN": "已配置",
    "zh-TW": "已設定",
    "en": "Configured",
    "ja": "設定済み",
    "ko": "설정됨"
  },
  "section.set.secretMissing": {
    "zh-CN": "未配置",
    "zh-TW": "未設定",
    "en": "Not configured",
    "ja": "未設定です",
    "ko": "설정되지 않음"
  },
  "section.set.secretDesc": {
    "zh-CN": "凭据只写不读：这一页永远拿不到明文，只看得到末 4 位。留空保存 = 不改，清空只有下面那颗按钮一条路。",
    "zh-TW": "憑據只寫不讀：這一頁永遠拿不到明文，只看得到末 4 位。留空儲存 = 不改，清空只有下面那顆按鈕一條路。",
    "en": "Secrets are write-only: this page never receives the plaintext, only the last 4 characters. Saving an empty box means “leave unchanged”; clearing is only possible via the button below.",
    "ja": "シークレットは書き込み専用です：この画面に平文は届かず、末尾 4 文字のみ表示します。空欄で保存＝変更なし、消去は下のボタンのみです。",
    "ko": "비밀 값은 쓰기 전용입니다: 이 화면은 평문을 받지 않고 마지막 4자만 봅니다. 빈칸 저장 = 변경 없음이며, 삭제는 아래 버튼으로만 가능합니다."
  },
  "section.set.secretNoHint": {
    "zh-CN": "已配置（太短，不给末位提示）",
    "zh-TW": "已設定（太短，不給末位提示）",
    "en": "Configured (too short to show a tail hint)",
    "ja": "設定済み（短すぎるため末尾ヒントなし）",
    "ko": "설정됨(너무 짧아 끝자리 힌트 없음)"
  },
  "section.set.secretNewLabel": {
    "zh-CN": "换一把新的（留空 = 不改）",
    "zh-TW": "換一把新的（留空 = 不改）",
    "en": "Replace with a new one (empty = unchanged)",
    "ja": "新しい値に置き換える（空欄＝変更なし）",
    "ko": "새 값으로 교체(빈칸 = 변경 없음)"
  },
  "section.set.secretLockedLabel": {
    "zh-CN": "被环境变量钉死了，这里改不了",
    "zh-TW": "被環境變數釘死了，這裡改不了",
    "en": "Pinned by an environment variable; not editable here",
    "ja": "環境変数で固定されており、ここでは変更できません",
    "ko": "환경 변수로 고정되어 여기서는 변경할 수 없습니다"
  },
  "section.set.secretHint": {
    "zh-CN": "至少 24 个可打印 ASCII 字符，首尾不许有空白，且不许与管理口令相同。",
    "zh-TW": "至少 24 個可列印 ASCII 字元，首尾不許有空白，且不許與管理口令相同。",
    "en": "At least 24 printable ASCII characters, no leading or trailing whitespace, and it must differ from the admin token.",
    "ja": "印字可能な ASCII で 24 文字以上、前後に空白を含まず、管理トークンと同一であってはいけません。",
    "ko": "출력 가능한 ASCII로 24자 이상, 앞뒤 공백 없이, 관리자 토큰과 달라야 합니다."
  },
  "section.set.secretClear": {
    "zh-CN": "清空这把凭据",
    "zh-TW": "清空這把憑據",
    "en": "Clear this secret",
    "ja": "このシークレットを消去",
    "ko": "이 비밀 값 삭제"
  },
  "section.set.clearTitle": {
    "zh-CN": "清空凭据",
    "zh-TW": "清空憑據",
    "en": "Clear secret",
    "ja": "シークレットの消去",
    "ko": "비밀 값 삭제"
  },
  "section.set.clearConfirm": {
    "zh-CN": "这一步会把存储里那一份删掉，且不可撤销。",
    "zh-TW": "這一步會把儲存裡那一份刪掉，且不可撤銷。",
    "en": "This deletes the copy in storage and cannot be undone.",
    "ja": "この操作はストレージ側の値を削除し、取り消せません。",
    "ko": "이 작업은 스토리지의 값을 삭제하며 되돌릴 수 없습니다."
  },
  "section.set.clearStillEnv": {
    "zh-CN": "环境变量里还有一份，所以生效值不会变 —— 这次清的只是存储里那一份。",
    "zh-TW": "環境變數裡還有一份，所以生效值不會變 —— 這次清的只是儲存裡那一份。",
    "en": "An environment variable still provides one, so the effective value will not change — only the stored copy is removed.",
    "ja": "環境変数にも値があるため実効値は変わりません —— 削除されるのは保存側のみです。",
    "ko": "환경 변수에도 값이 있어 실효 값은 바뀌지 않습니다 — 저장된 사본만 삭제됩니다."
  },
  "section.set.clearFatal": {
    "zh-CN": "环境变量里也没有。清完之后当前进程靠上一份快照还能跑，但下一次冷启动会起不来 —— 请立刻在这一页把它补回来。",
    "zh-TW": "環境變數裡也沒有。清完之後目前行程靠上一份快照還能跑，但下一次冷啟動會起不來 —— 請立刻在這一頁把它補回來。",
    "en": "No environment variable provides one either. The running process keeps going on its last snapshot, but the next cold start will fail — put it back on this page right away.",
    "ja": "環境変数にもありません。現行プロセスは直前のスナップショットで動き続けますが、次回のコールドスタートは失敗します —— この画面ですぐ入れ直してください。",
    "ko": "환경 변수에도 없습니다. 현재 프로세스는 직전 스냅샷으로 계속 동작하지만 다음 콜드 스타트는 실패합니다 — 이 화면에서 즉시 다시 넣으세요."
  },
  "section.set.cleared": {
    "zh-CN": "已清空。这份配置仍然装载得起来。",
    "zh-TW": "已清空。這份設定仍然裝載得起來。",
    "en": "Cleared. The configuration still loads.",
    "ja": "消去しました。設定は引き続き読み込めます。",
    "ko": "삭제했습니다. 설정은 계속 로드됩니다."
  },
  "section.set.clearedStillEnv": {
    "zh-CN": "已清空存储里那一份；环境变量里仍然有一份，生效值不变。",
    "zh-TW": "已清空儲存裡那一份；環境變數裡仍然有一份，生效值不變。",
    "en": "The stored copy was cleared; an environment variable still provides one, so the effective value is unchanged.",
    "ja": "保存側を消去しました。環境変数の値が残っているため実効値は変わりません。",
    "ko": "저장된 사본을 삭제했습니다. 환경 변수 값이 남아 실효 값은 그대로입니다."
  },
  "section.set.clearedFatal": {
    "zh-CN": "已清空 —— 这份配置现在装载不起来了。当前进程还在跑，但下一次冷启动会失败，请立刻补回来。",
    "zh-TW": "已清空 —— 這份設定現在裝載不起來了。目前行程還在跑，但下一次冷啟動會失敗，請立刻補回來。",
    "en": "Cleared — the configuration no longer loads. The running process survives, but the next cold start will fail; put it back now.",
    "ja": "消去しました —— 設定はもう読み込めません。現行プロセスは動作中ですが、次回のコールドスタートは失敗します。すぐ入れ直してください。",
    "ko": "삭제했습니다 — 설정을 더 이상 로드할 수 없습니다. 현재 프로세스는 살아 있지만 다음 콜드 스타트는 실패합니다. 즉시 복구하세요."
  },
  "section.set.noPendingEdits": {
    "zh-CN": "没有待保存的改动",
    "zh-TW": "沒有待儲存的改動",
    "en": "No pending edits",
    "ja": "保存待ちの変更はありません",
    "ko": "저장 대기 중인 변경이 없습니다"
  },
  "section.set.pendingEdits": {
    "zh-CN": "有 {n} 格改动还没保存",
    "zh-TW": "有 {n} 格改動還沒儲存",
    "en": "{n} field(s) changed but not saved",
    "ja": "未保存の変更が {n} 件あります",
    "ko": "저장되지 않은 변경 {n}건"
  },
  "section.set.nothingToSave": {
    "zh-CN": "没有改动可以提交",
    "zh-TW": "沒有改動可以提交",
    "en": "There is nothing to submit",
    "ja": "送信する変更がありません",
    "ko": "제출할 변경이 없습니다"
  },
  "section.set.dryRun": {
    "zh-CN": "先校验（不写）",
    "zh-TW": "先校驗（不寫）",
    "en": "Validate only",
    "ja": "検証のみ（書き込まない）",
    "ko": "검증만(쓰지 않음)"
  },
  "section.set.dryRunOk": {
    "zh-CN": "校验通过：真保存会改动 {n} 格。这一次一个字节都没有写。",
    "zh-TW": "校驗通過：真儲存會改動 {n} 格。這一次一個位元組都沒有寫。",
    "en": "Validation passed: a real save would change {n} field(s). Nothing was written this time.",
    "ja": "検証に成功：実際の保存では {n} 件が変わります。今回は 1 バイトも書いていません。",
    "ko": "검증 통과: 실제 저장 시 {n}건이 변경됩니다. 이번에는 한 바이트도 쓰지 않았습니다."
  },
  "section.set.save": {
    "zh-CN": "保存",
    "zh-TW": "儲存",
    "en": "Save",
    "ja": "保存する",
    "ko": "저장하기"
  },
  "section.set.saved": {
    "zh-CN": "已写入，回读到 {n} 格真的变了。",
    "zh-TW": "已寫入，回讀到 {n} 格真的變了。",
    "en": "Written; the read-back shows {n} field(s) actually changed.",
    "ja": "書き込みました。読み戻しで {n} 件が実際に変わりました。",
    "ko": "기록했습니다. 다시 읽은 결과 {n}건이 실제로 변경되었습니다."
  },
  "section.set.savedNoChange": {
    "zh-CN": "已写入，但回读之后生效值一格都没变（多半是被环境变量压着）。",
    "zh-TW": "已寫入，但回讀之後生效值一格都沒變（多半是被環境變數壓著）。",
    "en": "Written, but the read-back shows no effective value changed (most likely an environment variable still wins).",
    "ja": "書き込みましたが、読み戻すと実効値は一つも変わっていません（環境変数が優先されている可能性が高いです）。",
    "ko": "기록했지만 다시 읽은 실효 값은 하나도 바뀌지 않았습니다(환경 변수가 우선했을 가능성이 큽니다)."
  },
  "section.set.propagation": {
    "zh-CN": "其余实例最多 {s} 之后看到这次改动。",
    "zh-TW": "其餘實例最多 {s} 之後看到這次改動。",
    "en": "Other instances will see this change within {s} at most.",
    "ja": "他のインスタンスは最大 {s} でこの変更を認識します。",
    "ko": "다른 인스턴스는 최대 {s} 안에 이 변경을 인식합니다."
  },
  "section.set.seconds": {
    "zh-CN": "{n} 秒",
    "zh-TW": "{n} 秒",
    "en": "{n}s",
    "ja": "{n} 秒間",
    "ko": "{n}초"
  },
  "section.set.saveRejected": {
    "zh-CN": "校验没通过，本次一个字节都没有写入（{n} 条）。",
    "zh-TW": "校驗沒通過，本次一個位元組都沒有寫入（{n} 條）。",
    "en": "Validation failed; nothing was written ({n} problem(s)).",
    "ja": "検証に失敗し、1 バイトも書き込んでいません（{n} 件）。",
    "ko": "검증에 실패하여 아무것도 기록하지 않았습니다({n}건)."
  },
  "section.set.saveFailedHttp": {
    "zh-CN": "保存失败（接口返回了错误）。请重新读取之后再试。",
    "zh-TW": "儲存失敗（介面回傳了錯誤）。請重新讀取之後再試。",
    "en": "Save failed (the endpoint returned an error). Reload and try again.",
    "ja": "保存に失敗しました（エンドポイントがエラーを返しました）。再読み込みしてからやり直してください。",
    "ko": "저장에 실패했습니다(엔드포인트가 오류를 반환). 다시 읽은 뒤 재시도하세요."
  },
  "section.set.saveFailedNetwork": {
    "zh-CN": "保存失败（请求没发出去或没回来）。⚠️ 无法确认服务端有没有写入，请重新读取核对。",
    "zh-TW": "儲存失敗（請求沒發出去或沒回來）。⚠️ 無法確認伺服器有沒有寫入，請重新讀取核對。",
    "en": "Save failed (the request never completed). ⚠️ Whether the server wrote anything is unknown — reload and check.",
    "ja": "保存に失敗しました（リクエストが完了しませんでした）。⚠️ サーバー側が書き込んだかは不明です。再読み込みして確認してください。",
    "ko": "저장에 실패했습니다(요청이 완료되지 않음). ⚠️ 서버가 기록했는지 알 수 없으니 다시 읽어 확인하세요."
  },
  "section.set.err.unknown": {
    "zh-CN": "面板还不认识这条错误码：{code}",
    "zh-TW": "面板還不認識這條錯誤碼：{code}",
    "en": "This panel does not recognise the error code {code} yet",
    "ja": "パネルが未知のエラーコードです：{code}",
    "ko": "패널이 아직 모르는 오류 코드입니다: {code}"
  },
  "section.set.err.unknownField": {
    "zh-CN": "后端不认识这个字段名（或者它不是一个可改的旋钮）",
    "zh-TW": "後端不認識這個欄位名（或者它不是一個可改的旋鈕）",
    "en": "The backend does not know this field name (or it is not an editable knob)",
    "ja": "バックエンドがこの項目名を認識していません（または変更可能な項目ではありません）",
    "ko": "백엔드가 이 항목 이름을 모릅니다(또는 변경 가능한 항목이 아닙니다)"
  },
  "section.set.err.lockedByEnv": {
    "zh-CN": "被环境变量 {env} 钉死了，这次写入被整条拒绝（不是「写下去但不生效」）",
    "zh-TW": "被環境變數 {env} 釘死了，這次寫入被整條拒絕（不是「寫下去但不生效」）",
    "en": "Pinned by environment variable {env}; the write was rejected outright (not “written but ineffective”)",
    "ja": "環境変数 {env} で固定されており、この書き込みは丸ごと拒否されました（「書いたが効かない」ではありません）",
    "ko": "환경 변수 {env}로 고정되어 있어 이번 쓰기는 전부 거부되었습니다(“쓰였지만 적용 안 됨”이 아닙니다)"
  },
  "section.set.err.notAnInteger": {
    "zh-CN": "这一格只收整数",
    "zh-TW": "這一格只收整數",
    "en": "This field takes an integer only",
    "ja": "この項目は整数のみ受け付けます",
    "ko": "이 항목은 정수만 받습니다"
  },
  "section.set.err.belowMin": {
    "zh-CN": "不能小于 {min}",
    "zh-TW": "不能小於 {min}",
    "en": "Must not be less than {min}",
    "ja": "{min} 未満にはできません",
    "ko": "{min} 미만일 수 없습니다"
  },
  "section.set.err.notAString": {
    "zh-CN": "这一格只收字符串",
    "zh-TW": "這一格只收字串",
    "en": "This field takes a string only",
    "ja": "この項目は文字列のみ受け付けます",
    "ko": "이 항목은 문자열만 받습니다"
  },
  "section.set.err.empty": {
    "zh-CN": "不能是空的",
    "zh-TW": "不能是空的",
    "en": "Must not be empty",
    "ja": "空にはできません",
    "ko": "비워 둘 수 없습니다"
  },
  "section.set.err.notAUrl": {
    "zh-CN": "必须是一个 http:// 或 https:// 的地址",
    "zh-TW": "必須是一個 http:// 或 https:// 的位址",
    "en": "Must be an http:// or https:// URL",
    "ja": "http:// または https:// の URL である必要があります",
    "ko": "http:// 또는 https:// 주소여야 합니다"
  },
  "section.set.err.gatewayTokenRequired": {
    "zh-CN": "两边都没有网关口令，这份配置装载不起来",
    "zh-TW": "兩邊都沒有閘道口令，這份設定裝載不起來",
    "en": "Neither side provides a gateway token, so this configuration cannot load",
    "ja": "どちらにもゲートウェイトークンが無く、この設定は読み込めません",
    "ko": "양쪽 모두 게이트웨이 토큰이 없어 이 설정을 로드할 수 없습니다"
  },
  "section.set.err.whitespacePadded": {
    "zh-CN": "首尾有空白。带空白的口令客户端永远送不出来，症状是「口令明明是对的却一直 401」",
    "zh-TW": "首尾有空白。帶空白的口令客戶端永遠送不出來，症狀是「口令明明是對的卻一直 401」",
    "en": "Leading or trailing whitespace. A padded token can never be sent by a client — the symptom is “the token is right but everything 401s”",
    "ja": "前後に空白があります。空白付きのトークンはクライアントから送れず、「正しいはずなのに 401 が続く」症状になります",
    "ko": "앞뒤에 공백이 있습니다. 공백이 붙은 토큰은 클라이언트가 보낼 수 없어 “토큰이 맞는데 계속 401”이 됩니다"
  },
  "section.set.err.notSendable": {
    "zh-CN": "含有送不出去的字符（只收可打印 ASCII）",
    "zh-TW": "含有送不出去的字元（只收可列印 ASCII）",
    "en": "Contains characters that cannot be sent (printable ASCII only)",
    "ja": "送信できない文字が含まれています（印字可能な ASCII のみ）",
    "ko": "전송할 수 없는 문자가 있습니다(출력 가능한 ASCII만 허용)"
  },
  "section.set.err.tooShort": {
    "zh-CN": "太短了，至少要 {min} 个字符 —— 本网关没有任何登录限速，口令熵就是唯一的防线",
    "zh-TW": "太短了，至少要 {min} 個字元 —— 本閘道沒有任何登入限速，口令熵就是唯一的防線",
    "en": "Too short; at least {min} characters are required — this gateway has no login rate limit, so token entropy is the only defence",
    "ja": "短すぎます。最低 {min} 文字必要です —— 本ゲートウェイにはログイン制限が無く、トークンのエントロピーが唯一の防御です",
    "ko": "너무 짧습니다. 최소 {min}자가 필요합니다 — 이 게이트웨이에는 로그인 제한이 없어 토큰 엔트로피가 유일한 방어선입니다"
  },
  "section.set.err.sameAsAdminToken": {
    "zh-CN": "不能与管理口令相同：那样管理面会当场把自己锁死，连改回去的那一次保存也会失败",
    "zh-TW": "不能與管理口令相同：那樣管理面會當場把自己鎖死，連改回去的那一次儲存也會失敗",
    "en": "Must not equal the admin token: the admin plane would lock itself out, and even the save that undoes it would fail",
    "ja": "管理トークンと同一にはできません：管理面が自らをロックアウトし、元に戻す保存すら失敗します",
    "ko": "관리자 토큰과 같을 수 없습니다: 관리 화면이 스스로 잠기고, 되돌리는 저장조차 실패합니다"
  },
  "section.set.err.configUnloadable": {
    "zh-CN": "存储里那份配置读得出来，却构造不出一份合法配置，而逐字段判据说不出是哪一格",
    "zh-TW": "儲存裡那份設定讀得出來，卻建構不出一份合法設定，而逐欄位判據說不出是哪一格",
    "en": "The stored configuration can be read but yields no valid config, and the per-field checks cannot say which cell is at fault",
    "ja": "保存された設定は読めますが有効な設定にならず、項目ごとの判定でも原因を特定できません",
    "ko": "저장된 설정은 읽히지만 유효한 설정이 되지 않으며, 항목별 판정으로도 원인을 특정할 수 없습니다"
  },
  "section.set.examplesTitle": {
    "zh-CN": "怎么调这个网关",
    "zh-TW": "怎麼呼叫這個閘道",
    "en": "How to call this gateway",
    "ja": "このゲートウェイの呼び出し方",
    "ko": "이 게이트웨이 호출 방법"
  },
  "section.set.examplesDesc": {
    "zh-CN": "下面这段是从服务端的协议目录现拼的，口令一律画成占位符 —— 复制之前请自己换成真的。",
    "zh-TW": "下面這段是從伺服器的協定目錄現拼的，口令一律畫成佔位符 —— 複製之前請自己換成真的。",
    "en": "The snippet below is assembled from the server-side protocol catalogue; the token is always a placeholder — substitute your own before using it.",
    "ja": "以下のスニペットはサーバー側のプロトコルカタログから生成しています。トークンは常にプレースホルダです —— 使う前にご自分の値に置き換えてください。",
    "ko": "아래 스니펫은 서버의 프로토콜 카탈로그에서 생성됩니다. 토큰은 항상 자리표시자이니 사용 전에 실제 값으로 바꾸세요."
  },
  "section.set.examplesProtocol": {
    "zh-CN": "选协议",
    "zh-TW": "選協定",
    "en": "Protocol",
    "ja": "プロトコルを選択",
    "ko": "프로토콜 선택"
  },
  "section.set.examplesLang": {
    "zh-CN": "选形态",
    "zh-TW": "選形態",
    "en": "Form",
    "ja": "形式を選択",
    "ko": "형식 선택"
  },
  "section.set.lang.curl": {
    "zh-CN": "命令行",
    "zh-TW": "命令列",
    "en": "curl",
    "ja": "コマンドライン",
    "ko": "명령줄"
  },
  "section.set.lang.python": {
    "zh-CN": "Python",
    "zh-TW": "Python 版",
    "en": "Python 3",
    "ja": "Python 版",
    "ko": "Python 예제"
  },
  "section.set.lang.node": {
    "zh-CN": "Node.js",
    "zh-TW": "Node.js 版",
    "en": "Node.js (fetch)",
    "ja": "Node.js 版",
    "ko": "Node.js 예제"
  },
  "section.set.dangerTitle": {
    "zh-CN": "危险区",
    "zh-TW": "危險區",
    "en": "Danger zone",
    "ja": "危険な操作",
    "ko": "위험 구역"
  },
  "section.set.dangerDesc": {
    "zh-CN": "重置只动存储里那一把 config 键，其余业务数据（账号池、对外密钥、事件、用量）一个字节都不动。",
    "zh-TW": "重設只動儲存裡那一把 config 鍵，其餘業務資料（帳號池、對外金鑰、事件、用量）一個位元組都不動。",
    "en": "Resetting touches only the stored config key; account pool, issued keys, events and usage data are left untouched.",
    "ja": "リセットは保存された config キーのみを対象とし、アカウントプール・発行済みキー・イベント・利用統計には触れません。",
    "ko": "초기화는 저장된 config 키만 대상으로 하며 계정 풀·발급 키·이벤트·사용량 데이터는 건드리지 않습니다."
  },
  "section.set.resetBtn": {
    "zh-CN": "重置存储里那份配置",
    "zh-TW": "重設儲存裡那份設定",
    "en": "Reset the stored configuration",
    "ja": "保存された設定をリセット",
    "ko": "저장된 설정 초기화"
  },
  "section.set.resetTitle": {
    "zh-CN": "重置配置",
    "zh-TW": "重設設定",
    "en": "Reset configuration",
    "ja": "設定のリセット",
    "ko": "설정 초기화"
  },
  "section.set.resetConfirm": {
    "zh-CN": "这一步不可撤销：存储里那份配置会被整把抹掉，生效值回落到环境变量与内置默认值。",
    "zh-TW": "這一步不可撤銷：儲存裡那份設定會被整把抹掉，生效值回落到環境變數與內建預設值。",
    "en": "This cannot be undone: the stored configuration is wiped and effective values fall back to environment variables and built-in defaults.",
    "ja": "この操作は取り消せません：保存された設定は消去され、実効値は環境変数と既定値に戻ります。",
    "ko": "되돌릴 수 없습니다: 저장된 설정이 지워지고 실효 값은 환경 변수와 기본값으로 돌아갑니다."
  },
  "section.set.resetSafe": {
    "zh-CN": "重置之后这份配置仍然装载得起来。",
    "zh-TW": "重設之後這份設定仍然裝載得起來。",
    "en": "After the reset the configuration will still load.",
    "ja": "リセット後も設定は読み込めます。",
    "ko": "초기화 후에도 설정은 로드됩니다."
  },
  "section.set.resetWillBreak": {
    "zh-CN": "⚠️ 重置之后这份配置会装载不起来，原因如下：",
    "zh-TW": "⚠️ 重設之後這份設定會裝載不起來，原因如下：",
    "en": "⚠️ After the reset the configuration will fail to load, because:",
    "ja": "⚠️ リセット後は設定を読み込めなくなります。理由：",
    "ko": "⚠️ 초기화 후에는 설정을 로드할 수 없습니다. 이유:"
  },
  "section.set.resetDone": {
    "zh-CN": "已重置。生效值回落到环境变量与内置默认值。",
    "zh-TW": "已重設。生效值回落到環境變數與內建預設值。",
    "en": "Reset done. Effective values fell back to environment variables and built-in defaults.",
    "ja": "リセットしました。実効値は環境変数と既定値に戻りました。",
    "ko": "초기화했습니다. 실효 값이 환경 변수와 기본값으로 돌아갔습니다."
  },
  "section.set.resetDoneLost": {
    "zh-CN": "已重置。有 {n} 把凭据从此变成「未配置」—— 请立刻补回来，否则下一次冷启动会失败。",
    "zh-TW": "已重設。有 {n} 把憑據從此變成「未設定」—— 請立刻補回來，否則下一次冷啟動會失敗。",
    "en": "Reset done. {n} secret(s) are now unconfigured — put them back now or the next cold start will fail.",
    "ja": "リセットしました。{n} 件のシークレットが未設定になりました —— すぐ入れ直さないと次回のコールドスタートが失敗します。",
    "ko": "초기화했습니다. 비밀 값 {n}건이 미설정 상태가 되었습니다 — 즉시 복구하지 않으면 다음 콜드 스타트가 실패합니다."
  },
  "section.usg.title": {
    "zh-CN": "使用统计",
    "zh-TW": "使用統計",
    "en": "Usage Statistics",
    "ja": "利用統計",
    "ko": "사용 통계"
  },
  "section.usg.refresh": {
    "zh-CN": "刷新",
    "zh-TW": "重新整理",
    "en": "Refresh",
    "ja": "更新",
    "ko": "새로고침"
  },
  "section.usg.loading": {
    "zh-CN": "正在读取…",
    "zh-TW": "正在讀取…",
    "en": "Loading…",
    "ja": "読み込み中…",
    "ko": "불러오는 중…"
  },
  "section.usg.loadFailedHttp": {
    "zh-CN": "这一份统计没读出来（接口返回了错误）。屏幕上的破折号表示「不知道」，不是 0。",
    "zh-TW": "這一份統計沒讀出來（介面回傳了錯誤）。畫面上的破折號表示「不知道」，不是 0。",
    "en": "This report could not be read (the endpoint returned an error). The dashes mean “unknown”, not zero.",
    "ja": "この統計を取得できませんでした（エンドポイントがエラーを返しました）。ダッシュは「不明」であり 0 ではありません。",
    "ko": "이 통계를 읽지 못했습니다(엔드포인트가 오류를 반환). 대시는 “알 수 없음”이며 0이 아닙니다."
  },
  "section.usg.loadFailedNetwork": {
    "zh-CN": "这一份统计没读出来（请求没发出去或没回来）。屏幕上的破折号表示「不知道」，不是 0。",
    "zh-TW": "這一份統計沒讀出來（請求沒發出去或沒回來）。畫面上的破折號表示「不知道」，不是 0。",
    "en": "This report could not be read (the request never completed). The dashes mean “unknown”, not zero.",
    "ja": "この統計を取得できませんでした（リクエストが完了しませんでした）。ダッシュは「不明」であり 0 ではありません。",
    "ko": "이 통계를 읽지 못했습니다(요청이 완료되지 않음). 대시는 “알 수 없음”이며 0이 아닙니다."
  },
  "section.usg.approxNote": {
    "zh-CN": "所有计数都带 ≈：同一天里多个副本各写各的槽位，超过槽位数时会互相覆盖，统计只保证「不多报」。",
    "zh-TW": "所有計數都帶 ≈：同一天裡多個副本各寫各的槽位，超過槽位數時會互相覆蓋，統計只保證「不多報」。",
    "en": "Every count carries ≈: replicas write into a fixed number of per-day slots and overwrite each other beyond that, so these numbers are lower bounds.",
    "ja": "すべての件数に ≈ が付きます：1 日あたりのスロット数を超えるとレプリカ同士が上書きするため、この数値は下限です。",
    "ko": "모든 집계에 ≈ 가 붙습니다: 하루당 슬롯 수를 넘으면 복제본끼리 덮어쓰므로 이 수치는 하한값입니다."
  },
  "section.usg.incompleteNote": {
    "zh-CN": "这份数字是不完整的：有一部分分片是畸形的，已经被丢掉。",
    "zh-TW": "這份數字是不完整的：有一部分分片是畸形的，已經被丟掉。",
    "en": "These numbers are incomplete: some shards were malformed and had to be discarded.",
    "ja": "この数値は不完全です：一部のシャードが壊れており破棄されました。",
    "ko": "이 수치는 불완전합니다: 일부 샤드가 손상되어 버려졌습니다."
  },
  "section.usg.card.requests": {
    "zh-CN": "请求数",
    "zh-TW": "請求數",
    "en": "Requests",
    "ja": "リクエスト数",
    "ko": "요청 수"
  },
  "section.usg.card.successRate": {
    "zh-CN": "成功率",
    "zh-TW": "成功率",
    "en": "Success rate",
    "ja": "成功した割合",
    "ko": "성공률"
  },
  "section.usg.card.errorRate": {
    "zh-CN": "错误率",
    "zh-TW": "錯誤率",
    "en": "Error rate",
    "ja": "エラー率",
    "ko": "오류율"
  },
  "section.usg.card.avgLatency": {
    "zh-CN": "平均延迟（只算成功请求）",
    "zh-TW": "平均延遲（只算成功請求）",
    "en": "Average latency (successful requests only)",
    "ja": "平均レイテンシ（成功リクエストのみ）",
    "ko": "평균 지연 시간(성공한 요청만)"
  },
  "section.usg.card.tokens": {
    "zh-CN": "token 合计",
    "zh-TW": "token 合計",
    "en": "Total tokens",
    "ja": "トークン合計",
    "ko": "토큰 합계"
  },
  "section.usg.card.streaming": {
    "zh-CN": "流式请求数",
    "zh-TW": "串流請求數",
    "en": "Streaming requests",
    "ja": "ストリーミング要求数",
    "ko": "스트리밍 요청 수"
  },
  "section.usg.cell.none": {
    "zh-CN": "这一次读成功了，只是这一段没有可用的样本",
    "zh-TW": "這一次讀成功了，只是這一段沒有可用的樣本",
    "en": "The read succeeded; this window simply has no usable sample",
    "ja": "読み取りは成功しましたが、この期間には使えるサンプルがありません",
    "ko": "읽기는 성공했지만 이 구간에는 사용할 표본이 없습니다"
  },
  "section.usg.cell.noneNoShards": {
    "zh-CN": "这段时间还没有任何分片落盘，「有没有样本」正是分不出来的那件事",
    "zh-TW": "這段時間還沒有任何分片落盤，「有沒有樣本」正是分不出來的那件事",
    "en": "No shard has been persisted for this window, so whether samples exist is exactly what we cannot tell",
    "ja": "この期間はシャードが 1 つも保存されておらず、サンプルの有無こそが判別できない点です",
    "ko": "이 구간에는 저장된 샤드가 하나도 없어, 표본 존재 여부 자체를 알 수 없습니다"
  },
  "section.usg.cell.unknown": {
    "zh-CN": "这一块压根没读出来 —— 这是「不知道」，不是 0",
    "zh-TW": "這一塊壓根沒讀出來 —— 這是「不知道」，不是 0",
    "en": "This block could not be read at all — it means “unknown”, not zero",
    "ja": "このブロックはまったく読み取れませんでした —— 「不明」であり 0 ではありません",
    "ko": "이 블록은 전혀 읽지 못했습니다 — “알 수 없음”이지 0이 아닙니다"
  },
  "section.usg.msValue": {
    "zh-CN": "{n} 毫秒",
    "zh-TW": "{n} 毫秒",
    "en": "{n} ms",
    "ja": "{n} ミリ秒",
    "ko": "{n} 밀리초"
  },
  "section.usg.metric": {
    "zh-CN": "度量",
    "zh-TW": "度量",
    "en": "Metric",
    "ja": "指標",
    "ko": "지표"
  },
  "section.usg.metric.requests": {
    "zh-CN": "请求",
    "zh-TW": "請求",
    "en": "Requests",
    "ja": "リクエスト",
    "ko": "요청"
  },
  "section.usg.metric.tokens": {
    "zh-CN": "token",
    "zh-TW": "權杖",
    "en": "Tokens",
    "ja": "トークン",
    "ko": "토큰"
  },
  "section.usg.metric.errors": {
    "zh-CN": "错误",
    "zh-TW": "錯誤",
    "en": "Errors",
    "ja": "エラー",
    "ko": "오류"
  },
  "section.usg.timeRange": {
    "zh-CN": "时间范围",
    "zh-TW": "時間範圍",
    "en": "Time range",
    "ja": "期間",
    "ko": "기간"
  },
  "section.usg.range.d1": {
    "zh-CN": "今天",
    "zh-TW": "今天",
    "en": "Today",
    "ja": "本日",
    "ko": "오늘"
  },
  "section.usg.range.d3": {
    "zh-CN": "3 天",
    "zh-TW": "3 天",
    "en": "3 days",
    "ja": "3 日間",
    "ko": "3일"
  },
  "section.usg.range.d7": {
    "zh-CN": "7 天",
    "zh-TW": "7 天",
    "en": "7 days",
    "ja": "7 日間",
    "ko": "7일"
  },
  "section.usg.range.d30": {
    "zh-CN": "30 天",
    "zh-TW": "30 天",
    "en": "30 days",
    "ja": "30 日間",
    "ko": "30일"
  },
  "section.usg.chart.alt": {
    "zh-CN": "按 UTC 日分组的{metric}柱状图，共 {n} 天",
    "zh-TW": "按 UTC 日分組的{metric}長條圖，共 {n} 天",
    "en": "Bar chart of {metric} grouped by UTC day, {n} days total",
    "ja": "UTC 日ごとの{metric}棒グラフ、全 {n} 日",
    "ko": "UTC 일 단위 {metric} 막대그래프, 총 {n}일"
  },
  "section.usg.chart.off": {
    "zh-CN": "这个部署没有打开用量统计，所以根本没有在记账。",
    "zh-TW": "這個部署沒有打開用量統計，所以根本沒有在記帳。",
    "en": "Usage statistics are switched off in this deployment, so nothing is being recorded at all.",
    "ja": "このデプロイでは利用統計が無効なため、そもそも記録されていません。",
    "ko": "이 배포에서는 사용 통계가 꺼져 있어 아무것도 기록되지 않습니다."
  },
  "section.usg.chart.unavailable": {
    "zh-CN": "这段时间的数据没读出来 —— 这不是「没有流量」。",
    "zh-TW": "這段時間的資料沒讀出來 —— 這不是「沒有流量」。",
    "en": "The data for this window could not be read — this is not “no traffic”.",
    "ja": "この期間のデータを読み取れませんでした —— 「トラフィックが無い」という意味ではありません。",
    "ko": "이 구간의 데이터를 읽지 못했습니다 — “트래픽 없음”이라는 뜻이 아닙니다."
  },
  "section.usg.chart.empty": {
    "zh-CN": "这段区间里没有可以画的日子。",
    "zh-TW": "這段區間裡沒有可以畫的日子。",
    "en": "There is no day to plot in this window.",
    "ja": "この期間には描画できる日がありません。",
    "ko": "이 구간에는 그릴 날짜가 없습니다."
  },
  "section.usg.dailyTable": {
    "zh-CN": "逐日汇总（点一行下钻）",
    "zh-TW": "逐日彙總（點一行下鑽）",
    "en": "Daily breakdown (click a row to drill down)",
    "ja": "日別集計（行をクリックしてドリルダウン）",
    "ko": "일별 집계(행을 클릭하면 드릴다운)"
  },
  "section.usg.col.date": {
    "zh-CN": "日期（UTC）",
    "zh-TW": "日期（UTC）",
    "en": "Date (UTC)",
    "ja": "日付（UTC）",
    "ko": "날짜(UTC)"
  },
  "section.usg.col.requests": {
    "zh-CN": "请求",
    "zh-TW": "請求",
    "en": "Req.",
    "ja": "要求",
    "ko": "요청"
  },
  "section.usg.col.success": {
    "zh-CN": "成功",
    "zh-TW": "成功",
    "en": "OK",
    "ja": "成功数",
    "ko": "성공"
  },
  "section.usg.col.errors": {
    "zh-CN": "错误",
    "zh-TW": "錯誤",
    "en": "Err.",
    "ja": "エラー",
    "ko": "오류"
  },
  "section.usg.col.tokens": {
    "zh-CN": "token",
    "zh-TW": "權杖",
    "en": "Tokens",
    "ja": "トークン",
    "ko": "토큰"
  },
  "section.usg.col.streaming": {
    "zh-CN": "流式",
    "zh-TW": "串流",
    "en": "Stream",
    "ja": "ストリーム",
    "ko": "스트림"
  },
  "section.usg.table.none": {
    "zh-CN": "这段区间里没有可以列出的日子（读成功了，就是没有）。",
    "zh-TW": "這段區間裡沒有可以列出的日子（讀成功了，就是沒有）。",
    "en": "No day to list in this window (the read succeeded; there simply is none).",
    "ja": "この期間に一覧できる日はありません（読み取りは成功、単に存在しません）。",
    "ko": "이 구간에 나열할 날짜가 없습니다(읽기는 성공했고 단지 없습니다)."
  },
  "section.usg.table.unavailable": {
    "zh-CN": "这张表读不出来 —— 这不是「这几天没有请求」。",
    "zh-TW": "這張表讀不出來 —— 這不是「這幾天沒有請求」。",
    "en": "This table could not be read — it does not mean “no requests on those days”.",
    "ja": "この表は読み取れませんでした —— 「その期間にリクエストが無い」という意味ではありません。",
    "ko": "이 표를 읽지 못했습니다 — “해당 기간에 요청이 없었다”는 뜻이 아닙니다."
  },
  "section.usg.drill.open": {
    "zh-CN": "下钻查看 {date}",
    "zh-TW": "下鑽檢視 {date}",
    "en": "Drill into {date}",
    "ja": "{date} をドリルダウン",
    "ko": "{date} 드릴다운"
  },
  "section.usg.drill.title": {
    "zh-CN": "{date} 的分维明细",
    "zh-TW": "{date} 的分維明細",
    "en": "Breakdown for {date}",
    "ja": "{date} の内訳",
    "ko": "{date} 세부 분해"
  },
  "section.usg.drill.close": {
    "zh-CN": "收起",
    "zh-TW": "收合",
    "en": "Collapse",
    "ja": "折りたたむ",
    "ko": "접기"
  },
  "section.usg.drill.off": {
    "zh-CN": "这个部署没有打开用量统计，这一天没有任何记账。",
    "zh-TW": "這個部署沒有打開用量統計，這一天沒有任何記帳。",
    "en": "Usage statistics are switched off in this deployment; nothing was recorded for this day.",
    "ja": "このデプロイでは利用統計が無効で、この日の記録はありません。",
    "ko": "이 배포에서는 사용 통계가 꺼져 있어 이 날짜의 기록이 없습니다."
  },
  "section.usg.drill.unavailable": {
    "zh-CN": "这一天的明细没读出来 —— 四张表都是「不知道」，不是 0。",
    "zh-TW": "這一天的明細沒讀出來 —— 四張表都是「不知道」，不是 0。",
    "en": "The breakdown for this day could not be read — all four tables are “unknown”, not zero.",
    "ja": "この日の内訳を読み取れませんでした —— 4 つの表はすべて「不明」であり 0 ではありません。",
    "ko": "이 날짜의 분해를 읽지 못했습니다 — 네 개 표 모두 “알 수 없음”이며 0이 아닙니다."
  },
  "section.usg.drill.empty": {
    "zh-CN": "这一维在这一天没有记录。",
    "zh-TW": "這一維在這一天沒有記錄。",
    "en": "This dimension has no record for that day.",
    "ja": "この軸にはこの日の記録がありません。",
    "ko": "이 축에는 해당 날짜의 기록이 없습니다."
  },
  "section.usg.dim.hours": {
    "zh-CN": "按小时",
    "zh-TW": "按小時",
    "en": "By hour",
    "ja": "時間別",
    "ko": "시간별"
  },
  "section.usg.dim.hours.col": {
    "zh-CN": "小时（UTC）",
    "zh-TW": "小時（UTC）",
    "en": "Hour (UTC)",
    "ja": "時刻（UTC）",
    "ko": "시각(UTC)"
  },
  "section.usg.dim.byModel": {
    "zh-CN": "按模型",
    "zh-TW": "按模型",
    "en": "By model",
    "ja": "モデル別",
    "ko": "모델별"
  },
  "section.usg.dim.byModel.col": {
    "zh-CN": "模型",
    "zh-TW": "模型",
    "en": "Model",
    "ja": "モデル",
    "ko": "모델"
  },
  "section.usg.dim.byProtocol": {
    "zh-CN": "按协议",
    "zh-TW": "按協定",
    "en": "By protocol",
    "ja": "プロトコル別",
    "ko": "프로토콜별"
  },
  "section.usg.dim.byProtocol.col": {
    "zh-CN": "协议",
    "zh-TW": "協定",
    "en": "Protocol",
    "ja": "プロトコル",
    "ko": "프로토콜"
  },
  "section.usg.dim.byApiKey": {
    "zh-CN": "按密钥",
    "zh-TW": "按金鑰",
    "en": "By API key",
    "ja": "API キー別",
    "ko": "API 키별"
  },
  "section.usg.dim.byApiKey.col": {
    "zh-CN": "对外 API 密钥",
    "zh-TW": "對外 API 金鑰",
    "en": "Outbound API key",
    "ja": "発行済み API キー",
    "ko": "발급한 API 키"
  },
  "section.usg.hourLabel": {
    "zh-CN": "{h} 时",
    "zh-TW": "{h} 時",
    "en": "{h}:00",
    "ja": "{h} 時台",
    "ko": "{h}시대"
  },
  "section.usg.key.master": {
    "zh-CN": "主口令（不是一把签发出去的密钥）",
    "zh-TW": "主口令（不是一把簽發出去的金鑰）",
    "en": "Master token (not an issued key)",
    "ja": "マスタートークン（発行済みキーではありません）",
    "ko": "마스터 토큰(발급된 키가 아님)"
  },
  "section.usg.key.unattributed": {
    "zh-CN": "归不到任何一把密钥的流量",
    "zh-TW": "歸不到任何一把金鑰的流量",
    "en": "Traffic that could not be attributed to any key",
    "ja": "どのキーにも紐づけられなかったトラフィック",
    "ko": "어떤 키에도 귀속되지 않은 트래픽"
  },
  "section.usg.key.other": {
    "zh-CN": "其它（已被合并的长尾）",
    "zh-TW": "其他（已被合併的長尾）",
    "en": "Other (merged long tail)",
    "ja": "その他（まとめられたロングテール）",
    "ko": "기타(병합된 롱테일)"
  },
  "section.usg.note.tier2Off": {
    "zh-CN": "这个部署没有打开用量统计（Tier-2），所以根本没有在记账。",
    "zh-TW": "這個部署沒有打開用量統計（Tier-2），所以根本沒有在記帳。",
    "en": "Usage statistics (Tier-2) are off in this deployment, so nothing is being recorded.",
    "ja": "このデプロイでは利用統計（Tier-2）が無効なため、記録されていません。",
    "ko": "이 배포에서는 사용 통계(Tier-2)가 꺼져 있어 기록되지 않습니다."
  },
  "section.usg.note.clockUnavailable": {
    "zh-CN": "服务端时钟给不出一个有限数字，这一段的区间无从算起。",
    "zh-TW": "伺服器時鐘給不出一個有限數字，這一段的區間無從算起。",
    "en": "The server clock did not yield a finite value, so the window cannot be computed.",
    "ja": "サーバー時計が有限の値を返さず、期間を計算できません。",
    "ko": "서버 시계가 유한한 값을 주지 못해 구간을 계산할 수 없습니다."
  },
  "section.usg.note.readFailed": {
    "zh-CN": "读存储失败，这一整块数字都是「不知道」——去看容器日志。",
    "zh-TW": "讀儲存失敗，這一整塊數字都是「不知道」——去看容器日誌。",
    "en": "Reading storage failed; this entire block is “unknown” — check the container logs.",
    "ja": "ストレージの読み取りに失敗しました。このブロック全体が「不明」です —— コンテナログを確認してください。",
    "ko": "스토리지 읽기에 실패했습니다. 이 블록 전체가 “알 수 없음”입니다 — 컨테이너 로그를 확인하세요."
  },
  "section.usg.note.rangeClamped": {
    "zh-CN": "你要的区间被缩小过：有一部分早于保留期，或者晚于服务端此刻。",
    "zh-TW": "你要的區間被縮小過：有一部分早於保留期，或者晚於伺服器此刻。",
    "en": "The requested window was narrowed: part of it falls outside the retention period or lies in the future.",
    "ja": "要求した期間は縮められました：保持期間より前、またはサーバーの現在より先の部分があります。",
    "ko": "요청한 구간이 축소되었습니다: 일부가 보존 기간 이전이거나 서버 현재보다 미래입니다."
  },
  "section.usg.note.dateOutOfRetention": {
    "zh-CN": "这一天整个落在保留期之外 —— 不是那天没有请求，是那天已经不在了。",
    "zh-TW": "這一天整個落在保留期之外 —— 不是那天沒有請求，是那天已經不在了。",
    "en": "That day is entirely outside the retention period — it is not that there were no requests, the day is simply gone.",
    "ja": "その日は保持期間の外です —— リクエストが無かったのではなく、その日自体が残っていません。",
    "ko": "그 날짜는 보존 기간 밖입니다 — 요청이 없었던 것이 아니라 그 날짜 자체가 남아 있지 않습니다."
  },
  "section.usg.note.noShards": {
    "zh-CN": "读成功了，但这段区间一条分片都还没落盘（可能没流量，也可能实例没活到一次落盘）。",
    "zh-TW": "讀成功了，但這段區間一條分片都還沒落盤（可能沒流量，也可能實例沒活到一次落盤）。",
    "en": "The read succeeded, but no shard has been persisted for this window (either no traffic, or no instance survived long enough to flush).",
    "ja": "読み取りは成功しましたが、この期間のシャードが 1 つも保存されていません（トラフィックが無いか、インスタンスが書き込み前に終了した可能性）。",
    "ko": "읽기는 성공했지만 이 구간에 저장된 샤드가 없습니다(트래픽이 없었거나 인스턴스가 저장 전에 종료됨)."
  },
  "section.usg.note.allMalformed": {
    "zh-CN": "读到了分片，但每一个都是畸形的 —— 去查是谁往存储里写了这些东西。",
    "zh-TW": "讀到了分片，但每一個都是畸形的 —— 去查是誰往儲存裡寫了這些東西。",
    "en": "Shards were found but every one of them is malformed — find out who wrote them into storage.",
    "ja": "シャードは見つかりましたがすべて壊れています —— 誰がストレージに書き込んだのか調べてください。",
    "ko": "샤드를 찾았지만 모두 손상되었습니다 — 누가 스토리지에 기록했는지 확인하세요."
  },
  "section.usg.note.partialMalformed": {
    "zh-CN": "一部分分片是畸形的，这份数字不完整 —— 去查是谁往存储里写了这些东西。",
    "zh-TW": "一部分分片是畸形的，這份數字不完整 —— 去查是誰往儲存裡寫了這些東西。",
    "en": "Some shards are malformed, so these numbers are incomplete — find out who wrote them into storage.",
    "ja": "一部のシャードが壊れており、この数値は不完全です —— 誰が書き込んだのか調べてください。",
    "ko": "일부 샤드가 손상되어 수치가 불완전합니다 — 누가 기록했는지 확인하세요."
  },
  "section.usg.note.noRequestDetail": {
    "zh-CN": "这里给的是聚合，没有逐请求流水；要逐请求粒度请看容器 stdout。",
    "zh-TW": "這裡給的是彙總，沒有逐請求流水；要逐請求粒度請看容器 stdout。",
    "en": "Only aggregates are available here; for per-request detail read the container stdout.",
    "ja": "ここでは集計のみを提供します。リクエスト単位の詳細はコンテナの stdout を参照してください。",
    "ko": "여기서는 집계만 제공합니다. 요청 단위 상세는 컨테이너 stdout을 보세요."
  },
  "section.usg.note.unknown": {
    "zh-CN": "后端报了一条面板还不认识的说明。",
    "zh-TW": "後端回了一條面板還不認識的說明。",
    "en": "The backend reported a note this panel does not recognise yet.",
    "ja": "パネルがまだ認識していない注記がバックエンドから返されました。",
    "ko": "패널이 아직 모르는 안내를 백엔드가 반환했습니다."
  }
};
