# DeployCode · 分支部署工具

DeployCode 是一个本地多仓库管理与 SSH 一键部署工具：管理多个本地 Git 仓库和远端 Linux 服务器，把指定分支 / 标签 / 提交打包部署到服务器，并执行项目内的部署脚本。

提供 **Tauri 2 桌面客户端** 和 **Rust CLI** 两种入口，二者共享同一份配置与部署历史。

## 功能特性

### 仓库管理

- 添加本地 Git 仓库（IDE 式：选目录即用），支持多仓库标签页
- 分支切换 / 创建 / 删除，远程分支查看
- 提交历史、提交图（含分支 / 标签引用）、工作区状态、文件 diff
- 文件树浏览、文件预览 / 编辑、内容搜索与批量替换
- 提交改动、硬回退、fetch / pull / push
- 工作区文件监听，外部改动自动刷新界面

### 服务器管理

- SSH 密码 / 私钥（含 passphrase）认证，连接测试
- 安全检查：失败登录统计（按账号 + IP）、成功登录、在线会话、sshd 关键配置、防火墙状态（ufw / firewalld / iptables）
- 一键拉黑 / 解除 IP、踢出在线会话
- 服务器端自动防护：写入 guard 脚本 + `/etc/cron.d/deploycode-guard` 每分钟任务，按失败登录阈值自动封禁，阈值与统计窗口可配置

### 一键部署

`git archive` 打包 -> SFTP 上传 -> 远端解压 -> 执行脚本，全程实时日志：

- 可选择分支 / 标签 / 提交进行部署
- 上传进度显示；脚本超时或应用退出时终止远端脚本进程组（含子进程）
- 脚本目录默认 `docker/`，自动执行其中的 `.sh` 文件，并注入环境变量：
  `DEPLOY_BRANCH` / `DEPLOY_REV` / `DEPLOY_COMMIT` / `DEPLOY_TARGET`
- 支持只执行指定脚本，或跳过脚本执行
- 部署记录持久化（版本、服务器、完整日志），可一键重新部署

### CLI

命令行覆盖仓库 / 分支 / 服务器 / 部署 / 历史等操作，适合脚本化与服务器环境使用。

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
- 文件：`config.json`（服务器 / 仓库 / 设置）、`history.json`（部署记录）、`temp/`（临时归档）

> ⚠️ SSH 密码与私钥口令以明文保存在 `config.json` 中，请勿分享该文件。CLI 可用 `--data-dir` 指定其他数据目录，`deploy-code-cli where` 可查看当前路径。

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
deploy-code-cli history list -n 10

# 查看数据目录
deploy-code-cli where
```

## License

MIT
