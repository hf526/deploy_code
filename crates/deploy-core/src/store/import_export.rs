//! 配置导出 / 导入：敏感字段脱敏与「空 = 本次没提供，保留本机值」的合并语义。

use super::Store;
use crate::error::{CoreError, Result};
use crate::models::{
    AppConfig, BackupConfig, DbBackupSource, EnvFileConfig, ExportData,
    ImportCounts, ImportPreview, ServerConfig,
    SshAuth,
};

impl Store {
    /// 导出配置（JSON 格式，敏感字段已脱敏）。
    pub fn export_config(&self) -> Result<String> {
        let config = self.load_config()?;
        let export_data = ExportData::new(&config);
        serde_json::to_string_pretty(&export_data).map_err(CoreError::Serde)
    }

    /// 导入前的只读比对：解析文件并演练一遍合并语义，不写盘。
    /// 导出文件必然已脱敏，所以调用方必须先把结果给用户确认，再调 `import_config`。
    pub fn preview_import(&self, json_str: &str) -> Result<ImportPreview> {
        let export_data = parse_export(json_str)?;
        let mut config = self.load_config()?;
        Ok(merge_export(&mut config, &export_data))
    }

    /// 导入配置（合并模式：同 ID 覆盖，新 ID 追加，被脱敏清空的凭据保留本机值）。
    pub fn import_config(&self, json_str: &str) -> Result<ImportPreview> {
        let export_data = parse_export(json_str)?;
        self.mutate_config(|config| Ok(merge_export(config, &export_data)))
    }
}

/// 解析导出文件并校验版本。
fn parse_export(json_str: &str) -> Result<ExportData> {
    let data: ExportData = serde_json::from_str(json_str)
        .map_err(|e| CoreError::config(format!("配置文件格式错误：{}", e)))?;
    if data.version != "1.0" {
        return Err(CoreError::config(format!(
            "不支持的配置文件版本：{}",
            data.version
        )));
    }
    Ok(data)
}

/// 导出必然把敏感字段写成空串，所以「空」在这里的语义是「本次没有提供」，保留本机值。
fn keep_when_blank(incoming: &mut String, current: &str, kept: &mut usize) {
    if incoming.trim().is_empty() && !current.trim().is_empty() {
        *incoming = current.to_string();
        *kept += 1;
    }
}

/// 合并服务器凭据。逐字段判断，避免把导出的空值写进本机真实凭据。
fn merge_auth(incoming: &mut SshAuth, current: &SshAuth, kept: &mut usize) {
    // 本机用密钥、导入文件里却是一个空密码认证：整套保留本机，否则导入后必然连不上。
    let downgrade_to_blank_password = matches!(
        (&*incoming, current),
        (SshAuth::Password { password }, SshAuth::PrivateKey { .. }) if password.trim().is_empty()
    );
    if downgrade_to_blank_password {
        *incoming = current.clone();
        *kept += 1;
        return;
    }
    match (incoming, current) {
        (SshAuth::Password { password }, SshAuth::Password { password: saved }) => {
            keep_when_blank(password, saved, kept);
        }
        // 私钥口令在导出时统一被抹成 None，与「本来就没有口令」无法区分，只能按空值处理。
        (
            SshAuth::PrivateKey { passphrase, .. },
            SshAuth::PrivateKey { passphrase: Some(saved), .. },
        ) if passphrase.is_none() => {
            *passphrase = Some(saved.clone());
            *kept += 1;
        }
        _ => {}
    }
}

/// 数据库口令：导出时必然被抹成空串，空即「本次没有提供」，保留本机值。
/// 采集方式 / 容器名 / 库名 / schema 不是凭据，照文件走。
fn merge_db_source(incoming: &mut DbBackupSource, current: &DbBackupSource, kept: &mut usize) {
    keep_when_blank(&mut incoming.password, &current.password, kept);
}

/// 可空连接串：导出时统一抹成 `None`，与「本机没有这项」无法区分（同私钥口令）。
/// 本机存过就继续用本机的；换机导入时本机没有，留空让用户自己填，
/// 绝不能把 `Some("")` 落盘——那会被下游当成一条可用的连接串。
fn keep_option_when_absent(
    incoming: &mut Option<String>,
    current: &Option<String>,
    kept: &mut usize,
) {
    if incoming.is_none() && current.is_some() {
        *incoming = current.clone();
        *kept += 1;
    }
}

/// env 文件按数组下标对齐：导出保留了顺序与条数，脱敏后只剩空串，只能按序回填。
fn merge_env_files(incoming: &mut Vec<EnvFileConfig>, current: &[EnvFileConfig], kept: &mut usize) {
    for (index, file) in incoming.iter_mut().enumerate() {
        let Some(saved) = current.get(index) else { break };
        keep_when_blank(&mut file.local_path, &saved.local_path, kept);
        keep_when_blank(&mut file.remote_path, &saved.remote_path, kept);
    }
}

/// 按 id 合并一类配置：同 ID 覆盖（覆盖前用 `preserve` 把导出的空值换回本机值），新 ID 追加。
fn merge_by_id<T, K, P>(target: &mut Vec<T>, incoming: &[T], key: K, mut preserve: P) -> ImportCounts
where
    T: Clone,
    K: Fn(&T) -> &str,
    P: FnMut(&mut T, &T),
{
    let mut counts = ImportCounts::default();
    for item in incoming {
        match target.iter().position(|existing| key(existing) == key(item)) {
            Some(index) => {
                let mut merged = item.clone();
                preserve(&mut merged, &target[index]);
                target[index] = merged;
                counts.overwritten += 1;
            }
            None => {
                target.push(item.clone());
                counts.added += 1;
            }
        }
    }
    counts
}

/// 把导出内容合并进本机配置，返回本次的去向统计。
///
/// 预览与实际导入共用这一个函数，因此「确认框里看到的」与「真正落盘的」必然一致。
fn merge_export(config: &mut AppConfig, data: &ExportData) -> ImportPreview {
    let mut kept = 0usize;
    let mut preview = ImportPreview {
        exported_at: data.exported_at.clone(),
        ..Default::default()
    };

    preview.servers = merge_by_id(
        &mut config.servers,
        &data.servers,
        |item| item.id.as_str(),
        |incoming: &mut ServerConfig, current: &ServerConfig| {
            merge_auth(&mut incoming.auth, &current.auth, &mut kept);
            if let (Some(incoming), Some(current)) =
                (incoming.db_backup.as_mut(), current.db_backup.as_ref())
            {
                merge_db_source(incoming, current, &mut kept);
            }
            keep_option_when_absent(
                &mut incoming.supabase_url,
                &current.supabase_url,
                &mut kept,
            );
        },
    );
    preview.repos = merge_by_id(
        &mut config.repos,
        &data.repos,
        |item| item.id.as_str(),
        |incoming, current| merge_env_files(&mut incoming.env_files, &current.env_files, &mut kept),
    );
    preview.backup_targets = merge_by_id(
        &mut config.backup_targets,
        &data.backup_targets,
        |item| item.id.as_str(),
        // 目标连接串带口令，导出时整条抹空：本机有值就继续用本机的。
        |incoming, current| keep_when_blank(&mut incoming.url, &current.url, &mut kept),
    );
    preview.deploy_configs = merge_by_id(
        &mut config.deploy_configs,
        &data.deploy_configs,
        |item| item.id.as_str(),
        |_, _| {},
    );
    preview.backup_configs = merge_by_id(
        &mut config.backup_configs,
        &data.backup_configs,
        |item| item.id.as_str(),
        |incoming: &mut BackupConfig, current: &BackupConfig| {
            merge_db_source(&mut incoming.source, &current.source, &mut kept);
            keep_option_when_absent(
                &mut incoming.supabase_url,
                &current.supabase_url,
                &mut kept,
            );
        },
    );
    preview.pages_configs = merge_by_id(
        &mut config.pages_configs,
        &data.pages_configs,
        |item| item.id.as_str(),
        |_, _| {},
    );
    // 容器备份配置只引用服务器 id 与 compose 项目名，本身不含凭据，整体跟随导入文件。
    preview.container_configs = merge_by_id(
        &mut config.container_configs,
        &data.container_configs,
        |item| item.id.as_str(),
        |_, _| {},
    );

    // 设置整体跟随导入文件（换机迁移主要靠它），但导出的空凭据一律保留本机值。
    let mut settings = data.settings.clone();
    keep_when_blank(
        &mut settings.cloudflare_api_token,
        &config.settings.cloudflare_api_token,
        &mut kept,
    );
    keep_when_blank(
        &mut settings.cloudflare_account_id,
        &config.settings.cloudflare_account_id,
        &mut kept,
    );
    keep_when_blank(&mut settings.github_token, &config.settings.github_token, &mut kept);
    keep_when_blank(
        &mut settings.cronjob_api_key,
        &config.settings.cronjob_api_key,
        &mut kept,
    );
    // 旧版全局备份连接串导出时是空串，本机有值就继续用本机的，否则那条老兜底会被导出的空值清掉。
    keep_when_blank(
        &mut settings.supabase_url,
        &config.settings.supabase_url,
        &mut kept,
    );
    if settings.master_password_hash.is_none() && config.settings.master_password_hash.is_some() {
        settings.master_password_hash = config.settings.master_password_hash.clone();
        kept += 1;
    }
    // 三条「本调度日已触发」跟本机走，不跟导入文件：它是今晚还要不要跑的依据。
    // 整体覆盖会让「从那台今天已经备份过的机器」导过来的配置把本机今晚那次直接吃掉，
    // 而一次导入换来一次静默漏备份，是这里最贵的一种错。
    settings.scheduled_backup_last_run = config.settings.scheduled_backup_last_run.clone();
    settings.scheduled_container_last_run = config.settings.scheduled_container_last_run.clone();
    settings.scheduled_shutdown_last_run = config.settings.scheduled_shutdown_last_run.clone();
    config.settings = settings;

    preview.kept_local_secrets = kept;
    preview
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::{config_with_secrets, export_with_server, temp_store};

    #[test]
    fn export_then_import_preserves_local_credentials() {
        let (store, dir) = temp_store();
        store.save_config(&config_with_secrets()).unwrap();
        let exported = store.export_config().unwrap();

        // 导出之后本机改过主机名：非敏感字段应跟随文件，凭据不能被导出的空值冲掉。
        let mut current = store.load_config().unwrap();
        current.servers[0].host = "10.0.0.9".to_string();
        store.save_config(&current).unwrap();

        let preview = store.import_config(&exported).unwrap();
        let after = store.load_config().unwrap();

        assert_eq!(after.servers[0].host, "10.0.0.1");
        assert!(
            matches!(&after.servers[0].auth, SshAuth::Password { password } if password == "s3cret"),
            "导出的空密码覆盖了本机密码"
        );
        assert_eq!(after.repos[0].env_files[0].local_path, "D:/web/.env");
        assert_eq!(after.settings.github_token, "gh_secret");
        assert_eq!(after.settings.cronjob_api_key, "cj_secret");
        assert_eq!(preview.servers.overwritten, 1);
        // 密码 1 + env 本地/远端路径 2 + Token 2
        assert_eq!(preview.kept_local_secrets, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_does_not_borrow_the_other_machines_scheduled_run_dates() {
        let (store, dir) = temp_store();
        let mut source = AppConfig::default();
        // 对面那台机器今天已经跑过定时（导出文件里带着这三条日期）。
        source.settings.scheduled_backup_last_run = "2026-09-27".to_string();
        source.settings.scheduled_container_last_run = "2026-09-27".to_string();
        source.settings.scheduled_shutdown_last_run = "2026-09-27".to_string();
        store.save_config(&source).unwrap();
        let exported = store.export_config().unwrap();

        // 本机还没跑过：导入之后仍然是「没跑过」，否则今晚那次定时会被直接吃掉。
        let mut fresh = store.load_config().unwrap();
        fresh.settings.scheduled_backup_last_run = String::new();
        fresh.settings.scheduled_container_last_run = String::new();
        fresh.settings.scheduled_shutdown_last_run = String::new();
        store.save_config(&fresh).unwrap();

        store.import_config(&exported).unwrap();
        let after = store.load_config().unwrap().settings;
        assert_eq!(after.scheduled_backup_last_run, "");
        assert_eq!(after.scheduled_container_last_run, "");
        assert_eq!(after.scheduled_shutdown_last_run, "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_blankout_does_not_erase_local_db_credentials() {
        let (store, dir) = temp_store();
        let mut server = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "D:/id_ed25519".to_string(),
                passphrase: None,
            },
        );
        server.id = "s1".to_string();
        server.db_backup = Some(DbBackupSource {
            container: "pg".to_string(),
            database: "appdb".to_string(),
            username: "postgres".to_string(),
            password: "db_local".to_string(),
            ..DbBackupSource::default()
        });
        server.supabase_url = Some("postgres://u:db_local@h/db".to_string());
        let mut target =
            crate::models::BackupTarget::new("目标库".to_string(), "postgres://u:tgt_local@h:5432/db".to_string());
        target.id = "t1".to_string();
        let mut backup = BackupConfig::new(
            "每晚".to_string(),
            "s1".to_string(),
            DbBackupSource {
                database: "appdb".to_string(),
                password: "src_local".to_string(),
                ..DbBackupSource::default()
            },
        );
        backup.id = "b1".to_string();
        backup.supabase_url = Some("postgres://u:cfg_local@h/db".to_string());
        let mut settings = crate::models::Settings::default();
        // 旧版全局备份连接串：导出必须抹掉，导回必须保住本机这份（它仍是兜底解析用的一条）。
        settings.supabase_url = "postgresql://u:legacy_local@host/legacydb".to_string();
        store
            .save_config(&AppConfig {
                servers: vec![server],
                backup_targets: vec![target],
                backup_configs: vec![backup],
                settings,
                ..Default::default()
            })
            .unwrap();

        let exported = store.export_config().unwrap();
        for secret in [
            "db_local",
            "tgt_local",
            "src_local",
            "cfg_local",
            "legacy_local",
        ] {
            assert!(!exported.contains(secret), "导出内容泄露了 {secret}");
        }

        // 导回本机：被抹掉的凭据必须原样留着，否则一次导入就把备份功能打废。
        let preview = store.import_config(&exported).unwrap();
        let after = store.load_config().unwrap();
        assert_eq!(after.servers[0].db_backup.as_ref().unwrap().password, "db_local");
        assert_eq!(
            after.servers[0].supabase_url.as_deref(),
            Some("postgres://u:db_local@h/db")
        );
        assert_eq!(after.backup_targets[0].url, "postgres://u:tgt_local@h:5432/db");
        assert_eq!(after.backup_configs[0].source.password, "src_local");
        assert_eq!(
            after.backup_configs[0].supabase_url.as_deref(),
            Some("postgres://u:cfg_local@h/db")
        );
        // 服务器 1 + 目标 1 + 备份配置 2 + 旧版全局连接串 1
        assert_eq!(preview.kept_local_secrets, 6);
        assert_eq!(
            after.settings.supabase_url,
            "postgresql://u:legacy_local@host/legacydb"
        );

        // 换机导入（本机没有对应值）：不能落进 Some("")，那会被下游当成一条可用连接串。
        let (fresh, fresh_dir) = temp_store();
        fresh.import_config(&exported).unwrap();
        let moved = fresh.load_config().unwrap();
        assert_eq!(moved.servers[0].supabase_url, None);
        assert!(moved.backup_targets[0].url.is_empty());
        assert_eq!(moved.servers[0].db_backup.as_ref().unwrap().database, "appdb");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&fresh_dir);
    }

    #[test]
    fn preview_import_counts_added_and_overwritten_without_writing() {
        let (src, src_dir) = temp_store();
        src.save_config(&config_with_secrets()).unwrap();
        let mut two = src.load_config().unwrap();
        let mut extra = ServerConfig::new(
            "stage".to_string(),
            "10.0.0.2".to_string(),
            "deploy".to_string(),
            SshAuth::Password {
                password: "p2".to_string(),
            },
        );
        extra.id = "s2".to_string();
        two.servers.push(extra);
        src.save_config(&two).unwrap();
        let exported = src.export_config().unwrap();

        let (store, dir) = temp_store();
        store.save_config(&config_with_secrets()).unwrap();

        let preview = store.preview_import(&exported).unwrap();
        assert_eq!(preview.servers.added, 1);
        assert_eq!(preview.servers.overwritten, 1);
        assert_eq!(
            store.load_config().unwrap().servers.len(),
            1,
            "预览不得写盘"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn import_keeps_passphrase_but_follows_file_for_key_path() {
        let (store, dir) = temp_store();
        let mut server = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "C:/keys/id_ed25519".to_string(),
                passphrase: Some("pp".to_string()),
            },
        );
        server.id = "s1".to_string();
        store
            .save_config(&AppConfig {
                servers: vec![server],
                ..Default::default()
            })
            .unwrap();

        // 导出把口令抹成 None，这与「本来就没有口令」无法区分，只能按未提供处理。
        let mut incoming = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.8".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "C:/keys/new_path".to_string(),
                passphrase: None,
            },
        );
        incoming.id = "s1".to_string();
        store.import_config(&export_with_server(incoming)).unwrap();

        let after = store.load_config().unwrap();
        match &after.servers[0].auth {
            SshAuth::PrivateKey { key_path, passphrase } => {
                assert_eq!(key_path, "C:/keys/new_path");
                assert_eq!(passphrase.as_deref(), Some("pp"));
            }
            other => panic!("认证方式被改写: {other:?}"),
        }
        assert_eq!(after.servers[0].host, "10.0.0.8");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_does_not_downgrade_key_auth_to_blank_password() {
        let (store, dir) = temp_store();
        let mut server = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::PrivateKey {
                key_path: "C:/keys/id_ed25519".to_string(),
                passphrase: None,
            },
        );
        server.id = "s1".to_string();
        store
            .save_config(&AppConfig {
                servers: vec![server],
                ..Default::default()
            })
            .unwrap();

        let mut incoming = ServerConfig::new(
            "prod".to_string(),
            "10.0.0.8".to_string(),
            "root".to_string(),
            SshAuth::Password {
                password: String::new(),
            },
        );
        incoming.id = "s1".to_string();
        let preview = store.import_config(&export_with_server(incoming)).unwrap();

        let after = store.load_config().unwrap();
        assert!(
            matches!(&after.servers[0].auth, SshAuth::PrivateKey { key_path, .. } if key_path == "C:/keys/id_ed25519"),
            "空密码导入把密钥认证降级了"
        );
        assert_eq!(preview.kept_local_secrets, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

}
