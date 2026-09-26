//! 容器备份与迁移：把一台服务器上的 docker-compose 项目打包成本机备份包，
//! 再无损恢复到另一台服务器。
//!
//! 全部操作通过 SSH 在服务器上执行 `docker` / `compose` 命令完成，数据经本机中转
//! （两台服务器之间不需要互相配信任），流程为：
//! 1. 发现：[`discover`] 列出项目并解析服务 / 卷 / 镜像；
//! 2. 快照：在源机 `~/.deploycode/containers/` 下拼出 `project/`（项目目录原样）、
//!    `volumes/`（每个命名卷一个 tar）、`images/`（`docker save`）与 `manifest.json`，打成一个 tar；
//! 3. 下载到本机 `<数据目录>/containers/`，这份文件就是可以反复使用的容器备份包；
//! 4. 恢复：上传备份包 -> `docker load` 镜像 -> 建卷并回灌数据 -> 释放项目文件 ->
//!    `compose up -d` -> 校验容器状态（两份脚本见 [`scripts`]）。
//!
//! 项目名决定数据落在哪里：compose 的命名卷叫 `<项目名>_<短名>`，所以恢复时保持同一个
//! 项目名，容器起来后就会挂到刚灌好数据的卷上 —— 这是「无损」的关键。

mod discover;
mod scripts;

use discover::{
    build_stacks, compose_command, compose_ls_command, container_rows, dedup, parse_compose_ls,
    parse_probe, probe_script_command, remote_home, remote_root, DiscoveredProject,
    INSPECT_TIMEOUT_SECS, QUERY_TIMEOUT_SECS,
};
use scripts::{build_manifest, build_restore_script, build_snapshot_script, target_dir_or};

pub use discover::{ComposeService, ComposeStack, ComposeStackDetail, ComposeVolume};
pub use scripts::{BundleManifest, BundleVolume, read_bundle_manifest};

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc::UnboundedSender;

use crate::engine::kill_script;
use crate::error::{CoreError, Result};
use crate::models::{
    now_string, new_id, ContainerEvent, ContainerRecord, ContainerRecordKind, ContainerRequest,
    ContainerRestoreRequest, DeployStatus, ServerConfig,
};
use crate::nginx::stdout_lines;
use crate::process::shell_quote;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;
use crate::tasklog::TaskLogger;
use crate::util::{format_duration, human_size};

/// 容器任务事件发送端（GUI 转发为 Tauri 事件，CLI 直接打印）。
pub type ContainerEventSender = UnboundedSender<ContainerEvent>;

/// 已解析并登记好的容器任务。
pub struct ContainerPlan {
    /// 快照来源服务器；纯恢复任务时这里就是目标服务器。
    pub source: ServerConfig,
    pub project: String,
    pub pause_source: bool,
    pub include_volumes: bool,
    pub include_images: bool,
    /// 本机备份包路径。
    pub bundle: PathBuf,
    /// 目标（服务器、目录、是否启动）；`None` 表示只备份到本机。
    pub target: Option<(ServerConfig, String, bool)>,
}

/// 引擎要跑的那件事。
pub enum ContainerJob {
    /// 在源机打快照（可选：接着恢复到目标机）。
    Snapshot(ContainerPlan),
    /// 用本机已有的备份包恢复。
    Restore(ContainerPlan),
}

// ---------------------------------------------------------------------------
// 引擎
// ---------------------------------------------------------------------------


/// 容器备份 / 迁移引擎。
pub struct ContainerEngine {
    store: Arc<Store>,
}

impl ContainerEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    async fn connect(&self, server: &ServerConfig) -> Result<SshClient> {
        let timeout = self.store.load_config()?.settings.connect_timeout_secs;
        SshClient::connect(server, timeout).await
    }

    /// 列出该机上由 docker compose 管理的项目。
    pub async fn list_stacks(&self, server: &ServerConfig) -> Result<Vec<ComposeStack>> {
        let client = self.connect(server).await?;
        let result = async {
            let compose = compose_command(&client).await?;
            let (code, output) = client
                .exec_capture(&compose_ls_command(&compose), QUERY_TIMEOUT_SECS)
                .await?;
            if code != 0 {
                return Err(CoreError::ssh(format!(
                    "查询 compose 项目失败（请确认该服务器已安装并启动 Docker）: {}",
                    output.trim()
                )));
            }
            let projects = parse_compose_ls(&output)?;
            let rows = container_rows(&client, None).await?;
            Ok::<Vec<ComposeStack>, CoreError>(build_stacks(projects, &rows))
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 解析一个项目的服务、数据卷与镜像，并给出预检提示。
    pub async fn inspect_stack(
        &self,
        server: &ServerConfig,
        project: &str,
    ) -> Result<ComposeStackDetail> {
        let project = validate_project(project)?;
        let client = self.connect(server).await?;
        let result = async {
            let compose = compose_command(&client).await?;
            let detail = self
                .probe(&client, &project, &compose.join(" "))
                .await?;
            Ok::<ComposeStackDetail, CoreError>(detail)
        }
        .await;
        client.disconnect().await;
        result
    }

    /// 采集项目的服务 / 卷 / 镜像 / 体积（调用方需已连上服务器）。
    async fn probe(
        &self,
        client: &SshClient,
        project: &str,
        compose_display: &str,
    ) -> Result<ComposeStackDetail> {
        let rows = container_rows(client, Some(project)).await?;
        if rows.is_empty() {
            return Err(CoreError::config(format!(
                "项目 {project} 在这台服务器上已没有容器记录（容器被删除过），无法解析它的数据卷"
            )));
        }
        let stack = build_stacks(
            vec![DiscoveredProject::from_rows(project, &rows)],
            &rows,
        )
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::config(format!("项目解析失败: {project}")))?;
        if stack.working_dir.is_empty() {
            return Err(CoreError::config(format!(
                "项目 {project} 没有记录工作目录，无法定位要打包的文件"
            )));
        }
        let services: Vec<ComposeService> = rows
            .iter()
            .filter(|row| row.project == project)
            .map(ComposeService::from_row)
            .collect();
        let images = dedup(
            services
                .iter()
                .map(|service| service.image.clone())
                .collect::<Vec<_>>(),
        );

        let (code, output) = client
            .exec_capture(
                &probe_script_command(project, &stack.working_dir, &images),
                INSPECT_TIMEOUT_SECS,
            )
            .await?;
        if code != 0 {
            return Err(CoreError::ssh(format!(
                "解析项目数据失败: {}",
                output.trim()
            )));
        }
        let probe = parse_probe(&output, &services);

        let mut warnings = probe.warnings;
        if probe.anonymous_volumes > 0 {
            warnings.push(format!(
                "检测到 {} 个匿名卷：容器重建时不会自动挂回旧匿名卷，需要长期保存的数据请写进 compose 的命名卷",
                probe.anonymous_volumes
            ));
        }
        for bind in &probe.bind_mounts {
            warnings.push(format!(
                "挂载了项目目录之外的路径 {bind}：它不会被打包，目标机需要自备"
            ));
        }

        Ok(ComposeStackDetail {
            stack,
            services,
            volumes: probe.volumes,
            images,
            env_files: probe.env_files,
            compose_command: compose_display.to_string(),
            volume_bytes: probe.volume_bytes,
            image_bytes: probe.image_bytes,
            project_bytes: probe.project_bytes,
            warnings,
        })
    }

    /// 校验参数并登记一条「运行中」的容器任务记录。
    pub fn prepare(&self, req: &ContainerRequest) -> Result<(ContainerRecord, ContainerJob)> {
        let config = self.store.load_config()?;
        let source = Store::find_server(&config, &req.server_id)?.clone();
        let project = validate_project(&req.project)?;
        if !req.include_volumes && !req.include_images {
            return Err(CoreError::config("数据卷与镜像至少要勾选一项，否则备份包是空的"));
        }
        let target = match &req.target {
            Some(target) => {
                let server = Store::find_server(&config, &target.server_id)?.clone();
                if server.id == source.id {
                    return Err(CoreError::config("目标服务器不能与来源服务器相同"));
                }
                Some((server, validate_remote_dir(&target.target_dir)?, target.start_services))
            }
            None => None,
        };

        let id = new_id();
        let bundle = self.bundle_path(&project, &id);
        let record = ContainerRecord {
            kind: if target.is_some() {
                ContainerRecordKind::Migrate
            } else {
                ContainerRecordKind::Backup
            },
            id: id.clone(),
            project: project.clone(),
            server_id: source.id.clone(),
            server_name: source.name.clone(),
            target_server_id: target.as_ref().map(|t| t.0.id.clone()).unwrap_or_default(),
            target_server_name: target.as_ref().map(|t| t.0.name.clone()).unwrap_or_default(),
            target_dir: target.as_ref().map(|t| t.1.clone()).unwrap_or_default(),
            bundle_path: bundle.to_string_lossy().into_owned(),
            bundle_size: 0,
            services: Vec::new(),
            volumes: Vec::new(),
            images: Vec::new(),
            include_volumes: req.include_volumes,
            include_images: req.include_images,
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };
        self.store
            .upsert_container(&record, config.settings.container_history_limit)?;

        let job = ContainerJob::Snapshot(ContainerPlan {
            source,
            project,
            pause_source: req.pause_source,
            include_volumes: req.include_volumes,
            include_images: req.include_images,
            bundle,
            target,
        });
        Ok((record, job))
    }

    /// 为「从本机备份包恢复」登记记录：项目名、卷清单与服务都取自包内 manifest。
    pub fn prepare_restore(
        &self,
        req: &ContainerRestoreRequest,
    ) -> Result<(ContainerRecord, ContainerJob)> {
        let config = self.store.load_config()?;
        let bundle = validate_bundle_path(&req.bundle_path)?;
        let manifest = read_bundle_manifest(&bundle)?;
        let server = Store::find_server(&config, &req.target.server_id)?.clone();
        let dir = target_dir_or(&manifest, &req.target)?;

        let record = ContainerRecord {
            kind: ContainerRecordKind::Restore,
            id: new_id(),
            project: manifest.project.clone(),
            server_id: String::new(),
            server_name: String::new(),
            target_server_id: server.id.clone(),
            target_server_name: server.name.clone(),
            target_dir: dir.clone(),
            bundle_path: bundle.to_string_lossy().into_owned(),
            bundle_size: std::fs::metadata(&bundle).map(|meta| meta.len()).unwrap_or(0),
            services: manifest.services.clone(),
            volumes: manifest.volumes.iter().map(|v| v.name.clone()).collect(),
            images: manifest.images.clone(),
            include_volumes: !manifest.volumes.is_empty(),
            include_images: !manifest.images.is_empty(),
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };
        self.store
            .upsert_container(&record, config.settings.container_history_limit)?;

        let job = ContainerJob::Restore(ContainerPlan {
            source: server.clone(),
            project: manifest.project.clone(),
            pause_source: false,
            include_volumes: record.include_volumes,
            include_images: record.include_images,
            bundle,
            target: Some((server, dir, req.target.start_services)),
        });
        Ok((record, job))
    }

    /// 备份包落地位置：`<数据目录>/containers/<项目>-<时间>-<记录前缀>.tar`，免设置。
    pub fn bundle_path(&self, project: &str, record_id: &str) -> PathBuf {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let name = format!(
            "{}-{}-{}.tar",
            safe_component(project),
            stamp,
            short_id(record_id)
        );
        self.store.container_bundle_dir().join(name)
    }

    /// 备份包存放目录（界面展示与「打开目录」用）。
    pub fn bundle_dir(&self) -> PathBuf {
        self.store.container_bundle_dir()
    }

    /// 执行任务。无论成功失败都会返回带最终状态的记录。
    /// 备份包轮转：同一个（服务器 + compose 项目）连本次成功任务在内只保留 `keep` 个包，
    /// 窗口之外的从磁盘删掉，并把那条记录里的 `bundle_path` 抹空——
    /// 界面的「恢复到目标服务器」入口就是按它判断有没有包可用，留着悬空路径只会让人点了才报错。
    ///
    /// 定时备份一晚落一个 GB 级文件，没有这一步会把系统盘写满。`keep` 为 0 表示完全不轮转。
    /// 只认 `<数据目录>/containers/` 下的直接子文件：老记录可能指向别的机器上的路径，
    /// 也可能被用户自己挪走了，那些一律跳过。
    fn prune_bundles(
        &self,
        record: &ContainerRecord,
        keep: usize,
        limit: usize,
    ) -> Result<Vec<(String, u64)>> {
        if keep == 0 || record.server_id.is_empty() || record.project.is_empty() {
            return Ok(Vec::new());
        }
        let bundle_dir = self.store.container_bundle_dir();
        let mut records = self.store.load_containers()?;
        let mut stale: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.id != record.id
                    && item.kind != ContainerRecordKind::Restore
                    && item.server_id == record.server_id
                    && item.project == record.project
                    && !item.bundle_path.is_empty()
            })
            .map(|(index, _)| index)
            .collect();
        if stale.len() < keep {
            return Ok(Vec::new());
        }
        // started_at 是 "%Y-%m-%d %H:%M:%S"，字典序就是时间序；本次任务自己不在候选里，
        // 所以窗口按 keep - 1 算，新下来的这个包永远不会被自己挤掉。
        stale.sort_by(|a, b| records[*b].started_at.cmp(&records[*a].started_at));
        let mut removed = Vec::new();
        let mut touched: Vec<ContainerRecord> = Vec::new();
        for index in stale.into_iter().skip(keep - 1) {
            let path = PathBuf::from(&records[index].bundle_path);
            if path.parent().map(|parent| parent != bundle_dir).unwrap_or(true) {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if path.is_file() {
                // 删不掉（被别的进程占着）就原样留着这条，下次任务收尾时再试一次。
                if std::fs::remove_file(&path).is_err() {
                    continue;
                }
                removed.push((name, records[index].bundle_size));
            }
            // 走到这里：要么刚删掉，要么文件早就不在了。两种情况这条路径都指不到东西了，
            // 抹空它，界面的「恢复」入口会跟着收起。
            records[index].bundle_path = String::new();
            touched.push(records[index].clone());
        }
        for item in touched {
            self.store.upsert_container(&item, limit)?;
        }
        Ok(removed)
    }

    pub async fn run(
        &self,
        mut record: ContainerRecord,
        job: ContainerJob,
        events: Option<ContainerEventSender>,
    ) -> ContainerRecord {
        let started = std::time::Instant::now();
        let mut logger = TaskLogger::new(
            events,
            |level, message| ContainerEvent::Log { level, message },
            Some(|percent, message| ContainerEvent::Progress { percent, message }),
        );
        logger.send(ContainerEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始{} · {}",
            kind_label(&record.kind),
            container_headline(&record)
        ));

        let result = self.execute(&mut record, &job, &mut logger).await;

        match result {
            Ok(()) => {
                record.status = DeployStatus::Success;
                logger.success(format!(
                    "{}完成（备份包 {}，耗时 {}）",
                    kind_label(&record.kind),
                    human_size(record.bundle_size),
                    format_duration(started.elapsed().as_millis() as u64)
                ));
            }
            Err(err) => {
                record.status = DeployStatus::Failed;
                record.error = Some(err.to_string());
                logger.error(format!("{}失败: {err}", kind_label(&record.kind)));
            }
        }

        let settings = self
            .store
            .load_config()
            .map(|config| config.settings)
            .unwrap_or_default();
        let limit = settings.container_history_limit;

        // 只有成功收尾才轮转：失败那次的半截包不该占住窗口，更不该顺手把上一晚的好包挤掉。
        if record.status == DeployStatus::Success {
            match self.prune_bundles(&record, settings.container_bundle_keep, limit) {
                Ok(removed) => {
                    for (name, _) in &removed {
                        logger.info(format!("清理旧备份包 {name}"));
                    }
                    if !removed.is_empty() {
                        let freed: u64 = removed.iter().map(|(_, size)| size).sum();
                        logger.success(format!(
                            "本轮清理 {} 个旧备份包，释放 {}",
                            removed.len(),
                            human_size(freed)
                        ));
                    }
                }
                Err(err) => logger.warn(format!("备份包轮转失败: {err}")),
            }
        }

        record.log = logger.joined();
        record.finished_at = Some(now_string());
        record.duration_ms = started.elapsed().as_millis() as u64;

        if let Err(err) = self.store.upsert_container(&record, limit) {
            logger.error(format!("保存容器记录失败: {err}"));
            record.log = logger.joined();
        }

        if let Some(sender) = logger.take_events() {
            let _ = sender.send(ContainerEvent::Finished {
                record: record.clone(),
            });
        }
        record
    }

    async fn execute(
        &self,
        record: &mut ContainerRecord,
        job: &ContainerJob,
        logger: &mut TaskLogger<ContainerEvent>,
    ) -> Result<()> {
        let settings = self.store.load_config()?.settings;
        let timeout = settings.container_timeout_secs.max(300);
        match job {
            ContainerJob::Snapshot(plan) => {
                logger.info(format!(
                    "来源: {} ({}@{})",
                    plan.source.name, plan.source.username, plan.source.host
                ));
                let source = self.connect(&plan.source).await?;
                // 打包阶段能占多少进度取决于是否还要接着恢复：只备份时下载就是收尾。
                let (pack_span, pull_span) = if plan.target.is_some() {
                    ((2, 45), (46, 55))
                } else {
                    ((2, 70), (70, 99))
                };
                let snapshot = async {
                    let compose = compose_command(&source).await?;
                    let detail = self.probe(&source, &plan.project, &compose.join(" ")).await?;
                    let root = remote_root(&source, &detail.stack.working_dir).await?;
                    self.snapshot(&source, plan, &detail, &compose, &root, &record.id, pack_span, pull_span, timeout, logger)
                        .await
                }
                .await;
                source.disconnect().await;
                let (manifest, size) = snapshot?;

                record.bundle_size = size;
                record.services = manifest.services.clone();
                record.volumes = manifest.volumes.iter().map(|v| v.name.clone()).collect();
                record.images = manifest.images.clone();
                logger.success(format!(
                    "备份包已生成: {} ({})",
                    plan.bundle.display(),
                    human_size(size)
                ));

                if let Some((server, dir, start)) = &plan.target {
                    self.restore(server, dir, *start, &plan.bundle, &manifest, &record.id, timeout, logger)
                        .await?;
                }
                Ok(())
            }
            ContainerJob::Restore(plan) => {
                let (server, dir, start) = plan
                    .target
                    .as_ref()
                    .ok_or_else(|| CoreError::config("恢复任务缺少目标服务器"))?;
                let manifest = read_bundle_manifest(&plan.bundle)?;
                logger.info(format!(
                    "备份包: {} · 项目 {} · {}",
                    plan.bundle.display(),
                    manifest.project,
                    human_size(std::fs::metadata(&plan.bundle).map(|m| m.len()).unwrap_or(0))
                ));
                logger.info(format!(
                    "目标: {} ({}@{}) -> {dir}",
                    server.name, server.username, server.host
                ));
                self.restore(server, dir, *start, &plan.bundle, &manifest, &record.id, timeout, logger)
                    .await
            }
        }
    }

    /// 在源机打包并下载到本机，返回清单与包大小。
    #[allow(clippy::too_many_arguments)]
    async fn snapshot(
        &self,
        client: &SshClient,
        plan: &ContainerPlan,
        detail: &ComposeStackDetail,
        compose: &[String],
        root: &str,
        record_id: &str,
        pack_span: (u8, u8),
        pull_span: (u8, u8),
        timeout: u64,
        logger: &mut TaskLogger<ContainerEvent>,
    ) -> Result<(BundleManifest, u64)> {
        let manifest = build_manifest(plan, detail, compose, &plan.source);
        let id_dir = format!("{root}/{record_id}");
        let remote_tar = format!("{root}/{record_id}.tar");
        let script_path = format!("{id_dir}/snapshot.sh");
        let script = build_snapshot_script(record_id, root, plan, &manifest);

        logger.command("正在来源服务器上打包 ...");
        if let Some(parent) = plan.bundle.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CoreError::io_path(parent, e))?;
        }

        let part = bundle_part_path(&plan.bundle);
        let outcome = async {
            put_script(client, &script_path, &script).await?;
            let size = run_script(
                client,
                &script_path,
                &format!("{id_dir}/snapshot.pid"),
                timeout,
                pack_span,
                logger,
            )
            .await?;

            logger.progress(pull_span.0, "正在下载备份包到本机 ...");
            let mut last = u8::MAX;
            client
                .download_file(&remote_tar, &part, &mut |received, total| {
                    let percent = scale(received, total, pull_span);
                    if percent != last {
                        last = percent;
                        logger.progress(percent, &format!("下载中 {percent}%"));
                    }
                })
                .await?;
            // 大小对上才改名，避免留下一个看起来完整的半截包。
            let local = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if size > 0 && local != size {
                return Err(CoreError::backup(format!(
                    "备份包下载不完整（远端 {size} 字节，本机 {local} 字节）"
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o600));
            }
            std::fs::rename(&part, &plan.bundle)
                .map_err(|e| CoreError::io_path(&plan.bundle, e))?;
            Ok::<(), CoreError>(())
        }
        .await;

        // 无论成功失败都清掉远端工作目录与包（备份包已在本机，服务器上不留副本）。
        let _ = client
            .exec_capture(
                &format!(
                    "{}; rm -rf {} {}",
                    kill_script(&format!("{id_dir}/snapshot.pid")),
                    shell_quote(&id_dir),
                    shell_quote(&remote_tar)
                ),
                60,
            )
            .await;
        if let Err(err) = outcome {
            let _ = std::fs::remove_file(&part);
            return Err(err);
        }
        let size = std::fs::metadata(&plan.bundle).map(|m| m.len()).unwrap_or(0);
        Ok((manifest, size))
    }

    /// 上传备份包到目标机并执行恢复脚本。
    async fn restore(
        &self,
        server: &ServerConfig,
        dir: &str,
        start_services: bool,
        bundle: &Path,
        manifest: &BundleManifest,
        record_id: &str,
        timeout: u64,
        logger: &mut TaskLogger<ContainerEvent>,
    ) -> Result<()> {
        let client = self.connect(server).await?;
        // 登记号先拿到：上传中途失败也要能把目标机上的半成品删干净。
        let mut staged: Option<(String, String)> = None;
        let outcome = async {
            let root = remote_root(&client, dir).await?;
            let compose = compose_command(&client).await?;
            let id_dir = format!("{root}/{record_id}");
            let remote_bundle = format!("{id_dir}/bundle.tar");
            let script_path = format!("{id_dir}/restore.sh");
            let script = build_restore_script(
                record_id,
                root.clone(),
                dir,
                start_services,
                manifest,
                &compose,
            );
            staged = Some((id_dir.clone(), format!("{id_dir}/restore.pid")));

            client
                .exec_capture(&format!("mkdir -p {}", shell_quote(&id_dir)), 60)
                .await?;
            logger.info(format!("正在上传备份包到 {} ...", server.name));
            let local_size = std::fs::metadata(bundle).map(|m| m.len()).unwrap_or(0);
            let mut last = u8::MAX;
            client
                .upload_file(bundle, &remote_bundle, &mut |sent, total| {
                    let percent = scale(sent, total, (56, 84));
                    if percent != last {
                        last = percent;
                        logger.progress(percent, &format!("上传中 {percent}%"));
                    }
                })
                .await?;
            // 备份包里有 .env：落到目标机后立即收紧权限，并核对大小再往下走。
            let (code, output) = client
                .exec_capture(
                    &format!(
                        "chmod 600 {q} && stat -c%s {q}",
                        q = shell_quote(&remote_bundle)
                    ),
                    30,
                )
                .await?;
            let remote_size = if code == 0 {
                stdout_lines(&output)
                    .find_map(|line| line.trim().parse::<u64>().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            if local_size > 0 && remote_size != local_size {
                return Err(CoreError::backup(format!(
                    "备份包上传不完整或不可读（本机 {local_size} 字节，远端 {remote_size} 字节）"
                )));
            }
            logger.progress(85, "备份包已就位");

            put_script(&client, &script_path, &script).await?;
            run_script(
                &client,
                &script_path,
                &format!("{id_dir}/restore.pid"),
                timeout,
                (85, 99),
                logger,
            )
            .await?;
            Ok::<(), CoreError>(())
        }
        .await;
        if outcome.is_err() {
            if let Some((id_dir, pidfile)) = &staged {
                let _ = client
                    .exec_capture(
                        &format!(
                            "{}; rm -rf {}",
                            kill_script(pidfile),
                            shell_quote(id_dir)
                        ),
                        30,
                    )
                    .await;
            }
        }
        client.disconnect().await;
        outcome
    }

    /// 应用退出 / 取消时清理某台服务器上本记录留下的远端文件。
    pub async fn cleanup_remote(&self, server: &ServerConfig, record_id: &str) -> Result<()> {
        let settings = self.store.load_config()?.settings;
        let client = SshClient::connect(server, settings.connect_timeout_secs).await?;
        let result = async {
            // 退出清理拿不到项目目录，逐个候选根目录都扫一遍（root 只在其中之一）。
            let home = remote_home(&client).await.unwrap_or_default();
            let roots: Vec<String> = [
                (!home.is_empty()).then(|| format!("{home}/.deploycode/containers")),
                Some("/var/tmp/deploycode-containers".to_string()),
            ]
            .into_iter()
            .flatten()
            .collect();
            for root in roots {
                let id_dir = format!("{root}/{record_id}");
                client
                    .exec_capture(
                        &format!(
                            "{}; {} ; rm -rf {} {}",
                            kill_script(&format!("{id_dir}/snapshot.pid")),
                            kill_script(&format!("{id_dir}/restore.pid")),
                            shell_quote(&id_dir),
                            shell_quote(&format!("{root}/{record_id}.tar"))
                        ),
                        30,
                    )
                    .await?;
            }
            Ok::<(), CoreError>(())
        }
        .await;
        client.disconnect().await;
        result
    }
}

// ---------------------------------------------------------------------------
// 远端执行
// ---------------------------------------------------------------------------

/// 上传脚本：base64 落盘（避开 here-doc 与引号的所有转义差异），再 0700。
pub(crate) async fn put_script(client: &SshClient, remote: &str, script: &str) -> Result<()> {
    let quoted = shell_quote(remote);
    // 项目文件与卷数据里都有 secrets：先 umask 077 建目录，脚本本身也只留 0700。
    let (code, output) = client
        .exec_capture(
            &format!(
                "umask 077; mkdir -p {} && printf %s {} | base64 -d > {quoted} && chmod 700 {quoted}",
                shell_quote(&parent_of(remote)),
                shell_quote(&base64_encode(script.as_bytes()))
            ),
            60,
        )
        .await?;
    if code != 0 {
        let _ = client.exec_capture(&format!("rm -f {quoted}"), 15).await;
        return Err(CoreError::ssh(format!(
            "上传远端脚本失败 {remote}: {}",
            output.trim()
        )));
    }
    Ok(())
}

pub(crate) fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => dir.to_string(),
        _ => "/".to_string(),
    }
}

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// 执行远端脚本：解析 `###STAGE` / `###VOL` / `###SIZE` / `###DONE` 标记，进度映射进 span。
pub(crate) async fn run_script(
    client: &SshClient,
    script: &str,
    pidfile: &str,
    timeout: u64,
    span: (u8, u8),
    logger: &mut TaskLogger<ContainerEvent>,
) -> Result<u64> {
    let mut size = 0u64;
    let command = format!("bash {}", shell_quote(script));
    let result = client
        .exec_stream(&command, timeout, &mut |kind, line| {
            let line = line.trim_end().to_string();
            if let Some(rest) = line.strip_prefix("###STAGE:") {
                if let Some((index, text)) = rest.split_once(':') {
                    let text = text.trim();
                    if let Ok(stage) = index.trim().parse::<u8>() {
                        logger.info(text);
                        let percent = scale(u64::from(stage), 6, span);
                        logger.progress(percent.max(span.0), text);
                    }
                }
            } else if let Some(name) = line.strip_prefix("###VOL:") {
                logger.info(format!("导出数据卷 {}", name.trim()));
            } else if let Some(image) = line.strip_prefix("###IMAGE:") {
                logger.info(format!("保存镜像 {}", image.trim()));
            } else if let Some(service) = line.strip_prefix("###SERVICE:") {
                let text = service.trim();
                if text.ends_with(":running") {
                    logger.success(text);
                } else {
                    logger.warn(text);
                }
            } else if let Some(value) = line.strip_prefix("###SIZE:") {
                if let Ok(parsed) = value.trim().parse::<u64>() {
                    size = parsed;
                    logger.info(format!("备份包大小: {}", human_size(parsed)));
                }
            } else if line == "###DONE" {
                logger.progress(span.1, "完成");
            } else if !line.is_empty() {
                match kind {
                    OutputKind::Stdout => logger.info(&line),
                    OutputKind::Stderr => logger.warn(&line),
                }
            }
        })
        .await;

    // 脚本自身的 trap 也会删，这里双保险（超时被断时 trap 可能根本没跑到）。
    let _ = client
        .exec_capture(
            &format!("{}; rm -f {}", kill_script(pidfile), shell_quote(script)),
            30,
        )
        .await;

    match result {
        Ok(0) => Ok(size),
        Ok(code) => Err(CoreError::backup(format!(
            "远端脚本执行失败（退出码 {code}），请查看上方日志"
        ))),
        Err(err) => {
            logger.warn("已尝试终止远端脚本，请确认服务器上没有残留进程");
            Err(CoreError::backup(format!("{err}（已尝试终止远端脚本）")))
        }
    }
}

/// 把 `done / total` 映射进一个百分比区间。
pub(crate) fn scale(done: u64, total: u64, span: (u8, u8)) -> u8 {
    if total == 0 {
        return span.0;
    }
    let ratio = (done.min(total) * 100 / total).min(100);
    span.0 + (ratio * u64::from(span.1 - span.0) / 100) as u8
}

// ---------------------------------------------------------------------------
// 校验与工具
// ---------------------------------------------------------------------------

pub(crate) fn validate_project(project: &str) -> Result<String> {
    let project = project.trim();
    if project.is_empty() {
        return Err(CoreError::config("请选择要打包的 compose 项目"));
    }
    // compose 项目名：字母数字开头，之后允许 _ . -；这个子集同时能安全用作文件名。
    let valid = project.len() <= 128
        && project
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && project
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !valid {
        return Err(CoreError::config(format!("compose 项目名不合法: {project}")));
    }
    Ok(project.to_string())
}

/// 远端目录校验：必须是绝对路径、不含 `..`，且不能是系统根目录或常见父目录。
pub(crate) fn validate_remote_dir(value: &str) -> Result<String> {
    let dir = value.trim().trim_end_matches('/').replace('\\', "/");
    if dir.is_empty() {
        return Err(CoreError::config("目标目录不能为空，例如 /opt/blog"));
    }
    if !dir.starts_with('/') {
        return Err(CoreError::config("目标目录必须是服务器上的绝对路径"));
    }
    if dir
        .split('/')
        .any(|segment| segment == ".." || segment.contains('\n') || segment.contains('\r'))
    {
        return Err(CoreError::config("目标目录不能包含 .. 或换行"));
    }
    const FORBIDDEN: [&str; 16] = [
        "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib64", "/opt", "/proc", "/root", "/sbin",
        "/srv", "/sys", "/usr", "/var",
    ];
    if FORBIDDEN.contains(&dir.as_str()) {
        return Err(CoreError::config(format!(
            "目标目录 {dir} 是系统目录，请改用项目专用目录，例如 /opt/blog"
        )));
    }
    Ok(dir)
}

fn validate_bundle_path(value: &str) -> Result<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::config("请选择本机上的容器备份包"));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_file() {
        return Err(CoreError::not_found(format!(
            "备份包不存在: {}",
            path.display()
        )));
    }
    Ok(crate::process::canonicalize_path(&path))
}

pub(crate) fn bundle_part_path(bundle: &Path) -> PathBuf {
    let name = bundle
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("container.tar");
    bundle.with_file_name(format!("{name}.part"))
}

pub(crate) fn short_id(record_id: &str) -> String {
    record_id.chars().take(8).collect()
}

pub(crate) fn safe_component(value: &str) -> String {
    let text: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let text = text.trim_matches('.').to_string();
    if text.is_empty() {
        "stack".to_string()
    } else {
        text
    }
}

/// `<项目>_<短名>` 里的短名；不符合命名规律的名字原样返回。
pub(crate) fn volume_short(project: &str, volume: &str) -> String {
    volume
        .strip_prefix(&format!("{project}_"))
        .filter(|tail| !tail.is_empty())
        .unwrap_or(volume)
        .to_string()
}

/// 绝对路径相对项目目录的写法；不在该目录下时退化为文件名。
pub(crate) fn relative_to(working_dir: &str, absolute: &str) -> String {
    let base = working_dir.trim_end_matches('/');
    if !base.is_empty() && absolute.starts_with(&format!("{base}/")) {
        return absolute[base.len() + 1..].to_string();
    }
    absolute
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(absolute)
        .to_string()
}

pub(crate) fn kind_label(kind: &ContainerRecordKind) -> &'static str {
    match kind {
        ContainerRecordKind::Backup => "容器备份",
        ContainerRecordKind::Migrate => "容器迁移",
        ContainerRecordKind::Restore => "容器恢复",
    }
}

pub(crate) fn container_headline(record: &ContainerRecord) -> String {
    let source = if record.server_name.is_empty() {
        String::new()
    } else {
        format!("{} · ", record.server_name)
    };
    if record.target_server_name.is_empty() {
        format!("{source}{}", record.project)
    } else {
        format!("{}{} → {}", source, record.project, record.target_server_name)
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::discover::{container_query, pick_remote_root, ContainerRow};
    use super::scripts::{
        estimate_mb, list_text, restore_volume_list, snapshot_volume_list,
    };
    use super::*;
    use crate::models::{ContainerTarget, SshAuth};

    fn server(name: &str) -> ServerConfig {
        ServerConfig::new(
            name.to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        )
    }

    fn manifest() -> BundleManifest {
        BundleManifest {
            version: 1,
            created_at: now_string(),
            project: "blog".to_string(),
            source_host: "10.0.0.1:22".to_string(),
            working_dir: "/opt/blog".to_string(),
            compose_command: vec!["docker".to_string(), "compose".to_string()],
            config_files: vec!["docker-compose.yml".to_string()],
            services: vec!["web".to_string(), "db".to_string()],
            volumes: vec![BundleVolume {
                name: "blog_data".to_string(),
                short: "data".to_string(),
                image: "postgres:16".to_string(),
            }],
            images: vec!["nginx:alpine".to_string(), "postgres:16".to_string()],
            volume_bytes: 2 * 1024 * 1024 * 1024,
            image_bytes: 500 * 1024 * 1024,
            project_bytes: 40 * 1024 * 1024,
        }
    }

    fn plan() -> ContainerPlan {
        ContainerPlan {
            source: server("prod-a"),
            project: "blog".to_string(),
            pause_source: false,
            include_volumes: true,
            include_images: true,
            bundle: PathBuf::from("/tmp/blog-20260101-000000-abcd1234.tar"),
            target: None,
        }
    }

    fn services() -> Vec<ComposeService> {
        vec![
            ComposeService {
                name: "db".to_string(),
                container: "blog-db-1".to_string(),
                image: "postgres:16".to_string(),
                state: "running".to_string(),
                ports: String::new(),
            },
            ComposeService {
                name: "web".to_string(),
                container: "blog-web-1".to_string(),
                image: "nginx:alpine".to_string(),
                state: "running".to_string(),
                ports: "0.0.0.0:80->80/tcp".to_string(),
            },
        ]
    }

    #[test]
    fn project_names_are_validated_as_docker_names() {
        assert_eq!(validate_project(" blog-2.x ").unwrap(), "blog-2.x");
        assert!(validate_project("").is_err());
        assert!(validate_project("_blog").is_err());
        assert!(validate_project("blog;rm -rf").is_err());
        assert!(validate_project("blog/../etc").is_err());
        assert!(validate_project(&"a".repeat(200)).is_err());
    }

    #[test]
    fn remote_dirs_reject_system_roots() {
        assert_eq!(validate_remote_dir("/opt/blog/").unwrap(), "/opt/blog");
        assert_eq!(validate_remote_dir("/srv/app").unwrap(), "/srv/app");
        for bad in ["/", "/etc", "/var", "/opt", "/srv", "/home", "/usr", "/root"] {
            assert!(
                validate_remote_dir(bad).is_err(),
                "{bad} 不应作为项目目录"
            );
        }
        assert!(validate_remote_dir("blog").is_err());
        assert!(validate_remote_dir("/opt/../etc").is_err());
        assert!(validate_remote_dir("  ").is_err());
    }

    #[test]
    fn compose_ls_parses_json_and_ignores_stderr_lines() {
        let output = "[stderr] WARNING: No swap limit support\n[{\"Name\":\"blog\",\"Status\":\"running (2)\",\"ConfigFiles\":\"/opt/blog/docker-compose.yml,/opt/blog/override.yml\"}]\n";
        let projects = parse_compose_ls(output).unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "blog");
        assert_eq!(projects[0].files.len(), 2);
        assert_eq!(projects[0].files[1], "/opt/blog/override.yml");
        assert!(parse_compose_ls("").unwrap().is_empty());
        // 老版本 compose 不带 --format json 时输出的是表头，必须报错而不是当成「没有项目」。
        assert!(parse_compose_ls("NAME STATUS CONFIG FILES").is_err());
    }

    #[test]
    fn container_row_needs_all_fields() {
        let line = "blog\tweb\tblog-web-1\tnginx:alpine\trunning\t0.0.0.0:80->80/tcp\t/opt/blog\t/opt/blog/docker-compose.yml";
        let row = ContainerRow::from_line(line).unwrap();
        assert_eq!(row.project, "blog");
        assert_eq!(row.service, "web");
        assert_eq!(row.working_dir, "/opt/blog");
        assert!(ContainerRow::from_line("blog\tweb").is_none());
        // 非 compose 容器（项目标签为空）不参与统计。
        assert!(ContainerRow::from_line("\tx\ty\tz\trunning\t\t\t").is_none());
    }

    #[test]
    fn container_query_quotes_the_format_template() {
        let query = container_query(Some("blog"));
        // 模板里有空格与引号，必须整体被引用，否则 shell 会拆开。
        assert!(query.contains("--format '"), "{query}");
        assert!(query.contains("label=com.docker.compose.project=blog"));
        assert!(!container_query(None).contains("--filter"));
    }

    #[test]
    fn stacks_merge_ls_entries_with_container_labels() {
        let rows = vec![
            ContainerRow::from_line("blog\tweb\tblog-web-1\tnginx:alpine\trunning\t\t/opt/blog\t/opt/blog/docker-compose.yml").unwrap(),
            ContainerRow::from_line("blog\tdb\tblog-db-1\tpostgres:16\texited\t\t/opt/blog\t/opt/blog/docker-compose.yml").unwrap(),
        ];
        let projects = vec![DiscoveredProject {
            name: "blog".to_string(),
            status: "running (1)".to_string(),
            files: vec![],
        }];
        let stacks = build_stacks(projects, &rows);
        assert_eq!(stacks.len(), 1);
        assert_eq!(stacks[0].services, 2);
        assert_eq!(stacks[0].running, 1);
        assert_eq!(stacks[0].working_dir, "/opt/blog");
        assert_eq!(stacks[0].files, vec!["/opt/blog/docker-compose.yml".to_string()]);
    }

    #[test]
    fn probe_output_splits_volumes_binds_and_sizes() {
        let output = "P\t/opt/blog\nS\t40960\nE\t/opt/blog/.env\nN\tblog_data\t/var/lib/docker/volumes/blog_data/_data\t2097152\t1\nA\tdeadbeef\t/var/lib/docker/volumes/deadbeef/_data\t10\t1\nB\t/etc/ssl/certs\nI\tpostgres:16\t400000000\n";
        let probe = parse_probe(output, &services());
        assert_eq!(probe.project_bytes, 40960 * 1024);
        assert_eq!(probe.volumes.len(), 1);
        assert_eq!(probe.volumes[0].name, "blog_data");
        assert_eq!(probe.volumes[0].size_bytes, 2097152 * 1024);
        assert!(probe.volumes[0].readable);
        // 卷的兜底镜像按 `<项目>_<服务>` 命名规律命中服务。
        assert_eq!(probe.volumes[0].image, "postgres:16");
        assert_eq!(probe.anonymous_volumes, 1);
        assert_eq!(probe.env_files, vec!["/opt/blog/.env".to_string()]);
        assert_eq!(probe.image_bytes, 400000000);
        assert_eq!(probe.bind_mounts, vec!["/etc/ssl/certs".to_string()]);
    }

    #[test]
    fn unreadable_volume_warns() {
        let output = "N\tblog_data\t\t0\t0\n";
        let probe = parse_probe(output, &services());
        assert_eq!(probe.volumes.len(), 1);
        assert!(!probe.volumes[0].readable);
        assert_eq!(probe.warnings.len(), 1);
        assert!(probe.warnings[0].contains("容器内 tar"));
    }

    #[test]
    fn probe_script_passes_paths_as_positional_arguments() {
        let command = probe_script_command(
            "blog",
            "/opt/my blog/.env",
            &["postgres:16".to_string(), "nginx:alpine".to_string()],
        );
        assert!(command.starts_with("sh -c "));
        // 带空格的路径整体被引用，不进脚本体。
        assert!(command.contains("'sh' ") || command.contains(" sh "));
        assert!(command.contains("'/opt/my blog/.env'"), "{command}");
        assert!(command.contains("'postgres:16 nginx:alpine'"));
    }

    #[test]
    fn snapshot_script_quotes_values_and_keeps_stage_markers() {
        let mut plan = plan();
        plan.pause_source = true;
        let mut bundle = manifest();
        bundle.project = "b'log".to_string();
        let script = build_snapshot_script("abcd1234", "/root/.deploycode/containers", &plan, &bundle);
        assert!(script.contains("###STAGE:1"));
        assert!(script.contains("###STAGE:7"));
        assert!(script.contains("###DONE"));
        assert!(script.contains("PROJECT='b'\\''log'"), "项目名未被引用: {script}");
        // shell_quote 只在需要时加引号：纯安全字符按字面量写出即可。
        assert!(script.contains("PAUSE=1"));
        assert!(script.contains("ROOT=/root/.deploycode/containers"));
        assert!(script.contains("NEED_MB="));
        // 卷列表一行一项，制表符分隔名字与兜底镜像。
        assert!(script.contains("blog_data\tpostgres:16"));
        // 清单整体留在单引号字面量里，不经 shell 解释。
        assert!(script.contains("printf %s '{\"version\":1"));
        // 归档目标在工作目录之外，不会把 tar 自身打进去。
        assert!(script.contains("tar -cf \"$OUT\" -C \"$STAGE\" manifest.json project volumes images"));
    }

    #[test]
    fn restore_script_keeps_project_name_and_new_dir() {
        let script = build_restore_script(
            "ff00",
            "/root/.deploycode/containers".to_string(),
            "/srv/my blog",
            true,
            &manifest(),
            &["docker".to_string(), "compose".to_string()],
        );
        // 目录里的空格必须被引用，否则远端 shell 会把它拆成两个参数。
        assert!(script.contains("TARGET_DIR='/srv/my blog'"));
        assert!(script.contains("###STAGE:1"));
        assert!(script.contains("###STAGE:6"));
        assert!(script.contains("docker volume create --label \"com.docker.compose.project=$PROJECT\""));
        // 项目名不变，容器才会挂到刚灌好数据的命名卷上。
        assert!(script.contains("compose -p \"$PROJECT\" \"${CF[@]}\" up -d --no-build"));
        // 配置文件按新目录下的相对路径传给 -f：清单里存相对路径，循环里拼新目录。
        assert!(script.contains("CF+=(-f \"$TARGET_DIR/$f\")"));
        assert!(script.contains("<<'DEPLOYCODE_LIST'
docker-compose.yml
DEPLOYCODE_LIST"));
        assert!(script.contains("EXPECTED=2"));
        // 恢复脚本的卷清单是三列：名字 / 短名 / 镜像（短名写回 compose 标签）。
        assert!(script.contains("blog_data\tdata\tpostgres:16"));
    }

    /// 来源服务器上的文件名属于外部输入：整行等于 here-doc 终止符的条目必须丢掉。
    #[test]
    fn list_items_that_would_close_the_heredoc_are_dropped() {
        let values = vec![
            "docker-compose.yml".to_string(),
            "DEPLOYCODE_LIST".to_string(),
            "sub/ok.yml".to_string(),
            "  ".to_string(),
        ];
        assert_eq!(list_text(&values), "docker-compose.yml\nsub/ok.yml");

        let dropped = BundleVolume {
            name: "DEPLOYCODE_LIST".to_string(),
            short: "data".to_string(),
            image: "postgres:16".to_string(),
        };
        let bad_image = BundleVolume {
            name: "blog_data".to_string(),
            short: "data".to_string(),
            image: "evil\timage".to_string(),
        };
        // 卷名不合法整条丢掉；镜像名不合法只留空列，由脚本报「没有可用镜像」而不是拼出怪命令。
        assert_eq!(
            restore_volume_list(&[dropped, bad_image.clone()]),
            "blog_data\tdata\t"
        );
        assert_eq!(snapshot_volume_list(&[bad_image]), "blog_data\t");
    }

    #[test]
    fn restore_script_can_skip_startup_and_uses_v1_binary() {
        let script = build_restore_script(
            "ff00",
            "/var/tmp/deploycode-containers".to_string(),
            "/srv/blog",
            false,
            &manifest(),
            &["docker-compose".to_string()],
        );
        assert!(script.contains("START=0"));
        assert!(script.contains("COMPOSE_V2=0"));
        assert!(script.contains("ROOT=/var/tmp/deploycode-containers"));
    }

    #[test]
    fn manifest_estimate_keeps_tar_headroom() {
        let estimate = estimate_mb(&manifest());
        // 约 2.5GB 源数据 + 30% 余量，不能只按源大小估算。
        assert!(estimate > 3000, "estimate {estimate}");
    }

    #[test]
    fn remote_root_avoids_nesting_inside_project_dir() {
        assert_eq!(
            pick_remote_root("/root", "/opt/blog"),
            "/root/.deploycode/containers"
        );
        // 项目目录就是 home 时，暂存目录会被自己复制进去 -> 换到 /var/tmp。
        assert_eq!(
            pick_remote_root("/root", "/root"),
            "/var/tmp/deploycode-containers"
        );
        assert_eq!(
            pick_remote_root("/home/app", "/home"),
            "/var/tmp/deploycode-containers"
        );
    }

    #[test]
    fn volume_short_strips_project_prefix() {
        assert_eq!(volume_short("blog", "blog_data"), "data");
        assert_eq!(volume_short("blog", "other_data"), "other_data");
        assert_eq!(volume_short("blog", "blog_"), "blog_");
    }

    #[test]
    fn relative_paths_resolve_inside_project_dir() {
        assert_eq!(
            relative_to("/opt/blog", "/opt/blog/docker/docker-compose.yml"),
            "docker/docker-compose.yml"
        );
        assert_eq!(relative_to("/opt/blog", "/etc/compose.yml"), "compose.yml");
        assert_eq!(relative_to("/", "/opt/a.yml"), "a.yml");
    }

    #[test]
    fn target_dir_falls_back_to_manifest_dir() {
        let bundle = manifest();
        let target = ContainerTarget {
            server_id: "s2".to_string(),
            target_dir: "   ".to_string(),
            start_services: true,
        };
        assert_eq!(target_dir_or(&bundle, &target).unwrap(), "/opt/blog");
        let target = ContainerTarget {
            target_dir: "/srv/other".to_string(),
            ..target
        };
        assert_eq!(target_dir_or(&bundle, &target).unwrap(), "/srv/other");
    }

    #[test]
    fn base64_encoding_handles_unaligned_tail_and_utf8() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode("中文".as_bytes()), "5Lit5paH");
    }

    #[test]
    fn progress_span_scaling_is_clamped() {
        assert_eq!(scale(0, 100, (10, 20)), 10);
        assert_eq!(scale(100, 100, (10, 20)), 20);
        assert_eq!(scale(50, 100, (10, 20)), 15);
        // total 为 0（空文件）时不能除零。
        assert_eq!(scale(0, 0, (10, 20)), 10);
        // 快照第 7 步（打包）落在区间末段，恢复第 6 步（校验）同样到顶。
        assert_eq!(scale(7, 6, (2, 45)), 45);
        assert_eq!(scale(3, 6, (85, 99)), 92);
    }

    /// 备份包轮转：同一个（服务器 + 项目）只留最近 keep 个，
    /// 多出来的删文件并把记录里的 `bundle_path` 抹空；目录外的路径与别的项目一律不动。
    #[test]
    fn prune_bundles_keeps_the_newest_and_blanks_the_rest() {
        use crate::models::AppConfig;

        let dir = std::env::temp_dir().join(format!(
            "deploycode-container-prune-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Arc::new(Store::new(&dir));
        let engine = ContainerEngine::new(store.clone());
        let bundles = store.container_bundle_dir();
        std::fs::create_dir_all(&bundles).unwrap();
        // 故意放在备份目录之外：轮转不许删用户自己挪走的包。
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let prod = server("prod");
        let prod_id = prod.id.clone();
        let mut config = AppConfig::default();
        config.servers.push(prod);
        store.save_config(&config).unwrap();

        let bundle = |dir: &Path, name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, b"tar").unwrap();
            path
        };
        let record = |id: &str, project: &str, started_at: &str, path: &Path| ContainerRecord {
            kind: ContainerRecordKind::Backup,
            id: id.to_string(),
            project: project.to_string(),
            server_id: prod_id.clone(),
            server_name: "prod".to_string(),
            target_server_id: String::new(),
            target_server_name: String::new(),
            target_dir: String::new(),
            bundle_path: path.to_string_lossy().into_owned(),
            bundle_size: 3,
            services: Vec::new(),
            volumes: Vec::new(),
            images: Vec::new(),
            include_volumes: true,
            include_images: true,
            status: DeployStatus::Success,
            error: None,
            log: String::new(),
            started_at: started_at.to_string(),
            finished_at: None,
            duration_ms: 0,
        };

        let moved = bundle(&elsewhere, "blog-moved.tar");
        let oldest = bundle(&bundles, "blog-0.tar");
        let older = bundle(&bundles, "blog-1.tar");
        let mid = bundle(&bundles, "blog-2.tar");
        let newest_old = bundle(&bundles, "blog-3.tar");
        let other_project = bundle(&bundles, "api-0.tar");
        let current = bundle(&bundles, "blog-cur.tar");

        let history = vec![
            record("moved", "blog", "2026-09-20 03:30:00", &moved),
            record("oldest", "blog", "2026-09-21 03:30:00", &oldest),
            record("older", "blog", "2026-09-22 03:30:00", &older),
            record("mid", "blog", "2026-09-23 03:30:00", &mid),
            record("newest", "blog", "2026-09-24 03:30:00", &newest_old),
            record("api", "api", "2026-09-25 03:30:00", &other_project),
        ];
        for item in &history {
            store.upsert_container(item, 100).unwrap();
        }
        let running = record("cur", "blog", "2026-09-27 03:30:00", &current);

        // keep = 0 表示关掉轮转：一个都不许动。
        assert!(engine
            .prune_bundles(&running, 0, 100)
            .unwrap()
            .is_empty());
        assert!(oldest.is_file() && moved.is_file());

        // keep = 2：本次这个占一席，历史里只留最新的一个。
        let removed = engine.prune_bundles(&running, 2, 100).unwrap();
        let names: Vec<String> = removed.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(names, vec!["blog-2.tar", "blog-1.tar", "blog-0.tar"]);
        assert!(!oldest.is_file() && !older.is_file() && !mid.is_file());
        assert!(newest_old.is_file(), "窗口内的最新历史包要留下");
        assert!(current.is_file(), "刚做出来的包不该被自己挤掉");
        assert!(other_project.is_file(), "别的项目的包不在轮转范围");
        assert!(moved.is_file(), "备份目录外的文件一律不碰");

        let stored = store.load_containers().unwrap();
        let path_of = |id: &str| -> String {
            stored
                .iter()
                .find(|item| item.id == id)
                .expect("记录应还在")
                .bundle_path
                .clone()
        };
        // 记录留在列表里（历史与日志还有用），只是不再指向一个已经不存在的文件。
        assert_eq!(path_of("oldest"), "");
        assert_eq!(path_of("older"), "");
        assert_eq!(path_of("mid"), "");
        assert_eq!(path_of("newest"), newest_old.to_string_lossy());
        assert_eq!(path_of("moved"), moved.to_string_lossy());
        assert_eq!(path_of("api"), other_project.to_string_lossy());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
