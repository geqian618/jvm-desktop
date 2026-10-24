// tauri build 的 beforeBuildCommand（package.json 的 before:build）：
//   1. vite 构建前端资源；
//   2. cargo 构建 jvm-cli（CLI 本体，打包为 bundle 资源；安装后与 GUI 同目录时
//      成为命令 shim 的拷贝源，见 jvm-core switcher::resolve_shim_source）；
//   3. 把产物复制为 crates/jvm-tauri/jvm.exe（Windows）/ jvm（其余平台），
//      供各平台 tauri 配置的 bundle.resources 引用。
// 设置 TAURI_TARGET 时按指定三元组编译（macOS 分架构构建用），
// 产物位于 target/<三元组>/release/ 下。
import { execSync } from 'node:child_process'
import { copyFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const target = process.env.TAURI_TARGET || ''
const binName = process.platform === 'win32' ? 'jvm.exe' : 'jvm'

const run = (cmd) => execSync(cmd, { stdio: 'inherit', cwd: root })

run('npm run build')
run(`cargo build --release -p jvm-cli${target ? ` --target ${target}` : ''}`)

const built = join(root, 'target', target, 'release', binName)
const dest = join(root, 'crates', 'jvm-tauri', binName)
copyFileSync(built, dest)
console.log(`已复制 CLI 本体: ${built} -> ${dest}`)
