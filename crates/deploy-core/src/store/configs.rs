//! 部署 / 容器 / Pages 配置的校验、保存与删除（GUI 与 CLI 共用，保证两端行为一致）。

use super::Store;
use crate::error::{CoreError, Result};
use crate::models::{
    new_id, now_string, AppConfig, ContainerConfig, DeployConfig, PagesConfigEntry,
};

impl Store {
    /// 按 id / 名称查找部署配置；id 精确命中优先，避免与名称歧义。
    /// 服务器被删除后收敛部署配置：从目标列表里摘掉这一台，没有剩余目标的配置整条删除。
    /// GUI 与 CLI 共用，避免两条清理路径的语义分叉。
    pub fn detach_server_from_deploy_configs(config: &mut AppConfig, server_id: &str) {
        for saved in config.deploy_configs.iter_mut() {
            saved.server_ids.retain(|id| id != server_id);
        }
        config
            .deploy_configs
            .retain(|saved| !saved.server_ids.is_empty());
    }

    pub fn find_deploy_config<'a>(config: &'a AppConfig, key: &str) -> Result<&'a DeployConfig> {
        let key = key.trim();
        if let Some(item) = config.deploy_configs.iter().find(|item| item.id == key) {
            return Ok(item);
        }
        config
            .deploy_configs
            .iter()
            .find(|item| item.name == key)
            .ok_or_else(|| CoreError::not_found(format!("部署配置不存在: {key}")))
    }

    /// 新建或更新一条部署配置：规范化仓库 / 服务器引用并校验名称唯一。
    /// GUI 与 CLI 共用，保证两端保存行为一致。
    pub fn save_deploy_config(store: &Store, config: DeployConfig) -> Result<DeployConfig> {
        let mut config = config;
        config.name = config.name.trim().to_string();
        if config.name.is_empty() {
            return Err(CoreError::config("部署配置名称不能为空"));
        }
        config.target_dir = config.target_dir.trim().to_string();
        if config.target_dir.is_empty() {
            return Err(CoreError::config("部署目录不能为空"));
        }
        // 部署目标是 Linux 服务器上的绝对路径：相对路径 / ~ 会落到 SSH 登录目录，
        // 且与原子发布的 release 目录约定（要求绝对路径）冲突。
        if !config.target_dir.starts_with('/') {
            return Err(CoreError::config(
                "部署目录必须是服务器上的绝对路径（以 / 开头）",
            ));
        }
        config.rev = config.rev.trim().to_string();
        config.id = config.id.trim().to_string();
        config.repo_id = config.repo_id.trim().to_string();
        // 服务器列表：去空与去重，保留顺序（顺序即批量部署的执行顺序）。
        let mut server_keys = Vec::new();
        for key in &config.server_ids {
            let key = key.trim().to_string();
            if !key.is_empty() && !server_keys.contains(&key) {
                server_keys.push(key);
            }
        }
        config.server_ids = server_keys;
        if config.server_ids.is_empty() {
            return Err(CoreError::config("部署配置至少要选择一台服务器"));
        }
        config.script_dir = config.script_dir.trim().to_string();
        if config.script_dir.is_empty() {
            config.script_dir = "docker".to_string();
        }
        config.scripts = config
            .scripts
            .iter()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();

        store.mutate_config(|app| {
            // 仓库 / 服务器必须存在；名称与 host 也允许（兼容 CLI 习惯）。
            let repo = Store::find_repo(app, &config.repo_id)?.clone();
            config.repo_id = repo.id.clone();
            // 逐台解析成真实 id：按名称 / host 写进来的、以及解析后撞同一台的都在这里收敛。
            let mut resolved_servers = Vec::with_capacity(config.server_ids.len());
            for key in &config.server_ids {
                let id = Store::find_server(app, key)?.id.clone();
                if !resolved_servers.contains(&id) {
                    resolved_servers.push(id);
                }
            }
            config.server_ids = resolved_servers;

            let by_id = app.deploy_configs.iter().find(|item| item.id == config.id);
            if config.id.is_empty() {
                config.id = new_id();
            } else if by_id.is_none() {
                // 明确携带 id 却不存在（配置已被其它窗口 / CLI 删除）：报错而不是静默新建，
                // 避免「复活」已删除配置或用任意 id 注入条目。
                return Err(CoreError::not_found(format!(
                    "部署配置不存在（可能已被删除）: {}",
                    config.id
                )));
            }
            // 更新已有配置时以存储中的创建时间为准，避免调用方传入的旧快照覆盖。
            match by_id {
                Some(existing) if !existing.created_at.trim().is_empty() => {
                    config.created_at = existing.created_at.clone();
                }
                _ => {
                    if config.created_at.trim().is_empty() {
                        config.created_at = now_string();
                    }
                }
            }
            if app
                .deploy_configs
                .iter()
                .any(|item| item.id != config.id && item.name == config.name)
            {
                return Err(CoreError::config(format!(
                    "部署配置名称已存在: {}",
                    config.name
                )));
            }
            match app.deploy_configs.iter_mut().find(|item| item.id == config.id) {
                Some(existing) => *existing = config.clone(),
                None => app.deploy_configs.push(config.clone()),
            }
            Ok(config.clone())
        })
    }

    /// 删除一条部署配置（不影响已有部署记录，历史记录仍可重新部署）。
    /// 优先按 id 精确删除，避免出现「某配置名称恰好等于另一条配置 id」时误删两条。
    pub fn delete_deploy_config(store: &Store, key: &str) -> Result<bool> {
        let key = key.trim();
        store.mutate_config(|app| {
            let removed: Vec<String> = if let Some(item) =
                app.deploy_configs.iter().find(|item| item.id == key)
            {
                vec![item.id.clone()]
            } else {
                app.deploy_configs
                    .iter()
                    .filter(|item| item.name == key)
                    .map(|item| item.id.clone())
                    .collect()
            };
            if removed.is_empty() {
                return Ok(false);
            }
            app.deploy_configs.retain(|item| !removed.contains(&item.id));
            Ok(true)
        })
    }

    /// 按 id / 名称查找容器备份配置；id 精确命中优先，避免与名称歧义。
    pub fn find_container_config<'a>(
        config: &'a AppConfig,
        key: &str,
    ) -> Result<&'a ContainerConfig> {
        let key = key.trim();
        if let Some(item) = config
            .container_configs
            .iter()
            .find(|item| item.id == key)
        {
            return Ok(item);
        }
        config
            .container_configs
            .iter()
            .find(|item| item.name == key)
            .ok_or_else(|| CoreError::not_found(format!("容器备份配置不存在: {key}")))
    }

    /// 新建或更新一条容器备份配置：规范化服务器引用与迁移目标并校验名称唯一。
    /// 界面与调度器都从这里取参数，所以校验要在这里做完，别留到夜里执行时才报错。
    pub fn save_container_config(
        store: &Store,
        config: ContainerConfig,
    ) -> Result<ContainerConfig> {
        let mut config = config;
        config.name = config.name.trim().to_string();
        if config.name.is_empty() {
            return Err(CoreError::config("容器备份配置名称不能为空"));
        }
        config.project = config.project.trim().to_string();
        if config.project.is_empty() {
            return Err(CoreError::config("请选择要打包的 compose 项目"));
        }
        if !config.include_volumes && !config.include_images {
            return Err(CoreError::config("数据卷与镜像至少要勾选一项，否则备份包是空的"));
        }
        config.id = config.id.trim().to_string();
        config.server_id = config.server_id.trim().to_string();

        store.mutate_config(|app| {
            // 服务器必须以 id 形式存在；名称 / host 也允许（兼容 CLI）。
            let source = Store::find_server(app, &config.server_id)?.clone();
            config.server_id = source.id.clone();

            if let Some(target) = config.target.as_mut() {
                let key = target.server_id.trim().to_string();
                if key.is_empty() {
                    // 目标服务器留空 = 只备份到本机，不留一条指向来源机的空目标。
                    config.target = None;
                } else {
                    let server = Store::find_server(app, &key)?.clone();
                    if server.id == config.server_id {
                        return Err(CoreError::config("目标服务器不能与来源服务器相同"));
                    }
                    target.server_id = server.id.clone();
                    target.target_dir =
                        crate::container::validate_remote_dir(&target.target_dir)?;
                }
            }

            let by_id = app
                .container_configs
                .iter()
                .find(|item| item.id == config.id);
            if config.id.is_empty() {
                config.id = new_id();
            } else if by_id.is_none() {
                // 明确携带 id 却不存在（配置已被其它窗口删除）：报错而不是静默新建。
                return Err(CoreError::not_found(format!(
                    "容器备份配置不存在（可能已被删除）: {}",
                    config.id
                )));
            }
            // 更新已有配置时以存储中的创建时间为准，避免调用方传入的旧快照覆盖。
            match by_id {
                Some(existing) if !existing.created_at.trim().is_empty() => {
                    config.created_at = existing.created_at.clone();
                }
                _ => {
                    if config.created_at.trim().is_empty() {
                        config.created_at = now_string();
                    }
                }
            }
            if app
                .container_configs
                .iter()
                .any(|item| item.id != config.id && item.name == config.name)
            {
                return Err(CoreError::config(format!(
                    "容器备份配置名称已存在: {}",
                    config.name
                )));
            }
            match app
                .container_configs
                .iter_mut()
                .find(|item| item.id == config.id)
            {
                Some(existing) => *existing = config.clone(),
                None => app.container_configs.push(config.clone()),
            }
            Ok(config.clone())
        })
    }

    /// 删除一条容器备份配置（不影响已有任务记录，本机备份包也留在原处）。
    pub fn delete_container_config(store: &Store, key: &str) -> Result<bool> {
        let key = key.trim();
        store.mutate_config(|app| {
            let removed: Vec<String> = if let Some(item) = app
                .container_configs
                .iter()
                .find(|item| item.id == key)
            {
                vec![item.id.clone()]
            } else {
                app.container_configs
                    .iter()
                    .filter(|item| item.name == key)
                    .map(|item| item.id.clone())
                    .collect()
            };
            if removed.is_empty() {
                return Ok(false);
            }
            app.container_configs.retain(|item| !removed.contains(&item.id));
            // 被删的配置若还在定时列表里，一并摘掉，避免每天到点报「配置不存在」。
            app.settings
                .scheduled_container_config_ids
                .retain(|id| !removed.contains(id));
            Ok(true)
        })
    }

    /// 服务器被删除后收敛容器备份配置：来源被删的整条删除（没有来源就无从打包），
    /// 迁移目标被删的降级成「只备份到本机」——定时任务不该因为一台机器没了就天天报错。
    pub fn detach_server_from_container_configs(config: &mut AppConfig, server_id: &str) {
        for saved in config.container_configs.iter_mut() {
            if saved
                .target
                .as_ref()
                .is_some_and(|target| target.server_id == server_id)
            {
                saved.target = None;
            }
        }
        config
            .container_configs
            .retain(|saved| saved.server_id != server_id);
        let alive: Vec<String> = config
            .container_configs
            .iter()
            .map(|item| item.id.clone())
            .collect();
        config
            .settings
            .scheduled_container_config_ids
            .retain(|id| alive.contains(id));
    }

    /// 列出所有 Pages 配置条目。
    pub fn list_pages_configs(config: &AppConfig) -> Vec<&PagesConfigEntry> {
        config.pages_configs.iter().collect()
    }

    /// 新建或更新一条 Pages 配置：仓库必须存在，同一仓库内名称唯一。
    /// id 留空表示新建（补 uuid），携带不存在的 id 则报错而不是静默新建。
    /// GUI 与 CLI 共用，保证两端保存行为一致。
    pub fn save_pages_config(store: &Store, entry: PagesConfigEntry) -> Result<PagesConfigEntry> {
        let mut entry = entry;
        entry.name = entry.name.trim().to_string();
        entry.repo_id = entry.repo_id.trim().to_string();
        entry.id = entry.id.trim().to_string();
        entry.config = entry.config.normalize();
        if entry.config.provider != "cloudflare" && entry.config.provider != "github" {
            return Err(CoreError::config(
                "不支持的 Pages 平台（可选 cloudflare / github）",
            ));
        }
        // GitHub 的远端地址要到部署时才验证得动，保存时只保证平台与分支齐备（normalize 已补默认值）。

        store.mutate_config(|app| {
            let repo = Store::find_repo(app, &entry.repo_id)?.clone();
            entry.repo_id = repo.id.clone();
            entry.repo_name = repo.name.clone();
            if entry.name.is_empty() {
                // 配置按仓库一份保存，界面没有单独的名称输入，沿用仓库名。
                entry.name = repo.name.clone();
            }

            let by_id = app.pages_configs.iter().find(|item| item.id == entry.id);
            if entry.id.is_empty() {
                entry.id = new_id();
            } else if by_id.is_none() {
                // 明确携带 id 却不存在（已被 CLI / 其它窗口删除）：报错而不是静默新建。
                return Err(CoreError::not_found(format!(
                    "Pages 配置不存在（可能已被删除）: {}",
                    entry.id
                )));
            }
            // 更新已有配置时以存储中的创建时间为准，避免调用方传入的旧快照覆盖。
            match by_id {
                Some(existing) if !existing.created_at.trim().is_empty() => {
                    entry.created_at = existing.created_at.clone();
                }
                _ => {
                    if entry.created_at.trim().is_empty() {
                        entry.created_at = now_string();
                    }
                }
            }
            if app
                .pages_configs
                .iter()
                .any(|item| {
                    item.id != entry.id && item.repo_id == entry.repo_id && item.name == entry.name
                })
            {
                return Err(CoreError::config(format!(
                    "该仓库下已存在同名 Pages 配置：{}",
                    entry.name
                )));
            }
            match app
                .pages_configs
                .iter_mut()
                .find(|item| item.id == entry.id)
            {
                Some(existing) => *existing = entry.clone(),
                None => app.pages_configs.push(entry.clone()),
            }
            // 保存即绑定：Pages 部署按仓库解析默认配置（`get_repo_default_pages`），
            // 不写这条引用的话，界面上保存成功的配置对部署永远是透明的。
            if let Some(repo) = app.repos.iter_mut().find(|item| item.id == entry.repo_id) {
                repo.default_pages_config_id = Some(entry.id.clone());
            }
            Ok(entry.clone())
        })
    }

    /// 删除一条 Pages 配置（按 id 精确删除）。
    pub fn delete_pages_config(config: &mut AppConfig, id: &str) -> Result<bool> {
        let initial_count = config.pages_configs.len();
        config.pages_configs.retain(|e| e.id != id);
        if config.pages_configs.len() == initial_count {
            return Ok(false);
        }
        // 如果该仓库的默认 Pages 配置被删除，清空默认引用
        for repo in &mut config.repos {
            if repo.default_pages_config_id.as_ref() == Some(&id.to_string()) {
                repo.default_pages_config_id = None;
            }
        }
        Ok(true)
    }

    /// 获取仓库的默认 Pages 配置（通过 default_pages_config_id 查找）。
    pub fn get_repo_default_pages<'a>(config: &'a AppConfig, repo_id: &str) -> Option<&'a PagesConfigEntry> {
        let repo = config.repos.iter().find(|r| r.id == repo_id)?;
        let id = repo.default_pages_config_id.as_ref()?;
        config.pages_configs.iter().find(|e| e.id == *id)
    }

    /// 启动时迁移旧版 repo.pages 到 pages_configs 列表（只执行一次）。
    pub fn migrate_pages_configs(&self) -> Result<usize> {
        if self.load_config()?.pages_configs_migrated {
            return Ok(0);
        }
        self.mutate_config(|config| {
            if config.pages_configs_migrated {
                return Ok(0);
            }
            let mut additions: Vec<PagesConfigEntry> = Vec::new();
            for repo in &mut config.repos {
                if let Some(pages_config) = repo.pages.take() {
                    let entry = PagesConfigEntry {
                        id: new_id(),
                        name: "default".to_string(),
                        repo_id: repo.id.clone(),
                        repo_name: repo.name.clone(),
                        config: pages_config,
                        created_at: now_string(),
                    };
                    additions.push(entry.clone());
                    repo.default_pages_config_id = Some(entry.id);
                }
            }
            config.pages_configs.extend(additions.clone());
            config.pages_configs_migrated = true;
            Ok(additions.len())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RepoConfig, RunLocation, ServerConfig, SshAuth};
    use crate::store::testutil::temp_store;

    #[test]
    fn save_and_delete_deploy_config_normalizes_and_validates() {
        use crate::models::SshAuth;

        let dir =
            std::env::temp_dir().join(format!("deploycode-store-deploycfg-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        let repo = RepoConfig::new("demo".to_string(), "/tmp/demo".to_string());
        let repo_id = repo.id.clone();
        config.repos.push(repo);
        let server = ServerConfig::new(
            "prod".to_string(),
            "h1".to_string(),
            "u".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let server_id = server.id.clone();
        config.servers.push(server);
        store.save_config(&config).unwrap();

        // 仓库 / 服务器可以按名称传入，保存时规范化为 id；空字段回落到默认值。
        let saved = Store::save_deploy_config(
            &store,
            DeployConfig {
                id: String::new(),
                name: " 生产部署 ".to_string(),
                repo_id: "demo".to_string(),
                server_ids: vec!["prod".to_string()],
                target_dir: " /opt/app ".to_string(),
                rev: " main ".to_string(),
                run_scripts: true,
                script_dir: "  ".to_string(),
                scripts: vec![" deploy.sh ".to_string(), "".to_string()],
                upload_env: true,
                created_at: String::new(),
            },
        )
        .unwrap();
        assert_eq!(saved.repo_id, repo_id);
        assert_eq!(saved.server_ids, vec![server_id.clone()]);
        assert_eq!(saved.name, "生产部署");
        assert_eq!(saved.target_dir, "/opt/app");
        assert_eq!(saved.rev, "main");
        assert_eq!(saved.script_dir, "docker");
        assert_eq!(saved.scripts, vec!["deploy.sh".to_string()]);
        assert!(!saved.created_at.is_empty());

        // 不带 id 的新建配置重名时拒绝（GUI 的新增入口靠这条避免误覆盖已有配置）。
        let mut duplicate = saved.clone();
        duplicate.id = String::new();
        assert!(Store::save_deploy_config(&store, duplicate).is_err());

        // 任意一台服务器不存在就整条拒绝。
        let mut missing = saved.clone();
        missing.server_ids = vec![server_id.clone(), "nope".to_string()];
        assert!(Store::save_deploy_config(&store, missing).is_err());

        // 一台都没选时拒绝，避免留下永远发不出去的配置。
        let mut empty = saved.clone();
        empty.server_ids = Vec::new();
        assert!(Store::save_deploy_config(&store, empty).is_err());

        // 多台可以按名称 / host 传入：统一换成 id、保持执行顺序，指向同一台的重复项收敛成一条。
        let second = ServerConfig::new(
            "stage".to_string(),
            "h2".to_string(),
            "u".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let second_id = second.id.clone();
        store
            .mutate_config(|app| {
                app.servers.push(second);
                Ok(())
            })
            .unwrap();
        let mut many = saved.clone();
        // 新建走空 id：带未知 id 会被「配置不存在」守卫拒绝。
        many.id = String::new();
        many.name = "多机部署".to_string();
        many.server_ids = vec!["stage".to_string(), "prod".to_string(), "h2".to_string()];
        let many = Store::save_deploy_config(&store, many).unwrap();
        assert_eq!(many.server_ids, vec![second_id, server_id.clone()]);

        // 更新同一条配置不会重复插入，且保留原创建时间。
        let created_at = saved.created_at.clone();
        let mut renamed = saved.clone();
        renamed.name = "生产部署 2".to_string();
        renamed.created_at = String::new();
        let renamed = Store::save_deploy_config(&store, renamed).unwrap();
        assert_eq!(renamed.created_at, created_at);
        // 只有「多机部署」那一条是新增的：更新原配置没有产生重复条目。
        assert_eq!(store.load_config().unwrap().deploy_configs.len(), 2);

        // id 前后空白会被规范掉。
        let mut padded = saved.clone();
        padded.id = format!("  {}  ", saved.id);
        padded.target_dir = "/opt/app3".to_string();
        let padded = Store::save_deploy_config(&store, padded).unwrap();
        assert_eq!(padded.id, saved.id);
        assert_eq!(padded.target_dir, "/opt/app3");

        // 明确携带不存在的 id 拒绝（避免配置被删除后又被静默复活）。
        let mut unknown = saved.clone();
        unknown.id = "no-such-id".to_string();
        assert!(Store::save_deploy_config(&store, unknown).is_err());

        // 相对部署目录拒绝：会落到 SSH 登录目录，且与原子发布的绝对路径约定冲突。
        let mut relative = saved.clone();
        relative.id = String::new();
        relative.name = "相对目录".to_string();
        relative.target_dir = "opt/app".to_string();
        assert!(Store::save_deploy_config(&store, relative).is_err());

        assert!(Store::delete_deploy_config(&store, &saved.id).unwrap());
        // 只剩「多机部署」那一条：删除按 id 精确命中，不牵连其它配置。
        let left = store.load_config().unwrap().deploy_configs;
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].name, "多机部署");
        assert!(!Store::delete_deploy_config(&store, &saved.id).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_container_config_normalizes_target_and_prunes_schedule() {
        use crate::models::{ContainerConfig, ContainerTarget};

        let (store, dir) = temp_store();
        let mut config = AppConfig::default();
        for name in ["prod", "stage"] {
            config.servers.push(ServerConfig::new(
                name.to_string(),
                format!("h-{name}"),
                "u".to_string(),
                SshAuth::Password {
                    password: "x".to_string(),
                },
            ));
        }
        let ids: Vec<String> = config.servers.iter().map(|item| item.id.clone()).collect();
        store.save_config(&config).unwrap();

        // 服务器按名称传入也认：保存时换成 id，目标目录顺带规范掉结尾斜杠。
        let saved = Store::save_container_config(
            &store,
            ContainerConfig {
                id: String::new(),
                name: " 博客 ".to_string(),
                server_id: "prod".to_string(),
                project: " lf-blog ".to_string(),
                pause_source: false,
                include_volumes: true,
                include_images: false,
                target: Some(ContainerTarget {
                    server_id: "stage".to_string(),
                    target_dir: "/opt/blog/".to_string(),
                    start_services: true,
                }),
                created_at: String::new(),
                run_location: RunLocation::Local,
            },
        )
        .unwrap();
        assert_eq!(saved.name, "博客");
        assert_eq!(saved.project, "lf-blog");
        assert_eq!(saved.server_id, ids[0]);
        let target = saved.target.clone().expect("迁移目标应保留");
        assert_eq!(target.server_id, ids[1]);
        assert_eq!(target.target_dir, "/opt/blog");

        // 卷和镜像都不勾 = 空备份包，保存时就该拒绝，而不是等夜里跑出一个空包。
        let mut empty_bundle = saved.clone();
        empty_bundle.id = String::new();
        empty_bundle.name = "空包".to_string();
        empty_bundle.include_volumes = false;
        assert!(Store::save_container_config(&store, empty_bundle).is_err());

        // 目标与来源同一台：compose 项目名在单机上会撞车，直接拒绝。
        let mut same = saved.clone();
        same.id = String::new();
        same.name = "同机".to_string();
        if let Some(target) = same.target.as_mut() {
            target.server_id = ids[0].clone();
        }
        assert!(Store::save_container_config(&store, same).is_err());

        // 目标服务器留空 = 降级成只备份到本机，不留一条指向来源机的空目标。
        let mut no_target = saved.clone();
        no_target.id = String::new();
        no_target.name = "只备份".to_string();
        no_target.target = Some(ContainerTarget {
            server_id: "  ".to_string(),
            target_dir: String::new(),
            start_services: true,
        });
        let no_target = Store::save_container_config(&store, no_target).unwrap();
        assert!(no_target.target.is_none());

        // 定时列表指向被删配置时一起清掉，避免到点报「配置不存在」。
        store
            .mutate_config(|app| {
                app.settings
                    .scheduled_container_config_ids
                    .push(saved.id.clone());
                Ok(())
            })
            .unwrap();
        assert!(Store::delete_container_config(&store, &saved.id).unwrap());
        let after = store.load_config().unwrap();
        assert!(
            after
                .settings
                .scheduled_container_config_ids
                .iter()
                .all(|id| id != &saved.id),
            "已删除的容器配置仍留在定时列表里"
        );

        // 删掉来源服务器：整条配置失去意义，连同定时引用一起消失；
        // 只当过迁移目标的那条则降级为纯备份，任务照跑。
        let mut config = store.load_config().unwrap();
        config.container_configs.clear();
        config.container_configs.push(no_target.clone());
        config.container_configs.push(saved.clone());
        config.settings.scheduled_container_config_ids =
            vec![no_target.id.clone(), saved.id.clone()];
        store.save_config(&config).unwrap();
        Store::detach_server_from_container_configs(&mut config, &ids[1]);
        assert_eq!(config.container_configs.len(), 2);
        assert!(config.container_configs[1].target.is_none());
        Store::detach_server_from_container_configs(&mut config, &ids[0]);
        assert!(config.container_configs.is_empty());
        assert!(config.settings.scheduled_container_config_ids.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 保存的配置是「手动一键执行」与「定时执行」唯一的参数来源：
    /// `request()` 少带一个字段，夜里跑的就不是白天存的那次操作，而且不会报错。
    /// 所以这里把 存配置 -> request() -> 引擎 整条链钉住。
    #[test]
    fn saved_container_config_feeds_the_engine_unchanged() {
        use crate::container::{ContainerEngine, ContainerJob};
        use crate::models::{ContainerConfig, ContainerRecordKind, ContainerTarget};

        let (store, dir) = temp_store();
        // 引擎按 `Arc<Store>` 持有存储，与 src-tauri 里的用法保持一致。
        let store = std::sync::Arc::new(store);
        let mut config = AppConfig::default();
        for name in ["prod", "stage"] {
            config.servers.push(ServerConfig::new(
                name.to_string(),
                format!("h-{name}"),
                "u".to_string(),
                SshAuth::Password {
                    password: "x".to_string(),
                },
            ));
        }
        store.save_config(&config).unwrap();

        let migrate = Store::save_container_config(
            &store,
            ContainerConfig {
                id: String::new(),
                name: "博客迁移".to_string(),
                server_id: "prod".to_string(),
                project: "lf-blog".to_string(),
                pause_source: true,
                include_volumes: true,
                include_images: false,
                target: Some(ContainerTarget {
                    server_id: "stage".to_string(),
                    target_dir: "/opt/blog".to_string(),
                    start_services: false,
                }),
                created_at: String::new(),
                run_location: RunLocation::Local,
            },
        )
        .unwrap();

        let engine = ContainerEngine::new(store.clone());
        let (record, job) = engine.prepare(&migrate.request()).unwrap();
        assert_eq!(record.kind, ContainerRecordKind::Migrate);
        assert_eq!(record.server_name, "prod");
        assert_eq!(record.target_server_name, "stage");
        assert_eq!(record.target_dir, "/opt/blog");
        match job {
            ContainerJob::Snapshot(plan) => {
                assert_eq!(plan.project, "lf-blog");
                assert!(plan.pause_source, "暂停来源机的选项必须传到引擎");
                assert!(plan.include_volumes);
                assert!(!plan.include_images, "只勾卷时不该带上镜像");
                let (server, target_dir, start) = plan.target.as_ref().expect("迁移目标应传到引擎");
                assert_eq!(server.name, "stage");
                assert_eq!(target_dir, "/opt/blog");
                assert!(!*start, "配置里没勾启动服务，到点不该 compose up");
            }
            ContainerJob::Restore(_) => panic!("快照配置不该产出恢复任务"),
        }

        // 同一份配置去掉目标：降级成纯备份， kinds 与目标字段都要跟着变。
        let mut backup_only = migrate.clone();
        backup_only.id = String::new();
        backup_only.name = "博客只备份".to_string();
        backup_only.target = None;
        let backup_only = Store::save_container_config(&store, backup_only).unwrap();
        let (record, job) = engine.prepare(&backup_only.request()).unwrap();
        assert_eq!(record.kind, ContainerRecordKind::Backup);
        assert!(record.target_server_id.is_empty());
        match job {
            ContainerJob::Snapshot(plan) => assert!(plan.target.is_none()),
            ContainerJob::Restore(_) => panic!("快照配置不该产出恢复任务"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_pages_config_resolves_repo_and_keeps_one_entry_per_id() {
        use crate::models::PagesConfig;

        let dir = std::env::temp_dir().join(format!(
            "deploycode-store-pagescfg-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        let repo = RepoConfig::new("demo".to_string(), "/tmp/demo".to_string());
        let repo_id = repo.id.clone();
        config.repos.push(repo);
        store.save_config(&config).unwrap();

        let draft = PagesConfigEntry {
            id: String::new(),
            name: String::new(),
            repo_id: "demo".to_string(),
            repo_name: String::new(),
            config: PagesConfig {
                provider: " Cloudflare ".to_string(),
                project_name: " site ".to_string(),
                build_command: String::new(),
                output_dir: String::new(),
                branch: String::new(),
                publish_branch: String::new(),
            },
            created_at: String::new(),
        };

        // 新建：id / 创建时间由后端补齐，仓库按名称解析成 id 并回填名称与 repo_name。
        let saved = Store::save_pages_config(&store, draft).unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(saved.repo_id, repo_id);
        assert_eq!(saved.repo_name, "demo");
        assert_eq!(saved.name, "demo");
        assert!(!saved.created_at.is_empty());
        assert_eq!(saved.config.provider, "cloudflare");
        assert_eq!(saved.config.project_name, "site");
        // normalize 补的回填默认值：输出目录与两个分支名不能留空。
        assert_eq!(saved.config.output_dir, "dist");
        assert_eq!(saved.config.branch, "main");
        assert_eq!(saved.config.publish_branch, "gh-pages");
        assert_eq!(store.load_config().unwrap().pages_configs.len(), 1);
        // 保存会把这个仓库的默认 Pages 配置指过来，否则部署侧永远读不到这条配置。
        assert_eq!(
            store.load_config().unwrap().repos[0].default_pages_config_id,
            Some(saved.id.clone())
        );

        // 同一 id 再保存是覆盖，不产生第二条，创建时间以存储里的为准。
        let mut edited = saved.clone();
        edited.name = "官网".to_string();
        edited.created_at = "旧快照".to_string();
        let edited = Store::save_pages_config(&store, edited).unwrap();
        assert_eq!(edited.id, saved.id);
        assert_eq!(edited.name, "官网");
        assert_eq!(edited.created_at, saved.created_at);
        assert_eq!(store.load_config().unwrap().pages_configs.len(), 1);

        // 明确携带不存在的 id 拒绝；未列出的平台也拒绝。
        let mut unknown = saved.clone();
        unknown.id = "no-such-id".to_string();
        assert!(Store::save_pages_config(&store, unknown).is_err());
        let mut bad_provider = saved.clone();
        bad_provider.id = String::new();
        bad_provider.name = "其它平台".to_string();
        bad_provider.config.provider = "vercel".to_string();
        assert!(Store::save_pages_config(&store, bad_provider).is_err());
        // 仓库不存在时不会留下半条配置。
        let mut no_repo = saved.clone();
        no_repo.id = String::new();
        no_repo.repo_id = "missing".to_string();
        assert!(Store::save_pages_config(&store, no_repo).is_err());
        assert_eq!(store.load_config().unwrap().pages_configs.len(), 1);

        // 删除按 id 命中，并清掉指向它的默认配置引用。
        store
            .mutate_config(|app| {
                app.repos[0].default_pages_config_id = Some(saved.id.clone());
                Ok(())
            })
            .unwrap();
        assert!(store
            .mutate_config(|app| Store::delete_pages_config(app, &saved.id))
            .unwrap());
        let after = store.load_config().unwrap();
        assert!(after.pages_configs.is_empty());
        assert_eq!(after.repos[0].default_pages_config_id, None);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
