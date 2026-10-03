<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useDialogStore } from "../stores/dialog";

const { t } = useI18n();
const store = useDialogStore();
const confirmBtn = ref<HTMLButtonElement | null>(null);

const kind = computed(() => store.current?.opts.kind ?? "info");

// 弹窗出现（含排队中的下一条）时把焦点移到确认按钮，回车/ESC 即可操作
watch(
  () => store.current,
  async (cur) => {
    if (cur) {
      await nextTick();
      confirmBtn.value?.focus();
    }
  },
);

function onKeydown(e: KeyboardEvent) {
  if (!store.current) return;
  if (e.key === "Escape") {
    e.preventDefault();
    store.settle(false);
  } else if (e.key === "Enter") {
    // 焦点在弹窗按钮上时走原生 click（可能点的是取消），避免双重 settle
    const el = document.activeElement as HTMLElement | null;
    if (el?.tagName === "BUTTON" && el.closest(".dlg-card")) return;
    e.preventDefault();
    store.settle(true);
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onUnmounted(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <Teleport to="body">
    <Transition name="dlg">
      <div
        v-if="store.current"
        class="dlg-overlay"
        @mousedown.self="store.settle(false)"
      >
        <div
          class="dlg-card"
          role="alertdialog"
          aria-modal="true"
          :aria-label="store.current?.opts.title"
        >
          <div class="dlg-head">
            <span class="dlg-icon" :class="`is-${kind}`">
              <svg v-if="kind === 'warning'" viewBox="0 0 16 16" aria-hidden="true">
                <path
                  d="M8 2.6 14.3 13.2H1.7L8 2.6Z"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.4"
                  stroke-linejoin="round"
                />
                <path
                  d="M8 6.4v3.1"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
                <circle cx="8" cy="11.3" r="0.9" fill="currentColor" />
              </svg>
              <svg v-else-if="kind === 'error'" viewBox="0 0 16 16" aria-hidden="true">
                <circle cx="8" cy="8" r="6.4" fill="none" stroke="currentColor" stroke-width="1.4" />
                <path
                  d="M5.9 5.9l4.2 4.2M10.1 5.9 5.9 10.1"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
              </svg>
              <svg v-else-if="kind === 'success'" viewBox="0 0 16 16" aria-hidden="true">
                <circle cx="8" cy="8" r="6.4" fill="none" stroke="currentColor" stroke-width="1.4" />
                <path
                  d="M5.1 8.3l2 2 3.8-4.4"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.6"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                />
              </svg>
              <svg v-else viewBox="0 0 16 16" aria-hidden="true">
                <circle cx="8" cy="8" r="6.4" fill="none" stroke="currentColor" stroke-width="1.4" />
                <circle cx="8" cy="5.2" r="0.9" fill="currentColor" />
                <path
                  d="M8 7.4v3.4"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.5"
                  stroke-linecap="round"
                />
              </svg>
            </span>
            <span class="dlg-title">{{ store.current?.opts.title }}</span>
          </div>

          <div class="dlg-message">{{ store.current?.opts.message }}</div>

          <div class="dlg-actions">
            <button
              v-if="store.current?.showCancel"
              class="btn-ghost"
              @click="store.settle(false)"
            >
              {{ store.current?.opts.cancelText || t("common.cancel") }}
            </button>
            <button
              ref="confirmBtn"
              class="dlg-confirm"
              :class="{ danger: store.current?.opts.danger }"
              @click="store.settle(true)"
            >
              {{ store.current?.opts.confirmText || t("common.confirm") }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.dlg-overlay {
  position: fixed;
  inset: 0;
  z-index: 1000;
  background: rgba(31, 35, 41, 0.35);
  display: flex;
  align-items: center;
  justify-content: center;
  /* 略高于正中：贴近 Windows 对话框的视觉位置 */
  padding-bottom: 12vh;
}

.dlg-card {
  width: 420px;
  max-width: calc(100vw - 48px);
  background: var(--bg);
  border: 1px solid var(--border-strong);
  border-radius: 10px;
  box-shadow: 0 16px 48px rgba(31, 35, 41, 0.18);
  padding: 18px 20px 16px;
}

.dlg-head {
  display: flex;
  align-items: center;
  gap: 10px;
}

.dlg-icon {
  flex: none;
  width: 30px;
  height: 30px;
  border-radius: 50%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}

.dlg-icon svg {
  width: 17px;
  height: 17px;
}

.dlg-icon.is-warning {
  color: var(--warn);
  background: rgba(230, 162, 60, 0.13);
}

.dlg-icon.is-error {
  color: var(--danger);
  background: rgba(217, 72, 72, 0.1);
}

.dlg-icon.is-success {
  color: var(--ok);
  background: rgba(29, 154, 108, 0.1);
}

.dlg-icon.is-info {
  color: var(--accent);
  background: rgba(47, 111, 237, 0.1);
}

.dlg-title {
  font-size: 14px;
  font-weight: 600;
}

/* 与标题文字对齐（icon 30px + gap 10px）；\n 换行渲染，错误信息可选中复制 */
.dlg-message {
  margin: 12px 0 0 40px;
  font-size: 12.5px;
  line-height: 1.8;
  color: var(--text);
  white-space: pre-line;
  user-select: text;
  max-height: 42vh;
  overflow-y: auto;
}

.dlg-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 18px;
}

.dlg-actions .btn-ghost {
  padding: 5px 16px;
  font-size: 12.5px;
}

.dlg-confirm {
  background: var(--btn);
  color: #fff;
  padding: 5px 18px;
  font-size: 12.5px;
}

.dlg-confirm:hover {
  background: var(--btn-hover);
}

.dlg-confirm.danger {
  background: var(--danger);
}

.dlg-confirm.danger:hover {
  background: #c23c3c;
}

.dlg-enter-active {
  transition: opacity 0.16s ease;
}

.dlg-leave-active {
  transition: opacity 0.14s ease;
}

.dlg-enter-from,
.dlg-leave-to {
  opacity: 0;
}

.dlg-enter-active .dlg-card {
  animation: dlg-pop 0.18s cubic-bezier(0.2, 0.8, 0.3, 1.15);
}

@keyframes dlg-pop {
  from {
    transform: scale(0.94) translateY(6px);
  }
  to {
    transform: scale(1) translateY(0);
  }
}
</style>
