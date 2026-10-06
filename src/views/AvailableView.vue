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
// CLI 与 GUI 的安装目录名策略一致（都是 vendor@version），但匹配仍按
// 索引 id——目录名只是展示值。
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
      <table class="table">
      <thead>
        <tr>
          <th>{{ t("available.colName") }}</th>
          <th>{{ t("available.colVendor") }}</th>
          <th>{{ t("available.colDistVersion") }}</th>
          <th>{{ t("available.colVersion") }}</th>
          <th>{{ t("available.colType") }}</th>
          <th>{{ t("available.colSize") }}</th>
          <th>{{ t("available.colAction") }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="r in sortedReleases" :key="r.id">
          <td class="mono name-cell" :title="r.id">
            {{ r.distribution }}@{{ r.javaVersion }}
          </td>
          <td class="dist-cell">
            <span :title="vendorName(r)">{{ vendorName(r) }}</span>
          </td>
          <td class="muted">{{ r.distributionVersion || "-" }}</td>
          <td class="mono">{{ r.javaVersion }}</td>
          <td>
            <span v-if="isLts(parseInt(r.javaVersion, 10))" class="pill pill-ok">LTS</span>
            <span v-else class="muted">{{ supportLabel(r.termOfSupport) }}</span>
          </td>
          <td class="muted">{{ formatSize(r.size) }}</td>
          <td class="actions">
            <template v-if="progressOf(r)">
              <span v-if="progressOf(r)!.stage === 'download'" class="dl-row">
                <span class="dl-label">{{ t("available.downloading") }}</span>
                <span class="progress-track">
                  <span
                    class="progress-fill"
                    :style="{ width: progressOf(r)!.percent + '%' }"
                  />
                </span>
                <span class="dl-text">
                  <span class="dl-percent">{{ progressOf(r)!.percent.toFixed(0) }}%</span>
                  <span v-if="progressOf(r)!.speed" class="dl-speed muted">
                    · {{ formatSize(progressOf(r)!.speed ?? 0) }}/s
                  </span>
                </span>
                <button class="btn-ghost btn-sm" @click="store.cancelTask(r.id)">
                  {{ t("common.cancel") }}
                </button>
              </span>
              <span v-else-if="progressOf(r)!.stage === 'prepare'" class="loading-row">
                <span class="spin" /> {{ t("available.preparing") }}
              </span>
              <span v-else-if="progressOf(r)!.stage === 'extract'" class="loading-row">
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
/* 纯自适应（浏览器自动布局）：各列宽度由内容决定，表格铺满容器时
   富余宽度按各列内容宽度比例分摊——空白均匀、任何内容都不截断。
   全表不换行：折行会让列被压向最小宽度，出现 "370.8 MB" 断成两行；
   内容总宽超出窗口时由 table-scroll 横向滚动兜底 */
.table th,
.table td {
  white-space: nowrap;
}

/* 名称列锁在内容最大宽度附近，避免按比例分摊时它作为最宽列
   摊走过多富余宽度；超长的值截断省略，悬停 title 可看全名 */
.name-cell {
  max-width: 180px;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* 操作列锁定宽度：自动布局下，下载进度文本每次刷新都会改变该列
   的内容宽度，导致整表列宽重排（即「抖动」）；锁定后阶段切换、
   百分比/速度变化只在列内消化，其它列纹丝不动 */
.actions {
  width: 264px;
}

/* 行高恒定：空闲(两个按钮)与下载中(进度行)强制等高并统一垂直居中。
   两者都是 inline 盒但基线规则不同——按钮取文字基线，inline-flex 取
   首个子元素的基线，切换瞬间行盒高度差几像素就是「抖一下」的根源 */
/* 准备中/解压中一行：动画与文字的间隔（垂直居中由 .actions > * 的
   align-items: center 提供） */
.loading-row {
  gap: 6px;
}

/* 下载中一行：进度条弹性填充操作列剩余宽度；display/align 由
   .actions > * 统一提供，这里只补进度行内部的子元素间距 */
.dl-row {
  gap: 6px;
  width: 100%;
}

.dl-row .progress-track {
  flex: 1;
  width: auto;
  min-width: 32px;
  margin-right: 0;
}

/* 「下载中」标签：进度条前，宽度天然恒定 */
.dl-label {
  white-space: nowrap;
  font-size: 12px;
}

/* 百分比+速度段在进度条之后：速度段用自然宽度（永远紧贴前后元素，
   不留空隙）；百分比段固定 26px 吸收 "9%→100%" 的位数变化，速度起点
   因此固定。位数变化带来的整体宽度增减由弹性进度条吸收，取消按钮
   作为行尾元素位置恒定 */
.dl-text {
  display: inline-flex;
  gap: 4px;
  white-space: nowrap;
  font-size: 12px;
}

.dl-percent {
  width: 26px;
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

/* 行高恒定：操作列所有直接子元素（按钮组 / 进度行 / 准备中·完成·
   失败等状态文本）统一 28px 等高、垂直居中。状态文本是行内 span，
   height 不生效，必须先转 inline-flex；否则「按钮 → 文字」切换的
   瞬间文字撑不起高度，整行会缩一下 */
.actions > * {
  display: inline-flex;
  align-items: center;
  height: 28px;
  vertical-align: middle;
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
</style>
