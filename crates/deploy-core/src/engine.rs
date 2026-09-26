use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use crate::error::{CoreError, Result};
use crate::git::Git;
use crate::models::{
    elapsed_ms_since, now_string, DeployEvent, DeployRecord, DeployRequest, DeployStatus,
    EnvFileConfig, LogLevel, RepoConfig, RepoInfo, ServerConfig, Settings,
};
use crate::process::{sha256_file, shell_quote};
use crate::release;
use crate::security::SecurityReport;
use crate::ssh::{OutputKind, SshClient};
use crate::store::Store;
use crate::tasklog::TaskLogger;
use crate::util::{format_duration, human_size};

/// 部署结束后自动删除本地临时归档（成功或失败都会触发）。
///
/// 打包跑在 `spawn_blocking` 中，任务被 abort 时无法中断阻塞线程：guard 释放时归档文件
/// 可能尚未创建。因此除了立刻删除，还记录「已取消」标记，让打包线程结束后再清理一次，
/// 避免取消部署后临时文件残留在数据目录。
struct TempArchiveGuard {
    paths: Vec<PathBuf>,
    cancelled: Arc<AtomicBool>,
}

impl TempArchiveGuard {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Drop for TempArchiveGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 部署事件发送端（GUI 转发为 Tauri 事件，CLI 直接打印）。
pub type EventSender = UnboundedSender<DeployEvent>;

/// 一次批量的结果：每台一条记录，按执行顺序排列。
///
/// `blocked` 只在「第一台连准备都没通过」时有值：此时没有任何记录写盘、也没有
/// started/finished 事件发出，界面等不到收敛信号，必须由调用方把错误带回给请求方。
pub struct DeployBatch {
    pub records: Vec<DeployRecord>,
    pub blocked: Option<CoreError>,
}

/// 部署引擎：组织“打包 -> 上传 -> 解压 -> 执行脚本”的完整流程。
pub struct DeployEngine {
    store: Arc<Store>,
}

impl DeployEngine {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// 校验参数、解析版本并生成一条“运行中”的部署记录（写入历史）。
    pub fn prepare(&self, req: &DeployRequest) -> Result<DeployRecord> {
        let config = self.store.load_config()?;
        let repo = Store::find_repo(&config, &req.repo_id)?.clone();
        let server = Store::find_server(&config, &req.server_id)?.clone();

        if req.target_dir.trim().is_empty() {
            return Err(CoreError::deploy("请选择部署目录"));
        }
        // 必须是服务器上的绝对路径：相对路径会落进 SSH 登录目录，而 `~` 经 shell_quote
        // 是字面量（会在登录目录下建一个叫 ~ 的目录），装错位置比当场报错难查得多。
        // 保存部署配置与原子发布目录早就这么要求，这里补齐手填和服务器默认值那条路。
        let target_dir = release::normalize_target(&req.target_dir)?;

        let git = Git::open(&repo.path)?;
        if !git.is_repo() {
            return Err(CoreError::git(format!(
                "{} 尚未初始化 Git 仓库，请先在仓库页绑定远端仓库地址",
                repo.name
            )));
        }
        // 版本留空表示不走分支：直接打包当前工作区（含未提交改动）。
        let rev = req.rev.trim().to_string();
        let worktree = rev.is_empty();
        let resolved = if worktree { None } else { Some(git.resolve(&rev)?) };

        // 环境文件：解压后用本地文件覆盖服务器上的对应文件；提前校验，避免部署开始后才报错。
        let env_files = if req.upload_env {
            normalize_env_files(&repo.env_files)?
        } else {
            Vec::new()
        };
        for file in &env_files {
            if !std::path::Path::new(&file.local_path).is_file() {
                return Err(CoreError::deploy(format!(
                    "环境文件不存在: {}",
                    file.local_path
                )));
            }
        }

        // 工作区部署只把当前 HEAD 作为参考信息记录；空仓库没有 HEAD 也允许部署。
        let (commit, commit_short, commit_subject) = match &resolved {
            Some(resolved) => (
                resolved.hash.clone(),
                resolved.short.clone(),
                resolved.subject.clone(),
            ),
            None => git
                .resolve("HEAD")
                .map(|head| (head.hash, head.short, head.subject))
                .unwrap_or_default(),
        };
        let branch = if worktree {
            git.current_branch().unwrap_or_default()
        } else {
            rev.clone()
        };

        let record = DeployRecord {
            id: uuid::Uuid::new_v4().to_string(),
            repo_id: repo.id.clone(),
            repo_name: repo.name.clone(),
            rev,
            branch,
            worktree,
            commit,
            commit_short,
            commit_subject,
            server_id: server.id.clone(),
            server_name: server.name.clone(),
            target_dir,
            script_dir: normalize_script_dir(&req.script_dir, &config.settings.script_dir),
            scripts: normalize_scripts(&req.scripts),
            run_scripts: req.run_scripts,
            env_files,
            atomic_release: config.settings.atomic_release,
            release_dir: None,
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            started_at: now_string(),
            finished_at: None,
            duration_ms: 0,
        };

        self.store
            .upsert_history(&record, config.settings.history_limit)?;
        Ok(record)
    }

    /// 按同一份配置串行部署到多台服务器：任意一台失败即停止，剩余机器不再部署。
    ///
    /// 每台仍是一条独立记录（复用 `prepare` + `run`），事件按 recordId 分流，界面沿用现有单任务归约。
    /// `on_begin` 在每台真正开始前回调一次，供调用方登记这一台的取消句柄。
    /// 整个批次共用一个发送端并持有到最后一台结束，接收端才不会在中途认为任务已收尾。
    pub async fn run_targets(
        &self,
        base: &DeployRequest,
        server_ids: &[String],
        events: Option<EventSender>,
        mut on_begin: impl FnMut(&DeployRecord),
    ) -> DeployBatch {
        let total = server_ids.len();
        let mut results = Vec::with_capacity(total);
        let mut blocked = None;
        for (index, server_id) in server_ids.iter().enumerate() {
            let request = DeployRequest {
                server_id: server_id.clone(),
                ..base.clone()
            };
            let record = match self.prepare(&request) {
                Ok(record) => record,
                Err(err) => {
                    // 准备阶段失败说明这台连开始条件都不满足（服务器已被删除等）：
                    // 不写记录、也不继续，避免把同一个故障推到剩余机器上。
                    send_batch_log(
                        &events,
                        LogLevel::Error,
                        format!(
                            "[批次 {}/{}] 准备部署失败：{err}；已停止，剩余机器未部署",
                            index + 1,
                            total
                        ),
                    );
                    // 一台都没跑起来时批次不会发出任何 started/finished 事件，调用方无从知道
                    // 它已经结束：把首台的准备错误带出去，由命令层像单台部署那样同步报错。
                    if results.is_empty() {
                        blocked = Some(err);
                    }
                    break;
                }
            };
            send_batch_log(
                &events,
                LogLevel::Info,
                format!(
                    "[批次 {}/{}] 开始部署到 {}",
                    index + 1,
                    total,
                    record.server_name
                ),
            );
            on_begin(&record);
            let record = self.run(record, request, events.clone()).await;
            let failed = record.status != DeployStatus::Success;
            results.push(record);
            if failed {
                let done = results
                    .iter()
                    .filter(|item| item.status == DeployStatus::Success)
                    .count();
                send_batch_log(
                    &events,
                    LogLevel::Warn,
                    format!("已停止：{done}/{total} 台成功，剩余机器未部署"),
                );
                break;
            }
        }
        if total > 0 && results.len() == total {
            send_batch_log(
                &events,
                LogLevel::Success,
                format!("批量部署完成：{total} 台全部成功"),
            );
        }
        DeployBatch {
            records: results,
            blocked,
        }
    }

    /// 执行部署流程。无论成功失败都会返回带有最终状态的记录。
    pub async fn run(
        &self,
        mut record: DeployRecord,
        req: DeployRequest,
        events: Option<EventSender>,
    ) -> DeployRecord {
        let started = Instant::now();
        let mut logger = TaskLogger::new(
            events,
            |level, message| DeployEvent::Log { level, message },
            Some(|percent, message| DeployEvent::Progress { percent, message }),
        );

        logger.send(DeployEvent::Started {
            record_id: record.id.clone(),
        });
        logger.info(format!(
            "开始部署 {} @ {} -> {}",
            record.repo_name, record.commit_short, record.server_name
        ));

        // 原子发布：每次部署使用独立的版本目录，成功后切换 current 软链。
        let release = if record.atomic_release {
            Some(release::release_name(&record.commit_short, &record.id))
        } else {
            None
        };
        let result = self.execute(&record, &req, release.as_deref(), &mut logger).await;

        match result {
            Ok(()) => {
                record.status = DeployStatus::Success;
                if let Some(name) = &release {
                    record.release_dir = Some(name.clone());
                }
                logger.success(format!(
                    "部署成功（耗时 {}）",
                    format_duration(started.elapsed().as_millis() as u64)
                ));
            }
            Err(err) => {
                record.status = DeployStatus::Failed;
                record.error = Some(err.to_string());
                if let Some(name) = &release {
                    logger.warn(format!(
                        "未切换 current（线上版本不受影响），失败版本保留在 {}/{}/{name} 便于排查",
                        record.target_dir.trim_end_matches('/'),
                        release::RELEASES_DIR
                    ));
                }
                logger.error(format!("部署失败: {err}"));
            }
        }

        record.log = logger.joined();
        record.finished_at = Some(now_string());
        record.duration_ms = started.elapsed().as_millis() as u64;

        let limit = self
            .store
            .load_config()
            .map(|config| config.settings.history_limit)
            .unwrap_or(500);
        if let Err(first_err) = self.store.upsert_history(&record, limit) {
            // 写入失败不能让最终状态静默丢失：记录错误事件（CLI/GUI 均可见）后重试一次。
            logger.error(format!("保存部署记录失败: {first_err}"));
            record.log = logger.joined();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if let Err(second_err) = self.store.upsert_history(&record, limit) {
                logger.error(format!("重试保存部署记录仍失败: {second_err}"));
                record.log = logger.joined();
            }
        }

        if let Some(sender) = logger.take_events() {
            let _ = sender.send(DeployEvent::Finished {
                record: record.clone(),
            });
        }
        record
    }

    async fn execute(
        &self,
        record: &DeployRecord,
        req: &DeployRequest,
        release: Option<&str>,
        logger: &mut TaskLogger<DeployEvent>,
    ) -> Result<()> {
        let config = self.store.load_config()?;
        let settings = config.settings.clone();
        let repo = Store::find_repo(&config, &record.repo_id)?.clone();
        let server = Store::find_server(&config, &record.server_id)?.clone();

        let target = record.target_dir.trim().trim_end_matches('/').to_string();
        if target.is_empty() {
            return Err(CoreError::deploy("部署目录不能为空"));
        }
        // 原子发布时解压 / 执行脚本都在独立的 releases/<版本> 目录内进行。
        let deploy_dir = match release {
            Some(name) => format!("{target}/{}/{}", release::RELEASES_DIR, name),
            None => target.clone(),
        };

        logger.info(format!("仓库: {} ({})", repo.name, repo.path));
        if record.worktree {
            let base = if record.commit_short.is_empty() {
                String::new()
            } else {
                format!("（HEAD {} {}）", record.commit_short, record.commit_subject)
            };
            logger.info(format!("版本: 当前工作区（含未提交改动）{base}"));
        } else {
            logger.info(format!(
                "版本: {} [{}] {}",
                record.commit_short, record.rev, record.commit_subject
            ));
        }
        logger.info(format!(
            "服务器: {} ({}@{})",
            server.name, server.username, server.host
        ));
        logger.info(format!("部署目录: {target}"));
        if let Some(name) = release {
            logger.info(format!(
                "原子发布: 版本目录 {}/{}，成功后切换 current 软链",
                release::RELEASES_DIR,
                name
            ));
        }

        // 1. 本地打包
        logger.info("正在打包代码 ...");
        // 文件名带记录 ID，避免同一仓库/提交并发部署时互相覆盖临时归档。
        let revision = if record.worktree && record.commit.is_empty() {
            "worktree".to_string()
        } else {
            short_hash(&record.commit)
        };
        let stem = format!(
            "{}-{}-{}",
            sanitize_component(&repo.name),
            revision,
            record.id
        );
        let archive_name = format!("{stem}.tar.gz");
        let tar_path = self.store.prepare_temp_file(&format!("{stem}.tar"))?;
        let gz_path = self.store.prepare_temp_file(&archive_name)?;
        // 部署结束（含失败 / 取消）后删除本地临时压缩包，避免 temp 目录无限增长。
        let archive_guard = TempArchiveGuard::new(vec![tar_path.clone(), gz_path.clone()]);
        let archive_cancelled = archive_guard.cancelled.clone();
        let size = {
            let repo_path = repo.path.clone();
            let commit = record.commit.clone();
            let worktree = record.worktree;
            let tar = tar_path.clone();
            let gz = gz_path.clone();
            tokio::task::spawn_blocking(move || -> Result<u64> {
                let git = Git::open(&repo_path)?;
                let result = if worktree {
                    git.archive_worktree(&tar, &gz)
                } else {
                    git.archive(&commit, &tar, &gz)
                };
                // 任务被 abort 时 guard 已先行删除（此时文件可能还没建出来）：这里补删一次。
                if archive_cancelled.load(Ordering::SeqCst) {
                    let _ = std::fs::remove_file(&tar);
                    let _ = std::fs::remove_file(&gz);
                }
                result
            })
            .await
            .map_err(|err| CoreError::deploy(format!("打包任务异常: {err}")))??
        };
        logger.success(format!(
            "打包完成: {} ({})",
            archive_name,
            human_size(size)
        ));

        // 2. 建立 SSH 连接
        logger.info(format!("正在连接服务器 {} ...", server.name));
        let client = SshClient::connect(&server, settings.connect_timeout_secs).await?;
        logger.success(format!("SSH 连接成功 ({})", client.label()));

        // 3. 准备目录并上传
        let remote_dir = format!("{target}/.deploy_code");
        client.mkdir_p(&target).await?;
        if release.is_some() {
            client.mkdir_p(&deploy_dir).await?;
        }
        client.mkdir_p(&remote_dir).await?;
        let remote_archive = format!("{remote_dir}/{archive_name}");

        // 上传、解压、替换环境文件与执行脚本包成一体：无论哪一步失败，
        // 都在最后统一清理远端压缩包，避免失败/中止时在服务器上留下残包。
        let deployment: Result<()> = async {
            logger.info("正在上传代码包 ...");
            let mut last_percent = u8::MAX;
            client
                .upload_file(&gz_path, &remote_archive, &mut |sent, total| {
                    let percent = if total > 0 {
                        ((sent * 100 / total).min(100)) as u8
                    } else {
                        0
                    };
                    if percent != last_percent {
                        last_percent = percent;
                        logger.progress(percent, &format!("上传中 {percent}%"));
                    }
                })
                .await?;
            logger.success(format!("上传完成 ({})", human_size(size)));

            // 上传完整性校验：本地与服务器分别计算 SHA-256 并比对，避免断网 / 磁盘写满
            // 导致的半包被当作正常包解压；服务器缺少校验工具时跳过（老系统兼容）。
            match remote_sha256(&client, &remote_archive).await? {
                Some(remote_hash) => {
                    let gz = gz_path.clone();
                    let local_hash = tokio::task::spawn_blocking(move || sha256_file(&gz))
                        .await
                        .map_err(|err| CoreError::deploy(format!("校验任务异常: {err}")))??
                        .to_ascii_lowercase();
                    if local_hash != remote_hash {
                        return Err(CoreError::deploy(format!(
                            "上传校验失败：本地 {local_hash}，服务器 {remote_hash}（可能上传中断，请重试）"
                        )));
                    }
                    logger.success(format!("校验通过 (sha256 {})", short_hash(&local_hash)));
                }
                None => logger.warn("服务器缺少 sha256sum / shasum / openssl，跳过上传校验"),
            }

            // 4. 解压到目标目录（原子发布时为该版本的独立目录）
            logger.info("正在解压到目标目录 ...");
            let extract_cmd = format!(
                "tar -xzf {} -C {}",
                shell_quote(&remote_archive),
                shell_quote(&deploy_dir)
            );
            let (code, output) = client
                .exec_capture(&extract_cmd, settings.script_timeout_secs)
                .await?;
            if code != 0 {
                return Err(CoreError::deploy(format!(
                    "解压失败（退出码 {code}）: {}",
                    output.trim()
                )));
            }
            logger.success("解压完成");

            // 5. 上传并替换环境文件（在脚本执行前覆盖，确保脚本读到的是新内容）
            if !record.env_files.is_empty() {
                logger.info(format!("正在上传 {} 个环境文件 ...", record.env_files.len()));
                let base = deploy_dir.trim_end_matches('/');
                for file in &record.env_files {
                    let local = std::path::Path::new(&file.local_path);
                    let remote = format!("{base}/{}", file.remote_path);
                    if let Some((parent, _)) = file.remote_path.rsplit_once('/') {
                        client.mkdir_p(&format!("{base}/{parent}")).await?;
                    }
                    // 先上传到同目录临时文件，再原子替换：上传中断（断网/磁盘满）时
                    // 不会截断服务器上原有的环境文件。
                    let temp = format!("{remote}.deploycode-tmp");
                    if let Err(err) = client.upload_file(local, &temp, &mut |_, _| {}).await {
                        remove_remote_file(&client, &temp).await;
                        return Err(CoreError::deploy(format!(
                            "上传环境文件失败 {}: {err}",
                            file.remote_path
                        )));
                    }
                    let (code, output) = match client
                        .exec_capture(&env_replace_script(&remote, &temp), 60)
                        .await
                    {
                        Ok(result) => result,
                        Err(err) => {
                            // 超时或断连时临时文件还留在服务器上，里面是 env 的明文内容。
                            remove_remote_file(&client, &temp).await;
                            return Err(CoreError::deploy(format!(
                                "替换环境文件失败 {}: {err}",
                                file.remote_path
                            )));
                        }
                    };
                    if code != 0 {
                        remove_remote_file(&client, &temp).await;
                        return Err(CoreError::deploy(format!(
                            "替换环境文件失败 {}: {}",
                            file.remote_path,
                            output.trim()
                        )));
                    }
                    logger.success(format!("已替换环境文件: {}", file.remote_path));
                }
            }

            // 6. 执行项目脚本
            if record.run_scripts {
                self.run_scripts(record, req, &deploy_dir, release, &client, logger, &settings)
                    .await?;
            } else {
                logger.info("已按部署选项跳过脚本执行");
            }

            // 7. 原子发布：脚本全部成功后标记版本并切换 current 软链（失败时线上仍是旧版本）
            if let Some(name) = release {
                // 完成后才写标记：失败 / 中断的版本不会出现在可回滚列表中。
                let marker = format!("{deploy_dir}/{}", release::READY_MARKER);
                let (code, output) = client
                    .exec_capture(&format!("touch {}", shell_quote(&marker)), 30)
                    .await?;
                if code != 0 {
                    return Err(CoreError::deploy(format!(
                        "标记版本失败: {}",
                        output.trim()
                    )));
                }
                logger.info("正在切换 current 软链 ...");
                let (code, output) = client
                    .exec_capture(&release::switch_command(&target, name), 60)
                    .await?;
                if code != 0 {
                    return Err(CoreError::deploy(format!(
                        "切换 current 失败: {}",
                        output.trim()
                    )));
                }
                logger.success(format!("已切换 {target}/{} -> {}/{}", release::CURRENT_LINK, release::RELEASES_DIR, name));
                // 切换成功后部署事实已经生效：立即落库为成功，避免随后崩溃 / 取消
                // 导致记录与线上状态不一致（清理旧版本失败不影响部署结果）。
                if let Ok(mut saved) = self.store.find_record(&record.id) {
                    saved.status = DeployStatus::Success;
                    saved.release_dir = Some(name.to_string());
                    saved.finished_at = Some(now_string());
                    saved.duration_ms = elapsed_ms_since(&saved.started_at);
                    saved.log = logger.joined();
                    let limit = self
                        .store
                        .load_config()
                        .map(|config| config.settings.history_limit)
                        .unwrap_or(500);
                    let _ = self.store.upsert_history(&saved, limit);
                }
                match release::prune_releases(&client, &target, settings.release_keep).await {
                    Ok(removed) if !removed.is_empty() => {
                        logger.info(format!("已清理旧版本: {}", removed.join(", ")));
                    }
                    Ok(_) => {}
                    Err(err) => logger.warn(format!("清理旧版本失败（不影响本次部署）: {err}")),
                }
                logger.info(format!(
                    "请确保服务 / Web 站点指向 {target}/{}",
                    release::CURRENT_LINK
                ));
            }

            Ok(())
        }
        .await;

        if !settings.keep_remote_archive {
            let _ = client
                .exec_capture(&format!("rm -f {}", shell_quote(&remote_archive)), 60)
                .await;
        }

        deployment?;

        client.disconnect().await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_scripts(
        &self,
        record: &DeployRecord,
        req: &DeployRequest,
        deploy_dir: &str,
        release: Option<&str>,
        client: &SshClient,
        logger: &mut TaskLogger<DeployEvent>,
        settings: &Settings,
    ) -> Result<()> {
        // 脚本在解压出的目录内执行；pidfile 始终放在部署根目录的 .deploy_code 下，
        // 这样应用退出时按 record.target_dir 重连也能找到并终止脚本。
        let target = deploy_dir.trim().trim_end_matches('/');
        let root = record.target_dir.trim().trim_end_matches('/');
        let script_dir = normalize_script_dir(&req.script_dir, &settings.script_dir);

        let explicit = normalize_scripts(&req.scripts);
        let scripts: Vec<String> = if explicit.is_empty() {
            let cmd = format!(
                "cd {} && find {} -maxdepth 1 -type f -name '*.sh' 2>/dev/null | sort",
                shell_quote(target),
                shell_quote(&script_dir)
            );
            let (_, output) = client.exec_capture(&cmd, 60).await?;
            output
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty() && !line.starts_with("[stderr]"))
                .collect()
        } else {
            // 先全部校验存在性，避免执行到一半才发现缺失脚本。
            let mut paths = Vec::with_capacity(explicit.len());
            for name in explicit {
                let path = if name.contains('/') {
                    name.trim_start_matches("./").to_string()
                } else {
                    format!("{script_dir}/{name}")
                };
                let cmd = format!(
                    "cd {} && test -f {}",
                    shell_quote(target),
                    shell_quote(&path)
                );
                let (code, _) = client.exec_capture(&cmd, 60).await?;
                if code != 0 {
                    return Err(CoreError::deploy(format!("未找到脚本: {path}")));
                }
                paths.push(path);
            }
            paths
        };

        if scripts.is_empty() {
            logger.warn(format!("未在 {script_dir}/ 目录找到 .sh 脚本，跳过执行"));
            return Ok(());
        }

        logger.info(format!("共找到 {} 个脚本，开始执行", scripts.len()));
        // 原子发布时告诉脚本当前生效路径（脚本执行期间仍指向旧版本，切换在成功后进行）。
        let current_dir = match release {
            Some(_) => format!("{root}/{}", release::CURRENT_LINK),
            None => root.to_string(),
        };
        for script in scripts {
            logger.command(format!("$ bash {script}"));
            let pidfile = remote_pidfile(root, &record.id);
            // 记录脚本 PID：退出/超时时据此找到进程组并整组终止（含脚本拉起的子进程）。
            let inner = format!(
                "echo $$ > {pid} && exec bash {script}",
                pid = shell_quote(&pidfile),
                script = shell_quote(&script)
            );
            let cmd = format!(
                "cd {target} && rm -f {pid} && DEPLOY_BRANCH={branch} DEPLOY_REV={rev} \
                 DEPLOY_COMMIT={commit} DEPLOY_TARGET={target} DEPLOY_RELEASE={release} \
                 DEPLOY_CURRENT={current} bash -c {inner}",
                target = shell_quote(target),
                pid = shell_quote(&pidfile),
                branch = shell_quote(&record.branch),
                rev = shell_quote(&record.rev),
                commit = shell_quote(&record.commit),
                release = shell_quote(release.unwrap_or("")),
                current = shell_quote(&current_dir),
                inner = shell_quote(&inner)
            );
            let result = client
                .exec_stream(&cmd, settings.script_timeout_secs, &mut |kind, line| {
                    match kind {
                        OutputKind::Stdout => logger.info(line),
                        OutputKind::Stderr => logger.warn(line),
                    }
                })
                .await;
            match result {
                Ok(0) => {
                    let _ = remove_pidfile(client, &pidfile).await;
                    logger.success(format!("脚本执行完成: {script}"));
                }
                Ok(code) => {
                    let _ = remove_pidfile(client, &pidfile).await;
                    return Err(CoreError::deploy(format!(
                        "脚本执行失败（退出码 {code}）: {script}"
                    )));
                }
                Err(err) => {
                    // 超时或连接异常：脚本可能仍在运行，先终止再返回。
                    let _ = kill_remote_script(client, &pidfile).await;
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    /// 应用退出时清理仍在运行的远端部署脚本（重新连接并终止其进程组）。
    pub async fn cleanup_remote_script(
        &self,
        server: &ServerConfig,
        target_dir: &str,
        record_id: &str,
        timeout_secs: u64,
    ) -> Result<()> {
        let target = target_dir.trim().trim_end_matches('/');
        if target.is_empty() {
            return Ok(());
        }
        let keep_remote_archive = self
            .store
            .load_config()
            .map(|config| config.settings.keep_remote_archive)
            .unwrap_or(false);
        let pidfile = remote_pidfile(target, record_id);
        // 任务被中止时压缩包会以记录 UUID 命名残留在 .deploy_code 下，一并清理。
        // record_id 是 UUID，可安全拼进 glob；kill_script 已用子 shell 包裹，不会中断后续命令。
        let mut command = kill_script(&pidfile);
        if !keep_remote_archive {
            command.push_str(&format!(
                "; rm -f {}/.deploy_code/*-{record_id}.tar.gz",
                shell_quote(target)
            ));
        }
        let client = SshClient::connect(server, timeout_secs).await?;
        let result = client.exec_capture(&command, timeout_secs).await;
        client.disconnect().await;
        result.map(|_| ())
    }

    /// 测试服务器连通性，返回远端系统信息。
    pub async fn test_server(&self, server: &ServerConfig) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        let client = SshClient::connect(server, settings.connect_timeout_secs).await?;
        let (code, output) = client
            .exec_capture("uname -srm 2>/dev/null || echo unknown", 30)
            .await?;
        client.disconnect().await;
        if code != 0 {
            return Err(CoreError::ssh("连接成功，但执行远端命令失败"));
        }
        Ok(format!("连接成功 · {}", output.trim()))
    }

    /// 采集服务器安全检查报告（登录日志 / 防火墙 / sshd 配置 / 自动防护状态）。
    pub async fn server_security(&self, server: &ServerConfig) -> Result<SecurityReport> {
        let settings = self.store.load_config()?.settings;
        crate::security::collect(server, settings.connect_timeout_secs).await
    }

    /// 启用服务器端自动防护（失败 N 次自动拉黑，服务器上长期生效）。
    pub async fn enable_server_guard(
        &self,
        server: &ServerConfig,
        threshold: u32,
        window_mins: u64,
    ) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::enable_guard(server, settings.connect_timeout_secs, threshold, window_mins)
            .await
    }

    /// 停用服务器端自动防护。
    pub async fn disable_server_guard(&self, server: &ServerConfig) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::disable_guard(server, settings.connect_timeout_secs).await
    }

    /// 拉黑某来源 IP。
    pub async fn block_server_ip(&self, server: &ServerConfig, ip: &str) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::block_ip(server, settings.connect_timeout_secs, ip).await
    }

    /// 解除某来源 IP 的拉黑。
    pub async fn unblock_server_ip(&self, server: &ServerConfig, ip: &str) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::unblock_ip(server, settings.connect_timeout_secs, ip).await
    }

    /// 强制踢出服务器上的在线会话。
    pub async fn kick_server_session(&self, server: &ServerConfig, tty: &str) -> Result<String> {
        let settings = self.store.load_config()?.settings;
        crate::security::kick_session(server, settings.connect_timeout_secs, tty).await
    }
}

/// 计算仓库的展示信息（当前分支 / 远端 / 变动数量）。
pub fn repo_info(repo: &RepoConfig) -> RepoInfo {
    let path_exists = std::path::Path::new(&repo.path).is_dir();
    match Git::open(&repo.path) {
        Ok(git) if git.is_repo() => {
            let current_branch = git.current_branch().unwrap_or_else(|_| "-".to_string());
            let remote = git.remote_url().ok().flatten();
            let change_count = git.status().map(|s| s.changes.len()).unwrap_or(0);
            RepoInfo {
                id: repo.id.clone(),
                name: repo.name.clone(),
                path: repo.path.clone(),
                path_exists,
                is_repo: true,
                current_branch,
                remote,
                change_count,
                default_server_id: repo.default_server_id.clone(),
                default_target_dir: repo.default_target_dir.clone(),
                env_files: repo.env_files.clone(),
            }
        }
        _ => RepoInfo {
            id: repo.id.clone(),
            name: repo.name.clone(),
            path: repo.path.clone(),
            path_exists,
            is_repo: false,
            current_branch: "-".to_string(),
            remote: None,
            change_count: 0,
            default_server_id: repo.default_server_id.clone(),
            default_target_dir: repo.default_target_dir.clone(),
            env_files: repo.env_files.clone(),
        },
    }
}

/// 校验并规范化环境文件配置：跳过空条目，统一远端路径分隔符。
pub fn normalize_env_files(files: &[EnvFileConfig]) -> Result<Vec<EnvFileConfig>> {
    let mut normalized = Vec::with_capacity(files.len());
    for file in files {
        let local = file.local_path.trim();
        let remote = file.remote_path.trim();
        if local.is_empty() && remote.is_empty() {
            continue;
        }
        if local.is_empty() {
            return Err(CoreError::deploy("环境文件的本地路径不能为空"));
        }
        if remote.is_empty() {
            return Err(CoreError::deploy("环境文件的远端路径不能为空"));
        }
        normalized.push(EnvFileConfig {
            local_path: local.to_string(),
            remote_path: normalize_env_remote(remote)?,
        });
    }
    Ok(normalized)
}

/// 远端路径必须是部署目录下的相对路径：去掉 `./` 前缀，拒绝绝对路径与 `..`。
fn normalize_env_remote(value: &str) -> Result<String> {
    let rel = value.trim().replace('\\', "/");
    let rel = rel.trim_start_matches("./");
    let parts: Vec<&str> = rel.split('/').collect();
    let invalid = rel.is_empty()
        || rel.starts_with('/')
        || rel.chars().any(char::is_control)
        || parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..");
    if invalid {
        return Err(CoreError::deploy(format!("环境文件远端路径不合法: {value}")));
    }
    Ok(parts.join("/"))
}

fn normalize_script_dir(value: &str, fallback: &str) -> String {
    let value = value.trim().trim_matches('/');
    let value = value.strip_prefix("./").unwrap_or(value);
    if !value.is_empty() {
        value.to_string()
    } else {
        let fallback = fallback.trim().trim_matches('/');
        let fallback = fallback.strip_prefix("./").unwrap_or(fallback);
        if fallback.is_empty() {
            "docker".to_string()
        } else {
            fallback.to_string()
        }
    }
}

/// 脚本列表去空白、去重，并保持用户给定顺序。
fn normalize_scripts(values: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_string()))
        .map(|value| value.to_string())
        .collect()
}

/// 远端脚本 pidfile 路径（放在部署目录的 .deploy_code 下）。
fn remote_pidfile(target: &str, record_id: &str) -> String {
    format!(
        "{}/.deploy_code/deploy-{record_id}.pid",
        target.trim_end_matches('/')
    )
}

/// 终止 pidfile 中记录的脚本进程组，并删除 pidfile。
pub(crate) fn kill_script(pidfile: &str) -> String {
    let file = shell_quote(pidfile);
    // 先发信号再删 pidfile：中途被中断时，脚本仍可被下次清理定位到。
    // 进程组优先从 /proc 读取（Linux 通用），失败再退回 ps，兼容精简系统的 ps。
    // 整段包在子 shell 里：pidfile 缺失时的 exit 0 只退出子 shell，
    // 不会中断调用方拼在其后的 rm 等清理命令。
    format!(
        "( f={file}; p=$(cat \"$f\" 2>/dev/null); \
         case \"$p\" in ''|*[!0-9]*) rm -f \"$f\"; exit 0 ;; esac; \
         g=$(sed 's/.*) //' /proc/$p/stat 2>/dev/null | cut -d' ' -f3); \
         case \"$g\" in ''|*[!0-9]*) g=$(ps -o pgid= -p \"$p\" 2>/dev/null | tr -d ' ') ;; esac; \
         if [ -n \"$g\" ]; then \
           kill -TERM -\"$g\" 2>/dev/null || kill -TERM \"$p\" 2>/dev/null; \
           sleep 1; \
           kill -KILL -\"$g\" 2>/dev/null || kill -KILL \"$p\" 2>/dev/null; \
         else \
           kill -TERM \"$p\" 2>/dev/null; sleep 1; kill -KILL \"$p\" 2>/dev/null; \
         fi; rm -f \"$f\"; echo done )"
    )
}

/// 原子替换远端环境文件：把临时文件 `mv` 到正式路径，并保留原文件权限。
///
/// `mv` 之后必须紧跟 `|| exit 1`：整条命令的退出码取自最后一个语句，而以
/// `if ... ; then ...; fi` 结尾时条件不成立会返回 0，`mv` 的失败（目录不可写、
/// 磁盘满）就会被当成替换成功，线上 env 还是旧内容而部署却报成功。
/// `test -f` 兜住另一种静默失败：正式路径已存在且是目录时，`mv` 返回 0 却把文件
/// 移进了目录里，替换同样没有发生。
fn env_replace_script(remote: &str, temp: &str) -> String {
    let remote = shell_quote(remote);
    let temp = shell_quote(temp);
    format!(
        "p=$(stat -c %a {remote} 2>/dev/null || stat -f %Lp {remote} 2>/dev/null || true); \
         mv -f {temp} {remote} || exit 1; \
         test -f {remote} || exit 1; \
         if [ -n \"$p\" ]; then chmod \"$p\" {remote}; fi"
    )
}

/// 删除远端临时文件：清理失败不该盖住真正的部署结果。
async fn remove_remote_file(client: &SshClient, path: &str) {
    let _ = client
        .exec_capture(&format!("rm -f {}", shell_quote(path)), 30)
        .await;
}

/// 批次级别的日志行：只有多机部署会产生，单机流程的日志由 `TaskLogger` 负责。
fn send_batch_log(events: &Option<EventSender>, level: LogLevel, message: String) {
    if let Some(sender) = events {
        let _ = sender.send(DeployEvent::Log { level, message });
    }
}

async fn remove_pidfile(client: &SshClient, pidfile: &str) -> Result<()> {
    client
        .exec_capture(&format!("rm -f {}", shell_quote(pidfile)), 15)
        .await
        .map(|_| ())
}

async fn kill_remote_script(client: &SshClient, pidfile: &str) -> Result<()> {
    client
        .exec_capture(&kill_script(pidfile), 15)
        .await
        .map(|_| ())
}

/// 远端计算文件 SHA-256：优先 `sha256sum`，其次 `shasum` / `openssl`；
/// 都没有时返回 `None`，调用方跳过校验（兼容老系统）。
async fn remote_sha256(client: &SshClient, path: &str) -> Result<Option<String>> {
    let quoted = shell_quote(path);
    // 路径均为绝对路径（不以 - 开头），不带 `--` 以兼容精简系统的 sha256sum / shasum。
    let cmd = format!(
        "if command -v sha256sum >/dev/null 2>&1; then sha256sum {quoted}; \
         elif command -v shasum >/dev/null 2>&1; then shasum -a 256 {quoted}; \
         elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 {quoted}; \
         else exit 127; fi"
    );
    let (code, output) = client.exec_capture(&cmd, 120).await?;
    if code == 127 {
        return Ok(None);
    }
    if code != 0 {
        return Err(CoreError::deploy(format!(
            "服务器计算校验和失败: {}",
            output.trim()
        )));
    }
    Ok(parse_sha256_output(&output))
}

/// 从 sha256sum / shasum / openssl 的输出中提取 64 位十六进制摘要。
fn parse_sha256_output(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.split_whitespace()
            .find(|part| part.len() == 64 && part.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|part| part.to_ascii_lowercase())
    })
}

fn sanitize_component(value: &str) -> String {
    let mut result: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    result.truncate(40);
    if result.is_empty() {
        "repo".to_string()
    } else {
        result
    }
}

fn short_hash(hash: &str) -> String {
    hash.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_pidfile_normalizes_trailing_slash() {
        assert_eq!(
            remote_pidfile("/srv/app/", "abc"),
            "/srv/app/.deploy_code/deploy-abc.pid"
        );
    }

    #[test]
    fn parse_sha256_output_supports_common_formats() {
        let hash = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        // sha256sum / shasum：hash 在前
        assert_eq!(
            parse_sha256_output(&format!("{hash}  /opt/app/a.tar.gz")).as_deref(),
            Some(hash)
        );
        // openssl dgst：hash 在后
        assert_eq!(
            parse_sha256_output(&format!("SHA2-256(/opt/app/a.tar.gz)= {hash}")).as_deref(),
            Some(hash)
        );
        // 大写摘要统一转小写
        assert_eq!(
            parse_sha256_output(&format!("{}  /opt/app/a.tar.gz", hash.to_uppercase())).as_deref(),
            Some(hash)
        );
        // 无摘要 / 文件名里的短 hash 不应误匹配
        assert_eq!(parse_sha256_output("no hash here"), None);
        assert_eq!(
            parse_sha256_output("abc12345  /opt/app/repo-abc12345-x.tar.gz"),
            None
        );
    }

    #[test]
    fn kill_script_quotes_pidfile_and_kills_group() {
        let script = kill_script("/srv/my app/.deploy_code/deploy-a.pid");
        assert!(script.contains("'/srv/my app/.deploy_code/deploy-a.pid'"));
        assert!(script.contains("/proc/$p/stat"));
        assert!(script.contains("kill -TERM -\"$g\""));
        assert!(script.contains("kill -KILL -\"$g\""));
    }

    #[test]
    fn kill_script_ignores_invalid_pid_content() {
        let script = kill_script("/tmp/x.pid");
        assert!(script.contains("*[!0-9]*"));
        // 无效 pid 分支的 exit 0 必须只退出子 shell，不能中断调用方的后续清理命令。
        assert!(script.starts_with("( f="), "script = {script}");
        assert!(script.ends_with(')'), "script = {script}");
    }

    #[test]
    fn env_replace_script_lets_mv_decide_the_exit_code() {
        let remote = "'/srv/my app/.env'";
        let temp = "'/srv/my app/.env.deploycode-tmp'";
        let script = env_replace_script("/srv/my app/.env", "/srv/my app/.env.deploycode-tmp");
        assert!(
            script.contains(&format!("mv -f {temp} {remote} || exit 1")),
            "script = {script}"
        );
        // 恢复权限的 if 位于结尾，条件不成立时返回 0：它只能作为最后一个语句，
        // 前面任何一步失败都必须先 exit 掉。
        assert!(
            script.ends_with(&format!("if [ -n \"$p\" ]; then chmod \"$p\" {remote}; fi")),
            "script = {script}"
        );
        // 正式路径是目录时 mv 同样返回 0（文件被移进目录），要靠 test -f 识破。
        assert!(
            script.contains(&format!("test -f {remote} || exit 1")),
            "script = {script}"
        );
    }

    #[test]
    fn normalize_env_files_skips_empty_and_rejects_unsafe_paths() {
        let files = normalize_env_files(&[
            EnvFileConfig {
                local_path: " .env ".to_string(),
                remote_path: " .env ".to_string(),
            },
            EnvFileConfig {
                local_path: "C:/tmp/app.env".to_string(),
                remote_path: "docker\\app.env".to_string(),
            },
            EnvFileConfig {
                local_path: "C:/tmp/root.env".to_string(),
                remote_path: "././.env".to_string(),
            },
            EnvFileConfig::default(),
        ])
        .unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].local_path, ".env");
        assert_eq!(files[0].remote_path, ".env");
        assert_eq!(files[1].remote_path, "docker/app.env");
        // `./` 前缀会被去掉，但 `.env` 这类隐藏文件名不能被误伤。
        assert_eq!(files[2].remote_path, ".env");

        for bad in ["/etc/passwd", "../secret", "a/../b", "a//b", "./", "..", "a/"] {
            let files = vec![EnvFileConfig {
                local_path: "x".to_string(),
                remote_path: bad.to_string(),
            }];
            assert!(normalize_env_files(&files).is_err(), "应拒绝远端路径 {bad}");
        }

        let files = vec![EnvFileConfig {
            local_path: String::new(),
            remote_path: ".env".to_string(),
        }];
        assert!(normalize_env_files(&files).is_err());
    }

    #[test]
    fn prepare_validates_and_applies_env_files() {
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !git_ok {
            return;
        }
        let base =
            std::env::temp_dir().join(format!("deploycode-env-prepare-{}", uuid::Uuid::new_v4()));
        let repo_dir = base.join("work");
        std::fs::create_dir_all(&repo_dir).unwrap();
        let repo_arg = repo_dir.to_string_lossy().into_owned();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo_arg)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} 失败");
        };
        git(&["init"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo_dir.join("a.txt"), "hi").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-m", "init"]);

        let env_path = base.join("deploy.env");
        std::fs::write(&env_path, "KEY=1\n").unwrap();

        let store = std::sync::Arc::new(Store::new(base.join("data")));
        let (repo_id, server_id) = store
            .mutate_config(|config| {
                let mut repo = RepoConfig::new("demo".to_string(), repo_arg.clone());
                repo.env_files = vec![EnvFileConfig {
                    local_path: env_path.to_string_lossy().into_owned(),
                    remote_path: "docker\\.env".to_string(),
                }];
                config.repos.push(repo.clone());
                let server = ServerConfig::new(
                    "prod".to_string(),
                    "127.0.0.1".to_string(),
                    "root".to_string(),
                    crate::models::SshAuth::Password {
                        password: "x".to_string(),
                    },
                );
                let server_id = server.id.clone();
                config.servers.push(server);
                Ok((repo.id, server_id))
            })
            .unwrap();

        let engine = DeployEngine::new(store.clone());
        let request = DeployRequest {
            repo_id,
            rev: "HEAD".to_string(),
            server_id,
            target_dir: "/opt/demo".to_string(),
            run_scripts: false,
            script_dir: "docker".to_string(),
            scripts: Vec::new(),
            upload_env: true,
        };

        let record = engine.prepare(&request).unwrap();
        assert_eq!(record.env_files.len(), 1);
        assert_eq!(record.env_files[0].remote_path, "docker/.env");

        // 本地文件缺失时在开始部署前报错。
        std::fs::remove_file(&env_path).unwrap();
        let err = engine.prepare(&request).unwrap_err().to_string();
        assert!(err.contains("环境文件不存在"), "err = {err}");

        // uploadEnv=false 时不校验也不记录。
        std::fs::write(&env_path, "KEY=2\n").unwrap();
        let mut skip = request.clone();
        skip.upload_env = false;
        let record = engine.prepare(&skip).unwrap();
        assert!(record.env_files.is_empty());

        // 部署目录必须是服务器上的绝对路径：相对路径和 ~ 都会落进 SSH 登录目录，
        // 装错位置比当场报错难查得多（~ 经 shell_quote 是字面量）。
        for bad in ["opt/demo", "~/demo", "./demo"] {
            let mut relative = request.clone();
            relative.target_dir = bad.to_string();
            let err = engine.prepare(&relative).unwrap_err().to_string();
            assert!(err.contains("绝对路径"), "{bad} 应被拒绝，err = {err}");
        }
        // 结尾斜杠按发布目录约定收敛掉，写进记录的已是规范化结果。
        let mut trailing = request.clone();
        trailing.target_dir = "/opt/demo/".to_string();
        assert_eq!(engine.prepare(&trailing).unwrap().target_dir, "/opt/demo");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prepare_allows_worktree_deploy_without_revision() {
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !git_ok {
            return;
        }
        let base = std::env::temp_dir().join(format!(
            "deploycode-worktree-prepare-{}",
            uuid::Uuid::new_v4()
        ));
        let repo_dir = base.join("with-commit");
        let empty_dir = base.join("empty");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::create_dir_all(&empty_dir).unwrap();

        let run_git = |dir: &std::path::Path, args: &[&str]| {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} 失败");
        };
        run_git(&repo_dir, &["init"]);
        run_git(&repo_dir, &["config", "user.name", "Test"]);
        run_git(&repo_dir, &["config", "user.email", "test@example.com"]);
        run_git(&repo_dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo_dir.join("a.txt"), "hi").unwrap();
        run_git(&repo_dir, &["add", "-A"]);
        run_git(&repo_dir, &["commit", "-m", "init"]);
        // 空仓库：没有任何分支与提交，也应允许部署当前工作区。
        run_git(&empty_dir, &["init"]);

        let store = std::sync::Arc::new(Store::new(base.join("data")));
        let (repo_id, empty_id, server_id) = store
            .mutate_config(|config| {
                let repo = RepoConfig::new(
                    "demo".to_string(),
                    repo_dir.to_string_lossy().into_owned(),
                );
                let empty = RepoConfig::new(
                    "empty".to_string(),
                    empty_dir.to_string_lossy().into_owned(),
                );
                let server = ServerConfig::new(
                    "prod".to_string(),
                    "127.0.0.1".to_string(),
                    "root".to_string(),
                    crate::models::SshAuth::Password {
                        password: "x".to_string(),
                    },
                );
                let server_id = server.id.clone();
                config.repos.push(repo.clone());
                config.repos.push(empty.clone());
                config.servers.push(server);
                Ok((repo.id, empty.id, server_id))
            })
            .unwrap();

        let engine = DeployEngine::new(store);
        let request = |repo_id: &str| DeployRequest {
            repo_id: repo_id.to_string(),
            rev: String::new(),
            server_id: server_id.clone(),
            target_dir: "/opt/demo".to_string(),
            run_scripts: false,
            script_dir: "docker".to_string(),
            scripts: Vec::new(),
            upload_env: false,
        };

        let record = engine.prepare(&request(&repo_id)).unwrap();
        assert!(record.worktree);
        assert!(record.rev.is_empty());
        assert!(!record.commit.is_empty(), "工作区部署应记录当前 HEAD 作为参考");
        assert!(!record.branch.is_empty());

        let record = engine.prepare(&request(&empty_id)).unwrap();
        assert!(record.worktree);
        assert!(record.commit.is_empty());
        assert!(record.commit_short.is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn repo_info_distinguishes_non_git_and_missing_dirs() {        let dir =
            std::env::temp_dir().join(format!("deploycode-repo-info-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let repo = RepoConfig::new("plain".to_string(), dir.to_string_lossy().into_owned());

        let info = repo_info(&repo);
        assert!(info.path_exists);
        assert!(!info.is_repo);
        assert_eq!(info.change_count, 0);

        let missing = RepoConfig::new(
            "gone".to_string(),
            dir.join("not-exist").to_string_lossy().into_owned(),
        );
        let info = repo_info(&missing);
        assert!(!info.path_exists);
        assert!(!info.is_repo);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
