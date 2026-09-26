# DeployCode · 分支部署工具

DeployCode 是一个本地多仓库管理与 SSH 一键部署工具：管理多个本地 Git 仓库和远端 Linux 服务器，把指定分支 / 标签 / 提交打包部署到服务器，并执行项目内的部署脚本。

提供 **Tauri 2 桌面客户端** 和 **Rust CLI** 两种入口，二者共享同一份配置与部署历史。

## 功能特性

### 系统托盘

- 关闭窗口（点 X）只隐藏到系统托盘，程序继续在后台运行；托盘左键点击可重新显示窗口
- 托盘菜单支持「显示主窗口」与「退出程序」，只有「退出程序」才会彻底退出（退出前仍会清理远端部署/备份脚本与本地子进程）
- 托盘菜单文案跟随界面语言（中文 / English）

### 定时关机（Windows）

在 **设置 → 启动与自动化** 配置，到点让运行 DeployCode 的这台电脑关机：

- 每天定时：设置一个 24 小时制时间点，到点进入 60 秒可取消倒计时，结束后下发系统关机请求
- 倒计时关机：手动指定 1-1440 分钟后关机，随时可在设置页或底部状态栏取消
- 关机前先正常退出本程序：终止本地构建 / 部署子进程，并回收服务器上仍在执行的部署与备份脚本，避免远端留下半截任务
- 只在软件运行期间生效（窗口收到托盘也算）：软件启动前已错过的时间点不补跑，不会开机就被关机；要让每天都能重新排上，需同时开启「开机时自动启动」
- 底部状态栏常驻显示剩余时间与「取消关机」，倒计时进入最后 60 秒会转为醒目颜色

### 仓库管理

- 添加本地文件夹（IDE 式：选目录即用，不要求已经是 Git 仓库），支持多仓库标签页
- 非 Git 文件夹可绑定远端地址（origin）：自动 `git init` 后绑定，即可提交并推送到指定仓库
- 分支切换 / 创建 / 删除，远程分支查看
- 提交图（含分支 / 标签引用）：仓库详情页左栏「提交图」开关打开为底部面板，展示最近 200 条跨分支提交，点分支标签即可切分支；合并提交画成菱形，HEAD 与当前分支高亮
- 工作区状态与文件 diff 在左栏「源代码管理」面板
- 文件树浏览、文件预览 / 编辑、内容搜索与批量替换
- 提交改动、硬回退、fetch / pull / push
- 提交保护：自动检测 `.env` / `*.pem` / `id_rsa` / `credentials` 等疑似敏感文件，默认阻止提交，界面二次确认后才放行（CLI 需 `--allow-sensitive`）
- 工作区文件监听，外部改动自动刷新界面

### 服务器管理

- SSH 密码 / 私钥（含 passphrase）认证，连接测试
- 安全检查：失败登录统计（按账号 + IP）、成功登录、在线会话、sshd 关键配置、防火墙状态（ufw / firewalld / iptables）
- 一键拉黑 / 解除 IP、踢出在线会话
- 服务器端自动防护：写入 guard 脚本 + `/etc/cron.d/deploycode-guard` 每分钟任务，按失败登录阈值自动封禁，阈值与统计窗口可配置

### 一键部署

`git archive` 打包 -> SFTP 上传 -> 远端解压 -> 执行脚本，全程实时日志：

- 可选择分支 / 标签 / 提交进行部署；版本留空则打包当前工作区（含未提交改动，遵循 `.gitignore`）
- 部署配置列表化：服务器部署与 Pages 部署统一在「部署」页管理，每套配置可一键部署 / 编辑 / 删除；服务器部署参数（仓库、服务器、目录、版本、脚本、环境文件）保存后可复用
- Pages 配置同样在部署页新增 / 编辑 / 删除，一键构建发布 Cloudflare 或 GitHub Pages，状态与日志同步切换
- 原子发布（可选，设置中开启）：部署到「部署目录/releases/<版本>」，脚本成功后原子切换 `current` 软链；失败不影响线上版本，可在记录页一键回滚到任意历史版本
- 环境文件替换：按仓库配置「本地文件 → 部署目录相对路径」（如 `.env`、`docker/.env`），解压后、执行脚本前上传覆盖，部署页可临时关闭
- 上传进度显示；上传后比对本地 / 服务器 SHA-256，防止半包上线（服务器缺少校验工具时自动跳过）
- 部署过程中可「停止部署」：中止本地任务并终止远端脚本进程组、清理残留压缩包
- 脚本超时或应用退出时终止远端脚本进程组（含子进程）
- 脚本目录默认 `docker/`，自动执行其中的 `.sh` 文件，并注入环境变量：
  `DEPLOY_BRANCH` / `DEPLOY_REV` / `DEPLOY_COMMIT` / `DEPLOY_TARGET`（原子发布时 `DEPLOY_TARGET` 为版本目录，另有 `DEPLOY_RELEASE` 版本名与 `DEPLOY_CURRENT` 当前软链路径）
- 支持只执行指定脚本，或跳过脚本执行
- 部署记录持久化（版本、服务器、完整日志），可一键重新部署

### 数据库备份（PostgreSQL -> Supabase / Aiven / Neon）

把服务器上的 PostgreSQL schema 全量同步到远端 PostgreSQL，全程在服务器上完成，不经过本机：

- 来源支持 Docker 容器（`docker exec pg_dump`）或服务器本机 `pg_dump`，可配置库名 / 用户 / 密码 / schema
- 备份配置列表化管理（名称 + 服务器 + 来源 + 目标），可新增 / 编辑 / 删除 / 一键立即备份；升级自旧版服务器单份配置时会自动迁移
- 备份目标（Supabase / Aiven / Neon 等）可配置多个，每次备份选择其中一个；支持全局默认目标，配置 / 服务器可绑定自己的默认目标
- 全量覆盖：恢复前清空目标 schema 并恢复默认角色授权（Supabase 的 anon / authenticated / service_role 会自动恢复）
- 实时日志与进度；支持环境检查（pg_dump 版本 + 目标连通性）
- 备份记录持久化，可查看 / 删除 / 清空

### 容器备份与迁移（docker-compose）

在 **容器** 页选择一台服务器，扫描出它上面由 `docker compose` 管理的项目，把整套服务无损搬到另一台服务器：

- 一个项目打包成**一个本机备份包**（`.tar`）：项目目录原样（compose 文件、`.env`、bind mount 的本地文件、build 上下文）+ 每个命名数据卷的 tar + `docker save` 出的镜像 tar + `manifest.json` 清单
- **无损的关键是项目名**：compose 的命名卷叫 `<项目名>_<短名>`，恢复时保持同一个项目名，新容器就会挂到刚灌好数据的卷上
- 数据经本机中转（源机 -> 本机备份包 -> 目标机），两台服务器之间不需要互相配 SSH 信任；这份备份包可以反复用于恢复
- 只备份到本机、或备份后接着迁移到目标机，是同一条流程的两个出口；恢复目标目录默认照抄来源机的项目目录
- 打包前可选「暂停源服务、卷导完立即恢复」：数据库这类服务在写入中途的快照可能不一致
- 预检与提示：compose 命令可用性（`docker compose` / `docker-compose`）、远端磁盘空间、卷能否从宿主机直接读（不能则改用容器内 `tar`）、匿名卷与项目目录之外的 bind mount 会明确提示带不走
- 快照与恢复都在服务器上跑一个可中断的脚本（pidfile + 进程组终止），进度与结果实时显示，任务记录持久化；应用退出会回收两台服务器上残留的工作目录
- 已完成的记录可再次「恢复到服务器」（用本机备份包），需显式确认同名数据卷会被覆盖
- CLI：`deploy-code-cli container ls / inspect / backup / migrate / restore / list / show`

### Pages 部署（Cloudflare / GitHub）

仓库本地构建（可选）后一键发布静态产物，配置在「部署」页与服务器部署统一列表管理，按仓库选择平台：

Cloudflare Pages（`wrangler`）：

- 按仓库配置项目名 / 构建命令 / 输出目录（dist 等）/ 生产分支
- 一键「构建并部署」，实时日志；可跳过构建直接上传已有产物
- 自动创建 Pages 项目（已存在则跳过），从 wrangler 输出解析部署地址
- 支持环境检查（wrangler + Token / Account 校验）；部署记录持久化
- API Token 通过环境变量传递，不进入命令行

GitHub Pages（`gh-pages`）：

- 按仓库配置构建命令 / 输出目录 / 发布分支（默认 `gh-pages`）
- 产物推送到发布分支后由 GitHub 自动发布；`--dotfiles` 会一并保留 `.nojekyll` 等隐藏文件
- 复用本机 Git 凭据，无需额外 Token；远端地址须为 GitHub（可先在「仓库」页绑定）
- 地址自动推导为 `https://<用户名>.github.io/<仓库名>/`；推送分支与当前分支相同会被拒绝

两种平台都依赖本机 Node.js（wrangler / gh-pages 通过 `npx` 调用）。

### CLI

命令行覆盖仓库 / 分支 / 服务器 / 部署 / 备份 / 容器 / Pages / 历史等操作，适合脚本化与服务器环境使用。

## 技术栈

| 层 | 技术 |
| --- | --- |
| 前端 | React 19 · TypeScript · Vite 8 · Tailwind CSS 4 · Zustand · React Router · highlight.js |
| 桌面 | Tauri 2（NSIS 打包） |
| 后端 | Rust · tokio · russh + russh-sftp · clap · serde · notify · flate2 |
| 存储 | 本地 JSON（系统数据目录，见下文） |

## 项目结构

```text
deploy_code/
├─ src/                  # React 前端
├─ src-tauri/            # Tauri 桌面壳（命令注册、应用状态）
├─ crates/
│  ├─ deploy-core/       # 核心库：Git / SSH / 安全扫描 / 部署引擎 / 存储
│  └─ deploy-cli/        # 命令行工具（deploy-code-cli）
├─ scripts/              # 辅助脚本（图标生成等）
└─ .cargo/config.toml    # Windows 构建规避 360 误拦截的 rustflags
```

## 部署流程

1. 本地 `git archive` 导出所选版本，压缩为 `tar.gz`
2. SSH 连接服务器，创建目标目录与 `.deploy_code/` 临时目录
3. SFTP 上传压缩包（带进度）
4. 远端 `tar -xzf` 解压到目标目录
5. 依次执行 `script_dir`（默认 `docker/`）下的 `.sh` 脚本
6. 写入部署历史（状态、耗时、完整日志）

## 数据存储

配置与部署历史保存在系统数据目录，**不会写入代码仓库**：

- Windows：`%APPDATA%\deploycode\DeployCode\data`
- 文件：`config.json`（服务器 / 仓库 / 部署配置 / 备份目标 / 备份配置 / 设置）、`history.json`（部署记录）、`backups.json`（备份记录）、`containers.json`（容器备份 / 迁移记录）、`pages.json`（Pages 部署记录）、`containers/`（容器备份包）、`temp/`（临时归档）

> ⚠️ SSH 密码、私钥口令与备份目标连接串均以明文保存在 `config.json` 中，请勿分享该文件。CLI 可用 `--data-dir` 指定其他数据目录，`deploy-code-cli where` 可查看当前路径。

### 备份目标连接串

在 **设置 → 数据库备份目标** 中添加，支持任意 PostgreSQL（Supabase / Aiven / Neon / 自建）：

```text
postgresql://<user>:<password>@<host>:5432/<database>
```

- Supabase：从控制台 **Connect** 面板获取，推荐 Session Pooler（兼容 IPv4）：
  `postgresql://postgres.<project-ref>:<password>@aws-0-<region>.pooler.supabase.com:5432/postgres`
- Aiven / Neon：使用控制台提供的 PostgreSQL 连接串（Neon 需带 `?sslmode=require`）
- 备份脚本会在服务器上调用 `psql`；若服务器未安装 `postgresql-client`，docker 模式下会自动改用数据库容器内的 `psql`
- 凭据写入服务器临时脚本（权限 0700）并在结束后删除，请确保服务器仅受信任用户可登录

## 开发与构建

前置要求：

- Node.js 18+ 与 npm
- Rust stable（MSVC 工具链）
- Windows 需要 WebView2（Win10/11 通常自带）

```bash
npm install           # 安装前端依赖
npm run app:dev       # 桌面端开发（Tauri）
npm run dev           # 仅前端开发（http://localhost:1420）
npm run app:build     # 打包桌面端（产物在 target/release/bundle/nsis）
```

CLI：

```bash
cargo run -p deploy-cli -- --help        # 直接运行
cargo build -p deploy-cli --release      # 构建 release 二进制
```

测试与检查：

```bash
cargo test           # Rust 单元测试
npm run build        # tsc 类型检查 + 前端构建
```

## CLI 示例

```bash
# 添加服务器（私钥认证）
deploy-code-cli server add prod --host 192.168.1.10 --user root --key ~/.ssh/id_ed25519

# 添加仓库并设置默认服务器 / 部署目录
deploy-code-cli repo add /path/to/myapp --name myapp --server prod --dir /opt/myapp

# 查看分支 / 部署 main 分支 / 查看历史
deploy-code-cli branch list myapp --all
deploy-code-cli deploy myapp --rev main
# 使用客户端保存的部署配置（仓库、服务器、目录、脚本等取自配置，命令行参数可覆盖）
deploy-code-cli deploy --config 生产环境
deploy-code-cli history list -n 10

# 原子发布的历史版本（需在设置中开启「原子发布」）
deploy-code-cli release list prod --dir /opt/myapp
deploy-code-cli release switch prod --dir /opt/myapp 20260914-153001-3b2424f-abc123

# 数据库备份（服务器 PG -> Supabase / Aiven / Neon 等）
deploy-code-cli backup target add Aiven "postgresql://user:password@host:5432/db"
deploy-code-cli backup target add Neon "postgresql://user:password@host:5432/db"
deploy-code-cli backup target list
# 保存一套备份配置（服务器 + 来源 + 目标），之后按名称一键备份
deploy-code-cli backup config add prod-app --server prod --database app --username postgres --target Aiven
deploy-code-cli backup config list
deploy-code-cli backup test --config prod-app
deploy-code-cli backup run --config prod-app
# 也可以继续用服务器参数直接执行
deploy-code-cli backup test prod
deploy-code-cli backup run prod --target Aiven
deploy-code-cli backup list -n 10
deploy-code-cli backup show <记录ID>

# 容器备份与迁移（docker-compose 项目经本机中转）
deploy-code-cli container dir
deploy-code-cli container ls --server prod
deploy-code-cli container inspect --server prod --project blog
# 只打包到本机（默认带数据卷与镜像；--pause 会在导出卷前暂停源服务）
deploy-code-cli container backup --server prod --project blog --pause
# 打包并迁移到另一台服务器（来源服务器上的服务保持不变）
deploy-code-cli container migrate --server prod --project blog --to staging --dir /opt/blog
# 用本机已有的备份包再恢复到一台服务器
deploy-code-cli container restore --bundle "<数据目录>\containerslog-20260926-031000-ab12cd34.tar" --to staging
deploy-code-cli container list -n 10
deploy-code-cli container show <记录ID>

# Cloudflare Pages 部署
deploy-code-cli pages config myapp --project my-site --build "npm run build" --output dist
deploy-code-cli pages test myapp
deploy-code-cli pages run myapp
deploy-code-cli pages list -n 10
deploy-code-cli pages show <记录ID>

# 查看数据目录
deploy-code-cli where
```

## License

MIT
