<div align="center">

<img src="src/assets/icon.png" alt="JVM Desktop" width="120" />

# JVM Desktop

**A JDK version manager with GUI + CLI dual modes**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Release](https://img.shields.io/badge/release-v0.1.0-brightgreen.svg)](../../releases)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg)](#installation)
[![Rust](https://img.shields.io/badge/Rust-2021%20edition-orange.svg)](#installation)
[![Tauri](https://img.shields.io/badge/Tauri-2.x-24C8DB.svg)](#installation)
[![Vue](https://img.shields.io/badge/Vue-3-42b883.svg)](#installation)

[简体中文](README.md) | **English**

</div>

> Modeled on the open-source project [nvm-desktop](https://github.com/1111mp/nvm-desktop) with a same-lineage
> design: a JDK version manager for visually browsing / installing / switching OpenJDK distributions,
> with project-level version binding, GUI + CLI dual modes, across Windows / macOS / Linux.

## Table of Contents

- [Introduction](#introduction)
- [Features](#features)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [GUI Guide](#gui-guide)
- [CLI Command Reference](#cli-command-reference)
- [Data Directory](#data-directory)
- [Version Resolution Rules](#version-resolution-rules)
- [Configuration](#configuration)
- [Data Source Management](#data-source-management)
- [How It Works](#how-it-works)
- [Development Guide](#development-guide)
- [FAQ](#faq)
- [Contributing](#contributing)
- [License](#license)
- [Acknowledgments](#acknowledgments)

## Introduction

Everyday Java development often means switching between multiple JDK versions and vendors: Corretto 11
for company projects, Temurin 17 for open-source work, trying out GraalVM native images... Manually
editing `JAVA_HOME`, maintaining `PATH`, and remembering which version each project needs is tedious
and error-prone.

**JVM Desktop** (`jvm` on the command line) fully implements the nvm-desktop philosophy in Rust + Tauri
(the project binding file `.nvmdrc` becomes `.jvmrc` in the Java world), redesigned for the
multi-vendor, multi-platform JDK landscape:

- **A single data directory** manages all JDKs; the `current` link points to the default version —
  switching only rewrites the link;
- **`vendor@version` unique key** identifies each version; install, switch, and uninstall all revolve
  around it;
- **Project-level binding**: drop a `.jvmrc` (single line `vendor@version`) at the project root and it
  takes effect as soon as you enter the directory; commit it to Git to share with your team;
- **Shim mechanism**: `java` / `javac` and friends are forwarded through shims under
  `<data root>/bin`; shims resolve the version for the current directory and inject `JAVA_HOME` into
  child processes — maven / gradle need zero extra configuration.

## Features

- [x] **Multi-distribution support** — Temurin, Zulu, Corretto, GraalVM (Community), Dragonwell,
      Microsoft OpenJDK, Bisheng, and more; metadata from the [foojay Disco API](https://api.foojay.io/),
      switchable to the official Adoptium API or any foojay-compatible mirror
- [x] **GUI + CLI dual modes** — the graphical interface (Tauri 2 + Vue 3) handles browsing and
      installing, the command line handles scripting and day-to-day switching; both share the same
      data and the same resolution rules
- [x] **Project-level version binding** — two-level binding via `.jvmrc` + `projects.json`, resolved
      by walking up the directory tree
- [x] **Reliable downloads** — concurrent chunking, resumable transfers (`.part` + `.part.json`),
      streaming SHA-256 verification, with traceable checksum sources (API field → `<url>.sha256.txt`)
- [x] **Zero-privilege switching (Windows)** — directory junctions instead of symlinks, no
      administrator rights required; registry writes to `HKCU\Environment`, backed up before writing
      with automatic rollback on failure
- [x] **Centralized install directory management** — the local list only loads JDKs under the install
      directory (`installDir` in settings, default `<data root>/versions`); no scanning of other
      locations on the machine
- [x] **Environment self-check and self-healing** — `jvm doctor` diagnoses PATH / JAVA_HOME / shim /
      link / shadowing issues in one shot; the GUI detects dangling references after the data
      directory is moved or re-rooted, with one-click repair
- [x] **Proxy and mirrors** — HTTP proxy, custom data sources (foojay-compatible URL + optional API key)
- [x] **Chinese / English UI** — light / dark themes follow the system
- [x] **Cross-platform** — one core codebase for Windows / macOS / Linux

## Installation

### Environment Requirements

| Scenario | Requirement |
|---|---|
| Running GUI / CLI | Windows 10+ / macOS 11+ / Linux (WebKitGTK) |
| Building from source | Rust 1.77+ (required by Tauri 2), Node.js 20 LTS+, platform system dependencies (Tauri prerequisites) |

### Option 1: Download a Prebuilt Binary

Head to the [Releases](../../releases) page and download the installer for your platform:

- **Windows**: `.exe` NSIS installer (the installer configures PATH and environment variables automatically)
- **macOS**: `.dmg`
- **Linux**: `.deb` / `.AppImage`

### Option 2: Build from Source

```bash
git clone https://github.com/jvmdesktop/jvm-desktop.git
cd jvm-desktop

# Build the CLI only (single-file executable, no runtime dependencies)
cargo build --release -p jvm-cli
# Output: target/release/jvm

# Build the GUI (automatically builds the CLI first and copies it as the shim carrier)
npm install
npm run tauri build
```

## Quick Start

Place the CLI binary in any directory; the data directory is initialized automatically the first time
you run any command:

```bash
./jvm install temurin@17        # Install the latest Temurin 17
./jvm use temurin@17            # Set as the default version
java -version                   # Verify

cd my-project
./jvm use corretto@21 --project # Bind the current project to Corretto 21 (generates .jvmrc)
```

Platform differences on the first `jvm use`:

- **Windows**: automatically writes the user environment variables `JVM_HOME` / `JAVA_HOME` / `PATH`
  and broadcasts; reopen the terminal once after the first configuration — subsequent switches take
  effect immediately;
- **macOS / Linux**: the tool does not modify shell configs; add the following three lines to
  `~/.bashrc` or `~/.zshrc` as prompted and reopen the terminal (use the path printed by `jvm home`):

  ```bash
  export JVM_HOME="$HOME/.jvm"           # Data root
  export JAVA_HOME="$JVM_HOME/current"
  export PATH="$JVM_HOME/bin:$PATH"
  ```

## GUI Guide

The GUI is a desktop window with a custom title bar and five pages on the left:

| Page | Function |
|---|---|
| **All Versions** | Browse all GA packages filtered by vendor / major version / support type (LTS/MTS/STS); one-click install or download-only; live progress (download / extract / done) with cancel; already-downloaded packages are reused automatically |
| **Installed** | Local JDK list (only JDKs under the install directory `installDir` — no scanning of other locations on the machine); enable / set as default, uninstall, with occupancy detection |
| **Projects** | Project → version binding management: add directories, bind / unbind, delete `.jvmrc` (per setting) |
| **Settings** | Data source preset management, proxy, install / download directories, language, theme, auto-cleanup of archives, etc.; a health-check banner appears at the top when the data directory is unhealthy, with one-click repair |
| **Logs** | Runtime logs (same source as the `logs/` file logs) |

The UI supports Simplified Chinese / English and light / dark themes (following the system).

## CLI Command Reference

### `jvm --help`

```text
$ jvm --help
A JDK version manager (same-lineage design with nvm-desktop, GUI + CLI dual modes)

Usage: jvm [COMMAND]

Commands:
  list         List locally installed versions
  list-remote  List remotely available versions (latest GA of each LTS major by default)
  current      Show the JDK resolved for the current directory
  use          Switch the default version; with --project, bind the current directory (writes .jvmrc)
  install      Install a version, e.g. temurin@17 or temurin@17.0.12+7
  uninstall    Uninstall a version
  which        Print the actual path a command resolves to
  doctor       Diagnose PATH / JAVA_HOME / shim configuration
  home         Print the jvm data directory
  config       View and switch data sources (foojay Disco compatible API)
  help         Print help information or help for a subcommand

Options:
  -h, --help     Print help information
  -V, --version  Print the version number
```

### Command Overview

| Command | Description |
|---|---|
| `jvm list` | List locally installed versions (`->` marks the default; the version in effect for the current directory and its source are shown at the end) |
| `jvm list-remote` | List remotely available versions; shows the latest GA of each LTS major version by default |
| `jvm current` | Show the JDK resolved for the current directory, its source, and the actual `java` path |
| `jvm use <version>` | Switch the default version (rebuilds the `current` link, shims, and environment variables) |
| `jvm use <version> --project` | Bind the current directory to that version (generates `.jvmrc`) instead of switching the global default |
| `jvm install <version>` | Install a version (download → verify → extract → `java -version` validation → index registration) |
| `jvm install <version> --default` | Install and set as the default version directly |
| `jvm uninstall <version>` | Uninstall a version (if the version is in use, a clear message is shown instead of failing halfway) |
| `jvm which <tool>` | Print the actual path a command (`java` / `javac` / `jar` …) resolves to |
| `jvm doctor` | Environment self-check: data directory, index, `current` link, shims, PATH, JAVA_HOME, command shadowing |
| `jvm home` | Print the data directory paths (install directory / shim directory / logs / index…) |
| `jvm config` | Data source management (view / register / switch / edit / delete) |
| `jvm help [subcommand]` | Print help information |

Global options: `-h, --help`, `-V, --version`.

### Version Identifier Syntax

All commands accept the unified version identifier `<vendor>@<version>`, where version supports three
granularities:

| Form | Example | Meaning |
|---|---|---|
| Major version | `17` | The latest version of that major |
| Version prefix | `17.0.12` | Matches `17.0.12+7` etc. |
| Full version | `17.0.12+7` | Exact match |

`vendor` can be omitted; when omitted it defaults to `temurin` (e.g. `jvm install 21`).
Common vendors: `temurin`, `zulu`, `corretto`, `graalvm_community`, `dragonwell`, `microsoft`, `bisheng`.

### Common Workflows

```bash
# Browse
jvm list-remote                # Latest GA of each LTS (temurin by default)
jvm list-remote -v zulu        # Specify a vendor
jvm list-remote --all 17       # All GA versions of 17
jvm list-remote 21             # Latest GA of 21
jvm list-remote --jre          # List JREs instead of JDKs

# Install / uninstall
jvm install temurin@17               # Install the latest 17
jvm install temurin@17.0.12+7        # Exact version
jvm install 21 --default             # Install and set as default
jvm uninstall temurin@17

# Switch / query
jvm list                      # Locally installed (-> marks the default)
jvm use temurin@17            # Set as default (writes JAVA_HOME/PATH/shim)
jvm use 21 --project          # Bind the current directory (.jvmrc)
jvm current                   # Resolution result for the current directory
jvm which java                # The actual java resolved

# Self-check
jvm doctor                    # Environment self-check
jvm home                      # Data directory paths
```

### Data Source Management (CLI)

```bash
jvm config                                   # View the current data source and presets
jvm config registry --url https://jdk.internal.example/disco/v3.0 \
                     --name internal-mirror  # Register a preset (listed, not enabled by default)
jvm config registry --url <URL> --name internal-mirror --default   # Register and enable
jvm config use internal-mirror               # Switch data source (a direct URL also works)
jvm config edit internal-mirror --url <new URL>   # Edit a preset
jvm config remove internal-mirror            # Delete a custom preset (built-ins cannot be deleted)
```

`registry` / `use` / `edit` perform a real metadata request to verify reachability before saving —
if the endpoint is unreachable, nothing is written to disk and subsequent commands are unaffected;
`--force` skips the check.

## Data Directory

All data is concentrated in a **single data root directory** (run `jvm home` to see the actual path),
resolved in this order:

1. Environment variable `JVM_HOME` (multi-instance / testing scenarios);
2. `workspace/` under the install directory (default; if the current executable is
   `<data root>/bin/jvm`, the data root is its parent directory, avoiding recursive nesting);
3. Fall back to `.jvm` in the user's home directory when neither is available.

Directory layout:

```text
<data root>/
├── versions/            # Installed JDKs, one directory per version (temurin@17.0.12+7/)
├── bin/                 # Command shims: jvm, java, javac, jar, jshell, jlink, jpackage, javaw
├── current              # Link to the default version (Windows: junction; Unix: symlink)
├── default              # Single line vendor@version
├── downloads/           # Download cache (including .part resume files)
├── logs/                # Rolling logs (jvm.log.YYYY-MM-DD) and environment variable backups
├── index.json           # Index of locally installed versions
├── projects.json        # Project → version bindings
└── setting.json         # Settings: data source / proxy / language / theme / directories, etc.
```

Key points:

- **Directory names match version identifiers**: directories under `versions/` are named
  `vendor@version` (e.g. `temurin@17.0.12+7`), with only individual filesystem-illegal characters
  replaced — on-disk names correspond one-to-one with list displays;
- **`JAVA_HOME` points to `<data root>/current`**: switching the default version only rebuilds this
  link — environment variables stay untouched;
- **The install / download directories can be moved wholesale**: `installDir` / `downloadDir` in
  settings can point anywhere (e.g. put JDKs on an SSD and the download cache on a mechanical drive);
  leave empty for the defaults.

## Version Resolution Rules

The CLI, GUI, and shims share the **same** resolution logic. Running `java` in any directory resolves
the version by priority:

1. **`.jvmrc`** — search upward from the current directory for the project root's `.jvmrc`
   (single line `vendor@version`, committable to Git for team sharing);
2. **`projects.json`** — the nearest ancestor directory among registered project roots;
3. **Global `default`** — the default version set by `jvm use <version>`.

If a version resolved at any layer has been uninstalled or its directory is corrupted, you get a
clear error instead of silent failure.

## Configuration

`setting.json` (camelCase fields) is generated automatically alongside the data directory, maintained
by both the GUI settings page and `jvm config`:

| Field | Type | Default | Description |
|---|---|---|---|
| `language` | string | `zh-CN` | UI language: `zh-CN` / `en-US` |
| `theme` | string | `system` | Theme: `light` / `dark` / `system` |
| `installDir` | string | empty | JDK install directory; empty = `<data root>/versions` |
| `downloadDir` | string | empty | Directory for "download-only" archives; empty = `<data root>/downloads` |
| `registry` | string | empty | Current data source URL (foojay-compatible); empty = official Foojay source |
| `registryPresets` | array | 2 built-in presets | Data source presets (built-ins are editable but cannot be deleted) |
| `proxy` | string | empty | HTTP proxy, e.g. `http://127.0.0.1:7890`; empty = direct connection |
| `sourceApiKey` | string | empty | API key for a foojay deployment (optional) |
| `autoCleanDownload` | bool | `false` | Automatically delete archives in the download directory after a successful install |
| `unbindDeletesJvmrc` | bool | `false` | Also delete the project's `.jvmrc` when unbinding a project |

All JSON writes are atomic (write `.tmp` first, then rename); on corruption the tool falls back to
defaults and warns, never leaving the tool unusable.

Other data files:

- `index.json` — installed version index (id, vendor, version, feature, LTS, directory,
  `java -version` output captured at install time, install timestamp);
- `projects.json` — project binding list (path, name, bound JDK, added / last-used timestamps);
- `.jvmrc` — single-line `vendor@version` at the project root.

## Data Source Management

Two data source presets are built in:

| Preset | API | Notes |
|---|---|---|
| Foojay Disco (Official) | `https://api.foojay.io/disco/v3.0` | Default; the most complete metadata (vendors / major versions / package details) |
| Adoptium (Temurin) | `https://api.adoptium.net/v3` | Official Temurin API, no key required |

You can add any **foojay Disco compatible** mirror URL via `jvm config registry` or the GUI settings
page (intranet deployment / acceleration mirrors). Rules:

- Register / switch / change address: a real metadata request verifies reachability first —
  **if the check fails, nothing is persisted**;
- Custom presets can be deleted; built-in presets can have their name / address edited but cannot be
  deleted;
- If the deleted data source is the active one, the tool automatically falls back to the official
  default source.

## How It Works

### Overall Structure

A single Cargo workspace with three crates; **both the GUI and CLI are thin shells — all business
logic converges in `jvm-core`** (pure Rust, zero Tauri dependencies, unit-testable):

```text
┌──────────────── jvm-tauri (GUI shell) ───────────────┐
│ Vue 3 + TS + Vite + Pinia + vue-i18n                 │
│  Pages: All Versions / Installed / Projects / Settings / Logs │
└────────△───────────────────────△─────────────────────┘
         │ invoke / event (download progress) │
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

### jvm-core Module Responsibilities

Fully async (tokio); all paths are injected via `Home` (overridable with `JVM_HOME`), and unit tests
use temporary directories without touching the real user directory.

| Module | Responsibility |
|---|---|
| `source` | trait `RegistrySource` — one API spec, one implementation each: `FoojaySource` (default), `AdoptiumSource`, custom mirrors; unified output `Release` (vendor / version / feature / LTS / native platform packages) |
| `downloader` | reqwest + tokio: checksum resolution, Range probing, concurrent chunking, resumable transfers, streaming SHA-256 |
| `installer` | zip (Win) / tar.gz (Unix) extraction → detect the single root directory inside the archive → move into `versions/` → run `bin/java -version` to verify → write `index.json` |
| `switcher` | Write `default` → rebuild the `current` link → rebuild shims → write HKCU environment variables on Windows and broadcast; back up before writing, roll back on failure |
| `project` | `.jvmrc` read/write and `projects.json` management; `resolve(cwd)` walks up the directory tree |
| `shim` | busybox-style single binary dispatching by argv[0], zero tokio, zero logging (every `java` call goes through here, so it must be fast) |
| `store` | Atomic read/write and corruption fallback for `index.json` / `projects.json` / `setting.json` / `default` |
| `scan` | Load the local JDK list under the install directory (`installDir` / `versions/`) (external directory scanning has been removed) |
| `health` | Dangling-reference detection and one-click repair for the data directory |
| `flow` | Cross-module business orchestration (complete install / switch / bind / uninstall flows), shared by the CLI and GUI |
| `env` | Platform environment variable read/write abstraction (Windows registry / Unix process environment) |
| `logging` | `tracing` rolling logs written to `logs/` |

### Shim Mechanism

The `java`, `javac`, and other executables under `<data root>/bin` are **copies of the same binary**
(busybox-style self-dispatching):

- The executable decides its role by **file name (argv[0])** — named `jvm` it is the CLI; named
  `java` / `javac` / … it enters shim mode, parsing no arguments and initializing no logging,
  forwarding directly;
- The shim performs exactly the same version resolution as the CLI, based on the **current
  directory**, then forwards arguments once the real JDK is located; on Unix it `exec`s to replace
  the current process (signal and exit-code semantics match running it directly), on Windows it waits
  for the child process;
- The shim additionally injects `JAVA_HOME` / `JVM_HOME` into the child process — maven, gradle, and
  other build tools read environment variables directly, so **project-level binding works without
  relying on global variables in the shell**;
- It covers 7 high-frequency commands: `java`, `javac`, `jar`, `jshell`, `jlink`, `jpackage`,
  `javaw`; other JDK tools (the `JAVA_HOME` that `mvn` reads, etc.) are covered via environment
  variables.

### Download and Verification

A complete download proceeds as follows:

1. **Resolve the checksum** — priority: the sha256 field returned by the API → the `.sha256.txt`
   alongside the download URL; if neither is available, verification is skipped with a warning
   (installation is not blocked, for compatibility with older sources whose official site no longer
   publishes checksums);
2. **Probe** — `HEAD` / `Range-0-0` requests to get the total length and Range support (with
   response-header timeouts and retries);
3. **Chunked download** — when the server supports Range and the file is ≥ 16 MiB, download in
   concurrent chunks (minimum chunk 4 MiB, concurrency 1–16), each chunk an independent Range
   request, with progress persisted to the `<file>.part.json` sidecar file; after an interruption,
   retries resume chunk by chunk;
4. **Single-stream fallback** — when the total length is unknown (the source does not support Range),
   degrade to a single-stream download (no resume support);
5. **Integrity verification** — after downloading, stream-compute the SHA-256 of the whole file and
   compare; only on success is the `.part` atomically renamed to the final file;
6. **Resume cleanup** — `.part` files left behind by cancellation / failure are kept for a TTL to
   allow resumption; expired ones are cleaned automatically at CLI / GUI startup.

### Windows Environment Variable Management

- The `current` link uses a **directory junction** instead of a symlink, so creating it **requires no
  administrator privileges**;
- When switching the default version, write to `HKCU\Environment`: `JAVA_HOME` → `<data root>/current`,
  `JVM_HOME` → `<data root>`, and idempotently append `<data root>/bin` to the user `PATH` (added only
  once);
- Back up before writing, with automatic rollback on failure; after a successful write, broadcast
  `WM_SETTINGCHANGE` so new terminals pick it up;
- Pre-check that the target directory can be renamed before uninstalling / switching; when a process
  holds it, show a clear "close the relevant processes" message;
- "Cancel default version" fully restores `default` / `current` / shims / environment variables.

## Development Guide

### Directory Structure

```text
jvm-desktop/
├── crates/
│   ├── jvm-core/     # Core library: version sources, downloader, installer, switcher, shim, store (pure Rust, zero GUI dependencies)
│   ├── jvm-cli/      # CLI thin shell (clap): the jvm command, also the implementation carrier for the shim
│   └── jvm-tauri/    # Tauri 2.x thin shell: GUI command layer + shim entry point
├── src/              # Vue 3 + TS + Vite frontend (GUI)
│   ├── api/          # invoke wrappers
│   ├── components/   # Shared components
│   ├── i18n/         # Chinese/English copy
│   ├── stores/       # Pinia state
│   └── views/        # Five pages
├── docs/             # Video tutorials and other docs
├── index.html
├── package.json
├── Cargo.toml        # workspace
└── vite.config.ts
```

### Common Commands

```bash
# Build
cargo build --release -p jvm-cli          # CLI
npm run dev                               # Frontend dev server (the GUI dev server is launched by tauri)
npm run tauri dev                         # GUI development mode
npm run tauri build                       # GUI packaging (builds the CLI first and copies it as the shim carrier)

# Tests and checks
cargo test --workspace                    # All unit tests (including core logic isolated with temporary directories)
cargo clippy --workspace -- -D warnings   # Lint
```

### Code Conventions

- **Thin-shell principle**: the GUI / CLI only do argument and DTO conversion, caching, and event
  pushing; all business orchestration goes in `jvm_core::flow`, keeping both entry points
  behaviorally identical;
- **Path injection**: the core library never reads `$HOME` directly — everything goes through `Home`
  (overridable via `JVM_HOME`), and tests use `tempfile` temporary directories;
- **No blocking IO in async workers**: heavy IO such as extraction, deletion, and `java -version`
  verification runs via `spawn_blocking` on the Tauri side;
- **Atomic config writes**: `.tmp` first, then rename; fall back to defaults on corruption;
- **Windows compatibility**: file-occupancy pre-checks, junction links, registry backup and
  rollback — see the dedicated [How It Works](#how-it-works) section.

## FAQ

**Q: Version switches don't take effect in the current terminal?**
Changes to environment variables (`JAVA_HOME` / `PATH`) only affect **newly launched** terminals.
Everyday switching of the `current` link is immediate; for already-open terminals, just open a new
window. After the first configuration or a repair, `jvm use` will prompt you to reopen the terminal.

**Q: `java -version` fails with "no effective JDK found"?**
The current directory has no binding and no global default is set. Run `jvm use temurin@17` to set a
default, or run `jvm use temurin@17 --project` at the project root to generate a `.jvmrc`.

**Q: Uninstall reports the files are in use?**
Close the terminals / IDEs / build processes using that JDK and try again. The tool pre-checks for
occupancy before deleting, avoiding half-finished operations that leave a corrupted install behind.

**Q: Will other JDKs installed separately on my machine show up in the "Installed" list?**
No. The local list only loads JDKs under the install directory (`installDir` in settings, default
`<data root>/versions`); it does not scan `JAVA_HOME`, the Windows registry, or common system
locations — the external scanning from early versions was removed due to false positives (the
`current` link, the embedded `jre/` of JDK 8 being treated as a standalone JDK, etc.). To bring an
external JDK under management, point `installDir` in settings to its parent directory.

**Q: `doctor` reports "the first java on PATH is not the jvm shim"?**
Another JDK exists on the system whose `bin` directory comes before `<data root>/bin`. Reorder PATH,
or uninstall the other JDK.

**Q: Can the data directory be moved / moved to another drive?**
Yes. The entire data root can move along with the install directory, or you can point `JVM_HOME` to a
new location; after a move, the GUI settings page flags dangling references and supports one-click
repair. The install / download subdirectories can also be individually moved to other drives in
settings.

**Q: Does it work in intranet / proxied environments?**
Yes. Fill in an HTTP proxy in settings, or register an intranet foojay-compatible mirror with
`jvm config registry`.

**Q: Should `.jvmrc` be committed to Git?**
Recommended — it exists precisely so teams can share the project's JDK version; the format is a
single line `vendor@version`.


## License

[MIT](LICENSE) © JVM Desktop contributors

## Acknowledgments

- [nvm-desktop](https://github.com/1111mp/nvm-desktop) — the inspiration for the same-lineage design: data directory layout, shim forwarding, project-level version binding, and the GUI + CLI dual-mode approach
- [foojay Disco API](https://api.foojay.io/) — the default JDK metadata source
- [Adoptium API](https://api.adoptium.net/v3) — the official Temurin data source
- [Tauri](https://tauri.app/) and [Vue.js](https://vuejs.org/) — the GUI tech stack
