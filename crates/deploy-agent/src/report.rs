//! 回读通道：`status` 与 `records` 两个子命令的输出。
//!
//! 两者都往 stdout 打**一个 JSON 文档**（不是事件流），客户端一次 `ssh exec` 拿完就断线。
//! 之所以由 agent 来拼这份 JSON 而不是让客户端 `cat` 配置文件再自己解析：控制机上那份
//! `config.json` 含全部服务器的口令，回读只给摘要，凭据一个字节都不往回传。

use std::path::Path;

use deploy_core::agent::{disk_summary, AgentStatus, PROTO};
use deploy_core::{CoreError, Result, Store};

/// `deploy-agent status`：一份 JSON 文档进 stdout。
pub fn status(store: &Store) -> Result<()> {
    println!("{}", serde_json::to_string(&build_status(store)?)?);
    Ok(())
}

/// 拼状态。单独成函数是为了让它可测：打印那条路径没什么好测的。
pub fn build_status(store: &Store) -> Result<AgentStatus> {
    let now = chrono::Local::now();
    let config = store.load_config()?;
    let settings = &config.settings;
    let state = store.load_schedule_state();
    let (total_bytes, free_bytes, floor_bytes) = disk_summary(store.base_dir())?;
    let (bundle_bytes, bundle_count) = bundle_stats(store.base_dir());

    let backup_config_name = settings
        .scheduled_backup_config_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .and_then(|key| {
            config
                .backup_configs
                .iter()
                .find(|item| item.id == key || item.name == key)
                .map(|item| item.name.clone())
        })
        .unwrap_or_default();

    Ok(AgentStatus {
        proto: PROTO,
        version: env!("CARGO_PKG_VERSION").to_string(),
        // 定时里的 HH:MM 按控制机的本地时区解释，界面必须把这两项摊开给用户看，
        // 否则「设了 03:00 结果凌晨四点才跑」这种事没人能从本机这边查出来。
        timezone: now.format("%z").to_string(),
        local_time: now.format("%Y-%m-%d %H:%M:%S").to_string(),
        data_dir: store.base_dir().display().to_string(),
        total_bytes,
        free_bytes,
        floor_bytes,
        bundle_bytes,
        bundle_count,
        servers: config.servers.len(),
        backup_configs: config.backup_configs.len(),
        container_configs: config.container_configs.len(),
        backup_enabled: settings.scheduled_backup_enabled,
        backup_time: settings.scheduled_backup_time.clone(),
        backup_config_name,
        backup_last_run: state.backup_last_run.clone(),
        container_enabled: settings.scheduled_container_enabled,
        container_time: settings.scheduled_container_time.clone(),
        container_queue: settings.scheduled_container_config_ids.len(),
        container_last_run: state.container_last_run.clone(),
    })
}

/// `deploy-agent records --kind backup|container [--limit N]`。
pub fn records(store: &Store, args: &[String]) -> Result<()> {
    let kind = crate::flag(args, "--kind")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CoreError::config("records 需要 --kind backup|container"))?;
    let limit = crate::usize_flag(args, "--limit", 50).clamp(1, 1000);
    let json = match kind.trim().to_ascii_lowercase().as_str() {
        "backup" | "db" => {
            let mut list = store.load_backups()?;
            list.reverse();
            list.truncate(limit);
            serde_json::to_string(&list)?
        }
        "container" => {
            let mut list = store.load_containers()?;
            list.reverse();
            list.truncate(limit);
            serde_json::to_string(&list)?
        }
        other => {
            return Err(CoreError::config(format!(
                "--kind 只接受 backup 或 container，收到: {other}"
            )))
        }
    };
    println!("{json}");
    Ok(())
}

/// 备份包目录的占用统计（只算这两个目录的直接子文件，与轮转的口径一致）。
fn bundle_stats(base: &Path) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut count = 0usize;
    for dir in ["containers", "backups"] {
        let path = base.join(dir);
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_file() {
                bytes += meta.len();
                count += 1;
            }
        }
    }
    (bytes, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::models::{BackupConfig, BackupTarget, RunLocation, ServerConfig, SshAuth};
    use deploy_core::schedule::ScheduleState;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-agent-report-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_store() -> (Store, std::path::PathBuf) {
        let dir = temp_dir();
        (Store::new(&dir), dir)
    }

    fn server(id: &str) -> ServerConfig {
        let mut item = ServerConfig::new(
            id.to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            SshAuth::Password {
                password: "p".to_string(),
            },
        );
        item.id = id.to_string();
        item
    }

    /// 客户端按 `AgentStatus` 反序列化，所以字段名与类型必须能原样往返一次。
    fn round_trip(status: &AgentStatus) -> AgentStatus {
        serde_json::from_str(&serde_json::to_string(status).unwrap()).unwrap()
    }

    fn sample_status() -> AgentStatus {
        AgentStatus {
            proto: PROTO,
            version: "0.0.0".to_string(),
            timezone: "+0800".to_string(),
            local_time: "2026-09-27 03:00:00".to_string(),
            data_dir: "/var/lib/deploycode".to_string(),
            total_bytes: 100,
            free_bytes: 40,
            floor_bytes: 10,
            bundle_bytes: 7,
            bundle_count: 2,
            servers: 3,
            backup_configs: 1,
            container_configs: 2,
            backup_enabled: true,
            backup_time: "03:00".to_string(),
            backup_config_name: "主库".to_string(),
            backup_last_run: "2026-09-27".to_string(),
            container_enabled: false,
            container_time: "03:30".to_string(),
            container_queue: 0,
            container_last_run: String::new(),
        }
    }

    #[test]
    fn status_json_round_trips_through_the_client_shape() {
        let parsed = round_trip(&sample_status());
        assert_eq!(parsed.proto, PROTO);
        assert_eq!(parsed.backup_config_name, "主库");
        assert_eq!(parsed.timezone, "+0800");
        assert_eq!(parsed.container_last_run, "");
    }

    #[test]
    fn status_reports_the_agent_schedule_state_not_the_pushed_settings() {
        let (store, dir) = temp_store();
        let mut config = deploy_core::models::AppConfig::default();
        config.servers = vec![server("s1")];
        config.backup_targets = vec![BackupTarget::new(
            "supabase".to_string(),
            "postgres://u:p@h/db".to_string(),
        )];
        config.backup_configs = vec![BackupConfig {
            id: "b1".to_string(),
            name: "主库".to_string(),
            server_id: "s1".to_string(),
            source: deploy_core::models::DbBackupSource::default(),
            target_id: None,
            supabase_url: None,
            run_location: RunLocation::Remote,
        }];
        config.settings.scheduled_backup_enabled = true;
        config.settings.scheduled_backup_time = "02:15".to_string();
        config.settings.scheduled_backup_config_id = Some("主库".to_string());
        // 客户端下发的那份日期是客户端自己的，agent 必须报它本地记的那份。
        config.settings.scheduled_backup_last_run = "2020-01-01".to_string();
        store.save_config(&config).unwrap();
        store
            .save_schedule_state(&ScheduleState {
                backup_last_run: "2026-09-26".to_string(),
                container_last_run: String::new(),
            })
            .unwrap();

        let status = build_status(&store).unwrap();
        assert_eq!(status.backup_last_run, "2026-09-26");
        assert_ne!(status.backup_last_run, "2020-01-01");
        // 定时摘要取的是下发过来的那份设置，配置名按 id 或名称都要能认出来。
        assert!(status.backup_enabled);
        assert_eq!(status.backup_time, "02:15");
        assert_eq!(status.backup_config_name, "主库");
        assert_eq!(status.servers, 1);
        assert_eq!(status.backup_configs, 1);
        assert_eq!(status.proto, PROTO);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bundle_stats_counts_only_direct_files() {
        let dir = temp_dir();
        let bundles = dir.join("containers");
        std::fs::create_dir_all(bundles.join("nested")).unwrap();
        std::fs::write(bundles.join("a.tar"), vec![1u8; 12]).unwrap();
        std::fs::write(bundles.join("nested").join("b.tar"), vec![1u8; 99]).unwrap();
        let (bytes, count) = bundle_stats(&dir);
        assert_eq!((bytes, count), (12, 1), "子目录里的东西不算在轮转口径里");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn records_rejects_an_unknown_kind() {
        let (store, dir) = temp_store();
        let args: Vec<String> = vec!["--kind".to_string(), "pages".to_string()];
        let err = records(&store, &args).unwrap_err().to_string();
        assert!(err.contains("backup"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
