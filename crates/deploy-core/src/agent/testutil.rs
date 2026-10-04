//! agent 模块测试共用的 fixture（仅测试构建编译）。

use std::path::PathBuf;

use crate::models::{
    BackupConfig, ContainerConfig, ContainerTarget, DbBackupSource, RunLocation, ServerConfig,
    SshAuth,
};

    pub(crate) fn server(id: &str, name: &str) -> ServerConfig {
        let mut item = ServerConfig::new(
            name.to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::Password {
                password: "secret".to_string(),
            },
        );
        item.id = id.to_string();
        item
    }

    pub(crate) fn backup_config(id: &str, name: &str, remote: bool) -> BackupConfig {
        BackupConfig {
            id: id.to_string(),
            name: name.to_string(),
            server_id: "s1".to_string(),
            source: DbBackupSource::default(),
            target_id: None,
            supabase_url: None,
            run_location: if remote {
                RunLocation::Remote
            } else {
                RunLocation::Local
            },
        }
    }

    pub(crate) fn container_config(id: &str, name: &str, remote: bool) -> ContainerConfig {
        ContainerConfig {
            id: id.to_string(),
            name: name.to_string(),
            server_id: "s1".to_string(),
            project: name.to_string(),
            pause_source: false,
            include_volumes: true,
            include_images: true,
            target: Some(ContainerTarget {
                server_id: "s2".to_string(),
                target_dir: "/srv/app".to_string(),
                start_services: true,
            }),
            created_at: String::new(),
            run_location: if remote {
                RunLocation::Remote
            } else {
                RunLocation::Local
            },
        }
    }

    pub(crate) fn tempfile() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-agent-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
