//! Nginx 容器配置管理：发现服务器上的容器、查询 / 编辑 / 删除 conf.d 配置文件。
//!
//! 全部操作通过 SSH 在服务器上执行 `docker` / `nginx` 命令完成：
//! - 容器发现：`docker ps`
//! - 读取配置：`docker exec <容器> cat <路径>`
//! - 写入配置：SFTP 上传临时文件 + `docker cp` 进容器
//! - 变更保护：写入 / 删除后先跑 `nginx -t` 校验，失败自动回滚，通过后 `nginx -s reload`

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::models::ServerConfig;
use crate::process::shell_quote;
use crate::ssh::SshClient;
use crate::store::Store;

/// nginx 官方镜像默认的配置文件目录。
pub const DEFAULT_CONFIG_DIR: &str = "/etc/nginx/conf.d";

/// 单条 docker / nginx 命令的超时秒数（SSH 连接超时仍取设置项）。
const COMMAND_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NginxContainerInfo {
    pub name: String,
    pub image: String,
    pub ports: String,
    /// 名称或镜像包含 nginx 的容器，界面上优先展示。
    pub nginx: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NginxConfigFile {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NginxConfigContent {
    pub name: String,
    pub path: String,
    pub content: String,
}

pub struct NginxEngine {
    store: Arc<Store>,
}

impl NginxEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    async fn connect(&self, server: &ServerConfig) -> Result<SshClient> {
        let timeout = self.store.load_config()?.settings.connect_timeout_secs;
        SshClient::connect(server, timeout).await
    }

    /// 列出服务器上运行中的容器（nginx 容器排在前面）。
    pub async fn list_containers(&self, server: &ServerConfig) -> Result<Vec<NginxContainerInfo>> {
        let client = self.connect(server).await?;
        // 用 `|` 分隔：容器名与镜像名都不允许该字符，避免输出转义差异。
        let command = "docker ps --format '{{.Names}}|{{.Image}}|{{.Ports}}'";
        let result = client.exec_capture(command, COMMAND_TIMEOUT_SECS).await;
        client.disconnect().await;
        let (code, output) = result?;
        if code != 0 {
            return Err(CoreError::ssh(format!(
                "查询容器失败（请确认服务器已安装并启动 Docker）: {}",
                output.trim()
            )));
        }
        Ok(parse_containers(&output))
    }

    /// 列出容器内指定目录下的 `*.conf` 配置文件。
    pub async fn list_configs(
        &self,
        server: &ServerConfig,
        container: &str,
        dir: &str,
    ) -> Result<Vec<NginxConfigFile>> {
        let container = validate_container(container)?;
        let dir = validate_dir(dir)?;
        let client = self.connect(server).await?;
        // 目录通过 `sh` 的位置参数传入，避免把带空格/特殊字符的路径拼进脚本。
        let script = r#"dir="$1"; [ -d "$dir" ] || { echo "配置目录不存在: $dir" >&2; exit 2; }; for f in "$dir"/*.conf; do [ -f "$f" ] || continue; printf '%s\t%s\n' "$(basename "$f")" "$(wc -c < "$f")"; done"#;
        let command = format!(
            "docker exec {} sh -c {} sh {}",
            shell_quote(&container),
            shell_quote(script),
            shell_quote(&dir)
        );
        let result = client.exec_capture(&command, COMMAND_TIMEOUT_SECS).await;
        client.disconnect().await;
        let (code, output) = result?;
        if code != 0 {
            return Err(CoreError::ssh(format!(
                "读取配置目录失败 {dir}: {}",
                output.trim()
            )));
        }
        Ok(parse_config_files(&output))
    }

    /// 读取单个配置文件内容。
    pub async fn read_config(
        &self,
        server: &ServerConfig,
        container: &str,
        dir: &str,
        name: &str,
    ) -> Result<NginxConfigContent> {
        let container = validate_container(container)?;
        let dir = validate_dir(dir)?;
        let name = validate_file_name(name)?;
        let path = config_path(&dir, &name);
        let client = self.connect(server).await?;
        let result = read_config_at(&client, &container, &path).await;
        client.disconnect().await;
        Ok(NginxConfigContent {
            name,
            path,
            content: result?,
        })
    }

    /// 新增或覆盖配置文件：先 `nginx -t` 校验，失败回滚旧内容，通过后重载。
    pub async fn save_config(
        &self,
        server: &ServerConfig,
        container: &str,
        dir: &str,
        name: &str,
        content: &str,
    ) -> Result<String> {
        let container = validate_container(container)?;
        let dir = validate_dir(dir)?;
        let name = validate_file_name(name)?;
        let path = config_path(&dir, &name);
        let client = self.connect(server).await?;
        let result = self
            .save_config_on(&client, &container, &path, &name, content)
            .await;
        client.disconnect().await;
        result
    }

    async fn save_config_on(
        &self,
        client: &SshClient,
        container: &str,
        path: &str,
        name: &str,
        content: &str,
    ) -> Result<String> {
        // 保留旧内容用于校验失败时回滚；确认不存在时才按“新增文件”处理。
        let previous = read_config_optional(client, container, path).await?;
        self.write_remote_config(client, container, path, content)
            .await?;

        let (code, output) = nginx_test(client, container).await?;
        if code != 0 {
            let restored = self
                .rollback(client, container, path, previous.as_deref())
                .await;
            return Err(check_failed_error(name, &output, restored));
        }

        reload_nginx(client, container).await?;
        Ok(format!("{name} 已保存，nginx 校验通过并已重载"))
    }

    /// 删除配置文件：删除后校验，失败恢复原内容。
    pub async fn delete_config(
        &self,
        server: &ServerConfig,
        container: &str,
        dir: &str,
        name: &str,
    ) -> Result<String> {
        let container = validate_container(container)?;
        let dir = validate_dir(dir)?;
        let name = validate_file_name(name)?;
        let path = config_path(&dir, &name);
        let client = self.connect(server).await?;
        let result = self
            .delete_config_on(&client, &container, &path, &name)
            .await;
        client.disconnect().await;
        result
    }

    async fn delete_config_on(
        &self,
        client: &SshClient,
        container: &str,
        path: &str,
        name: &str,
    ) -> Result<String> {
        let previous = read_config_optional(client, container, path)
            .await?
            .ok_or_else(|| CoreError::not_found(format!("配置文件不存在: {path}")))?;

        let command = format!(
            "docker exec {} rm -f {}",
            shell_quote(container),
            shell_quote(path)
        );
        let (code, output) = client.exec_capture(&command, COMMAND_TIMEOUT_SECS).await?;
        if code != 0 {
            return Err(CoreError::ssh(format!(
                "删除配置文件失败 {path}: {}",
                output.trim()
            )));
        }

        let (code, output) = nginx_test(client, container).await?;
        if code != 0 {
            let restored = self
                .rollback(client, container, path, Some(&previous))
                .await;
            let hint = if restored {
                "已恢复删除前的内容"
            } else {
                "自动恢复失败，请手动修复"
            };
            return Err(CoreError::ssh(format!(
                "删除后校验失败 {name}（{hint}）: {}",
                output.trim()
            )));
        }

        reload_nginx(client, container).await?;
        Ok(format!("{name} 已删除并重载 Nginx"))
    }

    /// 手动校验并重载 nginx。
    pub async fn reload(&self, server: &ServerConfig, container: &str) -> Result<String> {
        let container = validate_container(container)?;
        let client = self.connect(server).await?;
        let result = async {
            let (code, output) = nginx_test(&client, &container).await?;
            if code != 0 {
                return Err(CoreError::ssh(format!(
                    "nginx 配置校验失败: {}",
                    output.trim()
                )));
            }
            reload_nginx(&client, &container).await?;
            Ok("Nginx 已重载".to_string())
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 把内容写入容器内的目标路径（本地临时文件 -> SFTP -> docker cp）。
    async fn write_remote_config(
        &self,
        client: &SshClient,
        container: &str,
        path: &str,
        content: &str,
    ) -> Result<()> {
        let local = self
            .store
            .prepare_temp_file(&format!("nginx-{}.conf", uuid::Uuid::new_v4()))?;
        let result = write_remote_config_inner(client, &local, container, path, content).await;
        let _ = std::fs::remove_file(&local);
        result
    }

    /// 回滚到修改前的内容；返回是否成功。
    async fn rollback(
        &self,
        client: &SshClient,
        container: &str,
        path: &str,
        previous: Option<&str>,
    ) -> bool {
        match previous {
            Some(content) => self
                .write_remote_config(client, container, path, content)
                .await
                .is_ok(),
            None => {
                let command = format!(
                    "docker exec {} rm -f {}",
                    shell_quote(container),
                    shell_quote(path)
                );
                client
                    .exec_capture(&command, COMMAND_TIMEOUT_SECS)
                    .await
                    .map(|(code, _)| code == 0)
                    .unwrap_or(false)
            }
        }
    }
}

async fn write_remote_config_inner(
    client: &SshClient,
    local: &Path,
    container: &str,
    path: &str,
    content: &str,
) -> Result<()> {
    std::fs::write(local, content).map_err(|e| CoreError::io_path(local, e))?;

    let remote = format!("/tmp/deploycode-nginx-{}.conf", uuid::Uuid::new_v4());
    if let Err(err) = client.upload_file(local, &remote, &mut |_, _| {}).await {
        remove_remote_temp(client, &remote).await;
        return Err(err);
    }

    let target = format!("{container}:{path}");
    let command = format!(
        "docker cp {} {}",
        shell_quote(&remote),
        shell_quote(&target)
    );
    let copied = client.exec_capture(&command, COMMAND_TIMEOUT_SECS).await;
    remove_remote_temp(client, &remote).await;
    let (code, output) = copied?;
    if code != 0 {
        return Err(CoreError::ssh(format!(
            "写入容器配置文件失败 {path}: {}",
            output.trim()
        )));
    }
    Ok(())
}

async fn remove_remote_temp(client: &SshClient, remote: &str) {
    let _ = client
        .exec_capture(&format!("rm -f {}", shell_quote(remote)), 30)
        .await;
}

/// 读取配置；仅在确认文件不存在时返回 `None`。
///
/// 容器异常 / 权限不足时 `cat` 与 `test -f` 的表现不同：后者成功说明文件存在但读不到，
/// 此时必须报错而不是当作新增，否则校验失败回滚会删掉原本存在的文件。
async fn read_config_optional(
    client: &SshClient,
    container: &str,
    path: &str,
) -> Result<Option<String>> {
    let command = format!(
        "docker exec {} cat {}",
        shell_quote(container),
        shell_quote(path)
    );
    let (code, output) = client.exec_capture(&command, COMMAND_TIMEOUT_SECS).await?;
    if code == 0 {
        // docker 成功时也可能向 stderr 打警告（如 config.json 损坏），不能混进配置内容。
        return Ok(Some(stdout_text(&output)));
    }

    // 用 sh 内置 test 判断文件是否存在：精简镜像里可能没有独立的 test/[/cat 二进制。
    let probe = format!(
        "docker exec {} sh -c {} sh {}",
        shell_quote(container),
        shell_quote(r#"test -f "$1""#),
        shell_quote(path)
    );
    let (probe_code, _) = client.exec_capture(&probe, COMMAND_TIMEOUT_SECS).await?;
    if probe_code == 0 {
        return Err(CoreError::ssh(format!(
            "读取配置文件失败 {path}: {}",
            output.trim()
        )));
    }
    Ok(None)
}

async fn read_config_at(client: &SshClient, container: &str, path: &str) -> Result<String> {
    let command = format!(
        "docker exec {} cat {}",
        shell_quote(container),
        shell_quote(path)
    );
    let (code, output) = client.exec_capture(&command, COMMAND_TIMEOUT_SECS).await?;
    if code != 0 {
        return Err(CoreError::not_found(format!(
            "读取配置文件失败 {path}: {}",
            output.trim()
        )));
    }
    Ok(stdout_text(&output))
}

async fn nginx_test(client: &SshClient, container: &str) -> Result<(i32, String)> {
    // 合并 stderr，让校验失败原因完整出现在同一条错误消息里。
    let command = format!("docker exec {} nginx -t 2>&1", shell_quote(container));
    client.exec_capture(&command, COMMAND_TIMEOUT_SECS).await
}

async fn reload_nginx(client: &SshClient, container: &str) -> Result<()> {
    let command = format!(
        "docker exec {} nginx -s reload 2>&1",
        shell_quote(container)
    );
    let (code, output) = client.exec_capture(&command, COMMAND_TIMEOUT_SECS).await?;
    if code != 0 {
        return Err(CoreError::ssh(format!(
            "重载 Nginx 失败: {}",
            output.trim()
        )));
    }
    Ok(())
}

fn check_failed_error(name: &str, output: &str, restored: bool) -> CoreError {
    let hint = if restored {
        "已回滚到修改前的内容"
    } else {
        "自动回滚失败，请手动修复"
    };
    CoreError::ssh(format!("配置校验失败 {name}（{hint}）: {}", output.trim()))
}

/// 只保留 stdout 行：`exec_capture` 会把 stderr 行合并进来并加上 `[stderr] ` 前缀，
/// docker 成功但输出警告时，这些行不能被当成容器 / 配置记录。
fn stdout_lines(output: &str) -> impl Iterator<Item = &str> {
    output.lines().filter(|line| !line.starts_with("[stderr] "))
}

/// 还原纯 stdout 文本：exec_capture 每行都会补 '\n'，过滤 stderr 行后按行拼回。
fn stdout_text(output: &str) -> String {
    let lines: Vec<&str> = stdout_lines(output).collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn parse_containers(output: &str) -> Vec<NginxContainerInfo> {
    let mut items = Vec::new();
    for line in stdout_lines(output) {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let name = parts.next().unwrap_or("").trim().to_string();
        let image = parts.next().unwrap_or("").trim().to_string();
        let ports = parts.next().unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let nginx = name.to_ascii_lowercase().contains("nginx")
            || image.to_ascii_lowercase().contains("nginx");
        items.push(NginxContainerInfo {
            name,
            image,
            ports,
            nginx,
        });
    }
    items.sort_by(|a, b| b.nginx.cmp(&a.nginx).then_with(|| a.name.cmp(&b.name)));
    items
}

fn parse_config_files(output: &str) -> Vec<NginxConfigFile> {
    let mut items = Vec::new();
    for line in stdout_lines(output) {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        // 文件名保持原样（可能含空格），只用 trim 判空，避免把 “ foo.conf” 显示成 “foo.conf” 后操作错文件。
        let name = parts.next().unwrap_or("");
        if name.trim().is_empty() {
            continue;
        }
        let size = parts
            .next()
            .unwrap_or("0")
            .trim()
            .parse::<u64>()
            .unwrap_or(0);
        items.push(NginxConfigFile {
            name: name.to_string(),
            size,
        });
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

fn config_path(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

fn validate_container(container: &str) -> Result<String> {
    let container = container.trim();
    if container.is_empty() {
        return Err(CoreError::config("请选择或填写 nginx 容器名"));
    }
    // Docker 容器名规则：字母/数字开头，之后允许 _ . -。
    let valid = !container.starts_with('-')
        && container
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !valid {
        return Err(CoreError::config(format!("容器名不合法: {container}")));
    }
    Ok(container.to_string())
}

fn validate_dir(dir: &str) -> Result<String> {
    let dir = dir.trim().trim_end_matches('/');
    if !dir.starts_with('/') {
        return Err(CoreError::config(
            "配置目录必须是绝对路径，例如 /etc/nginx/conf.d",
        ));
    }
    if dir.contains('\n') || dir.contains('\r') {
        return Err(CoreError::config("配置目录不能包含换行符"));
    }
    if dir
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return Err(CoreError::config("配置目录不能包含 . 或 .. 路径段"));
    }
    Ok(dir.to_string())
}

fn validate_file_name(name: &str) -> Result<String> {
    if name.trim().is_empty() {
        return Err(CoreError::config("请填写配置文件名"));
    }
    // 保留原样是为了能精确操作既有文件；首尾空白会让“显示名”和“实际名”指向不同文件。
    if name != name.trim() {
        return Err(CoreError::config("配置文件名不能以空白字符开头或结尾"));
    }
    if !name.ends_with(".conf") {
        return Err(CoreError::config("配置文件名必须以 .conf 结尾"));
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(CoreError::config("配置文件名不能包含路径分隔符或 .."));
    }
    // 允许空格/中文等既有文件名（命令侧统一 shell_quote）；控制字符会让输出解析产生歧义。
    if name.chars().any(|c| c.is_control()) {
        return Err(CoreError::config("配置文件名不能包含控制字符"));
    }
    if name.len() == ".conf".len() {
        return Err(CoreError::config("配置文件名不能为空"));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_file_name_rules() {
        assert_eq!(validate_file_name("users.conf").unwrap(), "users.conf");
        assert_eq!(
            validate_file_name("my-app_2.conf").unwrap(),
            "my-app_2.conf"
        );
        // 既有文件可能带空格/中文/引号：允许操作，命令侧由 shell_quote 保证安全。
        assert_eq!(
            validate_file_name("my site 站点.conf").unwrap(),
            "my site 站点.conf"
        );
        assert_eq!(validate_file_name("a'b.conf").unwrap(), "a'b.conf");
        assert!(validate_file_name("users").is_err());
        assert!(validate_file_name("../etc/passwd.conf").is_err());
        assert!(validate_file_name("a/b.conf").is_err());
        assert!(validate_file_name("a\nb.conf").is_err());
        assert!(validate_file_name(".conf").is_err());
        // 首尾空白会指向与显示名不同的文件，明确拒绝。
        assert!(validate_file_name(" users.conf ").is_err());
    }

    #[test]
    fn validate_dir_rules() {
        assert_eq!(
            validate_dir("/etc/nginx/conf.d/").unwrap(),
            "/etc/nginx/conf.d"
        );
        assert!(validate_dir("etc/nginx").is_err());
        assert!(validate_dir("/etc/../root").is_err());
        assert!(validate_dir("/").is_err());
    }

    #[test]
    fn validate_container_rules() {
        assert_eq!(validate_container(" my-nginx_1 ").unwrap(), "my-nginx_1");
        assert!(validate_container("").is_err());
        assert!(validate_container("-x").is_err());
        assert!(validate_container("a b").is_err());
    }

    #[test]
    fn parse_containers_prefers_nginx() {
        let output =
            "pg|postgres:16|5432/tcp\nweb|nginx:alpine|0.0.0.0:80->80/tcp\nproxy|my-proxy:1|\n";
        let items = parse_containers(output);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name, "web");
        assert!(items[0].nginx);
        assert!(!items[1].nginx);
        assert_eq!(items[2].name, "proxy");
    }

    #[test]
    fn parse_config_files_lists_name_and_size() {
        let output = "b.conf\t12\na.conf\t34\n\n";
        let items = parse_config_files(output);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "a.conf");
        assert_eq!(items[0].size, 34);
        assert_eq!(items[1].name, "b.conf");
    }

    #[test]
    fn parse_skips_stderr_lines_and_keeps_names_exact() {
        // exec_capture 会把 stderr 行加上前缀合并进来，docker 警告不能被当成记录。
        let containers =
            parse_containers("[stderr] WARNING: No swap limit support\nweb|nginx:alpine|80/tcp\n");
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].name, "web");

        let files = parse_config_files("[stderr] warning here\nmy site.conf\t12\n");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "my site.conf");
        assert_eq!(files[0].size, 12);
    }

    #[test]
    fn stdout_text_rebuilds_content_without_stderr() {
        assert_eq!(
            stdout_text("[stderr] WARNING: bad config\nserver {\n}\n"),
            "server {\n}\n"
        );
        // 空文件保持为空，不能凭空多出一个换行。
        assert_eq!(stdout_text(""), "");
        assert_eq!(stdout_text("[stderr] only warning\n"), "");
        assert_eq!(stdout_text("single line\n"), "single line\n");
    }

    #[test]
    fn config_path_joins_without_double_slash() {
        assert_eq!(
            config_path("/etc/nginx/conf.d/", "a.conf"),
            "/etc/nginx/conf.d/a.conf"
        );
    }
}
