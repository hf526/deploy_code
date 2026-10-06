//! 备份配置与备份目标的查找 / 替换 / 解析，以及服务器查找与旧版备份来源迁移。

use super::Store;
use crate::error::{CoreError, Result};
use crate::models::{
    new_id, AppConfig, BackupConfig, BackupTarget, RepoConfig, ServerConfig,
};

impl Store {
    /// 按 id / 名称 / 路径查找仓库。
    pub fn find_repo<'a>(config: &'a AppConfig, key: &str) -> Result<&'a RepoConfig> {
        config
            .repos
            .iter()
            .find(|r| r.id == key || r.name == key || paths_equal(&r.path, key))
            .ok_or_else(|| CoreError::not_found(format!("仓库不存在: {key}")))
    }

    /// 按 id / 名称查找备份配置。
    pub fn find_backup_config<'a>(config: &'a AppConfig, key: &str) -> Result<&'a BackupConfig> {
        config
            .backup_configs
            .iter()
            .find(|item| item.id == key || item.name == key)
            .ok_or_else(|| CoreError::not_found(format!("备份配置不存在: {key}")))
    }

    /// 哪些地方还在引用这些（即将被删除的）备份目标，返回可直接拼进报错的引用者名字。
    ///
    /// 删除必须过这一关：目标没了之后，绑定它的备份配置在解析目标时会一路往下兜底到
    /// 服务器绑定 / 全局旧连接串，而那一步对新目标做的是 `DROP SCHEMA ... CASCADE` ——
    /// 静默换库不是「备份到别处」那么轻。
    pub fn backup_target_referrers(config: &AppConfig, dropped_ids: &[&str]) -> Vec<String> {
        let mut who = Vec::new();
        if config
            .settings
            .default_backup_target_id
            .as_deref()
            .is_some_and(|id| dropped_ids.contains(&id))
        {
            who.push("设置里的默认备份目标".to_string());
        }
        for server in &config.servers {
            if server
                .backup_target_id
                .as_deref()
                .is_some_and(|id| dropped_ids.contains(&id))
            {
                who.push(format!("服务器「{}」", server.name));
            }
        }
        for item in &config.backup_configs {
            if item
                .target_id
                .as_deref()
                .is_some_and(|id| dropped_ids.contains(&id))
            {
                who.push(format!("备份配置「{}」", item.name));
            }
        }
        who
    }

    /// 用新列表整体替换备份目标：规范化校验 → 拒绝删除仍被引用的目标 → 落盘。
    ///
    /// 界面的列表增删和 CLI 的 `backup target remove` 共用这一条，两边口径必须一致。
    pub fn replace_backup_targets(&self, targets: &[BackupTarget]) -> Result<Vec<BackupTarget>> {
        let mut normalized: Vec<BackupTarget> = Vec::with_capacity(targets.len());
        for target in targets {
            let mut target = target.clone();
            target.name = target.name.trim().to_string();
            target.url = target.url.trim().to_string();
            if target.name.is_empty() {
                return Err(CoreError::config("备份目标名称不能为空"));
            }
            if !target.url.starts_with("postgres://") && !target.url.starts_with("postgresql://")
            {
                return Err(CoreError::config(format!(
                    "备份目标「{}」的连接串必须以 postgres:// 或 postgresql:// 开头",
                    target.name
                )));
            }
            if target.id.trim().is_empty() {
                target.id = new_id();
            }
            normalized.push(target);
        }
        let mut names: Vec<&str> = normalized.iter().map(|item| item.name.as_str()).collect();
        names.sort_unstable();
        if names.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(CoreError::config("备份目标名称不能重复"));
        }
        let mut ids: Vec<&str> = normalized.iter().map(|item| item.id.as_str()).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(CoreError::config("备份目标 ID 不能重复"));
        }

        let kept: Vec<String> = normalized.iter().map(|item| item.id.clone()).collect();
        let saved = normalized;
        self.mutate_config(|config| {
            let dropped: Vec<String> = config
                .backup_targets
                .iter()
                .map(|item| item.id.clone())
                .filter(|id| !kept.contains(id))
                .collect();
            if !dropped.is_empty() {
                let dropped_refs: Vec<&str> = dropped.iter().map(String::as_str).collect();
                let who = Self::backup_target_referrers(config, &dropped_refs);
                if !who.is_empty() {
                    let names: Vec<&str> = config
                        .backup_targets
                        .iter()
                        .filter(|item| dropped.iter().any(|id| *id == item.id))
                        .map(|item| item.name.as_str())
                        .collect();
                    return Err(CoreError::config(format!(
                        "备份目标「{}」仍被引用，不能删除。请先解绑：{}",
                        names.join("」「"),
                        who.join("、")
                    )));
                }
            }
            config.backup_targets = saved.clone();
            Ok(())
        })?;
        Ok(saved)
    }

    /// 把配置里保存的目标（id 或名称）解析成真实存在的目标 id。
    ///
    /// 解析不出来就报错，**不要**按「未绑定」清空：清空等于把这条配置交给备份时往下兜底，
    /// 那一晚它会打到另一个库上（见 [`backup_target_referrers`]）。
    pub fn resolve_backup_target_id(config: &AppConfig, raw: &str) -> Result<String> {
        let key = raw.trim();
        config
            .backup_targets
            .iter()
            .find(|item| item.id == key || item.name == key)
            .map(|item| item.id.clone())
            .ok_or_else(|| {
                CoreError::config(format!(
                    "备份目标不存在: {key}，请在数据库备份页重新为这条配置选一个目标"
                ))
            })
    }


    /// 按 id / 名称 / host 查找服务器。
    pub fn find_server<'a>(config: &'a AppConfig, key: &str) -> Result<&'a ServerConfig> {
        config
            .servers
            .iter()
            .find(|s| s.id == key || s.name == key || s.host == key)
            .ok_or_else(|| CoreError::not_found(format!("服务器不存在: {key}")))
    }

    pub fn upsert_server(config: &mut AppConfig, server: ServerConfig) -> Result<()> {
        match config.servers.iter_mut().find(|s| s.id == server.id) {
            Some(existing) => *existing = server,
            None => config.servers.push(server),
        }
        Ok(())
    }

    /// 把旧版「每台服务器一份 db_backup」迁移为全局备份配置（只执行一次）。
    /// 返回本次新建的配置数量。
    pub fn migrate_backup_configs(&self) -> Result<usize> {
        if self.load_config()?.backup_configs_migrated {
            return Ok(0);
        }
        self.mutate_config(|config| {
            if config.backup_configs_migrated {
                return Ok(0);
            }
            let mut additions: Vec<BackupConfig> = Vec::new();
            for server in &config.servers {
                let Some(source) = server.db_backup.clone() else {
                    continue;
                };
                if config
                    .backup_configs
                    .iter()
                    .chain(additions.iter())
                    .any(|item| item.server_id == server.id)
                {
                    continue;
                }
                let mut name = server.name.trim().to_string();
                if name.is_empty() {
                    name = format!("{}@{}", server.username, server.host);
                }
                if config
                    .backup_configs
                    .iter()
                    .chain(additions.iter())
                    .any(|item| item.name == name)
                {
                    let base = name.clone();
                    let mut index = 2;
                    loop {
                        let candidate = format!("{base} ({index})");
                        if !config
                            .backup_configs
                            .iter()
                            .chain(additions.iter())
                            .any(|item| item.name == candidate)
                        {
                            name = candidate;
                            break;
                        }
                        index += 1;
                    }
                }
                // 与服务器字段的优先级保持一致（绑定目标 > 自定义连接串）：两者都配置时只迁移赢家。
                // 否则 BackupConfig 中连接串优先于目标，会连到旧连接串指向的库。
                let target_id = server
                    .backup_target_id
                    .clone()
                    .filter(|value| !value.trim().is_empty());
                let supabase_url = if target_id.is_some() {
                    None
                } else {
                    server.supabase_url.clone()
                };
                // 服务器自己没绑目标的，旧行为是顺着「全局默认目标 → 旧版全局连接串」再往下找。
                // 而走已保存配置的备份不许再顺这两级（`backup.rs::resolve_backup` 拦换库），
                // 所以要把当时解析到的那一个搬成这条配置自己的目标：搬的是同一个库，不是换库；
                // 不搬的话这批老用户的定时备份从迁移那次起就一夜都不跑。
                let (target_id, supabase_url) = if target_id.is_some() || supabase_url.is_some() {
                    (target_id, supabase_url)
                } else {
                    match config
                        .settings
                        .default_backup_target_id
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        // 全局默认目标在场：搬它，与老链解析到的是同一个库。
                        Some(key) if config.backup_targets.iter().any(|item| item.id == *key) => {
                            (Some(key.to_string()), None)
                        }
                        // 全局默认指向已被删掉的目标：老链在这一级就直接报错、从不落旧版连接串
                        // （`backup.rs::resolve_target` 的 find_target 失败即返回）。把 DSN 搬过去，
                        // 每晚的 DROP SCHEMA + 导入就打在一个老链从没碰过的库上 —— 静默换库。
                        // 什么都不搬，让运行时报「没有可用的备份目标」。
                        Some(_) => (None, None),
                        // 没配全局默认目标：老链落到旧版全局连接串，把它搬成配置自己的连接串。
                        None => {
                            let legacy = config.settings.supabase_url.trim().to_string();
                            (None, (!legacy.is_empty()).then_some(legacy))
                        }
                    }
                };
                additions.push(BackupConfig {
                    id: new_id(),
                    name,
                    server_id: server.id.clone(),
                    source,
                    target_id,
                    supabase_url,
                });
            }
            let created = additions.len();
            config.backup_configs.extend(additions);
            config.backup_configs_migrated = true;
            Ok(created)
        })
    }
}

/// 判断两个本地路径是否指向同一处：统一分隔符、忽略结尾斜杠。
/// GUI 与 CLI 的仓库查重都走这里，避免各处自己写一套归一化而宽严不一。
pub fn paths_equal(a: &str, b: &str) -> bool {
    let normalize = |p: &str| {
        let normalized = p.replace('\\', "/");
        // 大小写不敏感只适用于 Windows；Linux 上 /srv/App 与 /srv/app 是两个不同仓库。
        #[cfg(windows)]
        let normalized = normalized.to_lowercase();
        normalized.trim_end_matches('/').to_string()
    };
    normalize(a) == normalize(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DbBackupSource, SshAuth};

    #[test]
    fn migrate_backup_configs_copies_server_sources_once() {
        use crate::models::{DbBackupSource, SshAuth};

        fn server(name: &str, host: &str) -> ServerConfig {
            let mut server = ServerConfig::new(
                name.to_string(),
                host.to_string(),
                "u".to_string(),
                SshAuth::Password {
                    password: "x".to_string(),
                },
            );
            server.db_backup = Some(DbBackupSource {
                mode: "docker".to_string(),
                container: "postgres".to_string(),
                database: "app".to_string(),
                username: "postgres".to_string(),
                password: "p".to_string(),
                schema: "public".to_string(),
            });
            server
        }

        let dir =
            std::env::temp_dir().join(format!("deploycode-store-migrate-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let mut config = AppConfig::default();
        let mut first = server("prod", "h1");
        first.backup_target_id = Some("t1".to_string());
        first.supabase_url = Some("postgresql://legacy@h/db".to_string());
        config.servers.push(first);
        // 同名服务器迁移时要生成不冲突的配置名；没有绑定目标时旧连接串照常迁移。
        let mut second = server("prod", "h2");
        second.supabase_url = Some("postgresql://legacy@h2/db".to_string());
        config.servers.push(second);
        store.save_config(&config).unwrap();

        assert_eq!(store.migrate_backup_configs().unwrap(), 2);
        let saved = store.load_config().unwrap();
        assert!(saved.backup_configs_migrated);
        assert_eq!(saved.backup_configs.len(), 2);
        assert_eq!(saved.backup_configs[0].name, "prod");
        assert_eq!(saved.backup_configs[1].name, "prod (2)");
        assert_eq!(saved.backup_configs[0].source.database, "app");
        // 服务器同时配置了绑定目标与旧连接串时，以绑定目标为准（与服务器字段优先级一致），
        // 旧的连接串不能一起迁移，否则会因配置中 URL 优先而备份到错误的库。
        assert_eq!(saved.backup_configs[0].target_id.as_deref(), Some("t1"));
        assert_eq!(saved.backup_configs[0].supabase_url, None);
        assert_eq!(saved.backup_configs[1].target_id, None);
        assert_eq!(
            saved.backup_configs[1].supabase_url.as_deref(),
            Some("postgresql://legacy@h2/db")
        );

        // 只迁移一次：即使配置被删除也不会在下次启动时重建。
        let removed_id = saved.backup_configs[0].id.clone();
        let mut pruned = saved;
        pruned.backup_configs.retain(|item| item.id != removed_id);
        store.save_config(&pruned).unwrap();
        assert_eq!(store.migrate_backup_configs().unwrap(), 0);
        assert_eq!(store.load_config().unwrap().backup_configs.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 只配了「全局默认目标」（或旧版全局连接串）的老用户：迁移要把当时解析到的那一个搬进配置。
    ///
    /// 走已保存配置的备份不许再顺全局兜底（那是换库风险，`backup.rs::resolve_backup` 拦），
    /// 不搬的话这批人从迁移那一夜起一次都备份不成，而界面上看着一切正常。
    /// 搬的是同一个库，不是换库。
    #[test]
    fn migrate_backup_configs_carries_the_resolved_global_target() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-store-migrate-global-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::new(&dir);
        let source = || DbBackupSource {
            database: "app".to_string(),
            username: "postgres".to_string(),
            ..DbBackupSource::default()
        };
        let server = |name: &str| {
            let mut item = ServerConfig::new(
                name.to_string(),
                "10.0.0.1".to_string(),
                "u".to_string(),
                SshAuth::Password {
                    password: "x".to_string(),
                },
            );
            item.db_backup = Some(source());
            item
        };

        // 一：全局默认目标在场 → 搬成这条配置自己的目标。
        let mut config = AppConfig::default();
        config.backup_targets.push(BackupTarget {
            id: "t-global".to_string(),
            name: "仓库库".to_string(),
            url: "postgresql://u:p@h/warehouse".to_string(),
        });
        config.settings.default_backup_target_id = Some("t-global".to_string());
        config.servers.push(server("prod"));
        store.save_config(&config).unwrap();
        assert_eq!(store.migrate_backup_configs().unwrap(), 1);
        let saved = store.load_config().unwrap();
        assert_eq!(saved.backup_configs[0].target_id.as_deref(), Some("t-global"));
        assert_eq!(saved.backup_configs[0].supabase_url, None);

        // 二：只有旧版全局连接串 → 搬成这条配置自己的连接串。
        let dir2 = std::env::temp_dir().join(format!(
            "deploycode-store-migrate-legacy-{}",
            uuid::Uuid::new_v4()
        ));
        let store2 = Store::new(&dir2);
        let mut config = AppConfig::default();
        config.settings.supabase_url = "postgresql://u:p@h/legacy".to_string();
        config.servers.push(server("prod"));
        store2.save_config(&config).unwrap();
        assert_eq!(store2.migrate_backup_configs().unwrap(), 1);
        let saved = store2.load_config().unwrap();
        assert_eq!(saved.backup_configs[0].target_id, None);
        assert_eq!(
            saved.backup_configs[0].supabase_url.as_deref(),
            Some("postgresql://u:p@h/legacy")
        );

        // 三：全局默认指向一个已被删掉的目标 → 不搬（搬过去只会得到一条解析不了的绑定），
        // 让运行时明确报「这条配置没有可用的备份目标」，而不是悄悄连到别的库上。
        let mut config = store.load_config().unwrap();
        config.backup_configs_migrated = false;
        config.backup_configs.clear();
        config.backup_targets.clear();
        config.settings.default_backup_target_id = Some("t-gone".to_string());
        store.save_config(&config).unwrap();
        assert_eq!(store.migrate_backup_configs().unwrap(), 1);
        let saved = store.load_config().unwrap();
        assert_eq!(saved.backup_configs[0].target_id, None);
        assert_eq!(saved.backup_configs[0].supabase_url, None);

        // 四：全局默认悬空、但旧版全局连接串还在 —— 最容易走错的组合。老链在悬空那一级
        // 就报错、从不落 DSN，把 DSN 搬过去等于每晚对老链从没碰过的库做 DROP SCHEMA + 导入；
        // 必须什么都不搬，让运行时明确报「没有可用的备份目标」。
        let mut config = store.load_config().unwrap();
        config.backup_configs_migrated = false;
        config.backup_configs.clear();
        config.backup_targets.clear();
        config.settings.default_backup_target_id = Some("t-gone".to_string());
        config.settings.supabase_url = "postgresql://u:p@h/legacy".to_string();
        store.save_config(&config).unwrap();
        assert_eq!(store.migrate_backup_configs().unwrap(), 1);
        let saved = store.load_config().unwrap();
        assert_eq!(saved.backup_configs[0].target_id, None);
        assert_eq!(saved.backup_configs[0].supabase_url, None);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    /// 仍被引用的备份目标不许删：静默解绑之后那次备份会沿兜底链路打到另一个库上，
    /// 而备份脚本对新库的第一步是 `DROP SCHEMA ... CASCADE`。
    #[test]
    fn replace_backup_targets_refuses_to_drop_a_referenced_target() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-store-targets-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);

        let mut target =
            BackupTarget::new("Aiven".to_string(), "postgresql://a@h/db".to_string());
        target.id = "t1".to_string();
        let mut saved = BackupConfig::new(
            "每晚".to_string(),
            "s1".to_string(),
            DbBackupSource {
                database: "app".to_string(),
                ..DbBackupSource::default()
            },
        );
        saved.id = "b1".to_string();
        saved.target_id = Some("t1".to_string());
        store
            .save_config(&AppConfig {
                backup_targets: vec![target.clone()],
                backup_configs: vec![saved],
                ..Default::default()
            })
            .unwrap();

        let err = store
            .replace_backup_targets(&[])
            .expect_err("被引用的目标不许删");
        let text = err.to_string();
        assert!(
            text.contains("Aiven") && text.contains("每晚"),
            "报错要同时点出目标与引用者：{text}"
        );
        let after = store.load_config().unwrap();
        assert_eq!(after.backup_targets.len(), 1, "被拒绝的删除不该落盘");
        assert_eq!(after.backup_configs[0].target_id.as_deref(), Some("t1"));

        // 解绑之后就放行 —— 拦的是静默换库，不是删目标本身。
        store
            .mutate_config(|config| {
                config.backup_configs[0].target_id = None;
                Ok(())
            })
            .unwrap();
        store.replace_backup_targets(&[]).unwrap();
        assert!(store.load_config().unwrap().backup_targets.is_empty());

        // 名称首尾空格照旧清掉，重名照旧拒绝。
        let renamed = store
            .replace_backup_targets(&[BackupTarget::new(
                "  目标库  ".to_string(),
                "postgresql://b@h/db".to_string(),
            )])
            .unwrap();
        assert_eq!(renamed[0].name, "目标库");
        let mut same = renamed[0].clone();
        same.id = "other-id".to_string();
        assert!(store.replace_backup_targets(&[renamed[0].clone(), same]).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

}
