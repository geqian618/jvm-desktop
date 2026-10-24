#!/usr/bin/env bash
# 把 tauri 产物文件名统一为小写连字符风格：
#   JVM.Desktop_0.1.0_amd64.deb / JVM Desktop_0.1.0_x64-setup.exe
#   -> jvm-desktop_0.1.0_amd64.deb / jvm-desktop_0.1.0_x64-setup.exe
# 兼容空格与点两种变体（不同 tauri CLI 版本对 productName 空格的转写不同）。
# 用法：normalize-names.sh <bundle 目录>...
set -euo pipefail
shopt -s nullglob

for dir in "$@"; do
  for f in "$dir"/JVM.Desktop_* "$dir"/JVM\ Desktop_*; do
    [ -e "$f" ] || continue
    base="$(basename "$f")"
    new="${base/JVM.Desktop/jvm-desktop}"
    new="${new/JVM Desktop/jvm-desktop}"
    [ "$base" = "$new" ] && continue
    mv "$f" "$dir/$new"
    echo "重命名: $base -> $new"
  done
done
