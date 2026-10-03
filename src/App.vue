<script setup lang="ts">
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useAppStore } from "./stores/app";
import InstalledView from "./views/InstalledView.vue";
import AvailableView from "./views/AvailableView.vue";
import ProjectsView from "./views/ProjectsView.vue";
import SettingsView from "./views/SettingsView.vue";
import LogView from "./views/LogView.vue";
import AppDialog from "./components/AppDialog.vue";
import appIcon from "./assets/icon.png";

const { t } = useI18n();
const store = useAppStore();
const activeTab = ref<"available" | "installed" | "projects" | "settings" | "logs">("available");
const ready = ref(false);

// 自定义标题栏的窗口控制
// 浏览器直开 dev 页面时没有 Tauri 环境：降级为不可控制窗口，不阻塞渲染
const win = getCurrentWindowSafe();
const isMaximized = ref(false);
function minimizeWin() {
  win?.minimize();
}
function toggleMaximizeWin() {
  win?.toggleMaximize();
}
function closeWin() {
  win?.close();
}

function getCurrentWindowSafe(): ReturnType<typeof getCurrentWindow> | null {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

// 自定义标题栏：按住空白处拖动窗口（data-tauri-drag-region 只在按到元素
// 本身时生效，标题栏几乎被子元素铺满，所以改用手动 startDragging）
function onTitlebarMouseDown(e: MouseEvent) {
  if (e.button !== 0) return;
  const el = e.target as HTMLElement;
  if (el.closest("button, input, select, textarea, a")) return;
  win
    ?.startDragging()
    .catch((err) => store.log("error", `窗口拖动失败: ${err}`));
}

function onTitlebarDblClick(e: MouseEvent) {
  const el = e.target as HTMLElement;
  if (el.closest("button, input, select, textarea, a")) return;
  toggleMaximizeWin();
}

onMounted(async () => {
  try {
    isMaximized.value = (await win?.isMaximized()) ?? false;
    win?.onResized(async () => {
      isMaximized.value = (await win?.isMaximized()) ?? false;
    });
  } catch {
    /* 忽略：窗口状态获取失败不影响功能 */
  }
  try {
    await store.init();
  } catch {
    // 开发模式下 Rust 重建窗口期的 invoke 可能整体失败：重试一次
    try {
      await new Promise((r) => setTimeout(r, 1500));
      await store.init();
    } catch (e) {
      console.error("init failed twice", e);
      store.log("error", `初始化失败: ${e}（点击各页面的「刷新」可重试）`);
    }
  } finally {
    ready.value = true;
  }
});
</script>

<template>
  <div class="app">
    <header
      class="topbar"
      @mousedown="onTitlebarMouseDown"
      @dblclick="onTitlebarDblClick"
    >
      <div class="brand">
        <img class="logo" :src="appIcon" alt="JVM Desktop" draggable="false" />
        <span class="brand-name">JVM Desktop</span>
      </div>
      <nav class="tabs">
        <button
          class="tab"
          :class="{ active: activeTab === 'available' }"
          @click="activeTab = 'available'"
        >
          {{ t("nav.all") }}
        </button>
        <button
          class="tab"
          :class="{ active: activeTab === 'installed' }"
          @click="activeTab = 'installed'"
        >
          {{ t("nav.installed") }}
        </button>
        <button
          class="tab"
          :class="{ active: activeTab === 'projects' }"
          @click="activeTab = 'projects'"
        >
          {{ t("nav.projects") }}
        </button>
        <button
          class="tab"
          :class="{ active: activeTab === 'settings' }"
          @click="activeTab = 'settings'"
        >
          {{ t("nav.settings") }}
        </button>
        <button
          class="tab"
          :class="{ active: activeTab === 'logs' }"
          @click="activeTab = 'logs'"
        >
          {{ t("nav.logs") }}
        </button>
      </nav>
      <div class="win-controls">
        <button class="win-btn" @click="minimizeWin">
          <svg width="10" height="10" viewBox="0 0 10 10">
            <path d="M1 5h8" stroke="currentColor" stroke-width="1" />
          </svg>
        </button>
        <button class="win-btn" @click="toggleMaximizeWin">
          <svg v-if="!isMaximized" width="10" height="10" viewBox="0 0 10 10">
            <rect x="1.5" y="1.5" width="7" height="7" fill="none" stroke="currentColor" />
          </svg>
          <svg v-else width="10" height="10" viewBox="0 0 10 10">
            <rect x="1.5" y="3.5" width="5" height="5" fill="none" stroke="currentColor" />
            <path d="M3.5 3.5V1.5h5v5h-2" fill="none" stroke="currentColor" />
          </svg>
        </button>
        <button class="win-btn close" @click="closeWin">
          <svg width="10" height="10" viewBox="0 0 10 10">
            <path d="M1.5 1.5l7 7M8.5 1.5l-7 7" stroke="currentColor" />
          </svg>
        </button>
      </div>
    </header>

    <main class="content">
      <template v-if="ready">
        <AvailableView v-if="activeTab === 'available'" />
        <InstalledView v-else-if="activeTab === 'installed'" />
        <ProjectsView v-else-if="activeTab === 'projects'" />
        <SettingsView v-else-if="activeTab === 'settings'" />
        <LogView v-else-if="activeTab === 'logs'" />
      </template>
      <div v-else class="loading-page"><span class="spin" /></div>
    </main>

    <AppDialog />
  </div>
</template>

<style scoped>
.app {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.topbar {
  display: flex;
  align-items: stretch;
  gap: 28px;
  padding: 0 0 0 20px;
  height: 38px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
  user-select: none;
}

.brand {
  display: flex;
  align-items: center;
  gap: 8px;
}

.logo {
  width: 20px;
  height: 20px;
  object-fit: contain;
  -webkit-user-drag: none;
  /* 光学对齐：杯身重心比文字中心低约 1px（实测），上移补偿 */
  transform: translateY(-1px);
}

.brand-name {
  font-weight: 600;
  font-size: 13px;
}

.tabs {
  display: flex;
  align-items: stretch;
  gap: 6px;
}

.tab {
  background: transparent;
  color: var(--text2);
  font-size: 13px;
  padding: 0 6px;
  border-radius: 0;
  border-bottom: 2px solid transparent;
  margin-bottom: -1px;
}

.tab:hover {
  color: var(--text);
}

.tab.active {
  color: var(--text);
  font-weight: 600;
  border-bottom-color: var(--btn);
}

.win-controls {
  display: flex;
  align-self: stretch;
  margin-left: auto;
}

.win-btn {
  width: 42px;
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border-radius: 0;
  color: var(--text2);
}

.win-btn:hover {
  background: var(--bg2);
  color: var(--text);
}

.win-btn.close:hover {
  background: var(--danger);
  color: #fff;
}

.content {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  padding: 16px 24px 24px;
}

.loading-page {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
}
</style>
