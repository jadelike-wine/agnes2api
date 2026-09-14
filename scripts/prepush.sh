#!/usr/bin/env bash
# ── agnes2api 推送前复跑清单 ────────────────────────────────────────────────
#
# 用法：
#   bash scripts/prepush.sh                # 逐格全跑，末尾打一张逐格表
#   bash scripts/prepush.sh --skip-smoke   # 同上，但跳过⑧（真机冒烟，要 docker）
#
# **为什么有这个脚本。** CI 看的是**已经推上去的那个提交**，推之前手上多出来的东西
# 它一个字都看不见。本仓的工作流经常把变异探针留在树里 ——
# 「判据全绿、而树上还挂着一个探针」这件事没有任何一道 CI 门禁会说话。
#
# 📜 它是 v1.0.0 重写时**新写的一份**，不是上一版那个三千行脚本的移植：
# 那一份里绝大多数格子讲的是 TypeScript 时代的关切（生成物对账链、vitest 收集门禁、
# README 迁移表）。**照搬过来的话，大半格子测的是已经不存在的东西**，
# 而一张大部分格子恒绿的表比没有表更坏 —— 人会开始相信它。
#
# ⚠️ **形态：`set -uo pipefail`，顶层没有 `-e`。这是有意的，加回去会毁掉它的用途。**
# 它必须**逐格跑完再汇总**：一格红了，后面那几格的状态照样要读得到 ——
# 而「哪几格红」恰恰是推送前唯一想读出来的结论。fail-fast 会在第一格停住。
# ⚠️ 但**每一格自己跑在 `( set -e; … )` 子壳里**：格内的意外失败（git 不在、
# 命令拼错）必须当场把这一格弄红，不许被「下一行照跑」吞掉。
#
# ⚠️⚠️ **本脚本绿 ≠ GitHub CI 绿。** 它跑的是同一批命令，但跑在你的机器上：
# 工具链版本、缓存、网络都不同。推完仍然要看那一跑的 CI。

set -uo pipefail
cd "$(dirname "$0")/.."

SKIP_SMOKE=0
[[ "${1:-}" == "--skip-smoke" ]] && SKIP_SMOKE=1

ROWS=()
FAILED=0

# 跑一格。`$1` 是表里那一行的名字，其余是要执行的命令。
cell() {
  local name="$1"; shift
  printf '\n\033[1m── %s\033[0m\n' "$name" >&2
  if ( set -e; "$@" ) >&2; then
    ROWS+=("✅  $name")
  else
    ROWS+=("❌  $name")
    FAILED=1
  fi
}

skip() { ROWS+=("⏭️   $1  SKIPPED"); }

# ── CI 那七道，同序 ────────────────────────────────────────────────────────
cell "① 跟踪文件不许是二进制" node scripts/check-no-binary.mjs
cell "② PNG 结构审计" node scripts/check-png.mjs --skip-inline
cell "③ 凭据扫描（工作树）" bash scripts/scan-secrets.sh
cell "③ 凭据扫描（全历史）" bash scripts/scan-secrets.sh --history
cell "④ 格式" cargo fmt --all -- --check
cell "⑤ clippy（警告当错误）" cargo clippy --all-targets -- -D warnings
cell "⑥ 全部判据" cargo test
cell "⑦ 构建" cargo build --release

# ── CI 结构上看不见的那几格 ────────────────────────────────────────────────

# 🔴 工作树必须干净。CI 看的是提交，看不见你手上还留着什么。
check_clean() {
  local dirty
  dirty="$(git status --porcelain)"
  if [[ -n "$dirty" ]]; then
    echo "工作树不干净（变异探针忘了撤？）：" >&2
    echo "$dirty" >&2
    return 1
  fi
}
cell "⑧ 工作树干净" check_clean

# 🔴 只有一个分支：main。
check_branch() {
  local b; b="$(git rev-parse --abbrev-ref HEAD)"
  [[ "$b" == "main" ]] || { echo "当前分支是 $b，本仓只留 main" >&2; return 1; }
}
cell "⑨ 在 main 上" check_branch

# 🔴 作者身份：xwteam，且**不许带 AI 署名尾注**。
#    这两条是同一次检查的两半：署名对了而尾注还在，仍然是不合格的提交。
check_author() {
  local bad
  bad="$(git log --format='%H %an <%ae>' origin/main..HEAD 2>/dev/null \
         | grep -v 'xwteam <xwteam@xwteam.cn>' || true)"
  if [[ -n "$bad" ]]; then
    echo "这些提交的作者不是 xwteam <xwteam@xwteam.cn>：" >&2
    echo "$bad" >&2
    return 1
  fi
  local trailer
  trailer="$(git log --format='%B' origin/main..HEAD 2>/dev/null \
             | grep -iE 'co-authored-by|generated with|claude' || true)"
  if [[ -n "$trailer" ]]; then
    echo "提交信息里有 AI 署名尾注：" >&2
    echo "$trailer" >&2
    return 1
  fi
}
cell "⑩ 作者身份与尾注" check_author

# 🔴 VERSION / Cargo.toml / Cargo.lock 三处一致，且 CHANGELOG 有这个版本的条目。
#    ⚠️ 前一半 `cargo test` 里已经有判据钉着；这里重跑一遍是为了**在同一张表上**
#    与 CHANGELOG 那一半一起给出结论 —— 发版时这两件事总是一起忘。
check_version() {
  local v; v="$(tr -d '[:space:]' < VERSION)"
  grep -q "^version = \"$v\"$" Cargo.toml || { echo "Cargo.toml 不是 $v" >&2; return 1; }
  grep -q "^## \[$v\]" CHANGELOG.md || { echo "CHANGELOG.md 里没有 [$v] 的条目" >&2; return 1; }
  echo "VERSION / Cargo.toml / CHANGELOG 都是 $v"
}
cell "⑪ 版本三处一致 + CHANGELOG 有条目" check_version

# ── 真机冒烟 ──────────────────────────────────────────────────────────────
#
# 🔴 **判据全绿不等于镜像跑得起来。** 交叉编译的 glibc 符号、entrypoint 的降权、
#    数据目录属主 —— 这几样只有真起一个容器才看得见，而它们坏掉的形态是
#    「容器起不来」或「起来了但 /health 说存储不可写」，一条判据都抓不到。
#
# ⚠️ **这个开关不是「静默跳过」**：跳过时逐格表里那一行写 `SKIPPED`，
#    末尾的判词也不再说「全过」。
smoke() {
  local tag="agnes2api-prepush:local"
  docker build -q -t "$tag" . >/dev/null
  local dir; dir="$(mktemp -d)"
  # ⚠️ 端口与主机名**分开写、不拼成字面量**：凭据扫描那道门禁对
  #    `IP:端口` 这个形态是**零白名单**的（它分不出「示例」和「某个部署的真地址」，
  #    而那正是它该有的态度）。把回环地址与端口拼成一个字面量写在这里 ——
  #    **哪怕写在注释里** —— 都会让本脚本自己红：那道门禁不区分注释和代码，
  #    而它不区分是对的。
  local port=18799
  local cid
  cid="$(docker run -d --rm -p "$port:8080" \
      -e GATEWAY_TOKEN=gw-prepush-0123456789abcdef \
      -e ADMIN_TOKEN=admin-prepush-0123456789abcdef \
      -v "$dir:/app/data" "$tag")"
  # 🔴 **收尾一律吞掉失败，绝不许影响这一格的结论。**
  #    entrypoint 会把绑定挂载 chown 成容器内那个用户（那正是它该做的），
  #    于是宿主这边的 `rm -rf` 拿到 `Operation not permitted`。
  #    不吞掉的话，一次**成功的**冒烟会因为删不掉临时目录而报红 ——
  #    **假红与假绿一样坏**：它教人忽略这张表。
  #    删不掉的那个目录留在 /tmp 里，由系统自己清。
  # shellcheck disable=SC2064
  trap "docker rm -f $cid >/dev/null 2>&1 || true; rm -rf '$dir' 2>/dev/null || true" RETURN
  local i body
  for i in $(seq 1 30); do
    body="$(curl -s -m 3 "http://localhost:$port/health" || true)"
    [[ -n "$body" ]] && break
    sleep 1
  done
  echo "  /health: $body"
  # 🔴 只判「起来了」不够：**属主不对时它照样 200，但 storage.writable 是 false**，
  #    而那种部署的表现是「面板能开、改什么都存不下」。
  grep -q '"writable":true' <<<"$body" \
    || { echo "存储不可写 —— 多半是数据目录属主与容器内用户对不上" >&2; return 1; }
  grep -q "\"version\":\"$(tr -d '[:space:]' < VERSION)\"" <<<"$body" \
    || { echo "镜像里的版本号与 VERSION 对不上 —— 拉到旧镜像了？" >&2; return 1; }
}
if [[ "$SKIP_SMOKE" == "1" ]]; then
  skip "⑫ 真机冒烟（Docker）"
else
  cell "⑫ 真机冒烟（Docker）" smoke
fi

# ── 逐格表 ────────────────────────────────────────────────────────────────
printf '\n\033[1m══ 推送前逐格表 ══\033[0m\n'
for r in "${ROWS[@]}"; do printf '%s\n' "$r"; done
if [[ "$FAILED" == "1" ]]; then
  printf '\n\033[31m有格子是红的，别推。\033[0m\n'
  exit 1
fi
if [[ "$SKIP_SMOKE" == "1" ]]; then
  printf '\n\033[33m其余各格都过了，但真机冒烟跳过了 —— 这不是「全过」。\033[0m\n'
else
  printf '\n\033[32m全过。\033[0m\n'
fi
printf '⚠️  本脚本绿 ≠ GitHub CI 绿：同一批命令，不同的机器。推完仍然要看那一跑。\n'
