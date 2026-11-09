# DeployCode · 分支部署工具

DeployCode 是一个本地多仓库管理与 SSH 运维工具：管理本地 Git 仓库和远端 Linux 服务器，把指定分支 / 标签 / 提交打包部署到服务器并执行项目脚本；同时提供服务器工作台（常驻 SSH、交互式终端、SFTP 文件传输、端口转发）、数据库备份、容器备份与迁移、Pages 发布、Nginx 配置管理和 cron-job.org 定时请求。

入口是 **Tauri 2 桌面客户端**，配置与各类记录保存在本机数据目录。侧边栏按用途分四组：**发布**（仓库、部署）、**服务器**（服务器、Nginx）、**备份与定时**（数据库备份、容器备份、定时请求）、**本机**（记录、设置）。

## 功能特性

### 系统托盘

- 关闭窗口（点 X）只隐藏到系统托盘，程序继续后台运行；托盘图标左键点击可重新显示窗口
- 托盘菜单支持「显示主窗口」与「退出程序」，只有「退出程序」才彻底退出（退出前会终止本地子进程并回收服务器上仍在执行的部署 / 备份 / 容器脚本）
- 托盘菜单与提示文案跟随界面语言（中文 / English）

### 仓库管理

- 添加本地文件夹（IDE 式：选目录即用，不要求已经是 Git 仓库），支持多仓库标签页
- 从远端 URL 克隆仓库；非 Git 文件夹可绑定远端地址（origin），自动 `git init` 后绑定
- 分支切换 / 创建 / 删除，查看远程分支
- 提交图：最近 200 条跨分支提交，显示分支 / 标签引用，点分支标签即可切换；合并提交画成菱形，HEAD 与当前分支高亮
- 工作区状态与文件 diff（左栏「源代码管理」面板），可提交 / 硬回退
- 文件树浏览、文件预览与编辑、内容搜索与批量替换
- fetch / pull / push
- 提交保护：自动检测 `.env` / `*.pem` / `id_rsa` / `credentials` 等疑似敏感文件，默认阻止提交，界面二次确认后才放行
- 工作区文件监听，外部改动自动刷新界面

### 服务器工作台

- SSH 密码 / 私钥（含 passphrase）认证，连接测试
- 每台服务器一条常驻 SSH 会话，断线自动退避重连；软件隐藏到托盘后仍保持
- 主机资源占用常驻采集：会话在线时约每 5 秒读取一次远端 `/proc` 的 CPU 与内存；采不到时显示「占用未取到」并给出原因，不会拿旧读数冒充实时
- 交互式终端：多标签、断线重连、分屏、搜索（区分大小写 / 逐条跳转）、字号调整、复制 / 粘贴快捷键；后台标签有新输出时点亮活动标记；工具栏提供快捷键帮助浮层
- 终端文件传输：SFTP 上传 / 下载，带进度与取消；远端 `rz` / `sz` 手势只弹提示（不实现 ZMODEM，文件请走 SFTP）
- 服务器列表支持紧凑 / 详细两档密度；行内悬停可快速打开终端或重连，右键菜单可直接上传 / 下载文件、复制 SSH 地址（不必先开标签）
- 端口转发（隧道）：一条规则 = 本机端口 → 远端 `host:port` + 启用开关；本机端口跨服务器查重，界面显示在线 / 重连中 / 断开 / 未知并可手动重连
- 安全检查：失败登录统计（按账号 + IP）、成功登录、在线会话、sshd 关键配置、防火墙状态（ufw / firewalld / iptables）
- 一键拉黑 / 解除 IP、踢出在线会话
- 服务器端自动防护：写入 guard 脚本与 `/etc/cron.d/deploycode-guard` 每分钟任务，按失败登录阈值自动封禁；阈值、统计窗口与免封白名单可配置，启用时校验来源地址避免把当前 SSH 会话封掉

### Nginx

- 选择服务器后发现运行 nginx 的容器，列出配置目录（默认 `/etc/nginx/conf.d`，可修改）下的 `*.conf`
- 新建 / 编辑 / 删除配置：保存与删除前先跑 `nginx -t` 校验，失败自动回滚旧内容；通过后自动 `nginx -s reload`，也可手动重载

### 一键部署

- 部署页统一管理「服务器部署」与「Pages 部署」配置列表，可新增 / 编辑 / 复制 / 删除 / 一键执行
- 服务器部署配置：仓库、目标服务器（可多台，顺序即批量执行顺序）、部署目录、版本（分支 / 标签 / 提交；留空则打包当前工作区，含未提交改动并遵循 `.gitignore`）、脚本（目录、指定脚本、是否执行）、环境文件替换
- 部署流程：`git archive` 打包 → tar.gz → SFTP 上传（带进度）→ 远端解压 → 上传并替换环境文件 → 执行脚本
- 上传后比对本地 / 服务器 SHA-256，防止半包上线（服务器缺少校验工具时自动跳过）
- 批量部署：发起前勾选目标服务器（默认全选），每台一条部署记录；任一台失败或准备失败即停止，剩余机器不再部署，并在界面明确标记批次中止
- 部署过程中可「停止部署」：中止本地任务并终止远端脚本进程组、清理残留压缩包；脚本超时或应用退出时同样回收远端脚本
- 原子发布（设置中开启）：解压到「部署目录/releases/<版本>」，脚本全部成功后原子切换 `current` 软链；失败不影响线上版本，可在记录页回滚到任意历史版本
- 脚本目录默认 `docker/`，按顺序执行其中的 `.sh`；注入环境变量 `DEPLOY_BRANCH` / `DEPLOY_REV` / `DEPLOY_COMMIT` / `DEPLOY_TARGET`，原子发布时另有 `DEPLOY_RELEASE` / `DEPLOY_CURRENT`
- 部署记录持久化（版本、服务器、完整日志），可一键重新部署

### Pages 部署

- 按仓库选择平台：Cloudflare Pages（`npx wrangler`）或 GitHub Pages（`npx gh-pages`）
- 配置项目名、构建命令、输出目录、生产分支（Cloudflare）或发布分支（GitHub，默认 `gh-pages`）
- 一键「构建并部署」，或跳过构建直接上传已有产物；实时日志，Cloudflare 部署地址自动解析
- 环境检查：Cloudflare 校验 wrangler / Token / Account，GitHub 校验远端可访问
- API Token 通过环境变量传递，不进入命令行；GitHub 平台复用本机 Git 凭据
- 发布前扫描产物目录：命中敏感文件（`.env` / `*.pem` / `*.key` / `credentials.json` 等）直接拒绝发布
- 部署记录持久化；两种平台都依赖本机 Node.js（通过 `npx` 调用）

### 数据库备份

把服务器上的 PostgreSQL schema 全量同步到远端 PostgreSQL（Supabase / Aiven / Neon / 自建），全过程在服务器上完成，不经过本机。

- 备份配置列表化：名称 + 服务器 + 来源 + 目标；可新增 / 编辑 / 复制 / 删除 / 立即执行
- 来源支持 Docker 容器（`docker exec pg_dump`）或服务器本机 `pg_dump`，可配置库名 / 用户 / 密码 / schema
- 备份目标在「数据库备份」页管理：可添加多个 PostgreSQL 连接串并指定全局默认目标
- 已保存的配置必须有自己的目标（或本次请求显式指定）：目标丢失时直接报错并点名该配置，不会静默换库
- 单条立即备份，或勾选多条串行批量执行（某条失败不影响后续，结束后给出汇总）
- 定时备份：每天固定时间执行一条选定的配置；应用运行期间生效（窗口关闭后仍在托盘运行），软件启动前已错过的时间点不补跑
- 全量覆盖：恢复前清空目标 schema 再导入；Supabase 的 anon / authenticated / service_role 授权自动恢复
- 环境检查（pg_dump 版本 + 目标连通性）、实时日志与进度、备份记录持久化（可查看 / 删除 / 清空）
- 可选在本机保留最近 N 份导出包（0 = 不留）；未被任何记录认领的备份包可在设置页手动清理

### 容器备份与迁移

- 选择服务器后扫描 `docker compose` 管理的项目（`docker compose` / `docker-compose` 均可），查看服务、卷与镜像
- 一个 compose 项目打包成一个本机备份包（`.tar`）：项目目录（compose 文件、`.env`、bind mount 的本地文件、build 上下文）+ 命名卷 tar + 镜像 tar + `manifest.json`
- 两个出口：只备份到本机；或备份完成后迁移到另一台服务器并 `compose up -d`（保持原项目名，命名卷才能挂回正确挂载点）
- 可用本机已有备份包再次恢复到服务器（需显式确认同名卷会被覆盖）
- 选项：是否包含卷 / 镜像；打包前是否暂停源服务（数据库类服务需要一致快照，卷导完立即恢复）
- 预检与提示：compose 可用性、远端磁盘空间、卷的读取方式（宿主机直接读或容器内 `tar`）；匿名卷与项目目录之外的 bind mount 会明确提示带不走
- 快照与恢复执行可中断的远端脚本（pidfile + 进程组终止），实时进度；任务记录持久化
- 恢复到目标机时若同名项目正在运行，会先停止它再覆盖卷，完成后重新启动；失败时若数据已被改动则保持停止并提示人工确认
- 备份包按（服务器, 项目）保留最近 N 个（默认 2，0 = 不轮转）；轮转只清理本机备份包目录，记录仍保留
- 定时容器备份：每天固定时间按顺序执行勾选的多条配置；应用运行期间生效（窗口关闭后仍在托盘运行），软件启动前已错过的时间点不补跑

### 定时请求（cron-job.org）

- 用 API Key 管理 cron-job.org 上的定时 HTTP 请求：新建 / 编辑 / 删除 / 启用停用 / 查看执行历史
- 支持 HTTP 方法、请求头、请求体、时区、超时时间与常用 cron 预设；cron 表达式与 cron-job.org 的 schedule 映射在 Rust 侧转换
- 任务只存在云端，本地仅保存 API Key；官方 API 默认 100 次/天，所以页面只在进入与手动刷新时拉取，不做轮询
- 没有 run-now 端点（暂停通过 `enabled=false`），执行结果查看任务历史

### 定时关机（Windows）

- 设置 → 启动与自动化：每天固定时间到点进入 60 秒可取消倒计时，结束后先正常退出程序（终止本地子进程、回收服务器上仍在执行的部署 / 备份 / 容器脚本），再下发系统关机
- 手动倒计时关机：1–1440 分钟后关机，可在设置页或底部状态栏取消
- 只在软件运行期间生效（窗口隐藏到托盘也算运行）；软件启动前已错过的时间点不补跑
- 刚唤醒电脑或检测到时钟跳变时放弃本次关机，避免掀开盖子就被关机；底部状态栏常驻显示剩余时间与取消按钮
- 非 Windows 不显示该功能

### 记录

- 部署记录：按仓库筛选、查看完整日志、重新部署、删除 / 清空
- 开启原子发布时，可在记录中查看历史版本并一键回滚

### 设置

- 外观：界面主题（浅色 / 深色 / 跟随系统）与界面语言（中文 / English）
- 启动与自动化：开机自启动（Windows 注册表 `Run`，无需管理员权限）、定时关机
- 云端凭据：Cloudflare（API Token + Account ID）、GitHub Token、cron-job.org API Key
- 数据目录：查看 / 打开本机数据目录，清理未被任何记录认领的备份包
- 配置备份：导出全部配置（凭据脱敏）与导入配置（按 ID 合并，空值不覆盖本机凭据）
- 各业务模块自己的参数跟着模块页走：部署默认值在部署页、备份目标在数据库备份页、容器保留与轮转在容器页

## 技术栈

| 层 | 技术 |
| --- | --- |
| 前端 | React 19 · TypeScript · Vite 8 · Tailwind CSS 4 · Zustand · React Router · xterm.js · highlight.js · i18next |
| 桌面 | Tauri 2（NSIS 打包） |
| 后端 | Rust · tokio · russh + russh-sftp · serde · notify · flate2 · sha2 |
| 存储 | 本地 JSON（系统数据目录） |

## 项目结构

```text
deploy_code/
├─ src/                        # React 前端
│  ├─ components/              # 通用组件（ui、侧边栏、状态栏、提交图等）
│  ├─ lib/                     # store、api/（invoke 封装）、types/、事件归约与纯逻辑 + 单测
│  ├─ locales/                 # zh-CN / en-US 文案
│  └─ pages/                   # 页面与页面私有子组件
├─ src-tauri/                  # Tauri 桌面壳：命令注册、应用状态、调度器、托盘
├─ crates/
│  └─ deploy-core/             # 核心库：Git / SSH / 部署 / 备份 / 容器 / Pages / Nginx / 存储
├─ scripts/                    # 辅助脚本（图标生成、界面截图）
├─ .github/workflows/ci.yml    # CI：tsc / vitest / cargo test
├─ .cargo/config.toml          # Windows 构建 rustflags（规避安全软件误拦截）
└─ build-installer.bat         # Windows 一键打包
```

业务逻辑全部在 `crates/deploy-core`，`src-tauri/src/commands` 只做参数校验与状态登记；`src/lib/api` 是前端调用 Tauri 命令的唯一入口，`src/lib/types` 是 Rust 模型的手工镜像。

## 部署流程

1. 本地 `git archive` 导出所选版本，压缩为 `tar.gz`（工作区模式通过临时索引打包，未跟踪文件遵循 `.gitignore`）
2. SSH 连接服务器，创建目标目录与 `.deploy_code/` 临时目录
3. SFTP 上传压缩包（带进度）
4. 远端 `tar -xzf` 解压到目标目录（原子发布时解压到 `releases/<版本>`）
5. 上传并替换配置的环境文件
6. 依次执行 `script_dir`（默认 `docker/`）下的 `.sh` 脚本
7. 原子发布时切换 `current` 软链；写入部署历史（状态、耗时、完整日志）

## 数据存储

配置与记录保存在系统数据目录，不会写入代码仓库：

- Windows：`%APPDATA%\deploycode\DeployCode\data`
- `config.json`：服务器 / 仓库 / 部署配置 / 备份目标 / 备份配置 / Pages 配置 / 容器配置 / 设置
- `history.json`：部署记录；`backups.json`：数据库备份记录；`pages.json`：Pages 部署记录；`containers.json`：容器备份 / 迁移记录
- `containers/`：容器备份包；`backups/`：本机保留的数据库导出包；`temp/`：临时归档；`locks/`：任务互斥锁文件

### 备份目标连接串

在「数据库备份」页的「备份目标」中添加，支持任意 PostgreSQL（Supabase / Aiven / Neon / 自建）：

```text
postgresql://<user>:<password>@<host>:5432/<database>
```

- Supabase：控制台 **Connect** 面板获取，推荐 Session Pooler（兼容 IPv4）：
  `postgresql://postgres.<project-ref>:<password>@aws-0-<region>.pooler.supabase.com:5432/postgres`
- Aiven / Neon：使用控制台提供的 PostgreSQL 连接串（Neon 需带 `?sslmode=require`）
- 备份脚本在服务器上调用 `psql`；服务器未安装 `postgresql-client` 时，docker 模式会自动改用数据库容器内的 `psql`
- 凭据写入服务器临时脚本（权限 0700）并在结束后删除，请确保服务器仅受信任用户可登录

## 安全说明

- **凭据明文存储**：SSH 密码、私钥口令、数据库连接串与各平台 Token 均以明文保存在本机 `config.json` 中，请勿分享该文件。
- **SSH 主机密钥不校验**：连接时信任所有主机公钥，不比对 `known_hosts`；请只在可信网络中使用。
- **敏感文件拦截**：提交前检测 `.env` / `*.pem` / `id_rsa` 等文件默认阻止，需界面二次确认；Pages 发布前扫描产物，命中敏感文件直接拒绝。
- **配置导出脱敏**：导出的配置文件会抹掉全部凭据；导入按 ID 合并，空白值不会覆盖本机已有凭据。

## 已知限制

- 定时关机与开机自启仅 Windows；桌面打包产物为 NSIS 安装包。
- 终端不实现 ZMODEM，文件传输请使用工具栏的 SFTP 上传 / 下载。
- 容器迁移不包含匿名卷与项目目录之外的 bind mount；恢复到目标机时会覆盖同名卷并重启同名项目。
- 交互式终端每台服务器最多 3 个、全局最多 8 个；文件传输每台 1 条、全局 3 条。它们与端口转发、资源采集共用同一条 SSH 会话的通道数（受服务端 `MaxSessions` 限制）。
- cron-job.org 官方 API 默认限额 100 次/天。
- 定时备份、定时容器与定时关机都只在软件运行期间生效（窗口关闭后仍在托盘运行）。

## 开发与构建

前置要求：

- Node.js **22.12+** 与 npm（Vite 8 / Vitest 5 的最低版本要求）
- Rust stable（MSVC 工具链）
- Windows 需要 WebView2（Win10/11 通常自带）

| 目的 | 命令 |
| --- | --- |
| 安装前端依赖 | `npm install` |
| 仅前端开发（http://localhost:1420） | `npm run dev` |
| 桌面端开发（Tauri，会启动 Vite） | `npm run app:dev` |
| 前端类型检查 + 构建 | `npm run build` |
| 仅 TypeScript 类型检查 | `npx tsc -p tsconfig.json --noEmit` |
| 前端单元测试（src/lib 纯逻辑） | `npm test` |
| 核心库测试 | `cargo test -p deploy-core` |
| 桌面壳测试（需先 `npm run build` 产出 `dist/`） | `cargo test -p deploy-code` |
| 打包桌面端（NSIS） | `npm run app:build`（产物：`target/release/bundle/nsis`） |
| Windows 一键打包（含 PATH 处理） | `build-installer.bat` |

CI（`.github/workflows/ci.yml`）在 Windows 上跑 `cargo test -p deploy-core`、`tsc` + `vitest`、`npm run build` + `cargo test -p deploy-code`，并在 Linux 上跑只在 Unix 编译的用例。

## License

MIT
