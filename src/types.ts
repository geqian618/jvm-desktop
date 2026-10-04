export interface RegistryPreset {
  name: string;
  url: string;
  /** 内置源（官方预置）：可编辑名称/地址，但不可删除 */
  builtIn?: boolean;
}

/** 设置留空时后端实际生效的默认目录 */
export interface PathDefaults {
  installDir: string;
  downloadDir: string;
}

export interface AppConfig {
  language: string;
  theme: string;
  installDir: string;
  downloadDir: string;
  registry: string;
  registryPresets: RegistryPreset[];
  proxy: string;
  sourceApiKey: string;
  autoCleanDownload: boolean;
}

export interface DistributionInfo {
  apiName: string;
  name: string;
  maintained: boolean;
  versions: string[];
}

export interface MajorVersion {
  majorVersion: number;
  termOfSupport: string;
  maintained: boolean;
  versions: string[];
}

export interface ReleaseInfo {
  id: string;
  distribution: string;
  javaVersion: string;
  distributionVersion: string;
  packageType: string;
  termOfSupport: string;
  archiveType: string;
  filename: string;
  size: number;
  directlyDownloadable: boolean;
  /** 包详情地址（foojay /ids/{id}），原样回传给后端用于按 id 补全直链 */
  infoUri?: string;
}

export interface LocalJdk {
  dir: string;
  name: string;
  /** 索引记录 id（vendor@version）；目录未登记时为 null */
  id: string | null;
  javaVersion: string | null;
  vendor: string | null;
  /** 安装日期（本地时区 YYYY-MM-DD，来自索引；未登记时为 null） */
  installedAt: string | null;
  inInstallDir: boolean;
  isCurrent: boolean;
}

export interface ProjectEntry {
  path: string;
  name: string;
  jdk: string;
  addedAt: string;
  lastUsedAt: string;
}

export type InstallStage =
  | "prepare"
  | "download"
  | "extract"
  | "done"
  | "error"
  | "cancelled";

export type LogLevel = "info" | "success" | "warn" | "error";

export interface AppLog {
  ts: number;
  level: LogLevel;
  message: string;
}

export interface InstallProgress {
  taskId: string;
  stage: InstallStage;
  downloaded: number;
  total: number;
  percent: number;
  message: string;
  /** 实时下载速度（B/s）：前端按进度事件差分 + EMA 平滑估算，仅 download 阶段有值 */
  speed?: number;
}

export interface Platform {
  os: string;
  arch: string;
}
