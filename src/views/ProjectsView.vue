<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useAppStore } from "../stores/app";
import { useDialogStore } from "../stores/dialog";
import { api } from "../api";

const { t } = useI18n();
const store = useAppStore();
const dialog = useDialogStore();

const newPath = ref("");
const newId = ref("");

// 筛选：关键字（项目名/路径）+ 关联的 JDK 版本
const keyword = ref("");
const filterJdk = ref("");
// 勾选待批量解除的项目路径
const selected = ref<Set<string>>(new Set());

const boundJdkIds = computed(() =>
  [...new Set(store.projects.map((p) => p.jdk))].sort(),
);

// 解绑是否连带删除 .jvmrc：跟随设置项，确认弹窗文案随之切换
const deletesJvmrc = computed(() => store.config?.unbindDeletesJvmrc ?? false);

// 默认选中第一个已安装版本；当前选中的 id 已不在列表中（被卸载、
// 列表刷新变空）时同样归位——否则残留值既匹配不到选项（控件空白）、
// 又会阻止占位项渲染。列表到达前或为空时显示占位提示
watch(
  () => store.installedIds,
  (ids) => {
    if (!newId.value || !ids.includes(newId.value)) {
      newId.value = ids[0] ?? "";
    }
  },
  { immediate: true },
);

const filteredProjects = computed(() => {
  const kw = keyword.value.trim().toLowerCase();
  return store.projects.filter((p) => {
    if (filterJdk.value && p.jdk !== filterJdk.value) return false;
    if (kw && !p.name.toLowerCase().includes(kw) && !p.path.toLowerCase().includes(kw))
      return false;
    return true;
  });
});

const allSelected = computed(
  () =>
    filteredProjects.value.length > 0 &&
    filteredProjects.value.every((p) => selected.value.has(p.path)),
);

function toggleRow(p: { path: string }, e: Event) {
  const on = (e.target as HTMLInputElement).checked;
  const next = new Set(selected.value);
  if (on) next.add(p.path);
  else next.delete(p.path);
  selected.value = next;
}

function toggleAll(e: Event) {
  const on = (e.target as HTMLInputElement).checked;
  selected.value = on
    ? new Set(filteredProjects.value.map((p) => p.path))
    : new Set();
}

async function pick() {
  try {
    const p = await apiPick();
    if (p) newPath.value = p.replace(/\\/g, "/");
  } catch (e) {
    store.log("error", `选择目录失败: ${e}`);
  }
}

async function apiPick(): Promise<string | null> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string | null>("pick_folder");
}

async function add() {
  if (!newPath.value || !newId.value) {
    store.log("warn", t("projects.needPath"));
    return;
  }
  try {
    await store.bindProject(newPath.value, newId.value);
    newPath.value = "";
    newId.value = "";
  } catch (e) {
    store.log("error", `绑定失败: ${e}`);
  }
}

async function open(path: string) {
  try {
    await api.openDir(path);
  } catch (e) {
    store.log("error", `打开目录失败: ${e}`);
  }
}

async function remove(path: string) {
  const ok = await dialog.confirm({
    title: t("projects.unbind"),
    message: t(
      deletesJvmrc.value ? "projects.confirmUnbindDelete" : "projects.confirmUnbind",
    ),
    kind: "warning",
    danger: true,
    confirmText: t("projects.unbind"),
  });
  if (ok) await store.unbindProject(path);
}

async function removeSelected() {
  const paths = [...selected.value];
  if (paths.length === 0) return;
  const ok = await dialog.confirm({
    title: t("projects.unbindSelected"),
    message: t(
      deletesJvmrc.value
        ? "projects.confirmUnbindManyDelete"
        : "projects.confirmUnbindMany",
      { n: paths.length },
    ),
    kind: "warning",
    danger: true,
    confirmText: t("projects.unbindSelected"),
  });
  if (!ok) return;
  for (const path of paths) {
    try {
      await store.unbindProject(path);
    } catch {
      // 单个失败继续其余（unbindProject 已把错误写入日志）
    }
  }
  selected.value = new Set();
}

onMounted(async () => {
  try {
    await store.refreshProjects();
  } catch (e) {
    store.log("error", `加载项目关联失败: ${e}`);
  }
});
</script>

<template>
  <div class="view">
    <div class="page-head">
      <h2>{{ t("projects.title") }}</h2>
      <span class="muted">{{ t("projects.count", { n: store.projects.length }) }}</span>
      <div class="spacer" />
      <button class="btn-ghost btn-sm" @click="store.refreshProjects()">
        {{ t("common.refresh") }}
      </button>
    </div>

    <div class="bind-card">
      <div class="bind-row">
        <button class="btn-ghost btn-sm" @click="pick">{{ t("projects.pick") }}</button>
        <input v-model="newPath" type="text" class="ctl mono grow" :placeholder="t('projects.pathPlaceholder')" />
        <!-- 原生 select 无 placeholder：没有选中值时就显示空白 -->
        <select v-model="newId" class="ctl w-jdk">
          <option v-for="id in store.installedIds" :key="id" :value="id">{{ id }}</option>
        </select>
        <button class="btn-primary btn-sm" @click="add">{{ t("projects.bind") }}</button>
      </div>
    </div>

    <div v-if="store.projects.length === 0" class="empty">
      {{ t("projects.empty") }}
    </div>

    <template v-else>
      <div class="filters">
        <input
          v-model="keyword"
          type="text"
          class="ctl w-kw"
          :placeholder="t('projects.searchPlaceholder')"
        />
        <select v-model="filterJdk" class="ctl w-jdk">
          <option value="">{{ t("projects.allJdks") }}</option>
          <option v-for="id in boundJdkIds" :key="id" :value="id">{{ id }}</option>
        </select>
        <span class="muted">{{ t("projects.count", { n: filteredProjects.length }) }}</span>
        <div class="spacer" />
        <button v-if="selected.size > 0" class="btn-danger btn-sm" @click="removeSelected">
          {{ t("projects.unbindSelected") }} ({{ selected.size }})
        </button>
      </div>

      <div v-if="filteredProjects.length === 0" class="empty">
        {{ t("projects.noMatch") }}
      </div>

      <div v-else class="table-scroll">
        <table class="table">
          <thead>
            <tr>
              <th class="w-check">
                <input
                  type="checkbox"
                  :checked="allSelected"
                  :title="t('projects.unbindSelected')"
                  @change="toggleAll"
                />
              </th>
              <th class="w-name">{{ t("projects.colName") }}</th>
              <th>{{ t("projects.colJdk") }}</th>
              <th>{{ t("projects.colPath") }}</th>
              <th>{{ t("projects.colLastUsed") }}</th>
              <th class="w-actions">{{ t("projects.colAction") }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="p in filteredProjects" :key="p.path">
              <td class="w-check">
                <input
                  type="checkbox"
                  :checked="selected.has(p.path)"
                  @change="toggleRow(p, $event)"
                />
              </td>
              <td class="w-name"><span class="name">{{ p.name }}</span></td>
              <td><span class="mono">{{ p.jdk }}</span></td>
              <td class="mono path" :title="p.path">{{ p.path }}</td>
              <td class="muted">{{ p.lastUsedAt.replace("T", " ").slice(0, 19) }}</td>
              <td class="actions">
                <button class="btn-ghost btn-sm" @click="open(p.path)">
                  {{ t("projects.open") }}
                </button>
                <button class="btn-danger btn-sm" @click="remove(p.path)">
                  {{ t("projects.unbind") }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>
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

.bind-card {
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 16px;
  margin-bottom: 14px;
}

.bind-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.ctl {
  width: 320px;
}

.grow {
  flex: 1;
  min-width: 200px;
}

.w-jdk {
  width: 260px;
}

/* 绑定行的 JDK 下拉窄一些，省出的宽度由前面的路径输入框（flex:1）吸收 */
.bind-card .w-jdk {
  width: 180px;
}

/* 筛选栏 */
.filters {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 14px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 10px 16px;
  flex-wrap: wrap;
}

.w-kw {
  width: 240px;
}

/* 勾选列 */
.w-check {
  width: 36px;
  text-align: center;
}

.empty {
  border: 1px dashed var(--border-strong);
  border-radius: var(--radius);
  padding: 50px 30px;
  text-align: center;
  color: var(--text2);
  line-height: 1.8;
}

.name {
  font-weight: 500;
}

.path {
  max-width: 320px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text2);
}

.w-name {
  min-width: 120px;
}

.w-actions {
  width: 150px;
}

.actions {
  white-space: nowrap;
}

/* 行内按钮间距（与已安装页一致） */
.actions .btn-sm {
  margin-right: 6px;
}

.actions .btn-sm:last-child {
  margin-right: 0;
}
</style>
