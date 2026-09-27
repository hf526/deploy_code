# 这里放要打进安装包的 agent 二进制

安装包会把本目录下所有文件作为资源带上（`tauri.conf.json` 的 `bundle.resources: ["agent/*"]`），
客户端运行时从 `<资源目录>/agent/deploy-agent` 读它，点「安装/更新」时经 SSH 传到控制机。

- 文件名必须是 `deploy-agent`，且是 **Linux** 可执行文件（客户端会校验 ELF 文件头，
  本机编出来的 `deploy-agent.exe` 会被直接拒绝，免得装到服务器上起不来）。
- 构建：`cargo build -p deploy-agent --release --target x86_64-unknown-linux-musl`，
  产物在 `target/x86_64-unknown-linux-musl/release/deploy-agent`，复制到这里即可。
  musl 静态链接是为了不挑发行版。
- 这个文件已 gitignore（几十 MB 的二进制不进仓库）。没有它时客户端照样能编译，
  只是控制机页面会显示「未内置」，点安装会报错并说明怎么补。
