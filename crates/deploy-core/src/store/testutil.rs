//! store 模块测试共用的 fixture（仅测试构建编译）。

use std::path::PathBuf;

use super::Store;
use crate::models::{
AppConfig, BackupRecord, DeployRecord, DeployStatus, EnvFileConfig, ExportData,
PagesDeployRecord, RepoConfig, ServerConfig, Settings, SshAuth,
};

pub(crate) fn temp_store() -> (Store, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "deploycode-import-{}",
        uuid::Uuid::new_v4()
    ));
    (Store::new(&dir), dir)
}

/// 带真实凭据的配置：密码认证服务器 + env 文件仓库 + 两个 API Token。
pub(crate) fn config_with_secrets() -> AppConfig {
    let mut server = ServerConfig::new(
        "prod".to_string(),
        "10.0.0.1".to_string(),
        "root".to_string(),
        SshAuth::Password {
            password: "s3cret".to_string(),
        },
    );
    server.id = "s1".to_string();
    let mut repo = RepoConfig::new("web".to_string(), "D:/web".to_string());
    repo.id = "r1".to_string();
    repo.env_files.push(EnvFileConfig {
        local_path: "D:/web/.env".to_string(),
        remote_path: ".env".to_string(),
    });
    let mut settings = Settings::default();
    settings.github_token = "gh_secret".to_string();
    settings.cronjob_api_key = "cj_secret".to_string();
    AppConfig {
        servers: vec![server],
        repos: vec![repo],
        settings,
        ..Default::default()
    }
}

/// 只替换认证信息的导出文件，其它字段用来验证「非敏感字段跟随文件」。
pub(crate) fn export_with_server(server: ServerConfig) -> String {
    let data = ExportData {
        version: "1.0".to_string(),
        exported_at: "2026-01-01 00:00:00".to_string(),
        servers: vec![server],
        repos: Vec::new(),
        backup_targets: Vec::new(),
        deploy_configs: Vec::new(),
        backup_configs: Vec::new(),
        pages_configs: Vec::new(),
        container_configs: Vec::new(),
        settings: Settings::default(),
    };
    serde_json::to_string(&data).unwrap()
}

pub(crate) fn history_record(id: &str, status: DeployStatus) -> DeployRecord {
    let status = serde_json::to_string(&status).unwrap();
    serde_json::from_str(&format!(
        r#"{{"id":"{id}","repoId":"repo","repoName":"demo","rev":"main","branch":"main",
        "commit":"abc","commitShort":"abc","commitSubject":"init","serverId":"srv",
        "serverName":"prod","targetDir":"/srv/app","scriptDir":"docker","scripts":[],
        "runScripts":false,"envFiles":[],"status":{status},"error":null,"log":"",
        "startedAt":"2026-01-01 00:00:00","finishedAt":null,"durationMs":0}}"#
    ))
    .unwrap()
}

pub(crate) fn backup_record(id: &str, status: DeployStatus) -> BackupRecord {
    BackupRecord {
        id: id.to_string(),
        server_id: "srv".to_string(),
        server_name: "prod".to_string(),
        database: "app".to_string(),
        schema: "public".to_string(),
        target_name: String::new(),
        target: "postgresql://u@h/db".to_string(),
        status,
        error: None,
        log: String::new(),
        dump_size: 0,
        bundle_path: String::new(),
        started_at: "2026-01-01 00:00:00".to_string(),
        finished_at: None,
        duration_ms: 0,
    }
}

pub(crate) fn pages_record(id: &str, status: DeployStatus) -> PagesDeployRecord {
    PagesDeployRecord {
        id: id.to_string(),
        provider: "cloudflare".to_string(),
        repo_id: "repo".to_string(),
        repo_name: "demo".to_string(),
        project_name: "demo".to_string(),
        branch: "main".to_string(),
        commit: "abc".to_string(),
        commit_short: "abc".to_string(),
        status,
        error: None,
        log: String::new(),
        url: None,
        started_at: "2026-01-01 00:00:00".to_string(),
        finished_at: None,
        duration_ms: 0,
    }
}
