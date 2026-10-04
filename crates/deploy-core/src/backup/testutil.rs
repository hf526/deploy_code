//! 备份模块测试共用的 fixture（仅测试构建编译）。

use crate::models::{BackupRecord, DbBackupSource, DeployStatus};

pub(crate) fn source() -> DbBackupSource {
    DbBackupSource {
        mode: "docker".to_string(),
        container: "postgres".to_string(),
        database: "app".to_string(),
        username: "postgres".to_string(),
        password: "p'ass".to_string(),
        schema: "public".to_string(),
    }
}

pub(crate) fn record() -> BackupRecord {
    BackupRecord {
        id: "abc".to_string(),
        server_id: "s1".to_string(),
        server_name: "prod".to_string(),
        database: "app".to_string(),
        schema: "public".to_string(),
        target_name: "Supabase".to_string(),
        target: "postgresql://postgres:***@db.example.com:5432/postgres".to_string(),
        status: DeployStatus::Running,
        error: None,
        log: String::new(),
        dump_size: 0,
        bundle_path: String::new(),
        started_at: String::new(),
        finished_at: None,
        duration_ms: 0,
    }
}
