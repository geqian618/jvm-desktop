# JVM Desktop（jvm）

nvm-desktop 同源设计的 **JDK 版本管理工具**：可视化浏览/安装/切换各 OpenJDK
发行版（Temurin、Zulu、Corretto、GraalVM、Dragonwell、Microsoft OpenJDK…），
支持**项目级版本绑定**（`.jvmrc`），GUI + CLI 双模式，跨 Windows / macOS / Linux。

> 项目当前进度与里程碑见 [docs/架构设计说明.md](docs/架构设计说明.md)。

## 仓库结构

```
crates/
├── jvm-core/   # 核心库：版本源(foojay/Adoptium)、下载器(分块/续传/校验)、安装器、切换器、shim
├── jvm-cli/    # CLI 薄壳（jvm 命令，同时是 java/javac 等 shim 的实现载体）
└── jvm-tauri/  # Tauri 2.x 薄壳（M2）
src/            # Vue 3 + Vite 前端（M2）
```

## CLI（M1）

```bash
cargo run -p jvm-cli -- ls-remote            # 各 LTS 最新 GA（temurin）
cargo run -p jvm-cli -- ls-remote -v zulu    # 指定发行商
cargo run -p jvm-cli -- ls-remote --all 17   # 某主版本全部更新

cargo run -p jvm-cli -- install temurin@17          # 安装最新 17
cargo run -p jvm-cli -- install temurin@17.0.12+7   # 精确版本
cargo run -p jvm-cli -- ls                          # 本地已装
cargo run -p jvm-cli -- use temurin@17              # 设为默认（写 JAVA_HOME/PATH/shim）
cargo run -p jvm-cli -- use 21 --project            # 当前目录绑定（.jvmrc）
cargo run -p jvm-cli -- current                     # 当前目录解析结果
cargo run -p jvm-cli -- which java                  # 解析到的实际 java
cargo run -p jvm-cli -- doctor                      # 环境自检
cargo run -p jvm-cli -- import "C:\\path\\to\\jdk" --as corretto@11.0.24+8
cargo run -p jvm-cli -- uninstall temurin@17
```

## 数据目录

`~/.jvm/`（或 `JVM_HOME` 指定）：`versions/` 安装目录、`bin/` 命令 shim、
`current` 指向默认版本的链接（Windows 为 junction）、`default` / `index.json` /
`projects.json` / `setting.json`、`logs/` 滚动日志。

## 版本解析优先级

当前目录向上找 `.jvmrc` → `projects.json` 最近项目根 → 全局 `default`。

## 测试

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## License

MIT
