<script setup lang="ts">
import { onMounted } from "vue";
import { useI18n } from "vue-i18n";
import { useAppStore } from "../stores/app";
import { useDialogStore } from "../stores/dialog";
import { api } from "../api";
import type { LocalJdk } from "../types";

const { t } = useI18n();
const store = useAppStore();
const dialog = useDialogStore();

// 与项目关联页保持一致：已登记的 JDK 显示规范 id（vendor@version，
// 即 .jvmrc 内容），未登记的目录显示名称
function displayName(jdk: LocalJdk): string {
  return jdk.id || jdk.name;
}

async function removeJdk(jdk: LocalJdk) {
  // 预检占用：默认版本或被项目关联时直接弹窗说明原因，不发卸载请求
  await store.refreshProjects();
  const boundBy = store.projects
    .filter((p) => jdk.id && p.jdk === jdk.id)
    .map((p) => p.name);
  if (jdk.isCurrent || boundBy.length > 0) {
    const reasons: string[] = [];
    if (jdk.isCurrent)
      reasons.push(t("installed.blockedDefault", { name: displayName(jdk) }));
    if (boundBy.length > 0)
      reasons.push(
        t("installed.blockedProjects", {
          name: displayName(jdk),
          projects: boundBy.join("、"),
        }),
      );
    await dialog.alert({
      title: t("installed.uninstall"),
      message: reasons.join("\n"),
      kind: "warning",
    });
    return;
  }

  const ok = await dialog.confirm({
    title: t("installed.uninstall"),
    message: t("installed.confirmUninstall"),
    kind: "warning",
    danger: true,
    confirmText: t("installed.uninstall"),
  });
  if (!ok) return;
  try {
    await store.uninstallJdk(jdk.dir);
  } catch (e) {
    // 删除失败（文件被占用等）弹窗可见，不再只写日志
    await dialog.alert({
      title: t("installed.uninstall"),
      message: String(e),
      kind: "error",
    });
  }
}

async function unsetDefault(jdk: LocalJdk) {
  const ok = await dialog.confirm({
    title: t("installed.unset"),
    message: t("installed.confirmUnset", { name: displayName(jdk) }),
    kind: "warning",
    danger: true,
    confirmText: t("installed.unset"),
  });
  if (!ok) return;
  try {
    await store.unsetDefault();
  } catch (e) {
    await dialog.alert({
      title: t("installed.unset"),
      message: String(e),
      kind: "error",
    });
  }
}

onMounted(() => store.refreshLocal());
</script>

<template>
  <div class="view">
    <div class="page-head">
      <h2>{{ t("installed.title") }}</h2>
      <span class="muted">{{ t("installed.count", { n: store.localJdks.length }) }}</span>
      <div class="spacer" />
      <div class="home-info">
        <span class="muted">{{ t("installed.currentHome") }}:</span>
        <span class="mono home-path">{{
          store.currentJavaHome || t("installed.none")
        }}</span>
      </div>
      <button class="btn-ghost btn-sm" @click="store.refreshLocal()">
        {{ t("common.refresh") }}
      </button>
    </div>

    <div v-if="store.localJdks.length === 0" class="empty">
      {{ t("installed.empty") }}
    </div>

    <div v-else class="table-scroll">
      <table class="table">
      <thead>
        <tr>
          <th class="w-name">{{ t("installed.colName") }}</th>
          <th>{{ t("installed.colVendor") }}</th>
          <th>{{ t("installed.colVersion") }}</th>
          <th>{{ t("installed.colPath") }}</th>
          <th>{{ t("installed.colInstallDate") }}</th>
          <th>{{ t("installed.colStatus") }}</th>
          <th class="w-actions">{{ t("installed.colAction") }}</th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="jdk in store.localJdks" :key="jdk.dir">
          <td class="w-name">
            <span class="name">{{ displayName(jdk) }}</span>
          </td>
          <td class="muted w-vendor">{{ jdk.vendor || t("installed.unknown") }}</td>
          <td>
            <span class="mono">{{
              jdk.javaVersion || t("installed.unknown")
            }}</span>
          </td>
          <td class="mono path w-path" :title="jdk.dir">{{ jdk.dir }}</td>
          <td class="mono muted w-date">{{ jdk.installedAt || "-" }}</td>
          <td>
            <span v-if="jdk.isCurrent" class="badge badge-current">
              <svg class="badge-icon" viewBox="0 0 16 16" aria-hidden="true">
                <circle cx="8" cy="8" r="6.6" fill="none" stroke="currentColor" stroke-width="1.4" />
                <path
                  d="M5.1 8.3l2 2 3.8-4.4"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.6"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
              {{ t("installed.current") }}
            </span>
            <span v-else class="badge badge-installed">
              <svg class="badge-icon" viewBox="0 0 16 16" aria-hidden="true">
                <circle cx="8" cy="8" r="6.6" fill="none" stroke="currentColor" stroke-width="1.4" />
                <path
                  d="M5.1 8.3l2 2 3.8-4.4"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.6"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
              {{ t("installed.installed") }}
            </span>
          </td>
          <td class="actions">
            <button
              v-if="!jdk.isCurrent"
              class="btn-primary btn-sm"
              @click="store.useJdk(jdk.dir)"
            >
              {{ t("installed.use") }}
            </button>
            <button
              v-else
              class="btn-ghost btn-sm"
              @click="unsetDefault(jdk)"
            >
              {{ t("installed.unset") }}
            </button>
            <button class="btn-ghost btn-sm" @click="api.openDir(jdk.dir)">
              {{ t("installed.open") }}
            </button>
            <button
              class="btn-danger btn-sm"
              :disabled="store.uninstallingDir !== null"
              @click="removeJdk(jdk)"
            >
              <template v-if="store.uninstallingDir === jdk.dir">
                <span class="spin" /> {{ t("installed.uninstalling") }}
              </template>
              <template v-else>{{ t("installed.uninstall") }}</template>
            </button>
          </td>
        </tr>
      </tbody>
    </table>
    </div>
  </div>
</template>

<style scoped>
.page-head {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 14px;
}

h2 {
  font-size: 16px;
  font-weight: 600;
}

.spacer {
  flex: 1;
}

.home-info {
  display: flex;
  align-items: center;
  gap: 8px;
  max-width: 420px;
}

.home-path {
  user-select: text;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.empty {
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius);
  padding: 60px 0;
  text-align: center;
  color: var(--text2);
}

.name {
  font-weight: 500;
}

.path {
  max-width: 300px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text2);
}

.w-name {
  min-width: 150px;
}

.w-actions {
  width: 210px;
}

.actions {
  white-space: nowrap;
}

/* 行内按钮间距（不使用 flex，保证 vertical-align: middle 生效） */
.actions .btn-sm {
  margin-right: 6px;
}

.actions .btn-sm:last-child {
  margin-right: 0;
}

.table td {
  vertical-align: middle;
}

.w-vendor {
  white-space: nowrap;
}

.w-path {
  max-width: 300px;
}

/* 安装日期列：等宽对齐 YYYY-MM-DD */
.w-date {
  white-space: nowrap;
}

/* 状态徽章：当前 = 深色实底，已安装 = 浅色描边 */
.badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 11px;
  padding: 3px 12px;
  border-radius: 999px;
  white-space: nowrap;
}

.badge-icon {
  width: 13px;
  height: 13px;
  flex: none;
}

.badge-current {
  background: var(--btn);
  color: #fff;
}

.badge-installed {
  background: var(--bg2);
  border: 1px solid var(--border-strong);
  color: var(--text);
}

/* 卸载中 spinner：红色系，匹配 danger 按钮主题 */
.btn-danger .spin {
  width: 11px;
  height: 11px;
  border-color: color-mix(in srgb, var(--danger) 35%, transparent);
  border-top-color: var(--danger);
  margin-right: 2px;
}
</style>
