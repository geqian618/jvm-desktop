import { defineStore } from "pinia";

export type DialogKind = "info" | "success" | "warning" | "error";

export interface DialogOptions {
  title: string;
  message: string;
  kind?: DialogKind;
  /** 确认按钮文案，默认「确认」 */
  confirmText?: string;
  /** 取消按钮文案，默认「取消」 */
  cancelText?: string;
  /** 破坏性操作（卸载/解除关联等）：确认按钮显示为红色 */
  danger?: boolean;
}

interface PendingDialog {
  opts: DialogOptions;
  /** alert 只有确认按钮，confirm 才显示取消 */
  showCancel: boolean;
  resolve: (ok: boolean) => void;
}

/**
 * 应用内弹窗（替代 plugin-dialog 的原生 message/ask，原生框无法定制样式）：
 * Promise 风格 API，视图层 await 后继续原逻辑；多个弹窗按打开顺序排队展示。
 */
export const useDialogStore = defineStore("dialog", {
  state: () => ({
    queue: [] as PendingDialog[],
  }),

  getters: {
    current: (state): PendingDialog | null => state.queue[0] ?? null,
  },

  actions: {
    /** 提示框：只有确认按钮 */
    async alert(opts: DialogOptions): Promise<void> {
      await this.open(opts, false);
    },
    /** 确认框：确认 + 取消，resolve 是否点了确认 */
    confirm(opts: DialogOptions): Promise<boolean> {
      return this.open(opts, true);
    },
    open(opts: DialogOptions, showCancel: boolean): Promise<boolean> {
      return new Promise((resolve) => {
        this.queue.push({ opts, showCancel, resolve });
      });
    },
    /** 关闭当前弹窗并回传结果（由 AppDialog 调用） */
    settle(ok: boolean) {
      const cur = this.queue.shift();
      cur?.resolve(ok);
    },
  },
});
