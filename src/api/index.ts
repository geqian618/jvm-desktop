import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  DistributionInfo,
  HealthIssue,
  LocalJdk,
  MajorVersion,
  PathDefaults,
  Platform,
  ProjectEntry,
  ReleaseInfo,
} from "../types";

export const api = {
  getConfig: () => invoke<AppConfig>("get_config"),
  saveConfig: (config: AppConfig) => invoke<void>("save_config", { config }),
  getPlatform: () => invoke<Platform>("get_platform"),
  getDefaultPaths: () => invoke<PathDefaults>("get_default_paths"),
  healthCheck: () => invoke<HealthIssue[]>("health_check"),
  healthRepair: () => invoke<string[]>("health_repair"),
  getDistributions: () => invoke<DistributionInfo[]>("get_distributions"),
  getMajorVersions: () => invoke<MajorVersion[]>("get_major_versions"),
  getAvailableVendors: (operatingSystem: string, architecture: string) =>
    invoke<string[]>("get_available_vendors", { operatingSystem, architecture }),
  getAllReleases: (
    distribution: string,
    operatingSystem: string,
    architecture: string,
    archiveType: string,
  ) =>
    invoke<ReleaseInfo[]>("get_all_releases", {
      distribution,
      operatingSystem,
      architecture,
      archiveType,
    }),
  getLocalJdks: () => invoke<LocalJdk[]>("get_local_jdks"),
  getCurrentJavaHome: () => invoke<string | null>("get_current_java_home"),
  getInstalledIds: () => invoke<unknown[]>("get_installed_ids"),
  installJdk: (release: ReleaseInfo, dirName: string) =>
    invoke<string>("install_jdk", { release, dirName }),
  downloadJdk: (release: ReleaseInfo) =>
    invoke<string>("download_jdk", { release }),
  getDownloadedIds: (releases: ReleaseInfo[]) =>
    invoke<string[]>("get_downloaded_ids", { releases }),
  cancelTask: (taskId: string) => invoke<void>("cancel_task", { taskId }),
  useJdk: (dir: string) => invoke<void>("use_jdk", { dir }),
  unsetDefault: () => invoke<void>("unset_default"),
  uninstallJdk: (dir: string) => invoke<void>("uninstall_jdk", { dir }),
  openDir: (path: string) => invoke<void>("open_dir", { path }),
  listProjects: () => invoke<ProjectEntry[]>("list_projects"),
  bindProject: (path: string, id: string) =>
    invoke<boolean>("bind_project", { path, id }),
  unbindProject: (path: string) => invoke<boolean>("unbind_project", { path }),
  pickFolder: () => invoke<string | null>("pick_folder"),
};
