# AGENTS.md — DeployCode 开发者与 AI 代理指南

Tauri 2 桌面端 + Rust CLI 的多仓库部署工具。GUI 与 CLI 共享同一份 `deploy-core` 业务实现。界面文案中文为主（i18n 支持 en-US）。

## 常用命令

环境：Windows + PowerShell 5.1；Node 18+；Rust stable（MSVC）。若终端找不到 `cargo`，先把 `%USERPROFILE%\.cargo\bin` 加入 PATH（`build-installer.bat` 已处理）。

| 目的 | 命令 |
| --- | --- |
| 安装前端依赖 | `npm install` |
| 仅前端开发（http://localhost:1420） | `npm run dev` |
| 桌面端开发（Tauri，会启动 Vite） | `npm run app:dev` |
| 前端类型检查 + 构建（提交前必须通过） | `npm run build` |
| 仅 TypeScript 类型检查（更快） | `npx tsc -p tsconfig.json --noEmit` |
| 前端单元测试（src/lib 纯逻辑） | `npm test`（vitest，单文件：`npx vitest run src/lib/liveTask.test.ts`） |
| 全部 Rust 测试 | `cargo test` |
| 核心库测试（最快，~2s） | `cargo test -p deploy-core` |
| 快速编译检查 | `cargo check -p deploy-core` |
| 运行 CLI | `cargo run -p deploy-cli -- --help` |
| 构建 CLI release | `cargo build -p deploy-cli --release` |
| 打包桌面端（NSIS） | `npm run app:build`（产物：`target/release/bundle/nsis`） |
| 构建控制机侧 agent（Linux 二进制，供客户端上传用） | `cargo build -p deploy-agent --release --target x86_64-unknown-linux-musl`（产物：`target/x86_64-unknown-linux-musl/release/deploy-agent`，复制到 `src-tauri/agent/deploy-agent` 即随安装包内置；本机没有 musl 工具链，可跑 `.github/workflows/build-agent.yml` 手动出包再下载） |
| Windows 一键打包（含 PATH 处理） | `build-installer.bat` |

改动完成的最低校验：`npx tsc -p tsconfig.json --noEmit` + `cargo test -p deploy-core`（涉及 Tauri 壳时跑 `cargo check`；改动 `src/lib/` 纯逻辑时跑 `npm test`）。仓库无 ESLint/Prettier/CI，不要依赖自动门禁（`.github/workflows/build-agent.yml` 只是手动出 agent 二进制的，不是门禁）。

## 分层架构与数据流

```text
src/ (React)                 UI：页面/组件/Zustand store
  └─ src/lib/api.ts          invoke 类型化封装（唯一调用 Tauri 命令的入口）
src-tauri/src/commands/      薄适配层：参数校验、事件转发、状态登记
  └─ crates/deploy-core/     全部业务逻辑（git/ssh/engine/backup/container/pages/store/...）
crates/deploy-cli/           CLI：与 GUI 复用 deploy-core，同源同行为
crates/deploy-agent/         控制机侧（Linux/systemd）：装在那台常驻机上，读自己那份 config.json 到点执行
```

- **业务逻辑只能写在 `deploy-core`**，`commands/*.rs` 与 CLI 都只做适配；否则 GUI 与 CLI 行为会分叉。
- crate 依赖方向：`deploy-cli` / `src-tauri` / `deploy-agent` → `deploy-core`。`deploy-core` 禁止依赖 tauri/clap/GUI。
- 新增 Tauri 命令的标准四步：
  1. `deploy-core` 实现业务函数（如需）；
  2. `src-tauri/src/commands/<模块>.rs` 加 `#[tauri::command(async)] pub fn xxx(...) -> Result<T>`；
  3. `src-tauri/src/lib.rs` 的 `tauri::generate_handler![...]` 注册；
  4. `src/lib/api.ts` 加封装 + `src/lib/types.ts` 加类型。
- 前端路由在 `src/App.tsx`（懒加载）；新增页面必须加 `lazy(() => import(...))` 与 `<Route>`。

### 后端 → 前端事件

| 事件名 | 载荷类型 | 发送位置 | 前端消费 |
| --- | --- | --- | --- |
| `deploy://event` | `DeployEvent` | `commands/deploy.rs` | `App.tsx` → `store.handleDeployEvent` |
| `backup://event` | `BackupEvent` | `commands/backup.rs` | `App.tsx` → `store.handleBackupEvent` |
| `pages://event` | `PagesEvent` | `commands/pages.rs` | `App.tsx` → `store.handlePagesEvent` |
| `container://event` | `ContainerEvent` | `commands/container.rs` | `App.tsx` → `store.handleContainerEvent` |
| `scheduler://notice` | `SchedulerNotice` | `scheduler.rs` | `App.tsx` toast |
| `shutdown://status` | `ShutdownStatus` | `commands/shutdown.rs` | `App.tsx` → `store.setShutdownStatus` → `StatusBar` 倒计时 |
| `repo://fs-changed` | `{ repoId }` | `commands/watch.rs` | `RepoDetailPage.tsx` |

容器备份 / 迁移是第四类长任务（`container://event`），同样走 `applyTaskEvent` 与 `liveContainer`。扫描类命令（`list_compose_stacks` / `inspect_compose_stack`）**刻意不占任务名额**：一次迁移要跑几十分钟，不能因此连看都看不了，而这些调用只读 `docker ps` / `compose ls`，与正在进行的任务不冲突。

前端订阅统一用 `src/lib/useTauriEvent.ts` 的 `useTauriEvent<T>(事件名, 回调)`，不要在页面里手写 `listen + disposed + unlisten` 样板。长任务（部署/备份/Pages/容器）状态集中在 `src/lib/store.ts` 的 `live*` 字段，事件归约走 `applyTaskEvent`（四类任务共用，新增同类任务复用它）；重载后由 `loadAll()` 用持久化记录对账收敛，不要在页面里另建事件状态。

## 类型同步规则（Rust ↔ TypeScript）

⚠️ **重要**: Rust 模型与 TS 类型**手工镜像，没有代码生成**；改一侧必须同步另一侧！

| Rust 源 | TS 镜像 |
| --- | --- |
| `crates/deploy-core/src/models.rs`（绝大多数结构体/枚举） | `src/lib/types.ts` |
| `crates/deploy-core/src/cronjob.rs`（`CronJob` / `CronJobDraft` / `CronSchedule` / `CronJobRun`） | 同上（`src/lib/cronJob.ts` 另镜像 `CRON_METHODS` 顺序） |
| `crates/deploy-core/src/tunnel.rs`（`ServerTunnelStatus` / `TunnelRuleStatus`；`TunnelRule` 在 models.rs） | `src/lib/types.ts`（`src/lib/tunnel.ts` 只放草稿校验与展示用纯逻辑） |
| `crates/deploy-core/src/container/`（`discover.rs` 的 `ComposeStack` / `ComposeService` / `ComposeVolume` / `ComposeStackDetail`；`scripts.rs` 的 `BundleManifest`） | `src/lib/types.ts`（`ContainerRequest` / `ContainerRecord` / `ContainerEvent` 在 models.rs，`container.ts` 只放展示用纯逻辑） |
| `src-tauri/src/commands/shutdown.rs`（`ShutdownStatus` / `PendingShutdown` / `ShutdownSource`，定义在 `state.rs`） | 同上 |
| `crates/deploy-core/src/security.rs`（`SecurityReport` 等） | 同上 |
| `src-tauri/src/commands/repos.rs`（`RepoDetail`） | 同上 |
| 命令签名 | `src/lib/api.ts` 的 `api.xxx` |

约定：

- Rust 侧统一 `#[serde(rename_all = "camelCase")]`（事件枚举用 `tag = "type"`，见 models.rs 的 DeployEvent / BackupEvent / PagesEvent 四处）；TS 侧一律 camelCase，两边字段名逐字对应。
- Tauri 命令名用 snake_case（`list_repos`）；`invoke` 传参用 camelCase（`{ repoId }` 自动映射到 Rust 的 `repo_id`），不要把参数名写成 snake_case。
- 可选字段：Rust `Option<T>` ↔ TS `T | null`（不是 `undefined`）。
- 枚举值：Rust `DeployStatus::Success` ↔ TS `"success"`（camelCase 序列化）。
- 新增字段三步走：`models.rs` → `types.ts` → 消费处（含 `store.ts`/页面）；漏改编译器不一定报错（`api.ts` 泛型仅约束返回类型）。
- **AI 注意**: 修改 Rust 模型后，务必检查 TS 类型是否同步更新，不要依赖 IDE 提示（可能不报错但运行时类型错误）。

## 编码约定

### Rust

- 错误统一用 `CoreError`（`crates/deploy-core/src/error.rs`），消息面向用户、用中文写清楚原因与下一步；新增错误类别时加 variant + 构造方法，不要用 `unwrap`/`panic!` 处理用户输入或 IO。
- 任务互斥失败必须返回 `CoreError::busy(...)`（如调度器需要区分「稍后重试」与真实失败），调用方用 `matches!(err, CoreError::Busy(_))` 判断，**不要对错误消息做字符串匹配**。
- 本地外部命令走 `crates/deploy-core/src/process.rs` 的封装（`run` / `run_timeout` / `run_with_env` / `run_shell_stream`），不要直接 `Command::new`。
- **拼接远端 shell 命令必须用 `shell_quote()`**（process.rs 的 `shell_quote`），所有变量都要包裹；`~` 是字面量语义，不要手写引号。
- 配置读写统一走 `Store::mutate_config` / `upsert_history` / `upsert_backup` / `upsert_pages_record`，不要直接写 JSON 文件；读接口失败要区分「文件损坏」与「不存在」。
- 跨进程互斥用 `Store::try_task_lock`；GUI 内还会用 `AppState` 的 claim + `ClaimGuard`/`*TaskGuard`（`src-tauri/src/state.rs`），新增长任务必须沿用该模式并处理退出清理。
- 单测写在文件底部 `#[cfg(test)] mod tests`，用临时目录；涉及 git 的测试参考 `crates/deploy-core/src/git/mod.rs` 底部写法。
- 大文件已按职责拆分：git 在 `crates/deploy-core/src/git/{mod,files,archive}.rs`，CLI 在 `crates/deploy-cli/src/commands/*.rs`。新代码放进对应子模块，不要再堆回主文件。
- 注释用中文，解释「为什么」（边界、竞态、兼容原因），不复述代码；不要留 TODO/stub。

### 前端

- 基础组件优先用 `src/components/ui.tsx`（Button/Input/Select/Field/Badge/Modal/Toast 等），先看有没有可复用的再写新的。
- 颜色只用 Tailwind 语义 token（`bg-panel` / `text-ink` / `border-line` / `text-pos|neg|warn` …，定义在 `src/index.css` 的 `@theme`），主题由 `document` 上的 `data-theme` 切换；禁止硬编码 hex/调色板颜色。
- 全局状态放 `src/lib/store.ts`；页面局部表单/展开态用 `useState`。异步操作要处理竞态（参考 `RepoDetailPage.tsx` 的 `repoIdRef` + `cancelled` 模式），否则切换仓库会串数据。
- 长任务（部署/备份/Pages/容器）的事件归约与重载对账在 `src/lib/liveTask.ts`（`applyTaskEvent` / `reconcileLiveTask`），新增同类任务直接复用，不要在 store 里另写一遍。
- 页面私有子组件放同目录子文件夹（如 `src/pages/repoDetail/`、`src/pages/deploy/`），页面级状态逻辑优先抽成同目录 `useXxx.ts` hook（参考 `repoDetail/useFileFilter.ts`）。
- 新增纯逻辑（如 `src/lib/*.ts`）时补同名 `*.test.ts`（vitest，node 环境；依赖 i18n/api 的模块在测试里 `vi.mock`），组件层不做单测、靠 tsc + 手工验证。
- 所有用户可见文案必须走 `t("...")`，并**同时**在 `src/locales/zh-CN.json` 与 `en-US.json` 增加同名 key（两边必须 1:1，当前 1009 对全对齐）。两份文件是 2 空格缩进 + CRLF，用脚本改时先确认 `json.dumps(..., indent=2, ensure_ascii=False)` 能原样往返再写回（读文件要用二进制或 `newline=""`：文本模式会把 CRLF 悄悄折成 LF，往返检查看着通过、写回却改掉整份文件的行尾）。状态文案/尺寸/耗时格式化用 `src/lib/utils.ts`。
- TS 严格模式全开（`strict`、`noUnusedLocals`、`noUnusedParameters`），不允许 `any`/`@ts-ignore`；类型不匹配时改类型而不是断言。
- 导入用相对路径（无 `@/` 别名）；组件文件 PascalCase，工具/状态文件 camelCase。

## 安全红线（不可回退）

- 远端命令必须 `shell_quote` 后再拼接（`process.rs` 底部 `shell_quote_quotes_tilde_and_escapes` 测试保护）。
- CLI 的 JSON 输出必须脱敏：服务器用 `redact_server`、备份目标用 `redact_target`、备份配置用 `redact_backup_config`（`crates/deploy-cli/src/commands/mod.rs`）；新增输出路径时同步脱敏。
- 导出配置（`models.rs` 的 `ExportData::new`）同样必须抹掉全部凭据：SSH 认证、`servers[].db_backup.password`、`servers[].supabase_url`、`backup_targets[].url`、`backup_configs[].source.password` / `supabase_url`、4 个 settings Token、`master_password_hash`。这里抹成**空串 / None**，不是 CLI 的 `***` 占位符——导入按「空 = 本次没提供，保留本机值」合并（`store::keep_when_blank`），只遮密码会导出一条没有口令的连接串，反过来顶掉本机可用那条。两头分别由 `models.rs::export_blanks_every_credential` 与 `store.rs::export_blankout_does_not_erase_local_db_credentials` 守住。
- 提交拦截：`.env` / `*.pem` / `id_rsa` 等敏感文件默认阻止，需显式确认（GUI 二次确认、CLI `--allow-sensitive`）；不要绕过。
- 密码/密钥在本机 `config.json` 中为明文存储（已知取舍，README 有说明），不要"顺手"改成加密而破坏兼容。

## 已知限制与坑

- **部署 / 备份 / Pages / 容器都是「列表 + 弹窗」配置模型**：服务器部署配置存 `AppConfig.deploy_configs`（`DeployConfig`，deploy-core `Store::save_deploy_config` / `delete_deploy_config`），一条配置可带**多台**服务器（`server_ids`，顺序即批量执行顺序；旧数据里的单个 `serverId` 由 serde alias 读成一台）；备份配置存 `AppConfig.backup_configs`；Pages 配置存 `AppConfig.pages_configs`（部署页只做列表展示，`list_pages_configs` / `delete_pages_config` 是唯一新增入口）；容器备份配置存 `AppConfig.container_configs`。新增同类配置时沿用该模式，不要回到「表单常驻页面」。
- **批量部署：一份配置多台机器，失败即停**：串行循环在 `DeployEngine::run_targets`（业务逻辑不进命令层），任一台失败或准备失败即停止、剩余机器不部署。每台仍是一条独立 `DeployRecord`，事件按 recordId 分流，所以前端 `applyTaskEvent` 不为批次另建状态、`live.recordId` 由 `started` 事件补齐。GUI 唯一入口是命令 `deploy_config_targets(config_id, server_ids)`，它只接受配置内已登记的目标、返回本批台数，部署页先弹目标勾选框（默认全选）；整批共用一次 `ClaimKind.Deploy` 抢占和一个任务句柄，`DeployBatchTracking` 随每台开始换绑登记项，取消当前这台即 abort 整个循环。CLI 不带 `--server` 时走同一条 `run_targets`。
- **Nginx 模块不落盘配置**：`crates/deploy-core/src/nginx.rs` 直接通过 SSH 在服务器上 `docker ps/exec/cp` 管理容器内 `*.conf`（默认 `/etc/nginx/conf.d`），保存 / 删除前跑 `nginx -t`，失败自动回滚；短操作不占任务锁、不发事件。新增容器侧操作时沿用 `NginxEngine` 与 `validate_container/validate_dir/validate_file_name`，并使用 `shell_quote`。
- **定时请求不落本地盘**：`crates/deploy-core/src/cronjob.rs` 是 cron-job.org 的 REST 客户端，任务只存在云端，本地仅 `settings.cronjobApiKey`。官方 API 默认 100 次/天，所以 `CronJobsPage` 只在进入页面和手动刷新时拉取，**不要加轮询**；开关状态改本地副本，不为此多花一次配额。它没有 run-now/pause 端点（暂停走 `PATCH {enabled:false}`），执行结果靠 `GET /jobs/{id}/history` 看。cron 表达式 ↔ 它的 `schedule` 整数数组只在 Rust 侧转换（`cron_to_schedule` / `schedule_to_cron`），前端不要重写一份。Cloudflare Workers Cron Triggers 暂无公开 API（只能 wrangler 或控制台配置，免费计划每账号 5 条 cron），未接入。
- **定时关机：倒计时在应用内，最后 20 秒交给系统**：`crates/deploy-core/src/shutdown.rs` 只负责校验分钟数与 `shutdown /s /t N /f` 的 argv；排定状态存在 `AppState.pending_shutdown`，到点由 `scheduler.rs` 取出（与「取消」共用一把锁，二者不会两头落空）→ 下发关机 → `app.exit(0)` 走既有退出清理，`OS_GRACE_SECS` 就是给清理留的窗口，所以排定后轮询从 20s 切到 1s（`COUNTDOWN_INTERVAL`）。三点注意：① 只在软件运行时生效，错过的时间点不补跑（唤醒电脑就被关机不可接受），`shutdown_last_run` 按调度日期去重、用户取消后当天不再排；② 不带 `/c`，这台机器的 `shutdown /?` 把 `/c` 归在 `/d` 说明下，参数被拒就等于到点不关机；③ 取消只在应用内有效，下发之后的系统级倒计时不做 `shutdown /a`（那会连用户自己排的一起撤销）。非 Windows 返回 `CoreError::Process`，设置页整块由 `isWindows` 隐藏。
- **容器备份与迁移：项目现扫，配置落盘**：`crates/deploy-core/src/container/`（`mod.rs` 引擎 / `discover.rs` 只读发现 / `scripts.rs` 备份包清单与两份远端 bash 脚本）。compose 项目**不落本地配置**，扫描列表每次进页面现扫（唯一来源是服务器上的容器标签）；持久化的是任务记录 `containers.json`、本机备份包目录 `<数据目录>/containers/`，以及 `AppConfig.container_configs`（`ContainerConfig`，可一键执行也可挂定时）。增删改走 `Store::save_container_config` / `delete_container_config`（校验服务器、项目、卷/镜像至少一项、目标目录为绝对路径），执行走命令 `start_container_config_backup(config_id)`——它与定时调度器共用一条路径，参数只从盘上的配置取，不在前端拼。删服务器时两端都要调 `Store::detach_server_from_container_configs`：来源被删整条删除，只当过迁移目标的降级成纯备份。快照与恢复都是「上传脚本 -> bash 跑 -> 解析 `###STAGE` / `###VOL` / `###SIZE` / `###DONE` 标记」，脚本用 base64 落盘避开 here-doc 与引号转义；**模板里不能出现裸的 `__`**，`render_template` 会把 `__X__` 当占位符吃掉。迁移刻意不动来源服务器（不停不删），回退靠源机自己 `compose up`。匿名卷与项目目录之外的 bind mount 带不走，只能提示（compose 重建容器时不会挂回旧匿名卷，这是 Docker 语义）。
- **容器定时备份是第二个调度循环**：`scheduler.rs::spawn_container` 与数据库备份的 `spawn` 分开跑，因为一条容器任务动辄几十分钟，「等上一条结束再取下一条」的排队节奏和 20 秒一次的触发判定混在一个循环里只会互相缠绕。设置是 `scheduled_container_enabled` / `_time` / `_config_ids`（列表，顺序即当晚执行顺序），到点把勾选的配置排成队列，靠 `has_active_container()` 推进，通知走同一个 `scheduler://notice`（`containerStarted` / `containerNoConfig` / `containerFailed`）。三点：① 10 分钟重试窗口只约束「一次都还没跑成」的等待，跑成第一条之后窗口就解除，否则一条 40 分钟的任务会把后面的队列全掐掉；② 定时**照配置原样执行**，配了目标就会迁移到另一台机器并 `compose up -d`；③ 定时会把备份包堆在本机，所以有下面的轮转。
- **容器备份包轮转在引擎的成功收尾处**：`container/mod.rs::prune_bundles` 由 `ContainerEngine::run` 在任务成功后调用（手动、定时、CLI 都走这一条），按 **(server_id, project)** 分组只保留最近 `settings.containerBundleKeep` 个包（默认 2，0 = 关闭轮转），窗口之外的删文件并把那条记录的 `bundle_path` 抹空——记录本身留在列表里（日志与耗时还有用），而界面的「恢复到目标服务器」入口判的就是 `record.bundlePath`，路径抹空后按钮自动收起，不会留下点了才报错的悬空项。三条边界：① 只在成功时跑，失败那次的半截包不占窗口也不触发删除；② 本次的记录不参与候选（按 id 排除），窗口按 `keep - 1` 算，新包永远不会被自己挤掉；③ 只删 `<数据目录>/containers/` 的**直接子文件**，用户自己挪出去的包、别的项目的包、`Restore` 类型的记录一律跳过；`remove_file` 失败就原样留着下次再试。
- **SSH 隧道：规则挂在服务器上，开机即自动接**：`crates/deploy-core/src/tunnel.rs`。每台服务器的规则存 `ServerConfig.tunnels`（`TunnelRule` = 本机端口 + 远端 `host:port` + 启用开关，不另存一份配置表），有启用项的服务器在 `lib.rs` 的 setup 里由 `TunnelManager::sync` 拉起常驻任务：绑本机端口 → `SshClient::connect_tunnel` → 每条本机连接开一个 direct-tcpip 通道（等价 `ssh -L`）；会话中断由 `watch_liveness` 靠 `Handle::is_closed()` 探测，3s 起指数退避重来、上限 60s，单次撑过 60s 就把退避退回起点。五条口径：① 绑端口和连服务器在**同一个循环里**，启动那会儿端口被别的应用占着（开机顺序不可控）或用户连点两次「重新连接」（旧监听还没来得及释放）都必须下一轮自己补回来，所以端口绑定失败不许 return、只记错误；② 隧道是长期空闲连接，必须开 SSH keepalive + TCP_NODELAY（`connect_tunnel`），否则半开时 `is_closed()` 永远为假、界面谎报「已连接」而每个请求干等——部署/备份那种几分钟就结束的短连接仍走 `connect`，不要一起改；③ 改规则走 `save_server_tunnels`（只收 server_id + 规则列表，不收整份服务器配置，免得表单里的旧凭据被盖回去），`sync` 用 `task_fingerprint`（主机 + 端口 + 凭据 + 启用规则的哈希）决定要不要重建，所以**改凭据会重连、改名不会**；④ 本机端口全局唯一，`assert_local_ports_free` 在保存时就跨服务器查重，宁可拒绝也不留到运行时静默失败；⑤ 只在软件运行期间有效（隐藏到托盘仍算运行），没做系统级开机自启；状态由 `list_tunnel_status` 供 `ServersPage` / `TunnelModal` 用 `useAutoRefresh` 拉取（纯内存读，不推事件、不轮询）。未验证面积：真机转发包与高并发（所有连接共用一条 SSH 会话，通道上限受服务端 sshd 配置约束）；单测覆盖的是端口占用/释放、绑定失败自愈、连不上时不谎报、老配置兼容与状态字段名。
- **定时备份整条搬到控制机**：`crates/deploy-core/src/agent.rs`（下发协议 + 管理端）与 `crates/deploy-agent/`（控制机上的常驻进程：`run` 跑两条调度循环，`trigger` / `restore` 由客户端经 SSH exec 临时起，`status` / `records` 回读）。一台控制机管多台源机，凭据与配置存在它自己那份 `/var/lib/deploycode/config.json` 里，到点不再依赖这台 Windows 开着机。七条口径：① 执行位是 `RunLocation`（`BackupConfig` / `ContainerConfig` 各带一个），**本机一律不代跑 remote 配置**——`assert_runs_here` 挡在 GUI 命令、CLI 与定时三处同一个口径上（包落错机器比报错难查得多），界面的「立即备份」按执行位分流到 `start_agent_backup` / `start_agent_container`；② 本机让不让位只认 `agent-sync.json` 里那份指纹（单独一份文件，因为 `save_settings` 是前端整体回写），而且必须在远端确认写入成功之后才记——记早了本机撒手而控制机没配置，记晚了反过来谎报「未下发」；③ 卸载 agent 与删除这台服务器共用 `Store::forget_agent_server`（收回执行位 + 清指向 + 作废指纹），指针对不上时一概不动，`ExportData` 也不带走这两样；④ 定时的 HH:MM 在下发那一刻按控制机的 `date +%z` 折算（`shift_for_agent`），前端不要另写一份换算；⑤ `trigger` 的事件泵必须待在 `select!` 的分支里（不在场就等于没有实时输出、"客户端断开"永远判不出来），中止时整条引擎 future 被 drop，所以控制机自己那条 Running 记录要显式 `abandon_backup_record` / `abandon_container_record` 收尾（`reconcile_interrupted` 此刻用不上——任务锁还在我们手上），半截 `.part` 由 `PartGuard` 的 Drop 收；⑥ 磁盘水位闸（`disk.rs`）只在 `Store::set_agent_mode(true)` 那一侧拦，本机不拦；⑦ 要上传的 agent 二进制**随安装包内置**：`tauri.conf.json` 的 `bundle.resources: ["agent/*"]`，产物落在 `<资源目录>/agent/deploy-agent`，由 `lib.rs` 在 setup 里登记给 `Store::set_bundled_agent_binary`（`deploy-core` 不许依赖 tauri，问不到资源目录）。`locate_binary` 只有两档：`<数据目录>/agent/deploy-agent`（开发期换产物用，**压过内置**，否则本地新编的那份永远传不上去）→ 安装包内置那份；谁在用哪个由界面那一行直接写出来（命令 `agent_binary_info`，只读展示，**界面上没有路径输入框了**）。不要再加回 `settings.agent_binary_path`：那是本机文件位置，写进配置就会被导出/导入带到别的机器上，而指错之后一路没有入口清掉。两档候选都要过 ELF 头检查（这台 Windows 上编出来的 `deploy-agent.exe` 传上去只会让 systemd 起不来，而那时旧服务已经被 stop 掉了）。写 glob 而不是裸路径，因为 `tauri-build` 在编译期就校验资源存在性，缺文件会连 `cargo check` 一起挂掉；`src-tauri/agent/` 只有 README 进仓库、二进制已 gitignore，`build-installer.bat` 缺它时问一句再决定要不要继续打包。未验证面积：真机 SSH 事件流（实时输出、断开即中止、记录收敛）从未实测，「内置产物 → 装到控制机 → 跑通一次备份」这条整链也没验过；单测覆盖的是下发内容的脱敏与时区折算、指纹与让位判定、守护进程两条循环的推进与状态回读形状、agent 二进制的查找优先级与 ELF 校验。
- **仍然偏大的文件**：`crates/deploy-core/src/git/mod.rs`（~1420 行，含测试）、`src/pages/RepoDetailPage.tsx`（~1250 行，主组件 hook 仍多）、`backup.rs`（~1830 行）、`engine.rs`（~1410 行）、`pages.rs`（~1330 行）、`container/mod.rs`（~1600 行，含测试；发现与备份包脚本已拆到 `container/discover.rs` 与 `container/scripts.rs`）、`agent.rs`（~1660 行，含测试）、`src/pages/DeployPage.tsx`（~1040 行）、`src/pages/ContainersPage.tsx`（~760 行，配置列表 / 配置弹窗 / 定时区 / 详情块 / 恢复弹窗都在 `src/pages/containers/`）。修改前先定位到具体函数，尽量复用已有子组件/hook；`RepoDetailPage` 的编辑器与搜索已拆到 `src/pages/repoDetail/`，提交图是主区域底部的停靠面板（左活动栏 `railGraph` 开关，复用 `src/components/CommitGraph.tsx` + `src/lib/graph.ts` 布局），`DeployPage` 的部署 / Pages 弹窗在 `src/pages/deploy/`，备份弹窗在 `src/pages/backup/`，本批目标勾选与配置表单共用 `src/components/ServerCheckList.tsx`。
- **测试覆盖范围**：前端只有 `src/lib/*.test.ts`（vitest：liveTask/utils/graph/store/cronJob/shutdown/selection/container/agent/tunnel，105 个用例），组件与 Tauri 交互仍靠 `tsc` + 手工验证；Rust 单测集中在 deploy-core（204）与 deploy-agent（17，覆盖两条调度循环与回读形状）、src-tauri 少量模块。无 lint/CI。
- **无图标/文案自动校验**：i18n key 漏加不会报错，只在界面显示原始 key，注意自查。
- **Git 历史信息量低**（提交信息多为「优化」），不要依赖 `git blame` 理解设计，以本文件与代码注释为准。
- `.cargo/config.toml` 的 `rustflags = ["--cfg", "deploycode"]` 是为绕过 360 误拦截，删除会导致构建路径回归被封锁目录。
- 数据目录：Windows 为 `%APPDATA%\deploycode\DeployCode\data`，可用 `cargo run -p deploy-cli -- where` 查看；`--data-dir` 可指定独立目录（调试时建议用临时目录，避免污染真实配置）。
