<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { formatSize, useAppStore } from "../stores/app";
import { useDialogStore } from "../stores/dialog";
import type { InstallProgress, ReleaseInfo } from "../types";

const { t } = useI18n();
const store = useAppStore();
const dialog = useDialogStore();
const supportType = ref("");

// 标记已安装：按索引记录 id（vendor@version）与包 id 匹配。
// 不按目录名匹配——CLI 与 GUI 的安装目录名策略不同（下划线消毒 vs 原样
// `+`），按目录名会让 CLI 装的版本显示"未安装"，再点安装报"已安装"。
const installedIds = computed(
  () =>
    new Set(
      store.localJdks.map((j) => j.id).filter((id): id is string => !!id),
    ),
);

function isInstalled(r: ReleaseInfo): boolean {
  return installedIds.value.has(r.id);
}

// 下载目录里已有归档的包：下载按钮置灰为「已下载」，安装可直接复用
function isDownloaded(r: ReleaseInfo): boolean {
  return store.downloadedIds.includes(r.id);
}

function progressOf(r: ReleaseInfo) {
  return store.installStates[r.id];
}

// 点击"失败"弹出完整错误（打包版没有控制台日志，这是唯一的错误出口）
function showError(p: InstallProgress) {
  dialog.alert({
    title: t("available.failed"),
    message: p.message || "unknown error",
    kind: "error",
  });
}

function load() {
  store.loadReleases();
}

onMounted(load);

// 全量数据一次拉取，切换发行版/压缩格式才需要重新请求；
// 大版本、维护状态筛选在本地完成（store.releases getter），零网络请求
watch(() => [store.filters.vendor, store.filters.archiveType], load);

watch(
  () => store.filters.maintainedOnly,
  () => {
    // 切换维护筛选后，若当前大版本不在列表中则重置为最新 LTS
    if (
      store.filters.featureVersion != null &&
      !store.featureVersions.includes(store.filters.featureVersion)
    ) {
      store.filters.featureVersion = store.featureVersions[0] ?? null;
    }
  },
);

function isLts(fv: number): boolean {
  return (
    store.majorVersions.find((m) => m.majorVersion === fv)?.termOfSupport.toUpperCase() ===
    "LTS"
  );
}

// store.releases 已按版本降序排序，这里只叠加支持类型筛选
const sortedReleases = computed(() => {
  const st = supportType.value;
  return store.releases.filter(
    (r) => !st || r.termOfSupport.toLowerCase() === st,
  );
});

// 行内支持类型标签：LTS 高亮，MTS/STS 原样小字
function supportLabel(term: string): string {
  return term || "-";
}

// 行内厂商列：api_name → 发行商显示名（下拉数据里带友好名称），缺失时原样
const distNames = computed(
  () => new Map(store.distributions.map((d) => [d.apiName, d.name])),
);
function vendorName(r: ReleaseInfo): string {
  return distNames.value.get(r.distribution) ?? r.distribution;
}
</script>

<template>
  <div class="view">
    <div class="page-header">
      <h2>{{ t("nav.all") }}</h2>
      <span class="muted">{{ t("available.count", { n: sortedReleases.length }) }}</span>
      <div class="spacer" />
      <button class="btn-ghost btn-sm" @click="load">
        {{ t("available.refresh") }}
      </button>
    </div>

    <div v-if="store.registryError" class="registry-error">
      <span>{{ t("available.loadFailed", { msg: store.registryError }) }}</span>
      <button class="btn-ghost btn-sm" @click="store.loadRegistryMeta()">
        {{ t("common.refresh") }}
      </button>
    </div>

    <div class="filters">
      <div class="filter">
        <label>{{ t("available.vendor") }}</label>
        <select v-model="store.filters.vendor" class="w-vendor">
          <option v-for="d in store.distributions" :key="d.apiName" :value="d.apiName">
            {{ d.name }}
          </option>
        </select>
      </div>
      <div class="filter">
        <label>{{ t("available.featureVersion") }}</label>
        <select v-model="store.filters.featureVersion" class="w-major">
          <option :value="null">{{ t("available.all") }}</option>
          <option v-for="fv in store.featureVersions" :key="fv" :value="fv">
            {{ fv }}
            <template v-if="isLts(fv)"> · {{ t("available.lts") }}</template>
          </option>
        </select>
      </div>
      <div class="filter">
        <label>{{ t("available.supportType") }}</label>
        <select v-model="supportType" class="w-archive">
          <option value="">{{ t("available.all") }}</option>
          <option value="lts">LTS</option>
          <option value="mts">MTS</option>
          <option value="sts">STS</option>
        </select>
      </div>
      <div class="filter">
        <label>{{ t("available.archive") }}</label>
        <select v-model="store.filters.archiveType" class="w-archive">
          <option value="zip">zip</option>
          <option value="tar.gz">tar.gz</option>
        </select>
      </div>
      <label class="checkbox">
        <input v-model="store.filters.maintainedOnly" type="checkbox" />
        {{ t("available.maintainedOnly") }}
      </label>
    </div>

    <div
      v-if="store.releasesLoading"
      class="state"
    >
      <span class="spin" />
      <span class="loading-text">{{ t("common.loading") }}</span>
    </div>

    <div v-else-if="store.releasesError" class="state error">
      {{ t("available.loadFailed", { msg: store.releasesError }) }}
    </div>

    <div v-else-if="sortedReleases.length === 0" class="state muted-state">
      {{ t("available.noData") }}
    </div>

    <div v-else class="table-scroll">
      <table class="table table-fixed">
      <colgroup>
        <col class="col-version" />
        <col class="col-dist" />
        <col class="col-vendor" />
        <col class="col-file" />
        <col class="col-type" />
        <col class="col-size" />
        <col class="col-actions" />
      </colgroup>
      <thead>
        <tr>
          <th>{{ t("available.colVersion") }}</th>
          <th>{{ t("available.colDistVersion") }}</th>
          <th>{{ t("available.colVendor") }}</th>
          <th>{{ t("available.colFile") }}</th>
          <th>{{ t("available.colType") }}</th>
          <th>{{ t("available.colSize") }}</th>
          <th>{{ t("available.colAction") }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="r in sortedReleases" :key="r.id">
          <td class="mono">{{ r.javaVersion }}</td>
          <td class="muted">{{ r.distributionVersion || "-" }}</td>
          <td class="dist-cell">
            <span :title="vendorName(r)">{{ vendorName(r) }}</span>
          </td>
          <td class="file-cell">
            <span class="mono" :title="r.filename">{{ r.filename }}</span>
          </td>
          <td>
            <span v-if="isLts(parseInt(r.javaVersion, 10))" class="pill pill-ok">LTS</span>
            <span v-else class="muted">{{ supportLabel(r.termOfSupport) }}</span>
          </td>
          <td class="muted">{{ formatSize(r.size) }}</td>
          <td class="actions">
            <template v-if="progressOf(r)">
              <span v-if="progressOf(r)!.stage === 'download'" class="dl-row">
                <span class="progress-track">
                  <span
                    class="progress-fill"
                    :style="{ width: progressOf(r)!.percent + '%' }"
                  />
                </span>
                <span class="dl-text">
                  {{ t("available.downloading") }} {{ progressOf(r)!.percent.toFixed(0) }}%
                </span>
                <button class="btn-ghost btn-sm" @click="store.cancelTask(r.id)">
                  {{ t("common.cancel") }}
                </button>
              </span>
              <span v-else-if="progressOf(r)!.stage === 'prepare'">
                <span class="spin" /> {{ t("available.preparing") }}
              </span>
              <span v-else-if="progressOf(r)!.stage === 'extract'">
                <span class="spin" /> {{ t("available.extracting") }}
              </span>
              <span v-else-if="progressOf(r)!.stage === 'done'" class="done">
                ✓ {{ t("available.done") }}
              </span>
              <span v-else-if="progressOf(r)!.stage === 'cancelled'" class="muted">
                {{ t("available.cancelled") }}
              </span>
              <span
                v-else
                class="failed clickable"
                :title="progressOf(r)!.message"
                @click="showError(progressOf(r)!)"
              >
                {{ t("available.failed") }}:
                <span class="err-text">{{ progressOf(r)!.message }}</span>
              </span>
            </template>
            <template v-else>
              <!-- 下载在前：可只下载压缩包；之后点「安装」会复用已校验的包，直接解压 -->
              <button
                class="btn-ghost btn-sm"
                :disabled="isDownloaded(r) || !r.directlyDownloadable"
                :title="r.directlyDownloadable ? undefined : t('available.licenseRequired')"
                @click="store.downloadOnly(r)"
              >
                {{ isDownloaded(r) ? t("available.downloaded") : t("available.downloadOnly") }}
              </button>
              <button
                class="btn-primary btn-sm"
                :disabled="isInstalled(r) || !r.directlyDownloadable"
                :title="r.directlyDownloadable ? undefined : t('available.licenseRequired')"
                @click="store.install(r)"
              >
                {{ isInstalled(r) ? t("available.installed") : t("available.install") }}
              </button>
            </template>
          </td>
        </tr>
      </tbody>
    </table>
    </div>
  </div>
</template>

<style scoped>
/* 固定列宽：某一行进入下载/安装状态（内容变宽）时其它列不受影响 */
.table-fixed {
  table-layout: fixed;
}

.col-version {
  width: 110px;
}

.col-dist {
  width: 100px;
}

.col-vendor {
  width: 150px;
}

/* 文件列也给基准宽度：fixed 布局下所有列都有宽时，窗口多余宽度
   按比例摊给各列，避免全部空白集中在一列里 */
.col-file {
  width: 360px;
}

.col-type {
  width: 70px;
}

.col-size {
  width: 90px;
}

.col-actions {
  width: 200px;
}

/* 下载中一行：进度条弹性填充剩余宽度，避免操作列留大片空白 */
.dl-row {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  width: 100%;
}

.dl-row .progress-track {
  flex: 1;
  width: auto;
  min-width: 32px;
  margin-right: 0;
}

.dl-text {
  white-space: nowrap;
  font-size: 12px;
}

/* 固定布局下列宽受限，长文件名截断显示 */
.file-cell {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* 厂商名同样可能较长（Eclipse Temurin / GraalVM Community 等） */
.dist-cell {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.page-header {
  display: flex;
  align-items: center;
  gap: 16px;
  margin-bottom: 16px;
}

h2 {
  font-size: 17px;
  font-weight: 600;
  margin-right: auto;
}

.spacer {
  flex: 1;
}

.registry-error {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  background: rgba(217, 72, 72, 0.07);
  border: 1px solid rgba(217, 72, 72, 0.3);
  border-radius: var(--radius);
  padding: 10px 16px;
  margin-bottom: 14px;
  color: var(--danger);
  font-size: 12px;
  word-break: break-all;
  user-select: text;
}

.filters {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 16px;
  margin-bottom: 16px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 12px 16px;
}

.filter {
  display: flex;
  align-items: center;
  gap: 8px;
}

.w-vendor {
  min-width: 180px;
}

.w-major {
  min-width: 90px;
}

.w-archive {
  min-width: 90px;
}

.checkbox {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
}

.state {
  background: var(--panel);
  border: 1px dashed var(--border);
  border-radius: var(--radius);
  padding: 50px 0;
  text-align: center;
  color: var(--text-secondary);
}

.loading-text {
  margin-left: 8px;
}

.state.error {
  color: var(--danger);
  word-break: break-all;
  padding: 30px 40px;
  user-select: text;
}

.actions {
  white-space: nowrap;
}

.actions > * {
  margin-right: 8px;
}

.actions > *:last-child {
  margin-right: 0;
}

.btn-sm {
  padding: 4px 12px;
  font-size: 12px;
}

.done {
  color: var(--success);
}

.failed {
  color: var(--danger);
}

.clickable {
  cursor: pointer;
  user-select: text;
}

.err-text {
  display: inline-block;
  max-width: 110px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
  font-size: 11px;
}

.pill-jre {
  color: var(--accent);
  border-color: rgba(47, 111, 237, 0.35);
  background: rgba(47, 111, 237, 0.06);
}

.col-type {
  width: 80px;
}
</style>
