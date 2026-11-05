// 自包含截图：spawn headless Chrome → 连 CDP → 抓 console → 截图 → 关闭
import { spawn } from "node:child_process";
import fs from "node:fs";

const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const OUT = process.argv[2];
// 其余参数: "name.png|#hash[:theme]" ...
const shots = process.argv.slice(3);

const chrome = spawn(CHROME, [
  "--headless=new", "--disable-gpu", "--no-proxy-server", "--no-first-run",
  "--hide-scrollbars", "--user-data-dir=" + OUT + "-prof",
  "--remote-debugging-port=0", "about:blank",
]);

const wsUrl = await new Promise((res, rej) => {
  let buf = "";
  const timer = setTimeout(() => rej(new Error("chrome timeout")), 15000);
  chrome.stderr.on("data", (d) => {
    buf += d.toString();
    const m = buf.match(/DevTools listening on (ws:\/\/\S+)/);
    if (m) { clearTimeout(timer); res(m[1]); }
  });
});
// ws://host:port/devtools/browser/... → http
const httpBase = wsUrl.replace("ws://", "http://").split("/devtools")[0];

const targets = await (await fetch(httpBase + "/json/list")).json();
const page = targets.find((t) => t.type === "page");

const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
const logs = [];

ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); return; }
  const p = msg.params;
  if (msg.method === "Runtime.exceptionThrown") {
    logs.push("EXC " + (p.exceptionDetails.exception?.description || p.exceptionDetails.text).slice(0, 300));
  } else if (msg.method === "Runtime.consoleAPICalled" && ["error", "warn"].includes(p.type)) {
    logs.push(`[${p.type}] ` + p.args.map((a) => a.value ?? a.description ?? "").join(" ").slice(0, 250));
  }
};
const send = (method, params = {}) => new Promise((res) => {
  const n = ++id;
  pending.set(n, res);
  ws.send(JSON.stringify({ id: n, method, params }));
});
await new Promise((r) => (ws.onopen = r));
await send("Runtime.enable");
await send("Page.enable");

// —— Tauri mock：让纯浏览器也能渲染真实界面 ——
const FIXTURES = {
  get_app_version: "0.1.10",
  get_data_dir: "C:\\Users\\demo\\AppData\\Roaming\\deploy-code",
  get_autostart: false,
  list_repos: [
    { id: "r1", name: "deploy_code", path: "E:\\code\\web_svcs\\deploy_code", pathExists: true, isRepo: true, currentBranch: "feat/server-mode-remote-control", remote: "git@github.com:demo/deploy-code.git", changeCount: 3, defaultServerId: "s1", defaultTargetDir: "/opt/app", envFiles: [] },
    { id: "r2", name: "blog", path: "E:\\code\\blog", pathExists: true, isRepo: true, currentBranch: "main", remote: "git@github.com:demo/blog.git", changeCount: 0, defaultServerId: "s1", defaultTargetDir: "/var/www/blog", envFiles: [] },
    { id: "r3", name: "qinglong-scripts", path: "E:\\code\\ql", pathExists: false, isRepo: true, currentBranch: "main", remote: null, changeCount: 0, defaultServerId: null, defaultTargetDir: "", envFiles: [] },
  ],
  list_servers: [
    { id: "s1", name: "深圳轻量云", host: "1.2.3.4", port: 22, username: "root", auth: { type: "privateKey", keyPath: "C:\\keys\\id_ed25519", passphrase: null }, defaultTargetDir: "/opt/app", dbBackup: null, backupTargetId: null, supabaseUrl: null, tunnels: [{ id: "t1", localPort: 18080, remoteHost: "127.0.0.1", remotePort: 8080, enabled: true }], createdAt: "2026-09-01 10:00:00" },
    { id: "s2", name: "香港中转", host: "5.6.7.8", port: 2222, username: "ubuntu", auth: { type: "password", password: "" }, defaultTargetDir: "/home/ubuntu/app", dbBackup: null, backupTargetId: null, supabaseUrl: null, tunnels: [], createdAt: "2026-09-15 10:00:00" },
    { id: "s3", name: "老机器（采不到）", host: "10.0.0.9", port: 22, username: "deploy", auth: { type: "privateKey", keyPath: "C:\\keys\\id_rsa", passphrase: null }, defaultTargetDir: "/srv/legacy", dbBackup: null, backupTargetId: null, supabaseUrl: null, tunnels: [], createdAt: "2026-10-02 10:00:00" },
    { id: "s4", name: "刚加的（还没采）", host: "10.0.0.30", port: 22, username: "root", auth: { type: "password", password: "" }, defaultTargetDir: "/opt/new", dbBackup: null, backupTargetId: null, supabaseUrl: null, tunnels: [], createdAt: "2026-11-05 02:59:00" },
  ],
  // 常驻连接的快照：一台正常在线、一台双指标越线（警示色）、一台会话断了（红色 + 悬停看原因）。
  list_host_status: [
    { serverId: "s1", serverName: "深圳轻量云", connected: true, retries: 0, lastError: null, connectedAt: "2026-11-05 02:58:12", forwarded: 2, rules: [{ ruleId: "t1", localPort: 18080, remoteLabel: "127.0.0.1:8080", bound: true, active: true, error: null }], metrics: { cpuPercent: 12, cpuCores: 4, memoryTotalBytes: 8074006528, memoryUsedBytes: 2415919104 }, metricsError: null, sampledAt: "2026-11-05 03:00:04" },
    { serverId: "s2", serverName: "香港中转", connected: true, retries: 0, lastError: null, connectedAt: "2026-11-05 02:59:40", forwarded: 0, rules: [], metrics: { cpuPercent: 93, cpuCores: 2, memoryTotalBytes: 4194304000, memoryUsedBytes: 3972005888 }, metricsError: null, sampledAt: "2026-11-05 03:00:05" },
    { serverId: "s3", serverName: "老机器（采不到）", connected: false, retries: 4, lastError: "连接 10.0.0.9:22 超时", connectedAt: null, forwarded: 0, rules: [], metrics: null, metricsError: "资源采集失败：连接已断开，正在重连", sampledAt: null },
    { serverId: "s4", serverName: "刚加的（还没采）", connected: false, retries: 0, lastError: null, connectedAt: null, forwarded: 0, rules: [], metrics: null, metricsError: null, sampledAt: null },
  ],
  get_shutdown_status: null,
  // 终端：给一个还活着的标签，验证 xterm 挂载、标签条排版与主题取色（真 PTY 要活服务器）。
  list_terminals: [
    { id: "t1", serverId: "s1", serverName: "深圳轻量云", cols: 100, rows: 30 },
    { id: "t2", serverId: "s2", serverName: "港机", cols: 100, rows: 30 },
  ],
  list_deploy_configs: [
    { id: "d1", repoId: "r1", name: "deploy_code → 深圳轻量云", rev: "main", serverIds: ["s1"], targetDir: "/opt/app", scripts: ["./scripts/deploy.sh"], scriptDir: "docker", runScripts: true, uploadEnv: false, createdAt: "2026-09-10 10:00:00" },
    { id: "d2", repoId: "r2", name: "blog 静态站", rev: "main", serverIds: ["s1", "s2"], targetDir: "/var/www/blog", scripts: [], scriptDir: "docker", runScripts: false, uploadEnv: true, createdAt: "2026-09-20 10:00:00" },
  ],
  list_backup_configs: [
    { id: "b1", name: "生产库每夜备份", serverId: "s1", source: { mode: "docker", container: "pg-prod", database: "appdb", username: "postgres", password: "", schema: "public" }, targetId: "t1", supabaseUrl: null },
  ],
  list_backup_targets: [
    { id: "t1", name: "本机 D 盘", url: "D:\\backups" },
  ],
  // 字段按 DeployRecord 全量给：少一个（例如 commitSubject）就会在 HistoryPage 的
  // shortPath 上抛 undefined，而 SPA 不重载，后续路由会跟着整页空白，看着像应用坏了。
  list_history: [
    { id: "h1", repoId: "r1", repoName: "deploy_code", rev: "main", branch: "main", worktree: false, atomicRelease: true, releaseDir: "/opt/app/releases/20261017-221000-3b2424f", commit: "3b2424f9c1a2e3f4a5b6c7d8e9f0a1b2c3d4e5f6", commitShort: "3b2424f", commitSubject: "feat: 一份配置部署到多台服务器", serverId: "s1", serverName: "深圳轻量云", targetDir: "/opt/app", scriptDir: "docker", scripts: ["deploy.sh"], runScripts: true, envFiles: [], status: "success", error: null, log: "", startedAt: "2026-10-17 22:10:00", finishedAt: "2026-10-17 22:11:32", durationMs: 92000 },
    { id: "h2", repoId: "r2", repoName: "blog", rev: "", branch: "(工作区)", worktree: true, atomicRelease: false, releaseDir: null, commit: "", commitShort: "", commitSubject: "", serverId: "s2", serverName: "香港中转", targetDir: "/var/www/blog", scriptDir: "docker", scripts: [], runScripts: false, envFiles: [{ localPath: "E:\\code\\blog\\.env.local", remotePath: ".env" }], status: "failed", error: "脚本 docker/build.sh 退出码 1", log: "", startedAt: "2026-10-18 09:02:00", finishedAt: "2026-10-18 09:02:41", durationMs: 41000 },
  ],
  get_settings: {
    scriptDir: "docker", runScripts: true, connectTimeoutSecs: 15, scriptTimeoutSecs: 1800,
    keepRemoteArchive: false, historyLimit: 500, supabaseUrl: "", defaultBackupTargetId: null,
    backupHistoryLimit: 200, backupTimeoutSecs: 3600, cloudflareApiToken: "", cloudflareAccountId: "",
    masterPasswordHash: null, githubToken: "", cronjobApiKey: "", pagesHistoryLimit: 200,
    containerHistoryLimit: 200, containerTimeoutSecs: 7200, containerBundleKeep: 2, dbBundleKeep: 0,
    language: "", atomicRelease: false, releaseKeep: 5,
    scheduledBackupEnabled: false, scheduledBackupTime: "03:00", scheduledBackupConfigId: null, scheduledBackupLastRun: "",
    scheduledShutdownEnabled: false, scheduledShutdownTime: "04:00", scheduledShutdownLastRun: "",
    scheduledContainerEnabled: false, scheduledContainerTime: "03:30", scheduledContainerConfigIds: [], scheduledContainerLastRun: "",
  },
};
const _callbacks = new Map();
await send("Page.addScriptToEvaluateOnNewDocument", {
  source: `
    const _callbacks = new Map();
    const _reg = (cb, once) => {
      const id = Math.floor(Math.random() * 1e9);
      _callbacks.set(id, (data) => { if (once) _callbacks.delete(id); return cb && cb(data); });
      return id;
    };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" }, currentMonitor: null },
      transformCallback: _reg,
      unregisterCallback: (id) => _callbacks.delete(id),
      runCallback: (id, data) => { const c = _callbacks.get(id); if (c) c(data); },
      callbacks: _callbacks,
      invoke: (cmd, args) => {
        const fx = window.__FIXTURES__;
        // 开标签这条路必须按参数造返回值：固定 id 会让「同一动作被触发两次」在界面上合并成
        // 一个标签，正好把要验的那一类缺陷遮掉。
        if (cmd === "open_terminal") {
          window.__OPEN_SEQ__ = (window.__OPEN_SEQ__ || 0) + 1;
          const srv = (fx.list_servers || []).find((s) => s.id === (args && args.serverId));
          return Promise.resolve({
            id: "open-" + window.__OPEN_SEQ__,
            serverId: (args && args.serverId) || "",
            serverName: srv ? srv.name : "?",
            cols: (args && args.cols) || 80,
            rows: (args && args.rows) || 24,
          });
        }
        if (cmd in fx) return Promise.resolve(fx[cmd]);
        if (cmd === "plugin:event|listen" || cmd === "plugin:event|unlisten") return Promise.resolve(1);
        if (cmd.startsWith("list_") || cmd.endsWith("_configs") || cmd.endsWith("_records")) return Promise.resolve([]);
        return Promise.resolve(null);
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
      unregisterListener: (event, id) => _callbacks.delete(id),
    };
    window.__FIXTURES__ = ${JSON.stringify(FIXTURES)};
  `,
});
await send("Network.enable");
ws; // Network 失败请求也收集进 logs
const _origOnMessage = ws.onmessage;
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.method === "Network.loadingFailed") {
    logs.push(`NETFAIL ${msg.params.errorText} ${msg.params.type}`);
  } else if (msg.method === "Network.responseReceived" && msg.params.response.status >= 400) {
    logs.push(`HTTP${msg.params.response.status} ${msg.params.response.url.slice(0, 120)}`);
  }
  _origOnMessage(ev);
};
await send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });

for (const spec of shots) {
  const [name, rest] = spec.split("|");
  const [hash, theme] = rest.split(":");
  await send("Page.navigate", { url: "http://127.0.0.1:4173/" + hash });
  await new Promise((r) => setTimeout(r, 4000));
  if (theme) {
    await send("Runtime.evaluate", { expression: `document.documentElement.dataset.theme="${theme}";` });
    await new Promise((r) => setTimeout(r, 400));
  }
  const shot = await send("Page.captureScreenshot", { format: "png" });
  if (shot.result?.data) {
    fs.writeFileSync(OUT + "/" + name, Buffer.from(shot.result.data, "base64"));
    console.log("saved", name);
  } else console.log("FAILED", name, JSON.stringify(shot.error || shot).slice(0, 120));

  const st = await send("Runtime.evaluate", {
    expression:
      'JSON.stringify({url:location.href.slice(0,60), rs:document.readyState, title:document.title, tauri:!!window.__TAURI_INTERNALS__, vite:!!window.__vite_plugin_react_preamble_installed__, rootLen:document.getElementById("root")?.innerHTML.length||0, terms:[...document.querySelectorAll(".xterm")].map((e)=>({hidden:!!e.closest(".hidden"), rows:e.querySelectorAll(".xterm-rows > div").length}))})',
    returnByValue: true,
  });
  console.log("STATE", name, st.result?.result?.value);
}

console.log("\n--- console ---");
console.log([...new Set(logs)].slice(0, 25).join("\n") || "(clean)");

await send("Browser.close");
process.exit(0);
