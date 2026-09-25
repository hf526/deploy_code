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
| Windows 一键打包（含 PATH 处理） | `build-installer.bat` |

改动完成的最低校验：`npx tsc -p tsconfig.json --noEmit` + `cargo test -p deploy-core`（涉及 Tauri 壳时跑 `cargo check`；改动 `src/lib/` 纯逻辑时跑 `npm test`）。仓库无 ESLint/Prettier/CI，不要依赖自动门禁。

## 分层架构与数据流

```text
src/ (React)                 UI：页面/组件/Zustand store
  └─ src/lib/api.ts          invoke 类型化封装（唯一调用 Tauri 命令的入口）
src-tauri/src/commands/      薄适配层：参数校验、事件转发、状态登记
  └─ crates/deploy-core/     全部业务逻辑（git/ssh/engine/backup/pages/store/...）
crates/deploy-cli/           CLI：与 GUI 复用 deploy-core，同源同行为
```

- **业务逻辑只能写在 `deploy-core`**，`commands/*.rs` 与 CLI 都只做适配；否则 GUI 与 CLI 行为会分叉。
- crate 依赖方向：`deploy-cli` / `src-tauri` → `deploy-core`。`deploy-core` 禁止依赖 tauri/clap/GUI。
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
| `scheduler://notice` | `SchedulerNotice` | `scheduler.rs` | `App.tsx` toast |
| `repo://fs-changed` | `{ repoId }` | `commands/watch.rs` | `RepoDetailPage.tsx` |

前端订阅统一用 `src/lib/useTauriEvent.ts` 的 `useTauriEvent<T>(事件名, 回调)`，不要在页面里手写 `listen + disposed + unlisten` 样板。长任务（部署/备份/Pages）状态集中在 `src/lib/store.ts` 的 `live*` 字段，事件归约走 `applyTaskEvent`（三类任务共用，新增同类任务复用它）；重载后由 `loadAll()` 用持久化记录对账收敛，不要在页面里另建事件状态。

## 类型同步规则（Rust ↔ TypeScript）

⚠️ **重要**: Rust 模型与 TS 类型**手工镜像，没有代码生成**；改一侧必须同步另一侧！

| Rust 源 | TS 镜像 |
| --- | --- |
| `crates/deploy-core/src/models.rs`（绝大多数结构体/枚举） | `src/lib/types.ts` |
| `crates/deploy-core/src/cronjob.rs`（`CronJob` / `CronJobDraft` / `CronSchedule` / `CronJobRun`） | 同上（`src/lib/cronJob.ts` 另镜像 `CRON_METHODS` 顺序） |
| `crates/deploy-core/src/security.rs`（`SecurityReport` 等） | 同上 |
| `src-tauri/src/commands/repos.rs`（`RepoDetail`） | 同上 |
| 命令签名 | `src/lib/api.ts` 的 `api.xxx` |

约定：

- Rust 侧统一 `#[serde(rename_all = "camelCase")]`（事件枚举用 `tag = "type"`，见 models.rs:20/515/590/658）；TS 侧一律 camelCase，两边字段名逐字对应。
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
- **拼接远端 shell 命令必须用 `shell_quote()`**（process.rs:477），所有变量都要包裹；`~` 是字面量语义，不要手写引号。
- 配置读写统一走 `Store::mutate_config` / `upsert_history` / `upsert_backup` / `upsert_pages_record`，不要直接写 JSON 文件；读接口失败要区分「文件损坏」与「不存在」。
- 跨进程互斥用 `Store::try_task_lock`；GUI 内还会用 `AppState` 的 claim + `ClaimGuard`/`*TaskGuard`（`src-tauri/src/state.rs`），新增长任务必须沿用该模式并处理退出清理。
- 单测写在文件底部 `#[cfg(test)] mod tests`，用临时目录；涉及 git 的测试参考 `crates/deploy-core/src/git/mod.rs` 底部写法。
- 大文件已按职责拆分：git 在 `crates/deploy-core/src/git/{mod,files,archive}.rs`，CLI 在 `crates/deploy-cli/src/commands/*.rs`。新代码放进对应子模块，不要再堆回主文件。
- 注释用中文，解释「为什么」（边界、竞态、兼容原因），不复述代码；不要留 TODO/stub。

### 前端

- 基础组件优先用 `src/components/ui.tsx`（Button/Input/Select/Field/Badge/Modal/Toast 等），先看有没有可复用的再写新的。
- 颜色只用 Tailwind 语义 token（`bg-panel` / `text-ink` / `border-line` / `text-pos|neg|warn` …，定义在 `src/index.css` 的 `@theme`），主题由 `document` 上的 `data-theme` 切换；禁止硬编码 hex/调色板颜色。
- 全局状态放 `src/lib/store.ts`；页面局部表单/展开态用 `useState`。异步操作要处理竞态（参考 `RepoDetailPage.tsx` 的 `repoIdRef` + `cancelled` 模式），否则切换仓库会串数据。
- 长任务（部署/备份/Pages）的事件归约与重载对账在 `src/lib/liveTask.ts`（`applyTaskEvent` / `reconcileLiveTask`），新增同类任务直接复用，不要在 store 里另写一遍。
- 页面私有子组件放同目录子文件夹（如 `src/pages/repoDetail/`、`src/pages/deploy/`），页面级状态逻辑优先抽成同目录 `useXxx.ts` hook（参考 `repoDetail/useFileFilter.ts`）。
- 新增纯逻辑（如 `src/lib/*.ts`）时补同名 `*.test.ts`（vitest，node 环境；依赖 i18n/api 的模块在测试里 `vi.mock`），组件层不做单测、靠 tsc + 手工验证。
- 所有用户可见文案必须走 `t("...")`，并**同时**在 `src/locales/zh-CN.json` 与 `en-US.json` 增加同名 key（两边必须 1:1，当前 720 对全对齐）。状态文案/尺寸/耗时格式化用 `src/lib/utils.ts`。
- TS 严格模式全开（`strict`、`noUnusedLocals`、`noUnusedParameters`），不允许 `any`/`@ts-ignore`；类型不匹配时改类型而不是断言。
- 导入用相对路径（无 `@/` 别名）；组件文件 PascalCase，工具/状态文件 camelCase。

## 安全红线（不可回退）

- 远端命令必须 `shell_quote` 后再拼接（有测试 `process.rs:515` 保护）。
- CLI 的 JSON 输出必须脱敏：服务器用 `redact_server`、备份目标用 `redact_target`、备份配置用 `redact_backup_config`（`crates/deploy-cli/src/commands/mod.rs`）；新增输出路径时同步脱敏。
- 提交拦截：`.env` / `*.pem` / `id_rsa` 等敏感文件默认阻止，需显式确认（GUI 二次确认、CLI `--allow-sensitive`）；不要绕过。
- 密码/密钥在本机 `config.json` 中为明文存储（已知取舍，README 有说明），不要"顺手"改成加密而破坏兼容。

## 已知限制与坑

- **部署 / 备份 / Pages 都是「列表 + 弹窗」配置模型**：服务器部署配置存 `AppConfig.deploy_configs`（`DeployConfig`，deploy-core `Store::save_deploy_config` / `delete_deploy_config`），备份配置存 `AppConfig.backup_configs`，Pages 配置仍按仓库存在 `RepoConfig.pages`（部署页只做列表展示，`list_pages_configs` / `delete_pages_config` 是唯一新增入口）。新增同类配置时沿用该模式，不要回到「表单常驻页面」。
- **Nginx 模块不落盘配置**：`crates/deploy-core/src/nginx.rs` 直接通过 SSH 在服务器上 `docker ps/exec/cp` 管理容器内 `*.conf`（默认 `/etc/nginx/conf.d`），保存 / 删除前跑 `nginx -t`，失败自动回滚；短操作不占任务锁、不发事件。新增容器侧操作时沿用 `NginxEngine` 与 `validate_container/validate_dir/validate_file_name`，并使用 `shell_quote`。
- **定时请求不落本地盘**：`crates/deploy-core/src/cronjob.rs` 是 cron-job.org 的 REST 客户端，任务只存在云端，本地仅 `settings.cronjobApiKey`。官方 API 默认 100 次/天，所以 `CronJobsPage` 只在进入页面和手动刷新时拉取，**不要加轮询**；开关状态改本地副本，不为此多花一次配额。它没有 run-now/pause 端点（暂停走 `PATCH {enabled:false}`），执行结果靠 `GET /jobs/{id}/history` 看。cron 表达式 ↔ 它的 `schedule` 整数数组只在 Rust 侧转换（`cron_to_schedule` / `schedule_to_cron`），前端不要重写一份。Cloudflare Workers Cron Triggers 暂无公开 API（只能 wrangler 或控制台配置，免费计划每账号 5 条 cron），未接入。
- **仍然偏大的文件**：`crates/deploy-core/src/git/mod.rs`（~1300 行，含测试）、`src/pages/RepoDetailPage.tsx`（~1130 行，主组件 hook 仍多）、`backup.rs`（~1600 行）、`engine.rs`（~1340 行）、`pages.rs`（~1300 行）、`src/pages/DeployPage.tsx`（~770 行）。修改前先定位到具体函数，尽量复用已有子组件/hook；`RepoDetailPage` 的编辑器与搜索已拆到 `src/pages/repoDetail/`，`DeployPage` 的部署 / Pages 弹窗在 `src/pages/deploy/`，备份弹窗在 `src/pages/backup/`。
- **测试覆盖范围**：前端只有 `src/lib/*.test.ts`（vitest：liveTask/utils/graph/store/cronJob，54 个用例），组件与 Tauri 交互仍靠 `tsc` + 手工验证；Rust 单测集中在 deploy-core 与 src-tauri 少量模块。无 lint/CI。
- **无图标/文案自动校验**：i18n key 漏加不会报错，只在界面显示原始 key，注意自查。
- **Git 历史信息量低**（提交信息多为「优化」），不要依赖 `git blame` 理解设计，以本文件与代码注释为准。
- `.cargo/config.toml` 的 `rustflags = ["--cfg", "deploycode"]` 是为绕过 360 误拦截，删除会导致构建路径回归被封锁目录。
- 数据目录：Windows 为 `%APPDATA%\deploycode\DeployCode\data`，可用 `cargo run -p deploy-cli -- where` 查看；`--data-dir` 可指定独立目录（调试时建议用临时目录，避免污染真实配置）。
