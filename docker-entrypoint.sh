#!/bin/sh
# 🔴 **这个文件不是可有可无的。**
#
# 镜像里的进程以 `appuser`(uid 1000) 跑，而 `docker compose` 的 bind mount
#（`./data:/app/data`）会把**宿主目录的属主**原样带进容器。宿主那边通常是 root 建的，
# 于是容器内 uid 1000 对它没有写权限 —— 网关起得来、`/health` 回 503 `storage.writable=false`，
# 而那个症状看着像代码 bug，排查方向整个跑偏。
#
# 做法：以 root 起来时先把挂载卷的属主修正掉，**再降权**执行真正的命令。
# 已经是非 root（`user:` 被显式指定）时什么都不做直接 exec —— 那种部署是运维自己
# 安排好属主的，我们不该也不能去 chown。
set -e

DATA_DIR_EFFECTIVE="${DATA_DIR:-/app/data}"

if [ "$(id -u)" = "0" ]; then
    mkdir -p "$DATA_DIR_EFFECTIVE" 2>/dev/null || true
    # `|| true`：只读挂载、NFS、宿主 ACL 都可能让 chown 失败。
    # 那种情况下让网关自己去报 `storage.writable=false`（它的报文比这里的更有用），
    # 而不是在 entrypoint 里把容器直接弄死。
    chown -R appuser:appuser "$DATA_DIR_EFFECTIVE" 2>/dev/null || true
    exec gosu appuser "$@"
fi

exec "$@"
