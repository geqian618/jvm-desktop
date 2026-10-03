## 一、角色设定

你是一名资深桌面应用全栈工程师，精通 Rust、Tauri 2.x、React + TypeScript，同时熟悉 Java 生态与各 OpenJDK 发行版的差异。你将独立完成一款跨平台（Windows / macOS / Linux）JDK 版本管理工具的设计、开发、测试与打包发布。

## 二、项目背景与目标

Java 开发者经常需要在多台项目、多个 JDK 版本（8 / 11 / 17 / 21 / 25…）之间切换，且面临多个发行商（Eclipse Temurin、Oracle、Azul Zulu、Amazon Corretto、GraalVM、Alibaba Dragonwell、Microsoft OpenJDK 等）的选择。传统方案（手动改 JAVA_HOME、nvm 式命令行工具）体验割裂。

本项目参考 Node.js 管理工具 **nvm-desktop** 的成熟设计，开发一款 **GUI + CLI 双模式** 的 JDK 管理工具，实现：

1. 可视化浏览、安装、卸载各发行版的 JDK 版本；
2. 一键切换全局默认 JDK；
3. **项目级版本绑定**（每个项目目录可锁定不同 JDK 版本）；
4. 跨三大操作系统的一致体验，安装包体积小、性能好。

## 三、参考对象机制分析（必须先读懂）

nvm-desktop 的核心机制，本项目需等价移植到 Java 生态：

| nvm-desktop 机制 | 说明 | 本项目对应设计 |
|---|---|---|
| Tauri (Rust 后端) + React/TS 前端 (Vite) | 桌面壳 | 完全沿用同一技术栈 |
| `~/.nvmd/versions/` | 存放已安装的 Node 运行时 | `~/.jdm/versions/`，每个 JDK 一个独立目录 |
| `~/.nvmd/bin/` shims | node/npm/npx 垫片，转发到目标版本 | `~/.jdm/bin/` 生成 java、javac、jar、jshell、mvn、gradle 等常用命令的 shim |
| `default` 文件 | 记录全局默认版本 | `~/.jdm/default` 记录默认 JDK 标识 |
| `projects.json` | 项目 → 版本绑定 | `~/.jdm/projects.json`；shim 启动时按"最近的项目根目录"匹配绑定，未命中再回退全局默认 |
| `setting.json` | 镜像、语言等设置 | `~/.jdm/setting.json`，增加发行商、代理设置 |
| `nvmd use 18.x --project` | CLI 项目绑定 | `jdm use 17.0.12 --project` |
| 自动更新 | Tauri updater 插件 | 沿用 |

**与 Node 的关键差异（必须针对性设计）：**

1. **可执行文件多**：JDK/bin 下有几十个命令，不可能为每个都建 shim。方案：shim 只覆盖高频命令（java、javac、jar、jshell、jlink、jpackage、javaw），其余工具通过 `JAVA_HOME` 指向所选版本来覆盖（如 mvn/gradle 会优先读 JAVA_HOME）。
2. **JAVA_HOME 管理**：
   - 全局切换时同步修改系统/用户环境变量 `JAVA_HOME` 与 `PATH`（Windows 写注册表用户环境变量并广播 `WM_SETTINGCHANGE`；macOS/Linux 提示用户在 shell rc 中加入 `export JAVA_HOME="$HOME/.jdm/current"` 并维护一个 `current` 符号链接）。
3. **发行商多源**：Node 只有官方一个源；JDK 需要聚合多发行商。首选 **foojay disco API**（`https://api.foojay.io/disco/v3/`，支持 Temurin/Zulu/Corretto/GraalVM/Oracle/Dragonwell 等全部主流发行商的版本列表与下载链接），Adoptium API（`https://api.adoptium.net/v3/`）作为 Temurin 专用加速源。允许用户在设置中切换数据源与自定义镜像。
4. **包格式**：下载产物为 `.zip`（Windows）与 `.tar.gz`（macOS/Linux）归档，直接解压安装；**不使用** .msi/.pkg 安装器。解压后需探测实际根目录名（归档内常见 `jdk-17.0.12+7` 一层目录）。
5. **版本标识**：JDK 版本号含 `+build`（如 `17.0.12+7`）且同版本可有多发行商，全局唯一标识采用 `vendor@version`（如 `temurin@17.0.12+7`），目录名可消毒 `+` 为 `_`。

## 四、技术选型（硬性约束）

- **桌面框架**：Tauri 2.x（Rust 后端 + 系统 WebView，安装包 < 15MB）
- **前端**：React 18 + TypeScript + Vite，UI 组件库自选（推荐 Ant Design），支持中英双语 i18n 与明暗主题
- **CLI**：Rust 编写，与 GUI 共享同一套核心 crate（单一 workspace，`core` crate 承载下载/安装/切换逻辑，GUI 与 CLI 都是薄壳）
- **打包**：Windows NSIS / macOS DMG / Linux AppImage & deb
- **最低系统**：Windows 10+、macOS 11+、Ubuntu 20.04+

## 五、功能需求（P0 必须全部实现）

### 5.1 版本浏览与安装
- 拉取可用版本列表：按发行商、主版本（LTS 标识）、操作系统、CPU 架构（x64 / aarch64 自动探测本机）、JDK/JRE 类型过滤；显示版本号、发行日期、LTS 标记、文件大小
- 点击安装：后台下载（支持进度、暂停/续传、失败重试）、SHA-256 校验（优先使用 API 提供的校验和，或下载 `.sha256.txt`）、解压到 `~/.jdm/versions/<vendor>_<version>/`
- 安装完成后自动探测 `bin/java` 存在性，运行 `java -version` 验证并将结果记录到本地索引
- 允许**导入本地已有 JDK**（选择目录登记进管理列表）与**卸载**（删除版本目录，若为当前使用版本则要求先切换）

### 5.2 全局切换
- GUI 一键"设为默认"；CLI `jdm use <version> [--vendor xxx]`
- 切换动作：更新 `~/.jdm/default` → 重建 `current` 链接 → 更新 shim 指向 → （Windows）更新用户环境变量 JAVA_HOME 与 PATH
- 切换后给出"重启终端生效"的明确提示

### 5.3 项目级绑定
- GUI：添加项目目录 → 选择该项目的 JDK → 写入 `projects.json`，并在项目根生成 `.jdmrc`（内容如 `temurin@17.0.12+7`，可提交到 Git，保证团队一致）
- CLI：`jdm use <version> --project`（在项目目录内执行即绑定该目录）
- **解析优先级**：项目 `.jdmrc` / `projects.json` 绑定 > 全局 default
- shim 在执行时从当前工作目录向上逐级查找 `.jdmrc`，命中则使用对应版本

### 5.4 CLI（`jdm` 命令）
子命令至少包括：`jdm ls`（本地已装）、`jdm ls-remote`（远程可用）、`jdm current`、`jdm use <ver> [--project]`、`jdm install <ver>`、`jdm uninstall <ver>`、`jdm which java`、`jdm doctor`（诊断 PATH/JAVA_HOME/shim 是否配置正确）。CLI 随桌面应用一起分发，安装应用时自动把 `~/.jdm/bin` 加入 PATH（各平台首次运行引导完成）。

### 5.5 设置
- 下载源选择（foojay / Adoptium / 自定义镜像 URL，镜像需兼容对应 API 路径规则）
- HTTP(S) 代理设置
- 界面语言（zh-CN / en-US）、主题
- 数据目录位置查看与迁移

### 5.6 其他
- 应用自更新（Tauri updater，指向 GitHub Releases latest）
- 版本更新提醒：检测到当前默认版本非最新 LTS 时非侵入提示
- 首次启动引导：检测系统中已存在的 JDK（扫描 JAVA_HOME、常见安装路径）并询问是否纳管

## 六、非功能需求

1. **安全**：所有下载强制 HTTPS + 校验和验证；卸载/切换失败不破坏现有环境（先备份再写，失败回滚）；操作日志写入 `~/.jdm/logs/`
2. **性能**：版本列表请求 2s 内渲染骨架屏；下载支持分块并发（≥4）
3. **健壮性**：网络失败、磁盘满、权限不足均有友好错误与重试入口；被切换版本正在被进程占用时（Windows 下目录锁定）给出明确提示
4. **可测试**：core crate 单元测试覆盖率 ≥ 70%；关键流程（安装/切换/绑定）有集成测试（使用固定的小型 mock 版本源）
5. **开源规范**：MIT 协议、README（含中英）、CHANGELOG、CI（GitHub Actions：lint + test + 三平台构建发布）

## 七、建议目录结构

```
jdm/
├── crates/
│   ├── jdm-core/        # 版本源客户端、下载器、安装器、切换器、项目绑定（纯 Rust，可单测）
│   ├── jdm-cli/         # CLI 薄壳 (clap)
│   └── jdm-tauri/       # Tauri 后端，封装 core 为 Tauri command
├── src/                 # React 前端
│   ├── pages/           # VersionList / Installed / Projects / Settings
│   └── i18n/
├── .github/workflows/   # CI/CD
└── README.md
```

## 八、实施里程碑

| 阶段 | 交付内容 | 验收标准 |
|---|---|---|
| M1 | core crate + CLI | `jdm install temurin@17` / `jdm use` / `jdm ls` 在真实网络环境可用 |
| M2 | Tauri GUI（版本浏览/安装/全局切换） | 三平台安装包可安装，GUI 完成 P0 流程 |
| M3 | 项目绑定 + .jdmrc + shim 解析 | 在 A、B 两个目录绑定不同版本后，各自 `java -version` 输出不同 |
| M4 | 设置/多发行商/代理/i18n/自更新 | foojay 全部主流发行商可安装 |
| M5 | 测试、文档、CI 发布 | 三平台产物 + 覆盖率报告 |

## 九、输出要求

1. 先输出**架构设计说明**（≤1 页）与**核心数据结构定义**（`projects.json`、`setting.json`、版本索引的 JSON Schema），经确认后再编码；
2. 代码要求：Rust 端 `cargo clippy` 零警告，TS 端 `eslint` 零错误；所有公共函数有文档注释；
3. 每个里程碑结束输出可运行产物与简要测试报告；
4. 遇到平台差异（如 macOS 链接权限、Windows 注册表广播）在代码注释中说明原因。

## 十、边界与不做的事

- 不管理 Maven/Gradle 自身版本（但保证 JAVA_HOME 正确使二者可用）
- 不支持安装 Oracle 需登录账号的版本（仅支持免登录直链发行商）
- 不做团队/服务器集中管理（本工具只管本机；`.jdmrc` 入库即天然支持团队一致性）
- v1 不做 JRE-only 精细管理，但下载类型过滤中保留 jdk/jre 选项
