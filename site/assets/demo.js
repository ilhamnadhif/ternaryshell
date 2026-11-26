/* TernaryShell — the live window on the home page. Sample servers only: nothing connects
   anywhere. A short tour plays when the window comes into view (paused while it is off
   screen); any click or key in it hands it over to the visitor. */
(() => {
  "use strict";
  const root = document.getElementById("demo-win");
  const scaler = document.getElementById("demo-scaler");
  const coachEl = document.getElementById("coach");
  if (!root || !scaler) return;

  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent;
  const MAC = /mac|iphone|ipad/i.test(platform);
  const K = {
    palette: MAC ? "<kbd>⌘</kbd><kbd>K</kbd>" : "<kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>K</kbd>",
    files: MAC ? "<kbd>⇧</kbd><kbd>⌘</kbd><kbd>F</kbd>" : "<kbd>Ctrl</kbd><kbd>Shift</kbd><kbd>F</kbd>",
    palShort: MAC ? "⌘K" : "Ctrl+Shift+K",
  };

  // ------------------------------------------------------------------ sample data
  const GROUPS = [
    { name: "Production", color: "#f06b63", tag: "PROD", hosts: ["web-01", "web-02", "db-primary", "bastion"] },
    { name: "Staging", color: "#e2b445", tag: "STG", hosts: ["api-staging", "worker-staging"] },
    { name: "Homelab", color: "#4fbf85", tag: "HOME", hosts: ["raspberry-pi", "nas"] },
    { name: "Ungrouped", color: null, tag: null, hosts: ["dev-vm"] },
  ];
  const HOSTS = {
    "web-01": { user: "deploy", addr: "10.0.1.21", icon: "server", login: "Key file · id_ed25519", how: "key", fw: 2, ms: 4, cwd: "/srv/app", tags: "web, nginx" },
    "web-02": { user: "deploy", addr: "10.0.1.22", icon: "server", login: "Key file · id_ed25519", how: "key", ms: 5, cwd: "/srv/app", tags: "web" },
    "db-primary": { user: "postgres", addr: "10.0.2.10", icon: "database", login: "SSH agent", how: "SSH agent", via: "bastion", ms: 9, tags: "db" },
    bastion: { user: "ops", addr: "bastion.acme.io", icon: "shield", login: "SSH agent", how: "SSH agent", ms: 7 },
    "api-staging": { user: "deploy", addr: "10.1.0.5", icon: "server", login: "Password", how: "password", ms: 11, tags: "api" },
    "worker-staging": { user: "deploy", addr: "10.1.0.6", icon: "cpu", login: "Password", how: "password", ms: 12 },
    "raspberry-pi": { user: "pi", addr: "192.168.1.40", icon: "board", login: "Password", how: "password", ms: 3 },
    nas: { user: "admin", addr: "192.168.1.10", icon: "drive", login: "Password", how: "password", ms: 2 },
    "dev-vm": { user: "ilham", addr: "192.168.64.3", icon: "desktop", login: "Key file · id_ed25519", how: "key", ms: 1 },
  };
  const groupOf = (h) => GROUPS.find((g) => g.hosts.includes(h)) || GROUPS[3];
  const RECENT = [["web-01", "2 min ago"], ["db-primary", "1 h ago"], ["api-staging", "yesterday"], ["raspberry-pi", "3 days ago"], ["nas", "last week"], ["dev-vm", "last week"]];

  const LOCAL = [
    ["d", "config"], ["d", "dist"], ["d", "scripts"],
    ["code", "deploy.sh", "677 B"], ["cfg", "docker-compose.yml", "412 B"], ["cfg", "nginx.conf", "1.2 KB"],
    ["code", "package.json", "2.3 KB"], ["f", "README.md", "4.1 KB"], ["zip", "storefront-1.4.2.tar.gz", "212 MB"], ["code", "tsconfig.json", "560 B"],
  ];
  const REMOTE = [
    ["d", "config", "", "rwxr-xr-x"], ["lnk", "current", "", "rwxrwxrwx", "→ releases/1.4.1"], ["d", "logs", "", "rwxr-xr-x"],
    ["d", "releases", "", "rwxr-xr-x"], ["d", "shared", "", "rwxr-xr-x"], ["zip", "backup.tar.gz", "419 MB", "rw-------"],
    ["code", "deploy.sh", "677 B", "rwxr-xr-x"], ["cfg", "docker-compose.yml", "412 B", "rw-r--r--"], ["cfg", "nginx.conf", "1.2 KB", "rw-r--r--"], ["cfg", "storefront.yml", "480 B", "rw-r--r--"],
  ];

  // ------------------------------------------------------------------ helpers
  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
  const icon = (n) => `<svg class="i"><use href="#i-${n}"/></svg>`;
  const ICON = { server: "server", database: "database", shield: "shield", cpu: "cpu", board: "board", drive: "drive", desktop: "desktop" };
  const now = () => performance.now();
  const STOP = Symbol("stop");
  let tour = 0; // bumps to cancel the running tour
  let paused = true;
  const wait = (ms) => new Promise((ok, no) => {
    const id = tour;
    let left = reduce ? Math.min(ms, 60) : ms;
    let last = now();
    const tick = () => {
      if (id !== tour) return no(STOP);
      const t = now();
      if (!paused) left -= t - last;
      last = t;
      if (left <= 0) ok(); else requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  const coach = (html) => { if (coachEl) coachEl.innerHTML = `<span>${html}</span>`; };

  // ------------------------------------------------------------------ state
  let S;
  let nextId = 1;
  function fresh() {
    S = { tabs: [], active: -1, collapsed: {}, hint: true, palette: null, quick: false, toast: null, xfer: null, uploaded: false, sel: { l: null, r: null } };
  }
  const tab = () => S.tabs[S.active];
  const prompt = (t) => {
    const user = HOSTS[t.host] ? HOSTS[t.host].user : t.user || "root";
    const host = HOSTS[t.host] ? t.host : t.hostName;
    return `<span class="c-g">${esc(user)}@${esc(host)}</span>:<span class="c-b">${esc(t.cwdShown || "~")}</span>$ `;
  };

  // ------------------------------------------------------------------ the window
  function build() {
    root.innerHTML = `
      <div class="win" id="w">
        <div class="lights"><b></b><b></b><b></b></div>
        <span class="ib" style="left:200px;top:9px">${icon("sidebar")}</span>
        <aside class="side">
          <div class="filter">${icon("search")}<span>Filter</span></div>
          <span class="ib" style="left:213px;top:48px" data-act="palette" title="New connection">${icon("plus")}</span>
          <div class="tree" id="w-tree"></div>
          <div class="side-f"><span class="nc">${icon("plus")}New connection</span><span class="r">${icon("tray")}${icon("gear")}</span></div>
        </aside>
        <div class="tabs" id="w-tabs"></div>
        <div class="tools">
          <span class="t" data-act="quick" title="Quick commands">${icon("bolt")}</span>
          <span class="t" data-act="split" title="Split right">${icon("columns")}</span>
          <span class="div"></span>
          <span class="files" id="w-files" data-act="files">${icon("folder")}<span>Files</span></span>
        </div>
        <div class="canvas" id="w-canvas"></div>
        <div id="w-over"></div>
      </div>`;
  }

  function renderTree() {
    const live = {};
    S.tabs.forEach((t) => { if (HOSTS[t.host]) live[t.host] = t.state === "ready" ? "#4fbf85" : "#e8a23c"; });
    const on = tab() && tab().host;
    let h = "";
    for (const g of GROUPS) {
      const c = S.collapsed[g.name];
      h += `<div class="gh" data-group="${g.name}"><span class="car">${icon(c ? "caret-right" : "caret-down")}</span><span class="ld" style="--c:${g.color || "transparent"}"></span>${g.name}<span class="ct">${g.hosts.length}</span></div>`;
      if (c) continue;
      for (const n of g.hosts) {
        const st = live[n] ? `<b class="st" style="background:${live[n]}"></b>` : "";
        h += `<div class="hr${n === on ? " sel" : ""}" data-host="${n}"><span class="gl">${icon(ICON[HOSTS[n].icon])}${st}</span>${n}</div>`;
      }
    }
    root.querySelector("#w-tree").innerHTML = h;
  }

  function renderTabs() {
    let h = "";
    S.tabs.forEach((t, i) => {
      const g = HOSTS[t.host] ? groupOf(t.host) : null;
      const dot = t.state === "ready" ? `<span class="dt"></span>` : `<span class="dt wait"></span>`;
      const lu = g && g.color ? `<span class="lu" style="--l:${g.color}"></span>` : "";
      h += `<div class="tab${i === S.active ? " on" : ""}" data-tab="${i}">${dot}<span class="nm">${esc(t.title)}</span><span class="x" data-close="${i}">${icon("x")}</span>${lu}</div>`;
    });
    h += `<span class="tab-add" data-act="palette">${icon("plus")}</span>`;
    root.querySelector("#w-tabs").innerHTML = h;
    const f = root.querySelector("#w-files");
    const t = tab();
    f.classList.toggle("on", !!(t && t.files));
    f.classList.toggle("busy", !!S.xfer);
    f.querySelector("span").textContent = S.xfer ? `Files ${Math.floor(S.xfer.p)}%` : "Files";
  }

  function renderCanvas() {
    const cv = root.querySelector("#w-canvas");
    const t = tab();
    if (!t) {
      cv.className = "canvas";
      cv.removeAttribute("style");
      cv.innerHTML = homeHtml();
      return;
    }
    const g = HOSTS[t.host] ? groupOf(t.host) : null;
    cv.className = "canvas" + (g && g.color ? " lab" : "");
    if (g && g.color) cv.style.setProperty("--l", g.color); else cv.removeAttribute("style");
    let h = "";
    if (t.state === "connecting") h += connectingHtml(t);
    else h += `<div class="term${t.panes.length > 1 ? " split" : ""}" id="w-term" style="bottom:${t.files ? 503 : 26}px">${t.panes.length > 1 ? t.panes.map((_, i) => `<div class="pane${i !== t.focus ? " dim" : ""}" data-pane="${i}"></div>`).join("") : ""}</div>`;
    if (t.files && t.state === "ready") h += filesHtml(t);
    h += statusHtml(t);
    cv.innerHTML = h;
    if (t.state === "ready") renderTerm();
  }

  function homeHtml() {
    let rows = "";
    for (const [n, when] of RECENT) {
      const x = HOSTS[n];
      rows += `<div class="hrow" data-host="${n}"><span class="gl">${icon(ICON[x.icon])}</span><span class="nm">${n}</span><span class="ad m">${x.user}@${x.addr}</span><span class="rt">${when}</span><span class="of" data-files="${n}">Open Files</span></div>`;
    }
    return `<div class="view home"><div class="home-in">
      <div class="search" data-act="palette">${icon("search")}<span>Search or type <span class="m">user@host:port</span></span></div>
      <div class="lbl">Recent</div>${rows}
      ${S.hint ? `<div class="hint">2 hosts in ~/.ssh/config aren’t in TernaryShell yet.<a data-act="import">Import…</a><a data-act="hide" style="color:var(--faint);margin-left:12px">Hide</a></div>` : ""}
    </div></div>`;
  }

  function connectingHtml(t) {
    const steps = t.steps.map((s, i) => {
      const cls = i < t.step ? "done" : i === t.step ? "run" : "";
      const b = i < t.step ? icon("check") : "";
      const tm = i < t.step ? `<span class="tm">${s[2]}</span>` : "";
      return `<div class="stp ${cls}"><span class="b">${b}</span>${esc(s[0])}${s[1] ? `<span class="sub2">· ${esc(s[1])}</span>` : ""}${tm}</div>`;
    }).join("");
    return `<div class="card"><h4>Connecting to ${esc(t.title)}</h4><div class="s m">${esc(t.addr)}</div><div class="steps">${steps}</div></div>`;
  }

  function statusHtml(t) {
    const g = HOSTS[t.host] ? groupOf(t.host) : null;
    const x = HOSTS[t.host];
    const tag = g && g.tag ? `<span class="tg" style="--l:${g.color}">${g.tag}</span>` : "";
    const cn = t.state === "ready" ? `<span class="cn"><b></b>Connected</span>` : `<span class="cn"><b class="wait"></b>Connecting…</span>`;
    let right = "";
    if (S.xfer) right += `<span class="sg acc"><span class="ring" style="--p:${S.xfer.p}"></span>${Math.floor(S.xfer.p)}% · 1 file</span>`;
    if (x && x.fw) right += `<span class="sg">${icon("arrows")}${x.fw} forwards</span>`;
    if (x && x.via) right += `<span class="sg">${icon("route")}via ${x.via}</span>`;
    if (t.state === "ready") right += `<span class="sg">${icon("pulse")}${x ? x.ms : 18} ms</span><span class="m">${t.panes.length > 1 ? "61" : "126"}×${t.files ? 17 : 46}</span>`;
    return `<div class="status">${tag}${cn}<span class="m">${esc(t.addr)}</span><span class="sp"></span>${right}</div>`;
  }

  function filesHtml(t) {
    const x = HOSTS[t.host] || { user: t.user || "root", cwd: "~" };
    const row = (side, r) => {
      const [k, name, size = "", perm = "", extra = ""] = r;
      const ic = k === "d" ? `<span class="ic dir">${icon("folder")}</span>` : k === "lnk" ? `<span class="ic lnk">${icon("link")}</span>` : k === "zip" ? `<span class="ic zip">${icon("file-zip")}</span>` : k === "cfg" ? `<span class="ic cfg">${icon("file-code")}</span>` : k === "code" ? `<span class="ic code">${icon("file-code")}</span>` : `<span class="ic">${icon("file")}</span>`;
      const sel = S.sel[side] === name ? " sel" : "";
      const nw = side === "r" && S.uploaded && name === S.uploaded ? " new" : "";
      const md = k === "d" ? "Today 09:02" : name === S.uploaded ? "Today 09:16" : "Today 08:41";
      return `<div class="fr${sel}${nw}" data-side="${side}" data-name="${esc(name)}">${ic}<span class="nm">${esc(name)}${extra ? `<em>${esc(extra)}</em>` : ""}</span><span class="sz">${size}</span><span class="md">${md}</span>${side === "r" ? `<span class="pm">${perm}</span>` : ""}</div>`;
    };
    const remote = REMOTE.filter((r) => !S.uploaded || r[1] !== S.uploaded);
    if (S.uploaded) remote.push([S.uploadedKind || "f", S.uploaded, S.uploadedSize || "212 MB", "rw-r--r--"]);
    remote.sort((a, b) => ((a[0] === "d" || a[0] === "lnk") === (b[0] === "d" || b[0] === "lnk") ? a[1].localeCompare(b[1]) : (a[0] === "d" || a[0] === "lnk") ? -1 : 1));
    const tools = `<span class="grp"><i>${icon("arrow-up")}</i><i>${icon("house")}</i><i>${icon("refresh")}</i></span><span class="grp"><i>${icon("folder-plus")}</i><i>${icon("eye-slash")}</i></span>`;
    const xfer = S.xfer ? `<div class="xfer"><h5>Uploading 1 file<span>Cancel all</span></h5><div class="meta">${Math.floor(S.xfer.p)}% · ${S.xfer.done} of ${S.xfer.size} · ${S.xfer.speed} · about ${S.xfer.left} s left</div><div class="bar" style="--p:${S.xfer.p}"><i></i></div><div class="row"><span class="ic">${icon("upload")}</span>${esc(S.xfer.name)}<span class="m">to ${esc(x.cwd || "~")}</span></div></div>` : "";
    return `<div class="fp" id="w-fp">
      <div class="fp-h">${icon("folder")}<b>Files</b><span class="m">sftp · ${esc(t.addr)}</span><span class="r">${icon("expand")}<span data-act="files">${icon("x")}</span></span></div>
      <div class="sides">
        <div class="fs" data-drop="l"><div class="fs-t"><span class="who">${icon("laptop")}<b>Local</b><span>this Mac</span></span>${tools}</div>
          <div class="crumbs">${icon("laptop")}~<span class="sep">${icon("caret-right")}</span>Projects<span class="sep">${icon("caret-right")}</span><b>storefront</b></div>
          <div class="cols"><span>Name ${icon("caret-up")}</span><span class="sz">Size</span><span class="md">Modified</span></div>
          <div class="rows">${LOCAL.map((r) => row("l", r)).join("")}</div></div>
        <div class="fs" data-drop="r"><div class="fs-t"><span class="who">${icon(ICON[(HOSTS[t.host] || { icon: "server" }).icon])}<b>${esc(t.title)}</b><span>${esc(x.user)}</span></span>${tools}<span class="sp"></span><span class="btn">${icon("shield-plain")}Root</span><span class="btn sec">${icon("upload")}Upload…</span></div>
          <div class="crumbs">${icon("server")}<span>/</span><span class="sep">${icon("caret-right")}</span>srv<span class="sep">${icon("caret-right")}</span><b>app</b></div>
          <div class="cols"><span>Name ${icon("caret-up")}</span><span class="sz">Size</span><span class="md">Modified</span><span class="pm">Permissions</span></div>
          <div class="rows">${remote.map((r) => row("r", r)).join("")}</div>${xfer}</div>
      </div></div>`;
  }

  function renderTerm() {
    const t = tab();
    if (!t || t.state !== "ready") return;
    const el = root.querySelector("#w-term");
    if (!el) return;
    const paneHtml = (p, i) => {
      const cur = `<span class="cursor${i === t.focus ? (S.focused ? " blink" : "") : " off"}"></span>`;
      const live = p.ended ? "" : `<div class="tl">${prompt(t)}${esc(p.input)}${cur}</div>`;
      return p.lines.map((l) => `<div class="tl">${l}</div>`).join("") + live;
    };
    if (t.panes.length > 1) {
      el.querySelectorAll(".pane").forEach((node, i) => { node.innerHTML = paneHtml(t.panes[i], i); node.scrollTop = node.scrollHeight; });
    } else {
      el.innerHTML = paneHtml(t.panes[0], 0);
      el.scrollTop = el.scrollHeight;
    }
  }

  function renderOver() {
    const o = root.querySelector("#w-over");
    let h = "";
    if (S.quick) {
      h += `<div class="pal" style="left:auto;right:56px;top:42px;width:300px"><div class="pal-l" style="padding:6px">
        ${[["Disk usage", "df -h", "⌃1"], ["Memory", "free -h", "⌃2"], ["Top processes", "uptime", "⌃3"]].map((q) => `<div class="pr" data-quick="${q[1]}"><span class="gl">${icon("terminal")}</span><span>${q[0]}<br><span class="ad m">${q[1] === "uptime" ? "top" : q[1]}</span></span><span class="k">${q[2]}</span></div>`).join("")}
      </div></div>`;
    }
    if (S.palette) h += `<div class="scrim" data-act="close"></div>` + paletteHtml();
    if (S.toast) h += `<div class="toast"><b>${S.toast[0]}</b><div>${S.toast[1]}</div></div>`;
    o.innerHTML = h;
  }

  function paletteItems(q) {
    const items = [];
    const ql = q.trim().toLowerCase();
    const addr = /^[\w.-]+@[\w.-]+(:\d+)?$/.test(q.trim()) ? q.trim() : null;
    if (addr) items.push({ kind: "addr", label: `Connect to ${addr}`, addr });
    const hosts = Object.keys(HOSTS).filter((n) => {
      const x = HOSTS[n];
      const hay = `${n} ${x.user}@${x.addr} ${x.tags || ""} ${groupOf(n).name} ${groupOf(n).tag || ""}`.toLowerCase();
      return !ql || hay.includes(ql) || n.replace(/-/g, "").includes(ql);
    });
    hosts.slice(0, addr ? 3 : 6).forEach((n) => items.push({ kind: "host", host: n }));
    const acts = [
      { kind: "act", label: "Open Files", k: MAC ? "⇧⌘F" : "Ctrl+Shift+F", act: "files", ic: "folder" },
      { kind: "act", label: "Split Right", k: MAC ? "⌘D" : "Ctrl+Shift+D", act: "split", ic: "columns" },
      { kind: "act", label: "New Connection…", k: MAC ? "⌘N" : "Ctrl+Shift+N", act: "new", ic: "plus" },
      { kind: "act", label: "Close Tab", k: MAC ? "⌘W" : "Ctrl+Shift+W", act: "close-tab", ic: "x" },
    ].filter((a) => !ql || a.label.toLowerCase().includes(ql));
    return items.concat(acts.slice(0, Math.max(0, 7 - items.length)));
  }

  function paletteHtml() {
    const P = S.palette;
    const items = paletteItems(P.q);
    P.items = items;
    if (P.i >= items.length) P.i = Math.max(0, items.length - 1);
    const q = P.q ? `<span>${esc(P.q)}</span><span class="caret" style="margin-left:1px"></span>` : `<span class="caret" style="margin-left:0"></span><span class="ph">Search connections and commands, or type user@host…</span>`;
    let list = "";
    let sec = "";
    items.forEach((it, i) => {
      const s = it.kind === "act" ? "Actions" : it.kind === "addr" ? "Quick connect" : "Connections";
      if (s !== sec) { list += `<div class="sh">${s}</div>`; sec = s; }
      const on = i === P.i ? " on" : "";
      if (it.kind === "host") {
        const x = HOSTS[it.host];
        list += `<div class="pr${on}" data-pi="${i}"><span class="gl">${icon(ICON[x.icon])}</span>${it.host}<span class="ad m">${x.user}@${x.addr}</span></div>`;
      } else if (it.kind === "addr") {
        list += `<div class="pr${on}" data-pi="${i}"><span class="gl">${icon("arrow-right")}</span>${esc(it.label)}</div>`;
      } else {
        list += `<div class="pr${on}" data-pi="${i}"><span class="gl">${icon(it.ic)}</span>${it.label}<span class="k">${it.k}</span></div>`;
      }
    });
    const cur = items[P.i];
    let prev = "";
    if (cur && cur.kind === "host") {
      const x = HOSTS[cur.host];
      const g = groupOf(cur.host);
      prev = `<h6>${cur.host}</h6><div class="m">${x.user}@${x.addr}</div><div class="kv"><span>Group</span><span>${g.name}</span><span>Login</span><span>${x.login}</span>${x.via ? `<span>Via</span><span>${x.via}</span>` : ""}${x.fw ? `<span>Forwards</span><span>18080, 11080</span>` : ""}${x.tags ? `<span>Tags</span><span>${x.tags}</span>` : ""}</div>`;
    } else if (cur && cur.kind === "addr") {
      const m = cur.addr.match(/^([^@]+)@([^:]+)(?::(\d+))?$/);
      prev = `<h6>Not saved</h6><div class="kv"><span>User</span><span class="m">${esc(m[1])}</span><span>Host</span><span class="m">${esc(m[2])}</span><span>Port</span><span class="m">${esc(m[3] || "22")}</span><span>Login</span><span>asks when connecting</span></div>`;
    }
    const body = items.length
      ? `<div class="pal-l">${list}</div><div class="pal-p">${prev}</div>`
      : `<div class="pal-empty">No matches<div>Type user@host to connect without saving.</div></div>`;
    const scopes = ["All", "Hosts", "Commands", "Tabs", "Actions"].map((s, i) => `<span class="${i === 0 ? "on" : ""}">${s}</span>`).join("");
    return `<div class="pal"><div class="pal-q">${icon("search")}${q}</div><div class="scopes">${scopes}</div><div class="pal-b">${body}</div></div>`;
  }

  function renderAll() { renderTree(); renderTabs(); renderCanvas(); renderOver(); }

  // ------------------------------------------------------------------ actions
  function newTab(host, extra) {
    const x = HOSTS[host];
    const t = {
      id: nextId++, host, title: host, addr: x ? `${x.user}@${x.addr}` : host, state: "connecting", step: 0, files: false,
      panes: [{ lines: [], input: "", ended: false }], focus: 0, cwd: x && x.cwd, cwdShown: null, ...extra,
    };
    if (x && x.via) {
      t.steps = [[`Connect to ${x.via}`, "port 22", "0.1 s"], [`Connect to ${host}`, `through ${x.via}`, "0.1 s"], ["Verify host keys", "", "0.0 s"], [`Log in with ${x.how}`, "", "0.2 s"], ["Open shell", "", "0.1 s"]];
    } else {
      const how = x ? x.how : "password";
      t.steps = [[`Connect to ${t.addr.split("@")[1]}`, "port 22", "0.1 s"], ["Verify host key", "", "0.0 s"], [`Log in with ${how}`, "", "0.2 s"], ["Open shell", "", "0.1 s"]];
    }
    return t;
  }

  async function connect(t) {
    for (; t.step < t.steps.length; t.step++) {
      if (S.tabs.indexOf(t) === S.active) renderCanvas();
      await sleepUI(reduce ? 40 : 230);
    }
    t.state = "ready";
    const x = HOSTS[t.host];
    t.cwdShown = x && x.cwd ? x.cwd : "~";
    t.panes[0].lines.push(`Last login: Tue Nov 18 09:12:44 2026 from 10.0.0.8`);
    if (S.tabs.includes(t)) renderAll();
  }

  // UI pauses that belong to the app (connecting) rather than the tour: never cancelled.
  const sleepUI = (ms) => new Promise((ok) => setTimeout(ok, ms));

  function openHost(host, opts = {}) {
    const found = S.tabs.findIndex((t) => t.host === host);
    if (found >= 0 && !opts.fresh) {
      S.active = found;
      if (opts.files) S.tabs[found].files = true;
      renderAll();
      return Promise.resolve(S.tabs[found]);
    }
    const t = newTab(host, opts.addr ? { addr: opts.addr, title: opts.addr.split("@")[1].split(":")[0], hostName: opts.addr.split("@")[1].split(":")[0], user: opts.addr.split("@")[0] } : {});
    if (opts.files) t.files = true;
    S.tabs.push(t);
    S.active = S.tabs.length - 1;
    renderAll();
    return connect(t).then(() => t);
  }

  function closeTab(i) {
    S.tabs.splice(i, 1);
    if (S.active >= S.tabs.length) S.active = S.tabs.length - 1;
    else if (i < S.active) S.active--;
    S.quick = false;
    renderAll();
  }

  function toggleFiles(on) {
    const t = tab();
    if (!t || t.state !== "ready") { coach(`Open a server first: Files works over its connection.`); return; }
    t.files = on === undefined ? !t.files : on;
    renderTabs();
    renderCanvas();
  }

  function split() {
    const t = tab();
    if (!t || t.state !== "ready") return;
    if (t.panes.length > 1) { t.focus = t.focus ? 0 : 1; renderCanvas(); return; }
    t.panes.push({ lines: [], input: "", ended: false });
    t.focus = 1;
    renderCanvas();
    coach("Two panes over the same connection: no second login.");
  }

  function toast(title, detail) {
    S.toast = [title, detail];
    renderOver();
    const mine = S.toast;
    setTimeout(() => { if (S.toast === mine) { S.toast = null; renderOver(); } }, 3600);
  }

  function upload(name, sizeLabel, kind) {
    if (S.xfer) return Promise.resolve();
    const t = tab();
    const localRow = LOCAL.find((x) => x[1] === name);
    const total = parseFloat(sizeLabel) || 0.6;
    const unit = /\bMB\b/.test(sizeLabel) ? "MB" : /\bKB\b/.test(sizeLabel) ? "KB" : "B";
    S.xfer = { name, size: sizeLabel, p: 0, done: `0 ${unit}`, speed: "84 MB/s", left: 3 };
    renderTabs(); renderCanvas();
    const dur = reduce ? 300 : unit === "MB" ? 2600 : 700;
    const start = now();
    return new Promise((ok) => {
      const step = () => {
        if (!S.xfer) return ok();
        const p = Math.min(100, ((now() - start) / dur) * 100);
        S.xfer.p = p;
        const decimals = unit === "KB" ? 1 : 0;
        S.xfer.done = `${(total * p / 100).toFixed(decimals)} ${unit}`;
        S.xfer.left = Math.max(1, Math.ceil((dur - (now() - start)) / 1000));
        if (tab() === t) { renderTabs(); patchXfer(); }
        if (p >= 100) {
          S.xfer = null;
          S.uploaded = name; S.uploadedSize = sizeLabel; S.uploadedKind = kind || (localRow && localRow[0]) || "f";
          renderTabs(); if (tab() === t) renderCanvas();
          toast("Uploaded 1 file.", `To ${(HOSTS[t.host] && HOSTS[t.host].cwd) || "~"} on ${t.title}`);
          return ok();
        }
        setTimeout(step, 80);
      };
      step();
    });
  }
  // Only the parts that move during a transfer, so the lists keep their hover and selection.
  function patchXfer() {
    const x = root.querySelector(".xfer");
    if (!x || !S.xfer) return renderCanvas();
    x.querySelector(".meta").textContent = `${Math.floor(S.xfer.p)}% · ${S.xfer.done} of ${S.xfer.size} · ${S.xfer.speed} · about ${S.xfer.left} s left`;
    x.querySelector(".bar").style.setProperty("--p", S.xfer.p);
    const st = root.querySelector(".status");
    const t = tab();
    if (st && t) st.outerHTML = statusHtml(t);
  }

  // ------------------------------------------------------------------ the shell
  const LS_WEB = [
    "total 412M",
    `drwxr-xr-x 2 deploy deploy 4.0K Nov 18 09:02 <span class="c-b">config</span>`,
    `lrwxrwxrwx 1 deploy deploy   14 Nov 18 09:05 <span class="c-c">current</span> -> releases/1.4.1`,
    `-rwxr-xr-x 1 deploy deploy  677 Nov 18 09:02 <span class="c-g">deploy.sh</span>`,
    `-rw-r--r-- 1 deploy deploy  412 Nov 18 09:02 docker-compose.yml`,
    `drwxr-xr-x 2 deploy deploy 4.0K Nov 18 09:10 <span class="c-b">logs</span>`,
    `drwxr-xr-x 5 deploy deploy 4.0K Nov 18 09:05 <span class="c-b">releases</span>`,
    `-rw------- 1 deploy deploy 419M Nov 17 23:00 backup.tar.gz`,
  ];
  const DEPLOY = [
    [`<span class="c-w">Deploying storefront 1.4.2 → production</span>`, 160],
    ["", 120],
    [`<span class="c-g">✔</span> Pulling ghcr.io/acme/storefront:1.4.2     <span class="c-d">(212 MB, 4.1s)</span>`, 420],
    [`<span class="c-g">✔</span> Running database migrations            <span class="c-d">(3 applied)</span>`, 380],
    [`<span class="c-g">✔</span> Warming caches                         <span class="c-d">(1,284 keys)</span>`, 300],
    [`<span class="c-y">●</span> Restarting web (rolling, 4 workers)`, 240],
    [`  worker 1/4 <span class="c-g">healthy</span>`, 200], [`  worker 2/4 <span class="c-g">healthy</span>`, 200], [`  worker 3/4 <span class="c-g">healthy</span>`, 200], [`  worker 4/4 <span class="c-g">healthy</span>`, 260],
    [`<span class="c-g">✔</span> Health check <span class="c-c">https://shop.example.com</span>     <span class="c-g">200 OK</span> 38 ms`, 220],
    ["", 80],
    [`<span class="c-g c-w">Deployed in 27s.</span>`, 0],
  ];
  function respond(t, cmd) {
    const c = cmd.trim();
    const x = HOSTS[t.host] || {};
    const sys = /raspberry|nas/.test(t.host) ? "Debian GNU/Linux 12" : "Ubuntu 24.04.1 LTS";
    if (!c) return [];
    if (c === "clear") return "clear";
    if (c === "ls" || c === "ls -l" || c === "ls -lh" || c === "ll") {
      if (t.cwdShown === "/srv/app") return c === "ls" ? [`<span class="c-b">config</span>  <span class="c-c">current</span>  <span class="c-g">deploy.sh</span>  docker-compose.yml  <span class="c-b">logs</span>  <span class="c-b">releases</span>  backup.tar.gz`] : LS_WEB;
      return [`<span class="c-b">backups</span>  <span class="c-b">scripts</span>  notes.txt`];
    }
    if (c === "./deploy.sh") return t.cwdShown === "/srv/app" ? "deploy" : [`bash: ./deploy.sh: No such file or directory`];
    if (c === "uptime" || c === "top") return [` 09:16:02 up 41 days,  3:12,  1 user,  load average: 0.08, 0.11, 0.09`];
    if (c === "whoami") return [esc(x.user || t.user || "root")];
    if (c === "hostname") return [esc(t.title)];
    if (c === "pwd") return [esc(t.cwdShown === "~" ? `/home/${x.user || "user"}` : t.cwdShown)];
    if (c === "uname -a") return [`Linux ${esc(t.title)} 6.8.0-49-generic #49-Ubuntu SMP x86_64 GNU/Linux`];
    if (c === "cat /etc/os-release" || c === "lsb_release -d") return [`PRETTY_NAME="${sys}"`];
    if (c === "df -h") return ["Filesystem      Size  Used Avail Use% Mounted on", "/dev/sda1        80G   31G   46G  41% /", "tmpfs           3.9G     0  3.9G   0% /dev/shm"];
    if (c === "free -h") return ["               total        used        free      shared  buff/cache   available", "Mem:           7.7Gi       2.1Gi       3.4Gi        12Mi       2.2Gi       5.3Gi", "Swap:          2.0Gi          0B       2.0Gi"];
    if (/^cd\b/.test(c)) { const d = c.slice(2).trim(); t.cwdShown = !d || d === "~" ? "~" : d.startsWith("/") ? d : (t.cwdShown === "~" ? "~/" : t.cwdShown + "/") + d; return []; }
    if (c === "exit" || c === "logout") return "exit";
    if (c === "help") return [`<span class="c-d">A demo shell. Try ls -lh, ./deploy.sh (on web-01), uptime, df -h, free -h, whoami or clear.</span>`];
    if (c === "sudo rm -rf /" || /rm -rf \//.test(c)) return [`<span class="c-d">Not on production, and not in a demo.</span>`];
    return [`${esc(c.split(/\s+/)[0])}: command not found`];
  }

  async function run(t, cmdOverride) {
    const p = t.panes[t.focus];
    const cmd = cmdOverride !== undefined ? cmdOverride : p.input;
    p.lines.push(prompt(t) + esc(cmd));
    p.input = "";
    const out = respond(t, cmd);
    if (out === "clear") { p.lines = []; }
    else if (out === "exit") { p.lines.push("logout", `<span class="c-d">Session ended. Press Enter to start a new shell.</span>`); p.ended = true; }
    else if (out === "deploy") {
      renderTerm();
      for (const [l, ms] of DEPLOY) { p.lines.push(l); if (tab() === t) renderTerm(); await sleepUI(reduce ? 20 : ms); }
    } else p.lines.push(...out);
    if (tab() === t) renderTerm();
  }

  async function typeInto(t, text) {
    const p = t.panes[t.focus];
    for (const ch of text) { p.input += ch; renderTerm(); await wait(reduce ? 0 : 55 + Math.random() * 45); }
    await wait(220);
  }

  // ------------------------------------------------------------------ the palette
  function openPalette() { S.palette = { q: "", i: 0, items: [] }; S.quick = false; renderOver(); }
  function closePalette() { S.palette = null; renderOver(); }
  function choose(i) {
    const it = S.palette && S.palette.items[i];
    closePalette();
    if (!it) return;
    if (it.kind === "host") openHost(it.host);
    else if (it.kind === "addr") openHost(it.addr, { addr: it.addr, fresh: true });
    else if (it.act === "files") toggleFiles(true);
    else if (it.act === "split") split();
    else if (it.act === "close-tab") { if (S.active >= 0) closeTab(S.active); }
    else if (it.act === "new") coach("In the app, New Connection opens the editor: server, login, network, session.");
  }

  // ------------------------------------------------------------------ the tour
  const pulse = (sel) => { const el = root.querySelector(sel); if (el) el.classList.add("pulse"); return el; };
  const unpulse = () => root.querySelectorAll(".pulse").forEach((e) => e.classList.remove("pulse"));

  async function playTour() {
    const id = ++tour;
    runningId = id;
    fresh(); renderAll();
    try {
      coach("Your servers in the sidebar, grouped by environment; the recent ones on Home.");
      await wait(2200);
      const row = root.querySelector('.hrow[data-host="web-01"]');
      if (row) row.classList.add("hov");
      coach("Opening <b>web-01</b>, a production server.");
      await wait(1100);
      const t = newTab("web-01");
      S.tabs.push(t); S.active = 0; renderAll();
      await Promise.all([connect(t), wait(1200)]);
      coach("Production is red: the tab, the top edge of the terminal and the <b>PROD</b> tag say so.");
      await wait(1800);
      await typeInto(t, "ls -lh"); await run(t); await wait(1500);
      await typeInto(t, "./deploy.sh"); await run(t); await wait(1300);
      coach(`${K.files} opens <b>Files</b> under the terminal, over the same connection.`);
      pulse("#w-files"); await wait(1200); unpulse();
      toggleFiles(true); await wait(1300);
      S.sel.l = "storefront-1.4.2.tar.gz"; renderCanvas(); await wait(700);
      coach("Drag a file across to upload it, and follow the batch.");
      await dragGhost("storefront-1.4.2.tar.gz");
      await Promise.race([upload("storefront-1.4.2.tar.gz", "212 MB"), new Promise((_, no) => { const iv = setInterval(() => { if (id !== tour) { clearInterval(iv); no(STOP); } else if (!S.xfer && S.uploaded) clearInterval(iv); }, 100); })]);
      await wait(1800);
      toggleFiles(false); await wait(500);
      coach(`${K.palette} finds any server by name, address, tag or group.`);
      openPalette(); await wait(800);
      for (const ch of "db") { S.palette.q += ch; renderOver(); await wait(260); }
      await wait(1300);
      choose(0);
      coach("<b>db-primary</b> is reached through <b>bastion</b>, like <span class=\"mono\">ssh -J</span>; each hop checks its own host key.");
      await wait(400);
      const db = tab();
      await new Promise((ok) => { const iv = setInterval(() => { if (!db || db.state === "ready" || id !== tour) { clearInterval(iv); ok(); } }, 50); });
      await wait(900);
      await typeInto(db, "uptime"); await run(db); await wait(1600);
      handOver();
    } catch (e) {
      if (e !== STOP) throw e;
    } finally {
      if (runningId === id) runningId = 0;
    }
  }

  async function dragGhost(name) {
    const from = root.querySelector(`.fr[data-side="l"][data-name="${name}"]`);
    const to = root.querySelector('.fs[data-drop="r"] .rows');
    if (!from || !to) return;
    const w = root.querySelector("#w").getBoundingClientRect();
    const s = w.width / 1280;
    const a = from.getBoundingClientRect();
    const b = to.getBoundingClientRect();
    const g = document.createElement("div");
    g.className = "drag-ghost";
    g.innerHTML = `${icon("file-zip")}${esc(name)}`;
    g.style.left = `${(a.left - w.left) / s + 40}px`;
    g.style.top = `${(a.top - w.top) / s}px`;
    root.querySelector("#w").appendChild(g);
    await wait(80);
    g.style.transform = `translate(${(b.left - a.left) / s + 60}px, ${(b.top - a.top) / s + 120}px)`;
    await wait(1100);
    g.remove();
  }

  function handOver() {
    unpulse();
    root.querySelectorAll(".hov").forEach((e) => e.classList.remove("hov"));
    coach(`Your turn: click a server, press ${K.palette}, or open Files.`);
  }

  function stopTour() {
    if (!tourRunning()) return;
    tour++;
    root.querySelectorAll(".drag-ghost").forEach((g) => g.remove());
    handOver();
  }
  let runningId = 0;
  const tourRunning = () => runningId !== 0 && runningId === tour;
  const startTour = () => playTour();

  // ------------------------------------------------------------------ input
  root.setAttribute("tabindex", "0");
  root.setAttribute("role", "application");
  root.setAttribute("aria-roledescription", "interactive demo");
  root.setAttribute("aria-label", "TernaryShell with sample servers. Click a server, press " + K.palShort + ", or open Files.");

  root.addEventListener("pointerdown", () => { stopTour(); S.focused = true; root.focus({ preventScroll: true }); });
  root.addEventListener("focus", () => { S.focused = true; renderTerm(); });
  root.addEventListener("blur", () => { S.focused = false; renderTerm(); });

  let lastClick = { at: 0, key: "" };
  root.addEventListener("click", (e) => {
    const el = e.target.closest("[data-act],[data-host],[data-files],[data-tab],[data-close],[data-group],[data-pi],[data-quick],.fr,.pane");
    if (!el) { if (S.quick) { S.quick = false; renderOver(); } return; }
    const d = el.dataset;
    if (d.close !== undefined) { e.stopPropagation(); closeTab(+d.close); return; }
    if (d.files) { openHost(d.files, { files: true }); return; }
    if (d.host) { openHost(d.host); return; }
    if (d.tab !== undefined) { S.active = +d.tab; S.quick = false; renderAll(); return; }
    if (d.group) { S.collapsed[d.group] = !S.collapsed[d.group]; renderTree(); return; }
    if (d.pi !== undefined) { choose(+d.pi); return; }
    if (d.quick) { S.quick = false; renderOver(); const t = tab(); if (t && t.state === "ready") run(t, d.quick); else coach("Quick commands run in the open terminal: open a server first."); return; }
    if (el.classList.contains("pane")) { const t = tab(); if (t) { t.focus = +d.pane; renderCanvas(); } return; }
    if (el.classList.contains("fr")) {
      const key = d.side + d.name;
      const dbl = key === lastClick.key && now() - lastClick.at < 400;
      lastClick = { at: now(), key };
      S.sel[d.side] = d.name; renderCanvas();
      if (dbl && d.side === "l" && !/^(config|dist|scripts)$/.test(d.name)) {
        const r = LOCAL.find((x) => x[1] === d.name);
        coach("Uploading to the folder open on the right.");
        upload(d.name, r && r[2] ? r[2] : "1 KB");
      } else if (dbl && d.side === "r") coach("In the app, double-clicking a file on the server opens it in the editor.");
      return;
    }
    switch (d.act) {
      case "palette": openPalette(); break;
      case "files": toggleFiles(); break;
      case "split": split(); break;
      case "quick": S.quick = !S.quick; renderOver(); break;
      case "close": closePalette(); break;
      case "import": coach("In the app, Import shows what will be added before it adds anything."); break;
      case "hide": S.hint = false; renderCanvas(); break;
    }
  });

  // Drag a local file onto the server side.
  let drag = null;
  root.addEventListener("pointerdown", (e) => {
    const fr = e.target.closest('.fr[data-side="l"]');
    if (!fr || /^(config|dist|scripts)$/.test(fr.dataset.name)) return;
    drag = { name: fr.dataset.name, x: e.clientX, y: e.clientY, moved: false };
  });
  window.addEventListener("pointerup", (e) => {
    if (!drag) return;
    const d = drag; drag = null;
    if (Math.hypot(e.clientX - d.x, e.clientY - d.y) < 12) return;
    const over = document.elementFromPoint(e.clientX, e.clientY);
    if (over && over.closest('.fs[data-drop="r"]')) {
      const r = LOCAL.find((x) => x[1] === d.name);
      upload(d.name, r && r[2] ? r[2] : "1 KB");
    }
  });

  const isPal = (e) => (MAC ? e.metaKey && !e.shiftKey : e.ctrlKey && e.shiftKey) && e.key.toLowerCase() === "k";
  document.addEventListener("keydown", (e) => {
    const inDemo = document.activeElement === root;
    const typingElsewhere = /^(INPUT|TEXTAREA|SELECT)$/.test((document.activeElement || {}).tagName || "");
    if (isPal(e) && !typingElsewhere && (inDemo || visible)) {
      e.preventDefault(); stopTour();
      if (!inDemo) root.focus({ preventScroll: true });
      S.palette ? closePalette() : openPalette();
      return;
    }
    if (!inDemo) return;
    stopTour();
    if (S.palette) {
      const P = S.palette;
      if (e.key === "Escape") { closePalette(); e.preventDefault(); }
      else if (e.key === "ArrowDown") { P.i = Math.min(P.i + 1, P.items.length - 1); renderOver(); e.preventDefault(); }
      else if (e.key === "ArrowUp") { P.i = Math.max(P.i - 1, 0); renderOver(); e.preventDefault(); }
      else if (e.key === "Enter") { choose(P.i); e.preventDefault(); }
      else if (e.key === "Backspace") { P.q = P.q.slice(0, -1); P.i = 0; renderOver(); e.preventDefault(); }
      else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey) { P.q += e.key; P.i = 0; renderOver(); e.preventDefault(); }
      return;
    }
    if (e.key === "Escape") { if (S.quick) { S.quick = false; renderOver(); } return; }
    const t = tab();
    // On Home, typing starts a search.
    if (!t && e.key.length === 1 && !e.metaKey && !e.ctrlKey) { openPalette(); S.palette.q = e.key; renderOver(); e.preventDefault(); return; }
    if ((MAC ? e.metaKey && e.shiftKey : e.ctrlKey && e.shiftKey) && e.key.toLowerCase() === "f") { toggleFiles(); e.preventDefault(); return; }
    if ((MAC ? e.metaKey && !e.shiftKey : e.ctrlKey && e.shiftKey) && e.key.toLowerCase() === "d") { split(); e.preventDefault(); return; }
    if (!t || t.state !== "ready") return;
    const p = t.panes[t.focus];
    if (e.key === "Enter") {
      e.preventDefault();
      if (p.ended) { p.ended = false; p.lines.push("", "Last login: Tue Nov 18 09:17:03 2026 from 10.0.0.8"); renderTerm(); return; }
      run(t);
    } else if (e.key === "Backspace") { p.input = p.input.slice(0, -1); renderTerm(); e.preventDefault(); }
    else if (e.key === "Tab") { e.preventDefault(); }
    else if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !p.ended && p.input.length < 80) { p.input += e.key; renderTerm(); e.preventDefault(); }
  });

  document.getElementById("demo-replay")?.addEventListener("click", () => { tour++; S.focused = false; startTour(); });

  // ------------------------------------------------------------------ scale and visibility
  const fit = () => { const s = scaler.clientWidth / 1280; root.style.transform = `scale(${s})`; };
  new ResizeObserver(fit).observe(scaler);
  fit();

  let visible = false;
  let started = false;
  const small = matchMedia("(max-width: 700px)");
  new IntersectionObserver(([en]) => {
    visible = en.isIntersecting && en.intersectionRatio > 0.25;
    paused = !visible;
    if (visible && !started && !small.matches) { started = true; startTour(); }
  }, { threshold: [0, 0.25, 0.5] }).observe(scaler);

  fresh();
  build();
  renderAll();
})();
