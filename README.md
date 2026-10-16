# JVM Desktop（jvm）

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](#环境要求)
[![Rust](https://img.shields.io/badge/Rust-2021%20edition-orange.svg)](#环境要求)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB.svg)](#环境要求)
[![Vue](https://img.shields.io/badge/Vue-3-42b883.svg)](#环境要求)

> **参考开源项目 [nvm-desktop](https://github.com/1111mp/nvm-desktop) 同源设计的 JDK 版本管理工具**：
> 可视化浏览 / 安装 / 切换各 OpenJDK 发行版
> （Temurin、Zulu、Corretto、GraalVM、Dragonwell、Microsoft OpenJDK、毕盛……），
> 支持**项目级版本绑定**（`.jvmrc`），GUI + CLI 双模式，跨 Windows / macOS / Linux。

## 目录

- [项目简介](#项目简介)
- [功能特性](#功能特性)
- [环境要求](#环境要求)
- [快速开始](#快速开始)
- [CLI 使用指南](#cli-使用指南)
- [GUI 使用指南](#gui-使用指南)
- [版本解析规则](#版本解析规则)
- [数据目录](#数据目录)
- [配置说明](#配置说明)
- [数据源管理](#数据源管理)
- [架构与工作原理](#架构与工作原理)
- [开发指南](#开发指南)
- [常见问题](#常见问题)
- [参与贡献](#参与贡献)
- [致谢](#致谢)
- [相关文档](#相关文档)
- [License](#license)

## 项目简介

日常 Java 开发经常要在多个 JDK 版本、多个发行商之间切换：公司项目用 Corretto 11、
开源项目用 Temurin 17、试试 GraalVM 原生镜像……手动改 `JAVA_HOME`、维护 `PATH`、
每个项目记住该用哪个版本，既繁琐又容易出错。

JVM Desktop（命令行下叫 `jvm`）是一款**参考开源项目
[nvm-desktop](https://github.com/1111mp/nvm-desktop) 同源设计开发的 JDK 版本管理工具**：
数据目录布局、shim 转发机制、项目级版本绑定与 GUI + CLI 双模式的整体设计均承袭该项目的
同源理念（项目绑定文件 `.nvmdrc` 在 Java 场景对应为 `.jvmrc`），并针对 JDK 多发行商、
多平台的场景用 Rust + Tauri 完整重新实现：

- **一个数据目录**集中管理所有 JDK，`current` 链接指向默认版本，切换只改链接；
- **`vendor@version` 唯一键**标识每个版本，安装、切换、卸载全部围绕它；
- **项目级绑定**：项目根放一个 `.jvmrc`（单行 `vendor@version`），进目录即生效，
  可提交 Git，团队共享；
- **shim 机制**：`java` / `javac` 等命令经 `~/.jvm/bin` 下的 shim 转发，
  shim 按当前目录解析版本，并为子进程注入 `JAVA_HOME`，maven / gradle 无需任何额外配置。

## 功能特性

- **多发行版支持** — Temurin、Zulu、Corretto、GraalVM（Community）、Dragonwell、
  Microsoft OpenJDK、毕盛（Bisheng）等，数据源基于 [foojay Disco API](https://api.foojay.io/)，
  亦可切换 Adoptium 官方 API 或任意 foojay 兼容镜像
- **GUI + CLI 双模式** — 图形界面（Tauri 2 + Vue 3）负责浏览与安装，命令行负责脚本化与
  日常切换，两者共享同一份数据与同一套解析规则
- **项目级版本绑定** — `.jvmrc` + `projects.json` 两级项目绑定，目录向上逐级解析
- **可靠的下载** — 分块并发、断点续传（`.part` + `.part.json`）、流式 SHA-256 校验，
  校验和来源可追溯（API 字段 → `<url>.sha256.txt`）
- **零权限切换（Windows）** — 目录 junction 替代 symlink，无需管理员权限；
  注册表写入 `HKCU\Environment`，先备份后写入、失败自动回滚
- **导入本地 JDK** — 扫描本机已有的 JDK 目录，纳入统一管理
- **环境自检** — `jvm doctor` 一键诊断 PATH / JAVA_HOME / shim / 链接 / 遮蔽问题
- **健康检查与自愈** — GUI 检测数据目录换根 / 搬移后的悬空引用，一键修复
- **代理与镜像** — HTTP 代理、自定义数据源（foojay 兼容 URL + 可选 API Key）
- **中英文界面** — vue-i18n，明 / 暗主题跟随系统
- **跨平台** — Windows / macOS / Linux 同一套核心逻辑

## 环境要求

| 场景 | 要求 |
|---|---|
| 运行 GUI | Windows 10+ / macOS 11+ / Linux（WebKitGTK） |
| 运行 CLI | 同上，单文件可执行程序 |
| 源码构建 | Rust 1.77+（Tauri 2 要求）、Node.js 20 LTS+、各平台系统依赖（Tauri  prerequisites） |

## 快速开始

### 方式一：使用预编译版本

前往 [Releases](../../releases) 页面下载对应平台的安装包：

- **Windows**：`.exe` NSIS 安装包（安装器会自动完成 PATH 配置）
- **macOS**：`.dmg`
- **Linux**：`.deb` / `.AppImage`

安装后启动 GUI，或在终端执行 `jvm --help` 查看命令。

### 方式二：从源码构建

```bash
git clone https://github.com/<your-org>/jvm-desktop.git
cd jvm-desktop

# 仅构建 CLI（jvm 命令）
cargo build --release -p jvm-cli
# 产物：target/release/jvm

# 构建 GUI（需要 Node 依赖与 Tauri 系统依赖）
npm install
npm run tauri build
```

把 CLI 产物放到任意目录后，首次执行任意命令即自动完成数据目录初始化：

```bash
./jvm install temurin@17     # 安装 Temurin 17 最新版
./jvm use temurin@17         # 设为默认版本
java -version                # 验证
```

## CLI 使用指南

CLI 与 GUI 是同一个数据目录的两个入口：GUI 装的版本 CLI 立即可用，反之亦然。

### 命令总览

| 命令 | 说明 |
|---|---|
| `jvm list` | 列出本地已安装版本（`->` 标记默认版本，末尾显示当前目录生效的版本及来源） |
| `jvm list-remote` | 列出远程可用版本，默认展示各 LTS 主版本的最新 GA |
| `jvm current` | 显示当前目录解析到的 JDK、来源与 `java` 实际路径 |
| `jvm use <版本>` | 切换默认版本（重建 `current` 链接、shim 与环境变量） |
| `jvm use <版本> --project` | 把当前目录绑定到该版本（生成 `.jvmrc`）而非切换全局默认 |
| `jvm install <版本>` | 安装版本（下载 → 校验 → 解压 → `java -version` 验证 → 登记索引） |
| `jvm install <版本> --default` | 安装并直接设为默认版本 |
| `jvm uninstall <版本>` | 卸载版本（占用中的版本会给出明确提示而非半途失败） |
| `jvm which <工具>` | 打印某命令（`java` / `javac` / `jar` …）解析到的实际路径 |
| `jvm doctor` | 环境自检：数据目录、索引、`current` 链接、shim、PATH、JAVA_HOME、命令遮蔽 |
| `jvm home` | 打印数据目录各路径（安装目录 / shim 目录 / 日志 / 索引……） |
| `jvm config` | 数据源管理（查看 / 注册 / 切换 / 编辑 / 删除） |
| `jvm help [子命令]` | 打印帮助信息 |

全局参数：`-h, --help`、`-V, --version`。

### 版本标识语法

所有命令接受统一的版本标识 `<vendor>@<version>`，version 支持三种粒度：

| 写法 | 示例 | 含义 |
|---|---|---|
| 主版本号 | `17` | 该主版本的最新版 |
| 版本前缀 | `17.0.12` | 匹配 `17.0.12+7` 等 |
| 完整版本 | `17.0.12+7` | 精确匹配 |

`vendor` 可省略，省略时默认 `temurin`（如 `jvm install 21`）。
常见 vendor：`temurin`、`zulu`、`corretto`、`graalvm_community`、`dragonwell`、`microsoft`、`bisheng`。

### 常用工作流

```bash
# 浏览
jvm list-remote                # 各 LTS 最新 GA（默认 temurin）
jvm list-remote -v zulu        # 指定发行商
jvm list-remote --all 17       # 17 的全部 GA 版本
jvm list-remote 21             # 21 的最新 GA
jvm list-remote --jre          # 列出 JRE 而非 JDK

# 安装 / 卸载
jvm install temurin@17               # 安装最新 17
jvm install temurin@17.0.12+7        # 精确版本
jvm install 21 --default             # 安装并设为默认
jvm uninstall temurin@17

# 切换 / 查询
jvm list                      # 本地已装（-> 为默认）
jvm use temurin@17            # 设为默认（写 JAVA_HOME/PATH/shim）
jvm use 21 --project          # 当前目录绑定（.jvmrc）
jvm current                   # 当前目录解析结果
jvm which java                # 解析到的实际 java

# 自检
jvm doctor                    # 环境自检
jvm home                      # 数据目录路径
```

### 数据源管理

```bash
jvm config                                   # 查看当前数据源与预设
jvm config registry --url https://jdk.internal.example/disco/v3.0 \
                     --name 内部镜像          # 注册预设（默认只入列）
jvm config registry --url <URL> --name 内部镜像 --default   # 注册并启用
jvm config use 内部镜像                       # 切换数据源（也支持直接传 URL）
jvm config edit 内部镜像 --url <新URL>        # 编辑预设
jvm config remove 内部镜像                    # 删除自定义预设（内置不可删）
```

`registry` / `use` / `edit` 在保存前会做一次真实的元信息请求校验可达性，
确认打不通就不会落盘影响后续命令；`--force` 可跳过校验。

## GUI 使用指南

GUI 为自定义标题栏的桌面窗口，左侧五个页面：

| 页面 | 功能 |
|---|---|
| **所有版本** | 按发行商 / 主版本 / 支持类型（LTS/MTS/STS）筛选浏览全部 GA 包，一键安装或仅下载，实时进度（下载 / 解压 / 完成）与取消，已下载的包自动复用 |
| **已安装** | 本地 JDK 列表（含导入扫描到的外部 JDK），启用 / 设为默认、卸载，占用检测 |
| **项目关联** | 项目 → 版本绑定管理：添加目录、绑定 / 解绑、删除 `.jvmrc`（按设置项） |
| **设置** | 数据源预设管理、代理、安装 / 下载目录、语言、主题、自动清理压缩包等；数据目录异常时顶部显示健康检查横幅，可一键修复 |
| **日志** | 运行日志（与 `logs/` 文件日志同源） |

界面支持简体中文 / English 与明 / 暗主题（跟随系统）。

## 版本解析规则

CLI、GUI、shim 使用**同一套**解析逻辑，任意目录下执行 `java` 时按以下优先级取版本：

1. **`.jvmrc`** — 从当前目录逐级向上查找项目根的 `.jvmrc`（单行 `vendor@version`，
   可提交 Git 供团队共享）；
2. **`projects.json`** — 命中已登记项目根中距离最近的祖先目录；
3. **全局 `default`** — `jvm use <版本>` 设置的默认版本。

任一层解析到的版本若已卸载或目录损坏，会有明确错误提示而不是静默失败。

## 数据目录

所有数据集中在**一个数据根目录**（`jvm home` 可查看实际路径），解析顺序：

1. 环境变量 `JVM_HOME`（多实例 / 测试场景）；
2. 安装目录下的 `workspace/`（默认；若当前可执行文件是 `<数据根>/bin/jvm`，
   数据根取其上一级，避免递归嵌套）；
3. 以上都不可得时回退用户主目录下 `.jvm`。

目录布局：

```text
<数据根>/
├── versions/            # 已安装 JDK，一版一目录（temurin@17.0.12+7/）
├── bin/                 # 命令 shim：jvm、java、javac、jar、jshell、jlink、jpackage、javaw
├── current              # 指向默认版本的链接（Windows: junction；Unix: symlink）
├── default              # 单行 vendor@version
├── downloads/           # 下载缓存（含 .part 断点文件）
├── logs/                # 滚动日志（jvm.log.YYYY-MM-DD）与环境变量备份
├── index.json           # 本地已装版本索引
├── projects.json        # 项目 → 版本绑定
└── setting.json         # 数据源 / 代理 / 语言 / 主题 / 目录等设置
```

要点：

- **目录名与版本标识一致**：`versions/` 下的目录名就是 `vendor@version`
  （如 `temurin@17.0.12+7`），仅替换个别文件系统非法字符，磁盘命名与列表展示一一对应；
- **`JAVA_HOME` 指向 `<数据根>/current`**：切换默认版本只重建这个链接，不动环境变量；
- **安装 / 下载目录可整体搬家**：设置里的 `installDir` / `downloadDir` 可指到任意位置
  （如把 JDK 放到 SSD、下载缓存放到机械盘），留空则用默认位置。

## 配置说明

`setting.json`（camelCase 字段）随数据目录自动生成，GUI 设置页与 `jvm config` 共同维护：

| 字段 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `language` | string | `zh-CN` | 界面语言：`zh-CN` / `en-US` |
| `theme` | string | `system` | 主题：`light` / `dark` / `system` |
| `installDir` | string | 空 | JDK 安装目录；空 = `<数据根>/versions` |
| `downloadDir` | string | 空 | 「仅下载」压缩包目录；空 = `<数据根>/downloads` |
| `registry` | string | 空 | 当前数据源 URL（foojay 兼容）；空 = Foojay 官方源 |
| `registryPresets` | array | 2 个内置预设 | 数据源预设（内置可编辑、不可删除） |
| `proxy` | string | 空 | HTTP 代理，如 `http://127.0.0.1:7890`；空 = 直连 |
| `sourceApiKey` | string | 空 | foojay 部署的 API Key（可选） |
| `autoCleanDownload` | bool | `false` | 安装成功后自动删除下载目录里的压缩包 |
| `unbindDeletesJvmrc` | bool | `false` | 解除项目关联时同时删除项目目录里的 `.jvmrc` |

所有 JSON 写入均为原子操作（先写 `.tmp` 再 rename）；文件损坏时回退默认值并告警，
不会让工具不可用。

其他数据文件：

- `index.json` — 已安装版本索引（id、vendor、version、feature、LTS、目录、
  安装验证时的 `java -version` 输出、安装时间）；
- `projects.json` — 项目绑定列表（路径、名称、绑定的 jdk、添加 / 最近使用时间）；
- `.jvmrc` — 项目根的单行 `vendor@version`。

## 数据源管理

内置两个数据源预设：

| 预设 | API | 说明 |
|---|---|---|
| Foojay Disco (Official) | `https://api.foojay.io/disco/v3.0` | 默认，元信息最全（发行商 / 主版本 / 包详情） |
| Adoptium (Temurin) | `https://api.adoptium.net/v3` | Temurin 官方 API，免 Key |

可通过 `jvm config registry` 或 GUI 设置页添加任意 **foojay Disco 兼容**的镜像地址
（内网部署 / 加速镜像），规则：

- 注册 / 切换 / 改地址前会做一次真实元信息请求校验可达性，**校验不通过不落盘**；
- 自定义预设可删除；内置预设可编辑名称 / 地址但不可删除；
- 删除的数据源正好是当前源时，自动回退官方默认源。

## 架构与工作原理

### 总体结构

单一 Cargo workspace，三个 crate；**GUI 与 CLI 都是薄壳，全部业务逻辑收敛在 `jvm-core`**
（纯 Rust、零 Tauri 依赖、可单测）：

```text
┌──────────────── jvm-tauri（GUI 壳）────────────────┐
│ Vue 3 + TS + Vite + Pinia + vue-i18n               │
│  页面: 所有版本 / 已安装 / 项目关联 / 设置 / 日志     │
└────────△───────────────────────△───────────────────┘
         │ invoke / event（下载进度）│
┌────────┴─────────┐      ┌─────────┴────────┐
│  jvm-tauri cmds  │      │  jvm-cli (clap)  │
└────────┬─────────┘      └─────────┬────────┘
         └────────────┬─────────────┘
              ┌───────┴────────────────────────────────┐
              │               jvm-core                 │
              │ source │ downloader │ installer        │
              │ switcher │ project │ shim │ store      │
              │ scan │ health │ flow │ env │ logging   │
              └────────────────────────────────────────┘
```

### jvm-core 模块职责

全异步（tokio）；所有路径经 `Home` 注入（支持 `JVM_HOME` 覆盖），单测使用临时目录，
不读写真实用户目录。

| 模块 | 职责 |
|---|---|
| `source` | trait `RegistrySource` 一种 API 规范一个实现：`FoojaySource`（默认）、`AdoptiumSource`、自定义镜像；统一输出 `Release`（vendor / version / feature / LTS / 本机平台包） |
| `downloader` | reqwest + tokio：校验和解析、Range 探测、分块并发、断点续传、流式 SHA-256 |
| `installer` | zip（Win）/ tar.gz（Unix）解压 → 探测归档内一层根目录 → 移入 `versions/` → 运行 `bin/java -version` 验证 → 写入 `index.json` |
| `switcher` | 写 `default` → 重建 `current` 链接 → 重建 shim → Windows 写 HKCU 环境变量并广播；先备份后写入，失败回滚 |
| `project` | `.jvmrc` 读写与 `projects.json` 管理；`resolve(cwd)` 逐级向上解析 |
| `shim` | busybox 式单二进制按 argv[0] 分发，零 tokio 零日志（每次 `java` 调用都走这里，必须快） |
| `store` | `index.json` / `projects.json` / `setting.json` / `default` 的原子读写与损坏兜底 |
| `scan` | 扫描本机外部 JDK 目录（导入管理） |
| `health` | 数据目录悬空引用检测与一键修复 |
| `flow` | 跨模块业务编排（安装 / 切换 / 绑定 / 卸载的完整流程），CLI 与 GUI 共用 |
| `env` | 平台环境变量读写抽象（Windows 注册表 / Unix 进程环境） |
| `logging` | `tracing` 滚动日志写入 `logs/` |

### 下载与校验机制

一次完整下载按以下顺序进行：

1. **解析校验和** — 优先级：API 返回的 sha256 字段 → 下载地址同名的 `.sha256.txt`；
   两者都不可得时跳过校验并告警（不阻断安装，兼容官网已下架校验和的旧版本源）；
2. **探测** — `HEAD` / `Range-0-0` 请求获取总长度与 Range 支持（带响应头超时与重试）；
3. **分块下载** — 服务器支持 Range 且文件 ≥ 16 MiB 时按块并发（最小块 4 MiB，并发 1–16），
   每块独立 Range 请求，进度持久化到 `<文件>.part.json` 边车文件，中断后重试按块续传；
4. **单流降级** — 总长未知（源不支持 Range）时退化为单流下载（不支持续传）；
5. **完整性校验** — 下载完成后流式计算整文件 SHA-256 并比对，通过后才把 `.part`
   原子改名为最终文件；
6. **断点回收** — 取消 / 失败残留的 `.part` 按 TTL 保留供续传，超期由 CLI / GUI
   启动时自动清理。

### shim 机制

`~/.jvm/bin` 下的 `java`、`javac` 等**都是同一个二进制的副本**（busybox 式自分发）：

- 可执行文件按**文件名（argv[0]）**决定角色——名为 `jvm` 时是 CLI，
  名为 `java` / `javac` / … 时进入 shim 模式，不解析参数、不初始化日志，直接转发；
- shim 按**当前目录**执行与 CLI 完全相同的版本解析，定位到真实 JDK 后转发参数；
  Unix 下 `exec` 替换当前进程（信号与退出码语义与直接运行一致），Windows 下等待子进程；
- shim 会为子进程额外注入 `JAVA_HOME` / `JVM_HOME`——maven、gradle 等构建工具
  直接读环境变量，**项目级绑定不依赖 shell 里的全局变量也能生效**；
- 覆盖 7 个高频命令：`java`、`javac`、`jar`、`jshell`、`jlink`、`jpackage`、`javaw`；
  其余 JDK 工具（`mvn` 自读的 `JAVA_HOME` 等）靠环境变量覆盖。

### Windows 环境变量管理

- `current` 使用**目录 junction**而非 symlink，创建链接**不需要管理员权限**；
- 切换默认版本时写入 `HKCU\Environment`：`JAVA_HOME` → `<数据根>/current`，
  `JVM_HOME` → `<数据根>`，用户 `PATH` 幂等追加 `<数据根>/bin`（只加一次）；
- 写入前先备份，失败自动回滚；写入成功后广播 `WM_SETTINGCHANGE` 让新终端生效；
- 卸载 / 切换前预检目标目录可重命名性，被进程占用时给出「关闭相关进程」的明确提示；
- 「取消默认版本」会完整还原 `default` / `current` / shim / 环境变量。

## 开发指南

### 目录结构

```text
jvm-desktop/
├── crates/
│   ├── jvm-core/     # 核心库：版本源、下载器、安装器、切换器、shim、store（纯 Rust，零 GUI 依赖）
│   ├── jvm-cli/      # CLI 薄壳（clap）：jvm 命令，同时是 shim 的实现载体
│   └── jvm-tauri/    # Tauri 2.x 薄壳：GUI 命令层 + shim 入口
├── src/              # Vue 3 + TS + Vite 前端（GUI）
│   ├── api/          # invoke 封装
│   ├── components/   # 通用组件
│   ├── i18n/         # 中英文案
│   ├── stores/       # Pinia 状态
│   └── views/        # 五个页面
├── docs/             # 设计文档
├── index.html
├── package.json
├── Cargo.toml        # workspace
└── vite.config.ts
```

### 常用命令

```bash
# 构建
cargo build --release -p jvm-cli          # CLI
npm run dev                               # 前端开发服务器（GUI dev 由 tauri 拉起）
npm run tauri dev                         # GUI 开发模式
npm run tauri build                       # GUI 打包（自动先构建 CLI 并拷贝为 shim 载体）

# 测试与检查
cargo test --workspace                    # 全部单元测试（含临时目录隔离的核心逻辑）
cargo clippy --workspace -- -D warnings   # lint
```

### 代码约定

- **薄壳原则**：GUI / CLI 只做参数与 DTO 转换、缓存与事件推送，业务编排一律放
  `jvm_core::flow`，保证两条入口行为一致；
- **路径注入**：核心库不直接读 `$HOME`，一切经 `Home`（`JVM_HOME` 可覆盖），
  测试用 `tempfile` 临时目录；
- **阻塞 IO 不进 async worker**：解压、删除、`java -version` 验证等重 IO 在
  Tauri 侧走 `spawn_blocking`；
- **配置原子写**：先 `.tmp` 再 rename，损坏回退默认值；
- **Windows 兼容**：文件占用预检、junction 链接、注册表备份回滚，见上文专节。

## 常见问题

**Q：切换版本后当前终端不生效？**
环境变量（`JAVA_HOME` / `PATH`）的变更只影响**新启动**的终端。日常切换 `current`
链接是即时的，已开终端重开一个窗口即可；首次配置或修复后 `jvm use` 会提示重开终端。

**Q：`java -version` 报错「未找到生效的 JDK」？**
当前目录没有绑定且未设置全局默认。先 `jvm use temurin@17` 设置默认，
或在项目根执行 `jvm use temurin@17 --project` 生成 `.jvmrc`。

**Q：卸载提示文件被占用？**
关掉正在使用该 JDK 的终端 / IDE / 构建进程后重试。工具在删除前会做占用预检，
避免半途失败留下损坏的安装。

**Q：`doctor` 报告「PATH 上第一个 java 不是 jvm shim」？**
系统里存在其他 JDK 且其 `bin` 目录排在 `<数据根>/bin` 之前。调整 PATH 顺序，
或卸载其他 JDK。

**Q：数据目录可以搬家 / 换盘吗？**
可以。整个数据根可随安装目录移动，也可用 `JVM_HOME` 指定新位置；
搬移后 GUI 设置页会提示悬空引用并支持一键修复。安装 / 下载子目录还可在设置里单独改到其他盘符。

**Q：内网 / 代理环境能用吗？**
可以。设置里填 HTTP 代理；或 `jvm config registry` 注册内网 foojay 兼容镜像。

**Q：`.jvmrc` 要提交到 Git 吗？**
推荐提交——它就是给团队共享项目 JDK 版本用的，格式为单行 `vendor@version`。

## 参与贡献

欢迎 Issue 与 Pull Request：

1. Fork 本仓库并创建特性分支（`feat/xxx` / `fix/xxx`）；
2. 提交前确保 `cargo test --workspace` 与 `cargo clippy --workspace -- -D warnings` 全部通过；
3. 核心逻辑改动请附带单元测试（临时目录隔离，不依赖真实用户目录）；
4. 提交信息使用语义化格式（`feat:` / `fix:` / `docs:` / `refactor:` …）。

## 致谢

本项目**参考开源项目 [nvm-desktop](https://github.com/1111mp/nvm-desktop) 的同源设计开发**。
nvm-desktop 以 Tauri 构建、用可视化界面管理多个 Node.js 版本，其数据目录布局、
shim 转发、项目级版本绑定（`.nvmdrc`）与 GUI + CLI 双模式的设计，为本项目提供了
直接的设计蓝本；本项目将其理念移植到 Java/JDK 场景并完整重新实现。
在此向 nvm-desktop 的作者与贡献者致谢。

同时感谢：

- [foojay Disco API](https://api.foojay.io/) 与 [Adoptium API](https://api.adoptium.net/v3) — 提供 OpenJDK 各发行版的版本元数据；
- [Tauri](https://tauri.app/)、[Vue](https://vuejs.org/) 等开源项目 — 构成了本项目的技术底座。

## 相关文档

- [架构设计说明](docs/架构设计说明.md) — 模块职责、关键决策、JSON Schema 草案、里程碑
- [使用教程视频](docs/使用教程.mp4)

## License

[MIT](LICENSE) © JVM Desktop contributors
