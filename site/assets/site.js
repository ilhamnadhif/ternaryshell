/* TernaryShell — the page around the demo: navigation, the visitor's system, install tabs,
   copy buttons, shortcuts and the latest release from GitHub. */
(() => {
  "use strict";
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => Array.from(el.querySelectorAll(s));
  const REPO = "ilhamnadhif/ternaryshell";

  // ------------------------------------------------------------------ nav
  const nav = $("#nav");
  const onScroll = () => nav.classList.toggle("solid", window.scrollY > 8);
  window.addEventListener("scroll", onScroll, { passive: true });
  onScroll();

  // ------------------------------------------------------------------ reveal
  if ("IntersectionObserver" in window) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
    }, { rootMargin: "0px 0px -8% 0px" });
    $$(".rv").forEach((el) => io.observe(el));
  } else {
    $$(".rv").forEach((el) => el.classList.add("in"));
  }

  // ------------------------------------------------------------------ the visitor's system
  const plat = ((navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent || "").toLowerCase();
  const ua = (navigator.userAgent || "").toLowerCase();
  const os = /win/.test(plat) ? "win" : /linux|x11|cros/.test(plat) && !/android/.test(ua) ? "linux" : "mac";
  const NAMES = { mac: "macOS", win: "Windows", linux: "Linux" };
  $$("[data-cta-main] span").forEach((s) => { s.textContent = `Download for ${NAMES[os]}`; });

  // Keys in the text follow the visitor's keyboard.
  const KEYS = {
    mac: { palette: "<kbd>⌘</kbd><kbd>K</kbd>", split: "<kbd>⌘</kbd><kbd>D</kbd>", files: "<kbd>⇧</kbd><kbd>⌘</kbd><kbd>F</kbd>", save: "<kbd>⌘</kbd><kbd>S</kbd>" },
    pc: { palette: "<kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>K</kbd>", split: "<kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>D</kbd>", files: "<kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>F</kbd>", save: "<kbd>Ctrl</kbd><kbd>S</kbd>" },
  };
  if (os !== "mac") $$("[data-k]").forEach((el) => { const k = KEYS.pc[el.dataset.k]; if (k) el.innerHTML = k; });

  // ------------------------------------------------------------------ install tabs
  const tabs = $$('.tabs-os [role="tab"]');
  const select = (id, focus) => {
    for (const t of tabs) {
      const on = t.id === `tab-${id}`;
      t.setAttribute("aria-selected", on);
      t.tabIndex = on ? 0 : -1;
      $(`#${t.getAttribute("aria-controls")}`).hidden = !on;
      if (on && focus) t.focus();
    }
  };
  tabs.forEach((t, i) => {
    t.addEventListener("click", () => select(t.id.slice(4)));
    t.addEventListener("keydown", (e) => {
      const d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
      if (!d) return;
      e.preventDefault();
      select(tabs[(i + d + tabs.length) % tabs.length].id.slice(4), true);
    });
  });
  select(os);
  $$("[data-os-link]").forEach((a) => a.addEventListener("click", () => select(a.dataset.osLink)));
  $$("[data-cta-main]").forEach((a) => a.addEventListener("click", () => select(os)));

  // ------------------------------------------------------------------ copy
  $$("[data-copy]").forEach((b) => b.addEventListener("click", async () => {
    const text = $(`#${b.dataset.copy}`).textContent.split("\n").map((l) => l.replace(/\s+#.*$/, "")).join("\n").trim();
    const label = $("span", b);
    try {
      await navigator.clipboard.writeText(text);
      b.classList.add("ok"); label.textContent = "Copied";
    } catch {
      label.textContent = "Select and copy";
    }
    setTimeout(() => { b.classList.remove("ok"); label.textContent = "Copy"; }, 1800);
  }));

  // ------------------------------------------------------------------ shortcuts
  const SHORTCUTS = [
    ["Quick connect, search", [["⌘", "K"]], [["Ctrl", "Shift", "K"]]],
    ["New connection", [["⌘", "N"]], [["Ctrl", "Shift", "N"]]],
    ["New tab, same server", [["⌘", "T"]], [["Ctrl", "Shift", "T"]]],
    ["Close pane or tab", [["⌘", "W"]], [["Ctrl", "Shift", "W"]]],
    ["Reconnect", [["⌘", "R"]], [["Ctrl", "Shift", "R"]]],
    ["Switch tab", [["⌘", "1–9"]], [["Alt", "1–9"]]],
    ["Split right, split down", [["⌘", "D"], ["⇧", "⌘", "D"]], [["Ctrl", "Shift", "D"], ["Ctrl", "Alt", "Shift", "D"]]],
    ["Files panel (SFTP)", [["⇧", "⌘", "F"]], [["Ctrl", "Shift", "F"]]],
    ["Previous, next pane", [["⌘", "["], ["⌘", "]"]], [["Ctrl", "Shift", "["], ["Ctrl", "Shift", "]"]]],
    ["Sidebar", [["⌘", "B"]], [["Ctrl", "Shift", "B"]]],
    ["Clear scrollback", [["⌥", "⌘", "K"]], [["Ctrl", "Shift", "L"]]],
    ["Export connections", [["⇧", "⌘", "E"]], [["Ctrl", "Shift", "E"]]],
    ["Save in the remote editor", [["⌘", "S"]], [["Ctrl", "S"]]],
    ["Settings", [["⌘", ","]], [["Ctrl", ","]]],
  ];
  const grid = $("#keygrid");
  const note = $("#keys-note");
  const NOTES = {
    mac: note ? note.innerHTML : "",
    pc: "Ctrl+Shift for the app, so every Ctrl+letter still reaches the remote shell. Alt sends Meta for readline and Emacs.",
  };
  const showKeys = (which) => {
    if (!grid) return;
    const col = which === "mac" ? 1 : 2;
    grid.innerHTML = SHORTCUTS.map((s) => {
      const keys = s[col].map((combo) => combo.map((k) => `<kbd>${k}</kbd>`).join("")).join('<span class="or">/</span>');
      return `<div class="krow"><span>${s[0]}</span><span class="kk">${keys}</span></div>`;
    }).join("");
    $$(".seg-os button").forEach((b) => b.setAttribute("aria-pressed", b.dataset.keys === which));
    if (note) note.innerHTML = NOTES[which];
  };
  $$(".seg-os button").forEach((b) => b.addEventListener("click", () => showKeys(b.dataset.keys)));
  showKeys(os === "mac" ? "mac" : "pc");

  // ------------------------------------------------------------------ the latest release
  const ASSETS = {
    pkg: /\.pkg$/, dmg: /\.dmg$/,
    "win-setup": /windows-x64-setup\.exe$/, "win-zip": /windows-x64\.zip$/,
    "deb-x64": /linux-amd64\.deb$/, "deb-arm": /linux-arm64\.deb$/,
    "rpm-x64": /linux-x86_64\.rpm$/, "rpm-arm": /linux-aarch64\.rpm$/,
    "appimage-x64": /linux-x86_64\.AppImage$/, "appimage-arm": /linux-aarch64\.AppImage$/,
    "tar-x64": /linux-x86_64\.tar\.gz$/, "tar-arm": /linux-aarch64\.tar\.gz$/,
  };
  const apply = (rel) => {
    if (!rel || !rel.tag_name) return;
    const v = rel.tag_name.replace(/^v/, "");
    $$("[data-version]").forEach((el) => { el.textContent = v; });
    $$("[data-release-link]").forEach((a) => { if (rel.html_url) a.href = rel.html_url; });
    for (const [key, re] of Object.entries(ASSETS)) {
      const a = (rel.assets || []).find((x) => re.test(x.name));
      if (!a) continue;
      $$(`[data-asset="${key}"]`).forEach((el) => { el.href = a.browser_download_url; });
    }
  };
  const KEY = "ts-release";
  let cached = null;
  try { cached = JSON.parse(sessionStorage.getItem(KEY) || "null"); } catch { /* storage off */ }
  if (cached && Date.now() - cached.at < 30 * 60 * 1000) {
    apply(cached.rel);
  } else if (window.fetch) {
    fetch(`https://api.github.com/repos/${REPO}/releases/latest`, { headers: { Accept: "application/vnd.github+json" } })
      .then((r) => (r.ok ? r.json() : null))
      .then((rel) => {
        if (!rel) return;
        const slim = { tag_name: rel.tag_name, html_url: rel.html_url, assets: (rel.assets || []).map((a) => ({ name: a.name, browser_download_url: a.browser_download_url })) };
        apply(slim);
        try { sessionStorage.setItem(KEY, JSON.stringify({ at: Date.now(), rel: slim })); } catch { /* storage off */ }
      })
      .catch(() => { /* the static links still work */ });
  }
})();
