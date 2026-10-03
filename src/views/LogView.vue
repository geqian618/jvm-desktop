<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useAppStore } from "../stores/app";

const { t } = useI18n();
const store = useAppStore();
const scrollEl = ref<HTMLElement | null>(null);
const copied = ref(false);
let copiedTimer: ReturnType<typeof setTimeout> | null = null;

// 新日志到达时自动滚到底部
watch(
  () => store.logs.length,
  async () => {
    await nextTick();
    const el = scrollEl.value;
    if (el) el.scrollTop = el.scrollHeight;
  },
);

function formatTime(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

function clearLogs() {
  store.logs = [];
}

async function copyAll() {
  const text = store.logs
    .map((l) => `[${formatTime(l.ts)}] [${l.level.toUpperCase()}] ${l.message}`)
    .join("\n");
  try {
    await navigator.clipboard.writeText(text);
    copied.value = true;
    if (copiedTimer) clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => (copied.value = false), 2000);
  } catch {
    /* 剪贴板不可用时静默忽略 */
  }
}
</script>

<template>
  <div class="view">
    <div class="page-head">
      <h2>{{ t("logs.title") }}</h2>
      <div class="spacer" />
      <button class="btn-ghost btn-sm" @click="copyAll">
        {{ copied ? t("logs.copied") : t("logs.copy") }}
      </button>
      <button class="btn-ghost btn-sm" @click="clearLogs">
        {{ t("logs.clear") }}
      </button>
    </div>

    <div ref="scrollEl" class="log-scroll">
      <div v-if="store.logs.length === 0" class="log-empty">
        {{ t("logs.empty") }}
      </div>
      <div
        v-for="(l, i) in store.logs"
        :key="i"
        class="log-line"
        :class="l.level"
      >
        <span class="log-time">{{ formatTime(l.ts) }}</span>
        <span class="log-level">{{ l.level.toUpperCase() }}</span>
        <span class="log-msg">{{ l.message }}</span>
      </div>
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

.log-scroll {
  flex: 1;
  min-height: 0;
  overflow: auto;
  background: var(--bg2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 10px 12px;
  font-family: Consolas, "Courier New", monospace;
  font-size: 12px;
  line-height: 1.9;
  user-select: text;
}

.log-empty {
  color: var(--text2);
  text-align: center;
  padding: 40px 0;
}

.log-line {
  white-space: pre-wrap;
  word-break: break-all;
}

.log-time {
  color: var(--text2);
  margin-right: 8px;
}

.log-level {
  display: inline-block;
  min-width: 52px;
  margin-right: 8px;
  font-weight: 600;
}

.log-line.info .log-level {
  color: var(--text2);
}

.log-line.success .log-level {
  color: var(--ok);
}

.log-line.warn .log-level {
  color: #b8860b;
}

.log-line.error .log-level {
  color: var(--danger);
}

.log-line.error .log-msg {
  color: var(--danger);
}
</style>
