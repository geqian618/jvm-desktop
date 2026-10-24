#!/usr/bin/env bash
# 把安装包发布到 $GITHUB_REF_NAME 对应的 GitHub Release：
# Release 已存在则上传覆盖（--clobber），不存在则创建。
# 三个平台 job 会并发发布同一个 Release，创建/上传冲突时整轮重试（最多 5 次）。
#
# 用法：publish-release.sh <安装包路径...>（支持 glob，调用方 shell 展开）
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "::error::未传入任何安装包路径"
  exit 1
fi

for f in "$@"; do
  if [ ! -f "$f" ]; then
    # glob 未命中时调用方会原样传入字面量，这里统一兜底检查
    echo "::error::安装包不存在: $f"
    exit 1
  fi
done

: "${GITHUB_REF_NAME:?GITHUB_REF_NAME 未设置}"
: "${GH_TOKEN:?GH_TOKEN 未设置}"

for attempt in 1 2 3 4 5; do
  if gh release view "$GITHUB_REF_NAME" >/dev/null 2>&1; then
    if gh release upload "$GITHUB_REF_NAME" "$@" --clobber; then
      echo "已上传到既有 Release $GITHUB_REF_NAME: $*"
      exit 0
    fi
  else
    # 并发下另一平台可能刚好先创建了 Release：创建失败后下一轮重试走 upload 分支
    if gh release create "$GITHUB_REF_NAME" "$@" \
        --title "JVM Desktop $GITHUB_REF_NAME" \
        --generate-notes; then
      echo "已创建 Release $GITHUB_REF_NAME 并上传: $*"
      exit 0
    fi
  fi
  echo "发布失败（第 $attempt/5 次），20 秒后重试..."
  sleep 20
done

echo "::error::Release 发布重试 5 次仍失败"
exit 1
