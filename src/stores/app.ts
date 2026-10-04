import { defineStore } from "pinia";
import { watch } from "vue";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { i18n } from "../i18n";
import type {
  AppConfig,
  AppLog,
  DistributionInfo,
  InstallProgress,
  LocalJdk,
  LogLevel,
  MajorVersion,
  Platform,
  ProjectEntry,
  ReleaseInfo,
} from "../types";

interface Filters {
  vendor: string;
  featureVersion: number | null;
  archiveType: string;
  maintainedOnly: boolean;
}

// 设置自动落盘的防抖计时器与 watcher 注册标记（模块级，store 生命周期内共享）
let persistTimer: ReturnType<typeof setTimeout> | null = null;
let persistWatcherBound = false;

/** 每个下载任务的速度估算状态：样本窗口 + 每秒才刷新一次的显示值 */
const speedTrackers = new Map<
  string,
  { samples: { ts: number; downloaded: number }[]; display: number; lastCalc: number }
>();

/** 设置兜底默认值（config 加载失败等极端场景） */
export const CONFIG_FALLBACK: AppConfig = {
  language: "zh-CN",
  theme: "system",
  installDir: "",
  downloadDir: "",
  registry: "https://api.foojay.io/disco/v3.0",
  registryPresets: [],
  proxy: "",
  sourceApiKey: "",
  autoCleanDownload: false,
};

export const useAppStore = defineStore("app", {
  state: () => ({
    config: null as AppConfig | null,
    /** 最近一次设置成功落盘的时间戳（设置页显示“已自动保存”提示用） */
    configSavedAt: 0,
    platform: null as Platform | null,
    localJdks: [] as LocalJdk[],
    projects: [] as ProjectEntry[],
    installedIds: [] as string[],
    currentJavaHome: null as string | null,
    distributions: [] as DistributionInfo[],
    majorVersions: [] as MajorVersion[],
    allPackages: [] as ReleaseInfo[],
    releasesLoading: false,
    releasesError: "",
    registryError: "",
    filters: {
      vendor: "",
      featureVersion: null,
      archiveType: "",
      maintainedOnly: false,
    } as Filters,
    installStates: {} as Record<string, InstallProgress>,
    /** 下载目录里已有归档的包 id：下载按钮置灰为「已下载」，安装可直接复用 */
    downloadedIds: [] as string[],
    logs: [] as AppLog[],
  }),

  getters: {
    // foojay v3 的操作系统取值域与后端 current_os()（std::env::consts::OS）
    // 一致：windows / macos / linux。macOS 无需按架构区分——旧的 "mac_os"
    // 取值传给 Adoptium 源会 404（foojay 虽兼容别名，两端也必须统一）。
    foojayOs(state): string {
      if (!state.platform) return "windows";
      return state.platform.os;
    },
    foojayArch(state): string {
      return state.platform?.arch === "aarch64" ? "aarch64" : "x64";
    },
    defaultArchiveType(state): string {
      return state.platform?.os === "windows" ? "zip" : "tar.gz";
    },
    featureVersions(state): number[] {
      // 仅显示当前发行版+平台下确实有包的大版本（0 数据的选项不显示）
      const available = new Set(
        state.allPackages.map((r) => parseInt(r.javaVersion, 10) || 0),
      );
      return state.majorVersions
        .filter(
          (m) =>
            (!state.filters.maintainedOnly || m.maintained) &&
            available.has(m.majorVersion),
        )
        .map((m) => m.majorVersion)
        .sort((a, b) => b - a);
    },
    // 当前筛选下的可下载包：全量数据已在本地，筛选/排序零网络请求
    releases(state): ReleaseInfo[] {
      const maintainedMajors = state.filters.maintainedOnly
        ? new Set(
            state.majorVersions
              .filter((m) => m.maintained)
              .map((m) => m.majorVersion),
          )
        : null;
      const fv = state.filters.featureVersion;
      return state.allPackages
        .filter((r) => {
          const major = parseInt(r.javaVersion, 10) || 0;
          if (fv != null && major !== fv) return false;
          if (maintainedMajors && !maintainedMajors.has(major)) return false;
          return true;
        })
        .sort((a, b) => compareVersions(b.javaVersion, a.javaVersion));
    },
  },

  actions: {
    /** 追加一条日志（上限 1000 条，超出丢弃最早的） */
    log(level: LogLevel, message: string) {
      this.logs.push({ ts: Date.now(), level, message });
      if (this.logs.length > 1000) {
        this.logs.splice(0, this.logs.length - 1000);
      }
      if (level === "error") {
        try {
          import("@tauri-apps/api/core")
            .then(({ invoke }) => invoke("debug_log", { message: `[store] ${message}` }))
            .catch(() => undefined);
        } catch {
          /* ignore */
        }
      }
    },

    async init() {
      // 先注册日志监听，保证后续动作都能被记录
      await listen<AppLog>("app-log", (event) => {
        this.log(event.payload.level, event.payload.message);
      });
      // 前端运行时错误也进日志页（main.ts 里全局捕获后广播到 window）
      window.addEventListener("app-error", (e) => {
        this.log("error", `前端错误: ${(e as CustomEvent).detail}`);
      });

      this.config = await api.getConfig();
      i18n.global.locale.value = this.config.language as "zh-CN" | "en-US";
      this.normalizeConfig();
      // 设置任何变更：先写内存 store（页面切换不丢），再防抖落盘
      if (!persistWatcherBound) {
        persistWatcherBound = true;
        watch(
          () => this.config,
          () => {
            if (persistTimer) clearTimeout(persistTimer);
            persistTimer = setTimeout(() => {
              void this.persistConfig();
            }, 600);
          },
          { deep: true },
        );
        // 语言字段变更立即切换界面语言（其余设置由后端每次操作时从磁盘读取）
        watch(
          () => this.config?.language,
          (lang) => {
            if (lang) i18n.global.locale.value = lang as "zh-CN" | "en-US";
          },
        );
        // 数据源切换：发行版/大版本/包列表是启动时按旧源加载的，不重拉的话
        // 「可用版本」页永远显示旧源数据。立即落盘（跳过 600ms 防抖，后端
        // 每次命令都从磁盘读设置，不落盘就会按旧源拉取），再清空旧数据重载。
        watch(
          () => this.config?.registry,
          (reg, old) => {
            if (!reg || old === undefined || reg === old) return;
            if (persistTimer) {
              clearTimeout(persistTimer);
              persistTimer = null;
            }
            void (async () => {
              await this.persistConfig();
              this.distributions = [];
              this.majorVersions = [];
              this.allPackages = [];
              this.filters.vendor = "";
              this.filters.featureVersion = null;
              await this.loadRegistryMeta();
            })();
          },
        );
      }
      this.platform = await api.getPlatform();
      this.filters.archiveType = this.defaultArchiveType;
      this.log("info", "应用启动");

      await listen<InstallProgress>("install-progress", (event) => {
        const p = event.payload;
        if (p.stage === "download") {
          // 速度估算：进度事件 10 条/秒，若每条都重算显示值，数字一秒会跳
          // 好几次。样本按事件收集（保准确），但显示值每 1s 才按最近 5s
          // 的滑动窗口重算一次——数字 1 秒最多变一次，数值是真实平均速率。
          // 重连空档期没有事件，恢复后首个样本按真实时间差计入，不含水分。
          const now = Date.now();
          const tr =
            speedTrackers.get(p.taskId) ?? { samples: [], display: 0, lastCalc: 0 };
          tr.samples.push({ ts: now, downloaded: p.downloaded });
          while (tr.samples.length > 2 && now - tr.samples[0].ts > 5000) {
            tr.samples.shift();
          }
          if (now - tr.lastCalc >= 1000) {
            tr.lastCalc = now;
            const first = tr.samples[0];
            if (now > first.ts && p.downloaded >= first.downloaded) {
              tr.display = ((p.downloaded - first.downloaded) * 1000) / (now - first.ts);
            }
          }
          speedTrackers.set(p.taskId, tr);
          this.installStates[p.taskId] = { ...p, speed: tr.display };
        } else {
          speedTrackers.delete(p.taskId);
          this.installStates[p.taskId] = p;
        }
        if (p.stage === "done") {
          this.refreshLocal();
          this.refreshDownloaded();
          setTimeout(() => delete this.installStates[p.taskId], 4000);
        } else if (p.stage === "error") {
          // 失败提示展示几秒后自动清除，避免永远停在"失败"
          setTimeout(() => delete this.installStates[p.taskId], 4000);
        } else if (p.stage === "cancelled") {
          setTimeout(() => delete this.installStates[p.taskId], 1500);
        }
      });

      await Promise.all([
        this.refreshLocal(),
        this.loadRegistryMeta().catch(() => undefined), // 失败不阻塞本地列表
      ]);
    },

    async refreshLocal() {
      try {
        this.localJdks = await api.getLocalJdks();
        this.currentJavaHome = await api.getCurrentJavaHome();
        this.log("info", `本地扫描完成，发现 ${this.localJdks.length} 个 JDK`);
      } catch (e) {
        this.log("error", `本地扫描失败: ${e}`);
      }
    },

    async refreshProjects() {
      try {
        this.projects = await api.listProjects();
        const ids = await api.getInstalledIds();
        // 后端 InstalledJdk（camelCase）→ 可绑定版本 id 列表
        this.installedIds = (ids as { id: string }[]).map((r) => r.id);
      } catch (e) {
        this.log("error", `项目关联列表加载失败: ${e}`);
      }
    },

    async loadRegistryMeta() {
      this.registryError = "";
      try {
        const [distributions, majorVersions] = await Promise.all([
          api.getDistributions(),
          api.getMajorVersions(),
        ]);
        this.distributions = distributions;
        this.majorVersions = majorVersions;
        // 过滤当前平台下没有任何 GA JDK 包的发行商（如 bisheng 只发 Linux 包，
        // 切过去必然空数据）。探测失败或结果为空（镜像源不支持该查询、
        // adoptium 等单发行商源）时保留全部，宁可多显示也不误删。
        try {
          const vendors = await api.getAvailableVendors(
            this.foojayOs,
            this.foojayArch,
          );
          const filtered = this.distributions.filter((d) =>
            vendors.includes(d.apiName),
          );
          if (filtered.length > 0) {
            this.distributions = filtered;
          }
        } catch (e) {
          // error 级会转发进后端日志文件，便于排查探测在真机上的实际表现
          this.log("error", `平台可用发行商探测失败，保留全部: ${e}`);
        }
        this.log(
          "info",
          `数据源加载完成：${this.distributions.length} 个发行商 / ${this.majorVersions.length} 个大版本`,
        );
      } catch (e) {
        this.registryError = String(e);
        this.log("error", `加载数据源元数据失败: ${e}`);
        return;
      }
      // 默认选中：列表第一个发行商；大版本默认「全部」
      if (!this.filters.vendor && this.distributions.length > 0) {
        this.filters.vendor = this.distributions[0].apiName;
      }
    },

    /** 一次拉取该发行版在当前平台下的全部 GA 包（不带 version 参数），筛选全在本地完成 */
    async loadReleases() {
      if (!this.filters.vendor) {
        this.allPackages = [];
        return;
      }
      this.releasesLoading = true;
      try {
        this.allPackages = await api.getAllReleases(
          this.filters.vendor,
          this.foojayOs,
          this.foojayArch,
          this.filters.archiveType,
        );
        this.releasesError = "";
        // 切换发行版/格式后，所选大版本在新数据中已无对应包时重置为「全部」
        if (
          this.filters.featureVersion != null &&
          !this.featureVersions.includes(this.filters.featureVersion)
        ) {
          this.filters.featureVersion = null;
        }
      } catch (e) {
        this.releasesError = String(e);
        this.log("error", `加载版本列表失败: ${e}`);
      } finally {
        this.releasesLoading = false;
      }
      await this.refreshDownloaded();
    },

    async install(release: ReleaseInfo) {
      // 安装目录名与下载文件名统一用 vendor@version（与列表名称列一致）
      const dirName = `${release.distribution}@${release.javaVersion}`;
      try {
        await api.installJdk(release, dirName);
      } catch (e) {
        // 后端拒绝（如"任务已在进行中"）没有进度事件，必须在日志页可见
        this.log("error", `发起安装失败: ${e}`);
        throw e;
      }
    },

    /** 仅下载压缩包到下载目录，不解压不安装 */
    async downloadOnly(release: ReleaseInfo) {
      try {
        await api.downloadJdk(release);
      } catch (e) {
        this.log("error", `发起下载失败: ${e}`);
        throw e;
      }
    },

    /** 同步「已下载」包列表（下载目录里已有归档即算，重启后仍然生效） */
    async refreshDownloaded() {
      try {
        this.downloadedIds = await api.getDownloadedIds(this.allPackages);
      } catch {
        // 非关键展示状态，失败时保持现状
      }
    },

    /** 取消下载/安装任务 */
    async cancelTask(taskId: string) {
      this.log("warn", "已发送取消请求");
      await api.cancelTask(taskId);
    },

    async useJdk(dir: string) {
      try {
        await api.useJdk(dir);
        this.log("success", `已设为默认 JDK: ${dir}`);
      } catch (e) {
        this.log("error", `设为默认失败: ${e}`);
        throw e;
      }
      await this.refreshLocal();
    },

    async uninstallJdk(dir: string) {
      try {
        await api.uninstallJdk(dir);
        this.log("success", `已卸载 JDK: ${dir}`);
      } catch (e) {
        this.log("error", `卸载 JDK 失败: ${e}`);
        throw e;
      }
      await this.refreshLocal();
    },

    async bindProject(path: string, id: string) {
      await api.bindProject(path, id);
      this.log("success", `已绑定项目 ${path} → ${id}`);
      await this.refreshProjects();
    },

    async unbindProject(path: string) {
      try {
        await api.unbindProject(path);
        this.log("success", `已解除绑定: ${path}`);
      } catch (e) {
        this.log("error", `解除绑定失败: ${e}`);
        throw e;
      }
      await this.refreshProjects();
    },

    /** 兼容旧配置：registry 存了不在预设列表中的地址时补进列表，保证当前使用的源可见可编辑 */
    normalizeConfig() {
      const cfg = this.config;
      if (!cfg) return;
      cfg.registryPresets ||= [];
      const cur = cfg.registry.trim().replace(/\/+$/, "");
      if (
        cur &&
        !cfg.registryPresets.some((p) => p.url.replace(/\/+$/, "") === cur)
      ) {
        cfg.registryPresets.push({
          name: i18n.global.t("settings.customSource"),
          url: cur,
        });
      }
    },

    /** 设置落盘（由 config 深度 watcher 防抖触发；内存 store 已先行生效） */
    async persistConfig() {
      if (!this.config) return;
      await api.saveConfig(JSON.parse(JSON.stringify(this.config)));
      this.configSavedAt = Date.now();
      this.log("success", "设置已自动保存");
    },
  },
});

export function compareVersions(a: string, b: string): number {
  const pa = a.split(/[.+_-]/).map((x) => parseInt(x, 10) || 0);
  const pb = b.split(/[.+_-]/).map((x) => parseInt(x, 10) || 0);
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d !== 0) return d;
  }
  return a.localeCompare(b);
}

export function formatSize(bytes: number): string {
  if (!bytes) return "-";
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
