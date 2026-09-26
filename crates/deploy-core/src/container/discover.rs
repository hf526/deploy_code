//! 发现服务器上的 docker-compose 项目：`compose ls` + 容器标签解析出服务、数据卷与镜像。
//!
//! 这里只有只读查询，不改动服务器上的任何东西；写操作的脚本在 `scripts`，编排在 `super`。

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::nginx::stdout_lines;
use crate::process::shell_quote;
use crate::ssh::SshClient;

/// 单条查询命令的超时（秒）。
pub(crate) const QUERY_TIMEOUT_SECS: u64 = 120;
/// 卷体积要 `du` 扫目录，GB 级数据卷给 10 分钟。
pub(crate) const INSPECT_TIMEOUT_SECS: u64 = 600;

// ---------------------------------------------------------------------------
// 发现结果
// ---------------------------------------------------------------------------

/// 服务器上发现到的一个 compose 项目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeStack {
    pub name: String,
    pub services: usize,
    pub running: usize,
    /// compose 给出的状态摘要，如 `running (3)`。
    pub status: String,
    /// compose 配置文件的绝对路径。
    pub files: Vec<String>,
    pub working_dir: String,
}

/// 项目里的一个服务（按容器解析，compose v1 / v2 都会打这些标签）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeService {
    pub name: String,
    pub container: String,
    pub image: String,
    pub state: String,
    pub ports: String,
}

/// 项目挂载的一个命名数据卷。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeVolume {
    pub name: String,
    pub mountpoint: String,
    pub size_bytes: u64,
    /// 导出时用来挂载该卷的镜像（取真正用到它的服务镜像，本机一定有，不需要拉取）。
    pub image: String,
    /// 宿主机上能直接读到卷目录；false 时改用容器内 `tar`。
    pub readable: bool,
}

/// 快照前的项目详情与预检结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposeStackDetail {
    pub stack: ComposeStack,
    pub services: Vec<ComposeService>,
    pub volumes: Vec<ComposeVolume>,
    pub images: Vec<String>,
    /// 一起带走的 env 文件（绝对路径）。
    pub env_files: Vec<String>,
    /// 该机可用的 compose 命令，仅用于界面展示。
    pub compose_command: String,
    pub volume_bytes: u64,
    pub image_bytes: u64,
    pub project_bytes: u64,
    /// 预检提示：不阻断任务，但界面上必须原样列出来。
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// 远端查询
// ---------------------------------------------------------------------------


pub(crate) fn compose_ls_command(compose: &[String]) -> String {
    format!("{} ls --all --format json", compose.join(" "))
}

/// 检测该机可用的 compose 命令（v2 插件优先，退回 v1 独立二进制）。
pub(crate) async fn compose_command(client: &SshClient) -> Result<Vec<String>> {
    let (code, _) = client
        .exec_capture("docker compose version >/dev/null 2>&1", 30)
        .await?;
    if code == 0 {
        return Ok(vec!["docker".to_string(), "compose".to_string()]);
    }
    let (code, _) = client
        .exec_capture("docker-compose version >/dev/null 2>&1", 30)
        .await?;
    if code == 0 {
        return Ok(vec!["docker-compose".to_string()]);
    }
    Err(CoreError::config(
        "该服务器上没有可用的 docker compose（`docker compose` 与 `docker-compose` 都不存在）",
    ))
}

/// 一条 compose 容器的标签信息。
#[derive(Debug, Clone)]
pub(crate) struct ContainerRow {
    pub(crate) project: String,
    pub(crate) service: String,
    pub(crate) container: String,
    pub(crate) image: String,
    pub(crate) state: String,
    pub(crate) ports: String,
    pub(crate) working_dir: String,
    pub(crate) config_files: String,
}

/// `docker ps --format` 的模板：字段间用真实制表符，Docker 会原样输出。
pub(crate) const ROW_TEMPLATE: &str = concat!(
    "{{.Label \"com.docker.compose.project\"}}\t",
    "{{.Label \"com.docker.compose.service\"}}\t",
    "{{.Names}}\t",
    "{{.Image}}\t",
    "{{.State}}\t",
    "{{.Ports}}\t",
    "{{.Label \"com.docker.compose.project.working_dir\"}}\t",
    "{{.Label \"com.docker.compose.project.config_files\"}}",
);

impl ContainerRow {
    pub(crate) fn from_line(line: &str) -> Option<Self> {
        let fields: Vec<&str> = line.split('\t').map(str::trim).collect();
        if fields.len() < 8 || fields[0].is_empty() {
            return None;
        }
        Some(Self {
            project: fields[0].to_string(),
            service: fields[1].to_string(),
            container: fields[2].to_string(),
            image: fields[3].to_string(),
            state: fields[4].to_string(),
            ports: fields[5].to_string(),
            working_dir: fields[6].to_string(),
            config_files: fields[7].to_string(),
        })
    }
}

pub(crate) fn container_query(project: Option<&str>) -> String {
    let filter = match project {
        Some(project) => format!(
            " --filter {}",
            shell_quote(&format!("label=com.docker.compose.project={project}"))
        ),
        None => String::new(),
    };
    format!(
        "docker ps -a{filter} --format {}",
        shell_quote(ROW_TEMPLATE)
    )
}

pub(crate) async fn container_rows(client: &SshClient, project: Option<&str>) -> Result<Vec<ContainerRow>> {
    let (code, output) = client
        .exec_capture(&container_query(project), QUERY_TIMEOUT_SECS)
        .await?;
    if code != 0 {
        return Err(CoreError::ssh(format!(
            "查询容器失败（请确认该服务器已安装并启动 Docker，且当前用户有 docker 权限）: {}",
            output.trim()
        )));
    }
    Ok(stdout_lines(&output)
        .filter_map(ContainerRow::from_line)
        .collect())
}

/// `docker compose ls` 输出里的一个项目。
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredProject {
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) files: Vec<String>,
}

impl DiscoveredProject {
    /// `inspect_stack` 不走 `compose ls`（要按项目过滤），用容器标签拼出同一条目。
    pub(crate) fn from_rows(project: &str, rows: &[ContainerRow]) -> Self {
        let mine: Vec<&ContainerRow> = rows
            .iter()
            .filter(|row| row.project == project)
            .collect();
        let running = mine.iter().filter(|row| row.state == "running").count();
        let files = mine
            .first()
            .map(|row| split_files(&row.config_files))
            .unwrap_or_default();
        Self {
            name: project.to_string(),
            status: format!(
                "{} ({})",
                if running > 0 { "running" } else { "exited" },
                mine.len()
            ),
            files,
        }
    }
}

pub(crate) fn split_files(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().replace('\\', "/"))
        .filter(|item| !item.is_empty())
        .collect()
}

pub(crate) fn parse_compose_ls(output: &str) -> Result<Vec<DiscoveredProject>> {
    let text = stdout_lines(output).collect::<Vec<_>>().join("\n");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|err| {
        CoreError::ssh(format!(
            "compose 项目列表解析失败（compose 版本过旧或输出被改写）: {err}"
        ))
    })?;
    let items = match value.as_array() {
        Some(items) => items,
        None => return Ok(Vec::new()),
    };
    Ok(items
        .iter()
        .filter_map(|item| {
            let name = item.get("Name")?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            Some(DiscoveredProject {
                name: name.to_string(),
                status: item
                    .get("Status")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
                files: item
                    .get("ConfigFiles")
                    .and_then(|value| value.as_str())
                    .map(split_files)
                    .unwrap_or_default(),
            })
        })
        .collect())
}

/// 把 `compose ls` 的项目与容器标签合并成界面要的那份列表。
pub(crate) fn build_stacks(projects: Vec<DiscoveredProject>, rows: &[ContainerRow]) -> Vec<ComposeStack> {
    let mut stacks: Vec<ComposeStack> = projects
        .into_iter()
        .map(|project| {
            let mine: Vec<&ContainerRow> = rows
                .iter()
                .filter(|row| row.project == project.name)
                .collect();
            let mut files = project.files.clone();
            for row in &mine {
                for file in split_files(&row.config_files) {
                    if !files.contains(&file) {
                        files.push(file);
                    }
                }
            }
            files.sort();
            files.dedup();
            let working_dir = mine
                .iter()
                .find_map(|row| {
                    let dir = row.working_dir.trim().replace('\\', "/");
                    (!dir.is_empty()).then_some(dir)
                })
                .or_else(|| {
                    files
                        .first()
                        .and_then(|file| file.rsplit_once('/').map(|(dir, _)| dir.to_string()))
                })
                .unwrap_or_default();
            ComposeStack {
                name: project.name.clone(),
                services: mine.len(),
                running: mine.iter().filter(|row| row.state == "running").count(),
                status: project.status,
                files,
                working_dir,
            }
        })
        .collect();
    stacks.sort_by(|a, b| a.name.cmp(&b.name));
    stacks
}

impl ComposeService {
    pub(crate) fn from_row(row: &ContainerRow) -> Self {
        Self {
            name: if row.service.is_empty() {
                row.container.clone()
            } else {
                row.service.clone()
            },
            container: row.container.clone(),
            image: row.image.clone(),
            state: row.state.clone(),
            ports: row.ports.clone(),
        }
    }
}

/// 卷 / 绑定挂载 / 目录体积的一次性查询脚本；参数走 `sh -c` 的位置参数，
/// 不把路径拼进脚本体（`~` 与空格在这里都只是字面量）。
pub(crate) const PROBE_TEMPLATE: &str = r#"
project="$1"; wd="$2"; images="$3"
docker volume ls -q >/dev/null 2>&1 || { echo 'docker 命令不可用或当前用户没有 docker 权限' >&2; exit 3; }
printf 'P\t%s\n' "$wd"
if [ -n "$wd" ] && [ -d "$wd" ]; then
  printf 'S\t%s\n' "$(du -sk "$wd" 2>/dev/null | cut -f1)"
  for e in "$wd"/.env "$wd"/*.env; do [ -f "$e" ] && printf 'E\t%s\n' "$e"; done
else
  printf 'S\t0\n'
fi
for v in $(docker volume ls -q --filter "label=com.docker.compose.project=$project" 2>/dev/null); do
  mp=$(docker volume inspect -f '{{.Mountpoint}}' "$v" 2>/dev/null)
  short=$(docker volume inspect -f '{{ with index .Labels "com.docker.compose.volume" }}{{.}}{{ end }}' "$v" 2>/dev/null)
  size=$(du -sk "$mp" 2>/dev/null | cut -f1)
  if [ -n "$mp" ] && [ -r "$mp" ]; then read=1; else read=0; fi
  if [ -n "$short" ]; then kind=N; else kind=A; fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$kind" "$v" "$mp" "${size:-0}" "$read"
done
ids=$(docker ps -aq --filter "label=com.docker.compose.project=$project" 2>/dev/null)
if [ -n "$ids" ]; then
  docker inspect -f '{{range .Mounts}}{{if eq .Type "bind"}}B\t{{.Source}}\n{{end}}{{end}}' $ids 2>/dev/null
fi
for img in $images; do
  size=$(docker image inspect -f '{{.Size}}' "$img" 2>/dev/null)
  printf 'I\t%s\t%s\n' "$img" "${size:-0}"
done
"#;

pub(crate) fn probe_script_command(project: &str, working_dir: &str, images: &[String]) -> String {
    format!(
        "sh -c {} sh {} {} {}",
        shell_quote(PROBE_TEMPLATE.trim()),
        shell_quote(project),
        shell_quote(working_dir),
        shell_quote(&images.join(" "))
    )
}

/// 探测脚本的解析结果。
pub(crate) struct ProbeResult {
    pub(crate) volumes: Vec<ComposeVolume>,
    pub(crate) volume_bytes: u64,
    pub(crate) image_bytes: u64,
    pub(crate) project_bytes: u64,
    pub(crate) anonymous_volumes: usize,
    pub(crate) bind_mounts: Vec<String>,
    pub(crate) env_files: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn parse_probe(output: &str, services: &[ComposeService]) -> ProbeResult {
    let mut volumes = Vec::new();
    let mut bind_mounts = Vec::new();
    let mut env_files = Vec::new();
    let mut anonymous_volumes = 0usize;
    let mut volume_bytes = 0u64;
    let mut image_bytes = 0u64;
    let mut project_bytes = 0u64;
    let mut working_dir = String::new();

    for line in stdout_lines(output) {
        let mut fields = line.trim_end().split('\t').map(str::trim);
        let kind = match fields.next() {
            Some(kind) => kind,
            None => continue,
        };
        let rest: Vec<String> = fields.map(str::to_string).collect();
        let field = |index: usize| rest.get(index).map(String::as_str).unwrap_or("");
        match kind {
            "P" => working_dir = field(0).to_string(),
            // `du -sk` 给的是 KB。
            "S" => project_bytes = parse_u64(field(0)) * 1024,
            "E" => {
                if !field(0).is_empty() && !env_files.iter().any(|item| item == field(0)) {
                    env_files.push(field(0).to_string());
                }
            }
            // N name mountpoint size_kb readable
            "N" => {
                let size = parse_u64(field(2)) * 1024;
                volume_bytes += size;
                volumes.push(ComposeVolume {
                    name: field(0).to_string(),
                    mountpoint: field(1).to_string(),
                    size_bytes: size,
                    image: image_for_volume(field(0), services),
                    readable: field(3) == "1",
                });
            }
            "A" => anonymous_volumes += 1,
            "B" => {
                let source = field(0);
                if !source.is_empty()
                    && !source.starts_with(&working_dir)
                    && !bind_mounts.iter().any(|item| item == source)
                {
                    bind_mounts.push(source.to_string());
                }
            }
            // I image size_bytes
            "I" => image_bytes += parse_u64(field(1)),
            _ => {}
        }
    }

    let mut warnings = Vec::new();
    let unreadable: Vec<&str> = volumes
        .iter()
        .filter(|volume| !volume.readable)
        .map(|volume| volume.name.as_str())
        .collect();
    if !unreadable.is_empty() {
        warnings.push(format!(
            "{} 个数据卷（{}）无法从宿主机目录直接读取，将改用容器内 tar 导出",
            unreadable.len(),
            unreadable.join("、")
        ));
    }

    ProbeResult {
        volumes,
        volume_bytes,
        image_bytes,
        project_bytes,
        anonymous_volumes,
        bind_mounts,
        env_files,
        warnings,
    }
}

/// 为该卷挑一个本机已有的镜像做容器内导出兜底：命名卷是 `<项目>_<短名>`，短名即服务名。
pub(crate) fn image_for_volume(volume_name: &str, services: &[ComposeService]) -> String {
    let short = volume_name
        .rsplit_once('_')
        .map(|(_, tail)| tail)
        .unwrap_or(volume_name);
    services
        .iter()
        .find(|service| service.name == short)
        .or_else(|| {
            services
                .iter()
                .find(|service| volume_name.starts_with(&format!("{}_", service.name)))
        })
        .or_else(|| services.first())
        .map(|service| service.image.clone())
        .unwrap_or_default()
}

pub(crate) fn parse_u64(value: &str) -> u64 {
    value.trim().parse().unwrap_or(0)
}

pub(crate) fn dedup(values: Vec<String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for value in values {
        let value = value.trim().to_string();
        if !value.is_empty() && !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

pub(crate) async fn remote_home(client: &SshClient) -> Result<String> {
    let (code, output) = client
        .exec_capture("printf '%s' \"$HOME\"", 30)
        .await?;
    let home = stdout_lines(&output)
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if code != 0 || !home.starts_with('/') {
        return Err(CoreError::ssh(
            "无法确定远端用户目录（$HOME），请检查 SSH 登录用户的 shell 配置",
        ));
    }
    Ok(home)
}

/// 远端工作根目录：默认 `~/.deploycode/containers`；项目目录把它包进去了就退回 `/var/tmp`。
pub(crate) async fn remote_root(client: &SshClient, working_dir: &str) -> Result<String> {
    Ok(pick_remote_root(&remote_home(client).await?, working_dir))
}

pub(crate) fn pick_remote_root(home: &str, working_dir: &str) -> String {
    let home = home.trim_end_matches('/');
    let project = working_dir.trim_end_matches('/');
    let candidate = format!("{home}/.deploycode/containers");
    // 项目目录就是 home（或其祖先）时，暂存目录会被自己复制进去，打包递归套娃。
    if !project.is_empty() && candidate.starts_with(project) {
        return "/var/tmp/deploycode-containers".to_string();
    }
    candidate
}
