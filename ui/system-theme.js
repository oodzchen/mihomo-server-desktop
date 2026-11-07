// Injected into both WebViews before page scripts. No IPC privileges are added.
// Cache native changes even before the page installs its listener or while hidden.
(() => {
  let current;
  function render() {
    if (current && document.documentElement) document.documentElement.dataset.systemTheme = current;
  }
  Object.defineProperty(window, "__MIHOMO_SYSTEM_THEME__", {
    value: Object.freeze({
      get current() { return current; },
      update(theme) {
        if (theme !== "light" && theme !== "dark") return;
        current = theme;
        render();
        window.dispatchEvent(new Event("mihomo-system-theme-changed"));
      },
    }),
  });
  document.addEventListener("DOMContentLoaded", render, { once: true });
})();
