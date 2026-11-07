const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { resolve } = require("node:path");
const vm = require("node:vm");

const bridge = readFileSync(resolve(__dirname, "../../ui/system-theme.js"), "utf8");
function page(withRoot = true, initial) {
  const window = new EventTarget();
  const document = Object.assign(new EventTarget(), {
    documentElement: withRoot ? { dataset: { theme: "system" } } : null,
  });
  const context = vm.createContext({ window, document, Event, Object });
  vm.runInContext(bridge + (initial ? `window.__MIHOMO_SYSTEM_THEME__.update(${JSON.stringify(initial)});` : ""), context);
  return { window, document, theme: window.__MIHOMO_SYSTEM_THEME__ };
}

test("native updates reach an open page immediately without changing its preference", () => {
  const { window, document, theme } = page();
  let changes = 0;
  window.addEventListener("mihomo-system-theme-changed", () => changes++);
  for (const appearance of ["dark", "light", "dark", "light"]) {
    theme.update(appearance);
    assert.equal(theme.current, appearance);
    assert.equal(document.documentElement.dataset.systemTheme, appearance);
    assert.equal(document.documentElement.dataset.theme, "system");
  }
  assert.equal(changes, 4);
});

test("a change before the DOM or page listener exists is cached and rendered later", () => {
  const { document, theme } = page(false, "dark");
  theme.update("light");
  assert.equal(theme.current, "light");
  document.documentElement = { dataset: {} };
  document.dispatchEvent(new Event("DOMContentLoaded"));
  assert.equal(document.documentElement.dataset.systemTheme, "light");
});

test("manual appearance stays separate, including while a hidden page receives changes", () => {
  const { document, theme } = page();
  document.documentElement.dataset.theme = "dark";
  theme.update("light");
  assert.equal(document.documentElement.dataset.theme, "dark");
  assert.equal(document.documentElement.dataset.systemTheme, "light");
});

test("new documents can be seeded with the latest native appearance", () => {
  const { document, theme } = page(true, "light");
  assert.equal(theme.current, "light");
  assert.equal(document.documentElement.dataset.systemTheme, "light");
});

test("the bridge exposes no IPC and ignores invalid appearances", () => {
  const { window, document, theme } = page(true, "dark");
  theme.update("system");
  theme.update(null);
  theme.update("invalid");
  assert.equal(theme.current, "dark");
  assert.equal(document.documentElement.dataset.systemTheme, "dark");
  assert.deepEqual(Object.keys(theme), ["current", "update"]);
  assert.equal(Object.isFrozen(theme), true);
  assert.equal(Object.getOwnPropertyDescriptor(window, "__MIHOMO_SYSTEM_THEME__").writable, false);
});
