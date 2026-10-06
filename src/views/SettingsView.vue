<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { open } from "@tauri-apps/plugin-dialog";
import { CONFIG_FALLBACK, useAppStore } from "../stores/app";
import { useDialogStore } from "../stores/dialog";
import { api } from "../api";
import type { HealthIssue, PathDefaults, RegistryPreset } from "../types";

const { t } = useI18n();
const store = useAppStore();
const dialog = useDialogStore();

// 与后端 jvm-core/src/source/mod.rs 的官方源一致；删除当前使用的自定义源时回落到此地址
const FOOJAY_URL = "https://api.foojay.io/disco/v3.0";

// 直接绑定 store 中的配置：改动即时写入内存 store（切换页面绝不丢失），
// 落盘由 store 层的 config 深度 watcher 防抖完成（见 stores/app.ts）。
const cfg = computed(() => store.config ?? CONFIG_FALLBACK);

const isWindows = computed(() => store.platform?.os === "windows");

// 保存成功的提示（store 落盘后打点）
const autoSaved = ref(false);
let hintTimer: ReturnType<typeof setTimeout> | null = null;
watch(
  () => store.configSavedAt,
  (v) => {
    if (!v) return;
    autoSaved.value = true;
    if (hintTimer) clearTimeout(hintTimer);
    hintTimer = setTimeout(() => (autoSaved.value = false), 2000);
  },
);

// 设置留空时后端实际生效的默认目录（输入框占位展示 + 一键填入的来源）
const defaults = ref<PathDefaults | null>(null);

// ---------- 数据目录体检 ----------
// 换根/搬移数据目录后，current 链接、目录覆盖、JAVA_HOME 等绝对路径引用
// 会悬空；进入设置页时检测，发现问题给出「一键修复」。

const healthIssues = ref<HealthIssue[]>([]);
const healthRepairing = ref(false);
const healthRepaired = ref<string[]>([]);

const hasFixableIssue = computed(() => healthIssues.value.some((i) => i.fixable));

async function runHealthCheck() {
  try {
    healthRepaired.value = [];
    healthIssues.value = await api.healthCheck();
  } catch {
    healthIssues.value = []; // 检测失败不打扰用户
  }
}

async function runHealthRepair() {
  healthRepairing.value = true;
  try {
    healthRepaired.value = await api.healthRepair();
    await runHealthCheck();
    // 修复可能清空了失效的目录覆盖，重拉设置同步输入框；
    // current/环境变量重建后本地列表与 JAVA_HOME 展示也要刷新
    await store.reloadConfig();
    await store.refreshLocal();
  } catch (e) {
    dialog.alert({
      title: t("settings.healthFixFailTitle"),
      message: String(e),
    });
  } finally {
    healthRepairing.value = false;
  }
}

onMounted(async () => {
  // CLI 与 GUI 共写 setting.json：进入设置页先重读一次磁盘，
  // 外部（CLI）注册/切换的数据源立即可见，也缩小快照写回的覆盖窗口。
  await store.reloadConfig();
  try {
    // 仅作占位展示与「一键填入」来源，不写回配置：留空 = 跟随后端动态
    // 默认；需要固定路径时用「一键填入」或浏览选择。此前把默认值回填进
    // 空配置会触发自动落盘，把"留空=动态默认"固化成当时的绝对路径。
    defaults.value = await api.getDefaultPaths();
  } catch {
    /* 拿不到就不显示占位与默认按钮 */
  }
  void runHealthCheck();
});

function applyPreset(url: string) {
  cfg.value.registry = url;
}

/// 路径输入框只读，路径变更只能经「浏览…」或「默认」到达这里。
/// 不做自动迁移——万一选错目录会把整个目录的数据一起搬过去；改为
/// 提醒用户手动拷贝需要的数据到新目录，拷什么、拷多少由用户自己决定。
async function changePath(field: "installDir" | "downloadDir", rawTarget: string) {
  const target = rawTarget.trim();
  const from = (cfg.value[field] || "").trim();
  if (!target || target === from) return;
  cfg.value[field] = target;
  if (from) {
    dialog.alert({
      title: t("settings.pathChangedTitle"),
      message: t("settings.pathChangedHint", { from, to: target }),
    });
    store.log("info", t("settings.pathChangedLog", { from, to: target }));
  }
}

async function browseDir(field: "installDir" | "downloadDir") {
  // 已配置路径时从该路径打开，方便确认/微调
  const start = cfg.value[field] || undefined;
  const sel = await open({ directory: true, multiple: false, defaultPath: start });
  if (typeof sel === "string") {
    await changePath(field, sel);
  }
}

function applyDefault(field: "installDir" | "downloadDir") {
  if (!defaults.value) return;
  void changePath(field, defaults.value[field]);
}

// ---------- 自定义数据源 ----------

// 内置源可编辑名称/地址，但不可删除；是否内置由配置里的 builtIn 标记决定
function isBuiltIn(p: RegistryPreset): boolean {
  return p.builtIn === true;
}

const sourceEditor = ref<{ index: number; name: string; url: string } | null>(
  null,
);
const sourceError = ref("");

function openAddSource() {
  sourceError.value = "";
  sourceEditor.value = { index: -1, name: "", url: "" };
}

/** 点击数据源条目：展开编辑配置，再次点击收起 */
function toggleSource(index: number) {
  if (sourceEditor.value?.index === index) {
    cancelSourceEditor();
    return;
  }
  const p = cfg.value.registryPresets[index];
  sourceError.value = "";
  sourceEditor.value = { index, name: p.name, url: p.url };
}

function cancelSourceEditor() {
  sourceEditor.value = null;
  sourceError.value = "";
}

function saveSourceEditor() {
  const ed = sourceEditor.value;
  if (!ed) return;
  const name = ed.name.trim();
  const url = ed.url.trim().replace(/\/+$/, "");
  if (!name || !url) {
    sourceError.value = t("settings.sourceNeeded");
    return;
  }
  if (!/^https?:\/\//i.test(url)) {
    sourceError.value = t("settings.sourceUrlInvalid");
    return;
  }
  if (
    cfg.value.registryPresets.some((p, i) => p.url === url && i !== ed.index)
  ) {
    sourceError.value = t("settings.sourceExists");
    return;
  }
  if (ed.index < 0) {
    cfg.value.registryPresets.push({ name, url });
    cfg.value.registry = url;
  } else {
    const p = cfg.value.registryPresets[ed.index];
    const wasSelected = cfg.value.registry === p.url;
    p.name = name;
    p.url = url;
    if (wasSelected) cfg.value.registry = url;
  }
  cancelSourceEditor();
}

async function removeSource(index: number) {
  const p = cfg.value.registryPresets[index];
  const ok = await dialog.confirm({
    title: t("settings.removeSource"),
    message: t("settings.confirmRemoveSource"),
    kind: "warning",
    danger: true,
    confirmText: t("settings.removeSource"),
  });
  if (!ok) return;
  cfg.value.registryPresets.splice(index, 1);
  if (sourceEditor.value?.index === index) cancelSourceEditor();
  if (cfg.value.registry === p.url) {
    cfg.value.registry = FOOJAY_URL;
  }
}
</script>

<template>
  <div class="settings view">
    <div class="page-head">
      <h2>{{ t("settings.title") }}</h2>
      <div class="spacer" />
      <span v-if="autoSaved" class="muted">{{ t("settings.autosaved") }}</span>
    </div>

    <div class="settings-scroll">
      <!-- 数据目录体检横幅：发现悬空引用时置顶提示，可一键修复 -->
      <section v-if="healthIssues.length" class="card health-banner">
        <div class="health-head">
          <span class="health-title">
            {{ t("settings.healthTitle", { n: healthIssues.length }) }}
          </span>
          <button
            v-if="hasFixableIssue"
            class="btn-primary btn-sm"
            :disabled="healthRepairing"
            @click="runHealthRepair"
          >
            {{ healthRepairing ? t("settings.healthFixing") : t("settings.healthFix") }}
          </button>
        </div>
        <ul class="health-list">
          <li v-for="issue in healthIssues" :key="issue.code + issue.message">
            <span class="health-msg">{{ issue.message }}</span>
            <span v-if="!issue.fixable" class="health-manual">
              {{ t("settings.healthManual") }}
            </span>
          </li>
        </ul>
        <p v-if="healthRepaired.length" class="health-done">
          {{ t("settings.healthFixed") }}{{ healthRepaired.join("；") }}
        </p>
      </section>

      <section class="card">
      <h3>{{ t("settings.general") }}</h3>

      <div class="row">
        <div class="row-main">
          <div class="row-title">{{ t("settings.language") }}</div>
          <div class="row-desc">{{ t("settings.languageHint") }}</div>
        </div>
        <div class="row-control">
          <select v-model="cfg.language" class="ctl ctl-select">
            <option value="zh-CN">中文</option>
            <option value="en-US">English</option>
          </select>
        </div>
      </div>

      <div class="row">
        <div class="row-main">
          <div class="row-title">{{ t("settings.installDir") }}</div>
          <div class="row-desc">{{ t("settings.installDirHint") }}</div>
        </div>
        <div class="row-control">
          <div class="ctl-group">
            <input
              v-model="cfg.installDir"
              type="text"
              class="ctl mono"
              readonly
              :placeholder="defaults?.installDir || ''"
            />
            <button
              class="btn-ghost btn-sm"
              :disabled="!defaults"
              @click="applyDefault('installDir')"
            >
              {{ t("settings.useDefault") }}
            </button>
            <button class="btn-ghost btn-sm" @click="browseDir('installDir')">
              {{ t("settings.browse") }}
            </button>
          </div>
        </div>
      </div>

      <div class="row">
        <div class="row-main">
          <div class="row-title">{{ t("settings.downloadDir") }}</div>
          <div class="row-desc">{{ t("settings.downloadDirHint") }}</div>
        </div>
        <div class="row-control">
          <div class="ctl-group">
            <input
              v-model="cfg.downloadDir"
              type="text"
              class="ctl mono"
              readonly
              :placeholder="defaults?.downloadDir || ''"
            />
            <button
              class="btn-ghost btn-sm"
              :disabled="!defaults"
              @click="applyDefault('downloadDir')"
            >
              {{ t("settings.useDefault") }}
            </button>
            <button class="btn-ghost btn-sm" @click="browseDir('downloadDir')">
              {{ t("settings.browse") }}
            </button>
          </div>
        </div>
      </div>

      <div class="row">
        <div class="row-main">
          <div class="row-title">{{ t("settings.autoClean") }}</div>
          <div class="row-desc">{{ t("settings.autoCleanHint") }}</div>
        </div>
        <div class="row-control">
          <label class="switch">
            <input v-model="cfg.autoCleanDownload" type="checkbox" />
            <span class="track" />
          </label>
        </div>
      </div>

      <div class="row">
        <div class="row-main">
          <div class="row-title">{{ t("settings.unbindDelete") }}</div>
          <div class="row-desc">{{ t("settings.unbindDeleteHint") }}</div>
        </div>
        <div class="row-control">
          <label class="switch">
            <input v-model="cfg.unbindDeletesJvmrc" type="checkbox" />
            <span class="track" />
          </label>
        </div>
      </div>

      <div class="row">
        <div class="row-main">
          <div class="row-title">{{ t("settings.registry") }}</div>
          <div class="row-desc">{{ t("settings.registryHint") }}</div>
        </div>
        <div class="row-control">
          <div class="source-list">
            <div
              v-for="(p, i) in cfg.registryPresets"
              :key="i"
              class="source-item"
              :class="{ active: cfg.registry === p.url }"
            >
              <button type="button" class="source-head" @click="toggleSource(i)">
                <span
                  class="source-radio"
                  :class="{ on: cfg.registry === p.url }"
                  :title="t('settings.useSource')"
                  @click.stop="applyPreset(p.url)"
                />
                <span class="source-name">{{ p.name }}</span>
                <span v-if="cfg.registry === p.url" class="pill-in-use">
                  {{ t("settings.inUse") }}
                </span>
                <span class="source-url mono">{{ p.url }}</span>
                <span
                  class="caret"
                  :class="{ open: sourceEditor && sourceEditor.index === i }"
                />
              </button>
              <form
                v-if="sourceEditor && sourceEditor.index === i"
                class="source-editor"
                @submit.prevent="saveSourceEditor"
              >
                <label>
                  <span class="field-label">{{ t("settings.sourceName") }}</span>
                  <input
                    v-model="sourceEditor.name"
                    type="text"
                    :placeholder="t('settings.sourceNamePh')"
                  />
                </label>
                <label>
                  <span class="field-label">{{ t("settings.sourceUrl") }}</span>
                  <input
                    v-model="sourceEditor.url"
                    type="text"
                    class="mono"
                    :placeholder="t('settings.sourceUrlPh')"
                  />
                </label>
                <span v-if="sourceError" class="error-text">{{ sourceError }}</span>
                <div class="form-actions">
                  <button
                    v-if="!isBuiltIn(p)"
                    type="button"
                    class="btn-ghost btn-sm btn-remove"
                    @click="removeSource(i)"
                  >
                    {{ t("settings.remove") }}
                  </button>
                  <span class="flex-spacer" />
                  <button type="submit" class="btn-primary btn-sm">
                    {{ t("settings.sourceSave") }}
                  </button>
                  <button type="button" class="btn-ghost btn-sm" @click="cancelSourceEditor">
                    {{ t("common.cancel") }}
                  </button>
                </div>
              </form>
            </div>

            <form
              v-if="sourceEditor && sourceEditor.index < 0"
              class="source-editor"
              @submit.prevent="saveSourceEditor"
            >
              <label>
                <span class="field-label">{{ t("settings.sourceName") }}</span>
                <input
                  v-model="sourceEditor.name"
                  type="text"
                  :placeholder="t('settings.sourceNamePh')"
                />
              </label>
              <label>
                <span class="field-label">{{ t("settings.sourceUrl") }}</span>
                <input
                  v-model="sourceEditor.url"
                  type="text"
                  class="mono"
                  :placeholder="t('settings.sourceUrlPh')"
                />
              </label>
              <span v-if="sourceError" class="error-text">{{ sourceError }}</span>
              <div class="form-actions">
                <span class="flex-spacer" />
                <button type="submit" class="btn-primary btn-sm">
                  {{ t("settings.sourceSave") }}
                </button>
                <button type="button" class="btn-ghost btn-sm" @click="cancelSourceEditor">
                  {{ t("common.cancel") }}
                </button>
              </div>
            </form>

            <button type="button" class="source-add" @click="openAddSource">
              + {{ t("settings.addSource") }}
            </button>
          </div>
        </div>
      </div>

      <div class="row">
        <div class="row-main">
          <div class="row-title">{{ t("settings.proxy") }}</div>
          <div class="row-desc">{{ t("settings.proxyHint") }}</div>
        </div>
        <div class="row-control">
          <div class="ctl-group">
            <input
              v-model="cfg.proxy"
              type="text"
              class="ctl mono"
              placeholder="http://127.0.0.1:7890"
            />
            <button class="btn-ghost btn-sm" @click="cfg.proxy = ''">
              {{ t("settings.useDefault") }}
            </button>
          </div>
        </div>
      </div>
    </section>

    <section class="card">
      <h3>{{ t("settings.switchTitle") }}</h3>
      <p class="note-text">
        {{ isWindows ? t("settings.switchHint") : t("settings.switchHintUnix") }}
      </p>
    </section>
    </div>
  </div>
</template>

<style scoped>
/* 卡片在独立区域内滚动，页头（标题+保存按钮）保持固定 */
.settings-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.page-head {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 16px;
}

h2 {
  font-size: 17px;
  font-weight: 600;
}

.spacer {
  flex: 1;
}

/* 体检横幅：发现问题置顶展示，警告色调 */
.health-banner {
  border-color: rgba(214, 158, 46, 0.5);
  background: rgba(214, 158, 46, 0.06);
}

.health-head {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-top: 14px;
}

.health-title {
  flex: 1;
  font-size: 13.5px;
  font-weight: 600;
}

.health-list {
  margin: 10px 0 0;
  padding: 0;
  list-style: none;
}

.health-list li {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 5px 0;
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--text2);
}

.health-msg {
  flex: 1;
  min-width: 0;
}

.health-manual {
  flex-shrink: 0;
  font-size: 11px;
  color: var(--danger);
}

.health-done {
  margin: 10px 0 6px;
  font-size: 12px;
  line-height: 1.7;
  color: var(--ok);
}

/* 分组卡片：撑满窗口内容区宽度 */
.card {
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 6px 28px 10px;
}

.card + .card {
  margin-top: 16px;
}

.card h3 {
  font-size: 15px;
  font-weight: 700;
  padding: 18px 0 4px;
}

/* 设置行：左侧标题+描述，右侧控件，行间浅分隔线 */
.row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 16px 40px;
  padding: 20px 0;
  border-bottom: 1px solid var(--border);
}

.row:last-child {
  border-bottom: none;
}

.row-main {
  flex: 1;
  min-width: 260px;
  display: flex;
  flex-direction: column;
  gap: 7px;
}

.row-title {
  font-size: 13.5px;
  font-weight: 600;
  color: var(--text);
}

.row-desc {
  font-size: 12px;
  color: var(--text2);
  line-height: 1.7;
}

.row-control {
  flex-shrink: 0;
  display: flex;
  flex-direction: column;
  align-items: flex-end;
  gap: 8px;
}

.ctl-group {
  display: flex;
  align-items: center;
  gap: 8px;
}

.ctl-group .btn-ghost {
  flex-shrink: 0;
}

.ctl {
  width: 400px;
  max-width: 100%;
}

.ctl-select {
  width: 160px;
}

.area {
  resize: vertical;
  line-height: 1.6;
}

/* 数据源列表：点击条目展开配置，左侧圆点选择使用哪个源 */
.source-list {
  width: 400px;
  max-width: 100%;
  border: 1px solid var(--border-strong);
  border-radius: 8px;
  background: var(--bg);
  overflow: hidden;
}

.source-list > * + * {
  border-top: 1px solid var(--border);
}

.source-head {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 9px 12px;
  background: transparent;
  border-radius: 0;
  text-align: left;
  font-size: 12.5px;
  color: var(--text);
  transition: background 0.12s;
}

.source-head:hover {
  background: var(--bg2);
}

.source-item.active .source-head {
  background: var(--bg2);
}

/* 选择圆点：实心 = 当前使用 */
.source-radio {
  flex-shrink: 0;
  width: 13px;
  height: 13px;
  border-radius: 50%;
  border: 1.5px solid #c9cdd4;
  background: var(--bg);
  transition:
    border-color 0.12s,
    background 0.12s,
    box-shadow 0.12s;
}

.source-radio:hover {
  border-color: var(--btn);
}

.source-radio.on {
  border-color: var(--btn);
  background: var(--btn);
  box-shadow: inset 0 0 0 3px var(--bg);
}

.source-name {
  flex-shrink: 0;
  font-weight: 500;
}

.source-item.active .source-name {
  font-weight: 600;
}

.pill-in-use {
  flex-shrink: 0;
  font-size: 10px;
  line-height: 1;
  padding: 3px 8px;
  border-radius: 999px;
  color: var(--ok);
  border: 1px solid rgba(29, 154, 108, 0.35);
  background: rgba(29, 154, 108, 0.08);
}

.source-url {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  text-align: right;
  font-size: 11.5px;
  color: var(--text2);
}

/* 展开/收起箭头：CSS 画的 V 形，随展开状态旋转 */
.caret {
  flex-shrink: 0;
  width: 7px;
  height: 7px;
  margin-right: 2px;
  border-right: 1.5px solid var(--text2);
  border-bottom: 1.5px solid var(--text2);
  transform: rotate(-45deg);
  transition: transform 0.15s;
}

.caret.open {
  transform: rotate(45deg);
}

/* 列表底部的「添加数据源」行 */
.source-add {
  display: block;
  width: 100%;
  padding: 8px 12px;
  background: transparent;
  border-radius: 0;
  text-align: center;
  font-size: 12px;
  color: var(--text2);
  transition: background 0.12s, color 0.12s;
}

.source-add:hover {
  background: var(--bg2);
  color: var(--text);
}

.btn-remove:hover:not(:disabled) {
  color: var(--danger);
  border-color: var(--danger);
  background: transparent;
}

/* 展开的配置面板：名称、地址、按钮各占一行 */
.source-editor {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 14px 14px;
  background: var(--bg2);
  border-top: 1px solid var(--border);
}

.source-editor label {
  display: flex;
  align-items: center;
  gap: 8px;
}

.field-label {
  flex-shrink: 0;
  min-width: 60px;
  text-align: right;
}

.source-editor label input {
  flex: 1;
  min-width: 0;
}

.form-actions {
  display: flex;
  align-items: center;
  gap: 8px;
}

.flex-spacer {
  flex: 1;
}

.error-text {
  color: var(--danger);
  font-size: 12px;
}

/* 开关：紧凑样式，选中 = 深色 */
.switch {
  position: relative;
  display: inline-block;
  width: 38px;
  height: 20px;
  cursor: pointer;
}

.switch input {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  opacity: 0;
  margin: 0;
  cursor: pointer;
}

.switch .track {
  position: absolute;
  inset: 0;
  border-radius: 999px;
  background: #c9cdd4;
  transition: background 0.15s;
}

.switch .track::after {
  content: "";
  position: absolute;
  top: 2px;
  left: 2px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: #fff;
  box-shadow: 0 1px 2px rgba(0, 0, 0, 0.2);
  transition: transform 0.15s;
}

.switch input:checked + .track {
  background: var(--btn);
}

.switch input:checked + .track::after {
  transform: translateX(18px);
}

.note-text {
  padding: 8px 0 14px;
  font-size: 12px;
  color: var(--text2);
  line-height: 1.8;
}
</style>
