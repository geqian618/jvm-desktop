import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { i18n } from "./i18n";
import "./style.css";

// 把错误文本转发到后端 dev 终端（诊断用； invoke 失败时静默忽略）
function reportError(text: string) {
  console.error(text);
  try {
    import("@tauri-apps/api/core")
      .then(({ invoke }) => invoke("debug_log", { message: text }))
      .catch(() => undefined);
  } catch {
    /* ignore */
  }
}

const app = createApp(App).use(createPinia()).use(i18n);

// 全局错误兜底：把 Vue 渲染错误与未捕获的 Promise 异常广播出去，
// 由 store 写入「日志」页——出现问题时界面保留最后的视图并可看到原因，
// 而不是无声白屏。
app.config.errorHandler = (err, _instance, info) => {
  reportError(`[vue] ${info}: ${err}`);
  window.dispatchEvent(
    new CustomEvent("app-error", { detail: `${info}: ${err}` }),
  );
};
window.addEventListener("unhandledrejection", (e) => {
  reportError(`[promise] ${e.reason}`);
  window.dispatchEvent(
    new CustomEvent("app-error", { detail: String(e.reason) }),
  );
});
window.addEventListener("error", (e) => {
  if (e.message) {
    reportError(`[js] ${e.message}`);
    window.dispatchEvent(new CustomEvent("app-error", { detail: e.message }));
  }
});

app.mount("#app");
