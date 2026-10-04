# 这里放要打进安装包的 agent 二进制

安装包会把本目录下所有文件作为资源带上（`tauri.conf.json` 的 `bundle.resources: ["agent/*"]`），
客户端运行时从 `<安装目录>/agent/deploy-agent` 读它，点「安装/更新」时经 SSH 传到控制机。

- 文件名必须是 `deploy-agent`，且是 **Linux** 可执行文件。客户端会校验 ELF 文件头，
  本机编出来的 `deploy-agent.exe` 会被直接拒绝 —— 装到服务器上 systemd 起不来，
  而那时旧服务已经被 stop 掉了。
- 这个文件已 gitignore（7 MB 的二进制不进仓库）。没有它时客户端照样能编译，
  只是控制机页面显示「未内置」，点安装会报错并说明怎么补；`build-installer.bat` 也会先问一句。

## 怎么出这份产物

这台 Windows 上没有交叉编译工具链（无 musl-gcc、无 docker、无 zig，WSL 未装发行版），
所以**在控制机上编**是最省事的路。仓库是 public，直接在服务器上来一条：

```bash
git clone https://github.com/hf526/deploy_code.git && cd deploy_code && \
docker run --rm -v "$PWD":/app -w /app rust:1.93-slim-bookworm bash -ec '
  apt-get update -qq && apt-get install -y -qq --no-install-recommends musl-tools file
  rustup target add x86_64-unknown-linux-musl
  cargo build -p deploy-agent --release --target x86_64-unknown-linux-musl
  file target/x86_64-unknown-linux-musl/release/deploy-agent'
```

产物在 `target/x86_64-unknown-linux-musl/release/deploy-agent`，取回来放到本目录即可。
2026-09-29 实测过：`release` 编译 3m29s，产物 7,486,880 字节，`static-pie linked`，
二进制里没有 `ld-linux` / `libc.so`（所以不挑发行版），sha256 以 `musl` 静态那份为准。

另外两条：

- 手动跑 `.github/workflows/build-agent.yml`，下载 artifact 解到这里。**这条还没实测过。**
- 哪天本机有了 musl 工具链，就直接 `cargo build -p deploy-agent --release --target x86_64-unknown-linux-musl`。

编完记得对一下版本：`deploy-agent --version` 要打印 `deploy-agent <版本> proto=<n>`，
其中 `<版本>` 等于 workspace 版本、`proto` 等于 `deploy_core::agent::PROTO`，客户端握手只认这个。
