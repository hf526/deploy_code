//! 备份来源与目标的校验、解析与优先级。

use crate::error::{CoreError, Result};
use crate::models::{
    AppConfig, BackupRequest, BackupTarget, DbBackupSource, ServerConfig, Settings,
};
use crate::store::Store;

use super::db_url::validate_target_url;

// ---------------------------------------------------------------------------
// 校验与解析
// ---------------------------------------------------------------------------

impl DbBackupSource {
    pub(super) fn is_docker(&self) -> bool {
        self.mode.eq_ignore_ascii_case("docker")
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn normalize_source(mut source: DbBackupSource) -> Result<DbBackupSource> {
    source.mode = source.mode.trim().to_lowercase();
    if source.mode.is_empty() {
        source.mode = "docker".to_string();
    }
    source.container = source.container.trim().to_string();
    source.database = source.database.trim().to_string();
    source.username = source.username.trim().to_string();
    source.password = source.password.trim().to_string();
    source.schema = source.schema.trim().to_string();
    if source.schema.is_empty() {
        source.schema = "public".to_string();
    }
    Ok(source)
}

fn validate_source(source: &DbBackupSource) -> Result<()> {
    if source.mode != "docker" && source.mode != "system" {
        return Err(CoreError::config(format!(
            "不支持的采集方式: {}（可选 docker / system）",
            source.mode
        )));
    }
    if source.is_docker() && source.container.is_empty() {
        return Err(CoreError::config("docker 模式需要填写容器名"));
    }
    if source.database.is_empty() {
        return Err(CoreError::config("数据库名不能为空"));
    }
    if source.username.is_empty() {
        return Err(CoreError::config("数据库用户名不能为空"));
    }
    if source.schema.contains('"') || source.schema.chars().any(char::is_control) {
        return Err(CoreError::config(
            "schema 名称包含非法字符（不能含双引号或换行等控制字符）",
        ));
    }
    // reset SQL 使用 $do$ 作为 dollar-quote 标签，schema 中出现同名标签会截断 DO 块。
    if source.schema.contains("$do$") {
        return Err(CoreError::config("schema 名称不能包含 $do$"));
    }
    Ok(())
}

/// 已解析的备份任务来源（服务器 + 来源 + 目标）。
pub(super) struct ResolvedBackup {
    pub(super) server: ServerConfig,
    pub(super) source: DbBackupSource,
    pub(super) target: ResolvedTarget,
}

/// 解析一次备份请求：
/// - 指定了 `backup_config_id` 时，服务器 / 来源 / 目标取自保存的配置；
/// - 否则沿用服务器上的旧版单份来源配置；
/// - `source` 字段可直接覆盖来源（测试未保存的表单），`database` / `schema` 再覆盖对应字段。
pub(super) fn resolve_backup(config: &AppConfig, req: &BackupRequest) -> Result<ResolvedBackup> {
    let (server, source, config_target_id, config_url, saved_name) =
        match non_empty(&req.backup_config_id) {
            Some(key) => {
                let saved = Store::find_backup_config(config, key)?;
                (
                    Store::find_server(config, &saved.server_id)?.clone(),
                    req.source.clone().unwrap_or_else(|| saved.source.clone()),
                    saved.target_id.clone(),
                    saved.supabase_url.clone(),
                    Some(saved.name.clone()),
                )
            }
            None => {
                let server = Store::find_server(config, &req.server_id)?.clone();
                let source = req
                    .source
                    .clone()
                    .or_else(|| server.db_backup.clone())
                    .ok_or_else(|| CoreError::config("请先为该服务器配置数据库备份来源"))?;
                (server, source, None, None, None)
            }
        };

    let mut source = normalize_source(source)?;
    if let Some(database) = non_empty(&req.database) {
        source.database = database.to_string();
    }
    if let Some(schema) = non_empty(&req.schema) {
        source.schema = schema.to_string();
    }
    validate_source(&source)?;

    // 请求参数（连接串 / 目标）> 配置自身的覆盖目标 > 服务器 / 全局默认。
    // 必须按来源区分：请求里显式指定的目标不能被配置里保存的连接串压过，反之亦然。
    if let Some(name) = saved_name.as_deref() {
        // 走的是已保存的配置，就必须有它自己（或请求显式带来）的目标。配置原本绑着目标 T、
        // 之后 T 被删掉时，保存环节会把悬空 id 清成未绑定；这时候若照旧往下找服务器绑定 /
        // 全局默认，备份会悄悄打到另一个库上，而脚本对这个库做的是 `DROP SCHEMA ... CASCADE`。
        // 宁可这一晚不跑并说清楚原因，也不能换库。
        let has_own_target = non_empty(&req.target_id).is_some()
            || non_empty(&req.supabase_url).is_some()
            || non_empty(&config_target_id).is_some()
            || non_empty(&config_url).is_some();
        if !has_own_target {
            return Err(CoreError::config(format!(
                "备份配置「{name}」没有可用的备份目标（原来绑定的目标可能已被删除），请在数据库备份页为它重新选一个目标"
            )));
        }
    }
    let target = resolve_target(
        &config.backup_targets,
        &server,
        &config.settings,
        &req.target_id,
        &req.supabase_url,
        &config_target_id,
        &config_url,
    )?;

    Ok(ResolvedBackup {
        server,
        source,
        target,
    })
}

/// 已解析的备份目标。
pub(super) struct ResolvedTarget {
    pub(super) name: String,
    pub(super) url: String,
}

/// 解析备份目标，优先级：
/// 请求直接连接串 > 请求指定目标 > 配置连接串 > 配置指定目标 >
/// 服务器绑定目标 > 服务器自定义连接串 > 全局默认目标 > 旧版全局连接串。
///
/// 后四级是给「没建备份配置、沿用服务器旧版单份来源」那条老路留的。走已保存的配置时
/// 不许落到它们身上 —— 拦截在 [`resolve_backup`]，因为报错要说清是哪条配置。
fn resolve_target(
    targets: &[BackupTarget],
    server: &ServerConfig,
    settings: &Settings,
    request_target_id: &Option<String>,
    request_url: &Option<String>,
    config_target_id: &Option<String>,
    config_url: &Option<String>,
) -> Result<ResolvedTarget> {
    if let Some(url) = non_empty(request_url) {
        return Ok(ResolvedTarget {
            name: "自定义连接串".to_string(),
            url: validate_target_url(url)?,
        });
    }
    if let Some(key) = non_empty(request_target_id) {
        return find_target(targets, key);
    }
    if let Some(url) = non_empty(config_url) {
        return Ok(ResolvedTarget {
            name: "自定义连接串".to_string(),
            url: validate_target_url(url)?,
        });
    }
    if let Some(key) = non_empty(config_target_id) {
        return find_target(targets, key);
    }
    if let Some(key) = non_empty(&server.backup_target_id) {
        return find_target(targets, key);
    }
    if let Some(url) = non_empty(&server.supabase_url) {
        return Ok(ResolvedTarget {
            name: "服务器自定义连接串".to_string(),
            url: validate_target_url(url)?,
        });
    }
    if let Some(key) = non_empty(&settings.default_backup_target_id) {
        return find_target(targets, key);
    }
    let legacy = settings.supabase_url.trim();
    if !legacy.is_empty() {
        return Ok(ResolvedTarget {
            name: "旧版连接串".to_string(),
            url: validate_target_url(legacy)?,
        });
    }
    Err(CoreError::config(
        "请先在设置页配置备份目标（数据库备份 → 备份目标）",
    ))
}

fn find_target(targets: &[BackupTarget], key: &str) -> Result<ResolvedTarget> {
    let target = targets
        .iter()
        .find(|target| target.id == key || target.name == key)
        .ok_or_else(|| CoreError::config(format!("备份目标不存在: {key}")))?;
    Ok(ResolvedTarget {
        name: target.name.clone(),
        url: validate_target_url(&target.url)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::testutil::source;

    #[test]
    fn validate_source_rejects_bad_mode_and_empty_fields() {
        let mut bad = source();
        bad.mode = "k8s".to_string();
        assert!(validate_source(&bad).is_err());

        let mut bad = source();
        bad.container = "  ".to_string();
        assert!(validate_source(&normalize_source(bad).unwrap()).is_err());

        let mut bad = source();
        bad.database = String::new();
        assert!(validate_source(&bad).is_err());
    }

    #[test]
    fn resolve_target_prefers_override_then_targets_then_legacy() {
        let mut server = ServerConfig::new(
            "s".to_string(),
            "h".to_string(),
            "u".to_string(),
            crate::models::SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let mut settings = Settings::default();
        assert!(resolve_target(&[], &server, &settings, &None, &None, &None, &None).is_err());

        let targets = vec![
            BackupTarget::new("Aiven".to_string(), "postgresql://aiven@host/db".to_string()),
            BackupTarget::new("Neon".to_string(), "postgresql://neon@host/db".to_string()),
        ];

        // 全局旧连接串作为兜底。
        settings.supabase_url = "postgresql://legacy@host/db".to_string();
        let resolved =
            resolve_target(&targets, &server, &settings, &None, &None, &None, &None).unwrap();
        assert_eq!(resolved.url, "postgresql://legacy@host/db");

        // 全局默认目标优先于旧连接串。
        settings.default_backup_target_id = Some(targets[0].id.clone());
        let resolved =
            resolve_target(&targets, &server, &settings, &None, &None, &None, &None).unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 服务器绑定目标优先于全局默认。
        server.backup_target_id = Some(targets[1].id.clone());
        let resolved =
            resolve_target(&targets, &server, &settings, &None, &None, &None, &None).unwrap();
        assert_eq!(resolved.name, "Neon");

        // 请求指定目标（可用名称匹配）优先于服务器绑定。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &None,
            &None,
            &None,
        )
        .unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 请求指定目标优先于配置自身的连接串（否则会被配置覆盖到错误的库）。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &None,
            &None,
            &Some("postgresql://config@host/db".to_string()),
        )
        .unwrap();
        assert_eq!(resolved.name, "Aiven");

        // 请求直接连接串优先级最高。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &Some("Aiven".to_string()),
            &Some("postgresql://raw@host/db".to_string()),
            &None,
            &None,
        )
        .unwrap();
        assert_eq!(resolved.url, "postgresql://raw@host/db");

        // 无请求参数时，配置连接串优先于配置目标。
        let resolved = resolve_target(
            &targets,
            &server,
            &settings,
            &None,
            &None,
            &Some(targets[1].id.clone()),
            &Some("postgresql://config@host/db".to_string()),
        )
        .unwrap();
        assert_eq!(resolved.url, "postgresql://config@host/db");

        // 不存在的目标报错。
        assert!(matches!(
            resolve_target(
                &targets,
                &server,
                &settings,
                &Some("missing".to_string()),
                &None,
                &None,
                &None
            ),
            Err(CoreError::Config(_))
        ));

        // 非 postgres 连接串报错。
        assert!(matches!(
            resolve_target(
                &targets,
                &server,
                &settings,
                &Some("mysql://x".to_string()),
                &None,
                &None,
                &None
            ),
            Err(CoreError::Config(_))
        ));
    }

    #[test]
    fn resolve_backup_uses_saved_config_then_request_overrides() {
        use crate::models::{BackupConfig, SshAuth};

        let server = ServerConfig::new(
            "prod".to_string(),
            "h".to_string(),
            "u".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        );
        let mut config = AppConfig {
            servers: vec![server.clone()],
            ..AppConfig::default()
        };
        let targets = vec![BackupTarget::new(
            "Aiven".to_string(),
            "postgresql://aiven@host/db".to_string(),
        )];
        config.backup_targets = targets.clone();
        let mut saved = BackupConfig::new(
            "生产库".to_string(),
            server.id.clone(),
            DbBackupSource {
                mode: "system".to_string(),
                container: String::new(),
                database: "app".to_string(),
                username: "postgres".to_string(),
                password: "p".to_string(),
                schema: "app".to_string(),
            },
        );
        saved.target_id = Some(config.backup_targets[0].id.clone());
        config.backup_configs.push(saved.clone());

        let request = BackupRequest {
            // 使用配置时允许 server_id 留空（CLI 可只给 --config）。
            server_id: String::new(),
            backup_config_id: Some(saved.id.clone()),
            source: None,
            target_id: None,
            supabase_url: None,
            database: Some("override".to_string()),
            schema: None,
        };
        let resolved = resolve_backup(&config, &request).unwrap();
        assert_eq!(resolved.server.id, server.id);
        assert_eq!(resolved.source.database, "override");
        assert_eq!(resolved.source.mode, "system");
        assert_eq!(resolved.target.name, "Aiven");

        // 也可以在配置基础上直接覆盖来源（测试未保存的表单）。
        let request = BackupRequest {
            backup_config_id: Some(saved.id.clone()),
            source: Some(source()),
            database: None,
            schema: None,
            ..request
        };
        let resolved = resolve_backup(&config, &request).unwrap();
        assert_eq!(resolved.source.mode, "docker");
        assert_eq!(resolved.source.container, "postgres");
        assert_eq!(resolved.source.database, "app");

        // 配置里保存了自定义连接串时，请求显式指定的目标仍必须生效。
        let mut with_url = config.clone();
        with_url.backup_configs[0].supabase_url = Some("postgresql://saved@host/db".to_string());
        with_url.backup_targets = vec![
            BackupTarget::new("Aiven".to_string(), "postgresql://aiven@host/db".to_string()),
            BackupTarget::new("Neon".to_string(), "postgresql://neon@host/db".to_string()),
        ];
        let neon = with_url.backup_targets[1].id.clone();
        let request = BackupRequest {
            server_id: String::new(),
            backup_config_id: Some(saved.id.clone()),
            source: None,
            target_id: Some(neon),
            supabase_url: None,
            database: None,
            schema: None,
        };
        let resolved = resolve_backup(&with_url, &request).unwrap();
        assert_eq!(resolved.target.name, "Neon");

        // 未指定配置且服务器没有来源时报错。
        let empty = AppConfig {
            servers: vec![server],
            ..AppConfig::default()
        };
        let request = BackupRequest {
            server_id: empty.servers[0].id.clone(),
            backup_config_id: None,
            source: None,
            target_id: None,
            supabase_url: None,
            database: None,
            schema: None,
        };
        assert!(matches!(
            resolve_backup(&empty, &request),
            Err(CoreError::Config(_))
        ));

        // 不存在的配置报错。
        let request = BackupRequest {
            backup_config_id: Some("missing".to_string()),
            ..request
        };
        assert!(matches!(
            resolve_backup(&config, &request),
            Err(CoreError::NotFound(_))
        ));
    }

    /// 绑定的目标被删掉之后，宁可报错也不换库：脚本对新目标做的是 `DROP SCHEMA ... CASCADE`。
    #[test]
    fn saved_config_without_target_does_not_silently_swap_library() {
        use crate::models::{BackupConfig, SshAuth};

        let mut server = ServerConfig::new(
            "prod".to_string(),
            "h".to_string(),
            "u".to_string(),
            SshAuth::Password {
                password: "x".to_string(),
            },
        );
        // 老版本留下的两份可用连接串：删掉配置绑定的目标后，解析链本会落到这里。
        server.supabase_url = Some("postgresql://other@host/otherdb".to_string());
        let mut settings = Settings::default();
        settings.supabase_url = "postgresql://legacy@host/legacydb".to_string();

        let mut config = AppConfig {
            servers: vec![server],
            settings,
            ..AppConfig::default()
        };
        let mut saved = BackupConfig::new(
            "生产库".to_string(),
            config.servers[0].id.clone(),
            DbBackupSource {
                mode: "system".to_string(),
                database: "app".to_string(),
                username: "postgres".to_string(),
                ..DbBackupSource::default()
            },
        );
        // 保存环节把悬空的 target_id 清成了 None，配置自己也没有连接串。
        saved.target_id = None;
        config.backup_targets = vec![BackupTarget::new(
            "Aiven".to_string(),
            "postgresql://aiven@host/db".to_string(),
        )];
        config.backup_configs.push(saved.clone());

        let request = BackupRequest {
            server_id: String::new(),
            backup_config_id: Some(saved.id.clone()),
            source: None,
            target_id: None,
            supabase_url: None,
            database: None,
            schema: None,
        };
        let err = match resolve_backup(&config, &request) {
            Ok(_) => panic!("配置没有目标时必须报错，不许换库"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("生产库"),
            "要说清是哪条配置没有目标：{err}"
        );

        // 请求里显式带了目标时照旧放行 —— 拦的是「悄悄换库」，不是「用配置跑一次」。
        let resolved = resolve_backup(
            &config,
            &BackupRequest {
                server_id: String::new(),
                backup_config_id: Some(saved.id.clone()),
                source: None,
                target_id: Some(config.backup_targets[0].id.clone()),
                supabase_url: None,
                database: None,
                schema: None,
            },
        )
        .unwrap();
        assert_eq!(resolved.target.name, "Aiven");

        // 没建配置的旧路（单份来源）仍然允许用服务器 / 全局连接串兜底。
        let resolved = resolve_backup(
            &config,
            &BackupRequest {
                server_id: config.servers[0].id.clone(),
                backup_config_id: None,
                source: Some(config.backup_configs[0].source.clone()),
                target_id: None,
                supabase_url: None,
                database: None,
                schema: None,
            },
        )
        .unwrap();
        assert_eq!(resolved.target.url, "postgresql://other@host/otherdb");
    }
}
