//! 孤儿备份包清理：`deploy-agent prune [--list]`。
//!
//! 控制机是无头的，记录按 `*_history_limit` 裁掉之后残留的包没有任何界面可以清 ——
//! 这个子命令就是那条出口，由客户端经 SSH exec 临时调起。判据与本机设置页的
//! 「未认领备份包」完全一致（[`Store::orphan_bundles`] / [`Store::delete_orphan_bundles`]）：
//! 只认两个备份包目录的直接子文件、删前重核记录、备份或容器任务在跑时拒绝
//! （Busy → 退出码 3，客户端必须还原成 `CoreError::busy`）。
//!
//! 输出与 `status` / `records` 同一条约定：stdout 只有一个 JSON 文档，人读的报错走 stderr。

use deploy_core::agent::AgentPruneReport;
use deploy_core::{Result, Store};

/// `deploy-agent prune [--list]`。
pub fn command(store: &Store, args: &[String]) -> Result<()> {
    if crate::bool_flag(args, "--list") {
        println!("{}", serde_json::to_string(&store.orphan_bundles()?)?);
    } else {
        println!("{}", serde_json::to_string(&prune_report(store)?)?);
    }
    Ok(())
}

/// 算出并删掉所有未认领的备份包。单独成函数是为了让它可测：打印那条路径没什么好测的。
///
/// 先把孤儿列出来，再交给 `delete_orphan_bundles`（它内部会重核记录并自己拿任务锁）。
fn prune_report(store: &Store) -> Result<AgentPruneReport> {
    let paths: Vec<String> = store
        .orphan_bundles()?
        .into_iter()
        .map(|item| item.path)
        .collect();
    let (deleted, freed) = store.delete_orphan_bundles(&paths)?;
    Ok(AgentPruneReport {
        deleted,
        freed_bytes: freed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deploy_core::models::{BackupRecord, DeployStatus};
    use deploy_core::{CoreError, Store};

    fn temp_store() -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-agent-prune-{}",
            uuid::Uuid::new_v4()
        ));
        (Store::new(&dir), dir)
    }

    fn record(id: &str, bundle_path: &str) -> BackupRecord {
        BackupRecord {
            id: id.to_string(),
            server_id: "s1".to_string(),
            server_name: "源机".to_string(),
            database: "app".to_string(),
            schema: "public".to_string(),
            target_name: "supabase".to_string(),
            target: "postgres://u:***@h/db".to_string(),
            status: DeployStatus::Success,
            error: None,
            log: String::new(),
            dump_size: 10,
            bundle_path: bundle_path.to_string(),
            started_at: "2026-10-05 03:00:00".to_string(),
            finished_at: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn prune_deletes_orphans_and_keeps_referenced_bundles() {
        let (store, dir) = temp_store();
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("kept.sql.gz"), vec![1u8; 7]).unwrap();
        std::fs::write(backups.join("orphan.sql.gz"), vec![2u8; 11]).unwrap();
        std::fs::write(backups.join("half.sql.gz.part"), vec![3u8; 5]).unwrap();
        let kept = backups.join("kept.sql.gz").canonicalize().unwrap();
        store
            .upsert_backup(&record("r1", &kept.display().to_string()), 50)
            .unwrap();

        // 孤儿清单只报没人认领的：被记录引用的包与进行中的 .part 都不算，且能按
        // 客户端那边的形状（camelCase 的 OrphanBundle）原样往返。
        let orphans = store.orphan_bundles().unwrap();
        assert_eq!(orphans.len(), 1, "{orphans:?}");
        assert_eq!(orphans[0].file_name, "orphan.sql.gz");
        let json = serde_json::to_string(&orphans).unwrap();
        assert!(json.contains("\"fileName\""), "{json}");
        assert!(serde_json::from_str::<
        Vec<deploy_core::store::OrphanBundle>,
        >(&json)
        .is_ok());

        // 执行清理：孤儿删掉，引用中的包留在原地。
        let report = prune_report(&store).unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(report.freed_bytes, 11);
        assert!(backups.join("kept.sql.gz").is_file());
        assert!(!backups.join("orphan.sql.gz").exists());
        // 没人管的 .part 不归这条命令管（对账清扫 sweep_orphan_part_files 的地盘）。
        assert!(backups.join("half.sql.gz.part").is_file());
        // 报告能按客户端的形状原样往返（freed_bytes 必须是 freedBytes）。
        let round = serde_json::from_str::<AgentPruneReport>(
            &serde_json::to_string(&report).unwrap(),
        )
        .unwrap();
        assert_eq!((round.deleted, round.freed_bytes), (1, 11));
        // 再跑一遍：已经没有可清的，报告归零。
        let again = prune_report(&store).unwrap();
        assert_eq!((again.deleted, again.freed_bytes), (0, 0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prune_refuses_while_a_task_lock_is_held() {
        let (store, dir) = temp_store();
        let containers = dir.join("containers");
        std::fs::create_dir_all(&containers).unwrap();
        std::fs::write(containers.join("orphan.tar"), vec![1u8; 3]).unwrap();
        // 同一数据目录的另一个进程（daemon 正在跑任务）握着容器锁：此刻删包会把
        // 还没写进记录的半截包当成孤儿，必须拒绝而不是继续。
        let _held = Store::new(&dir).try_task_lock("container").unwrap().unwrap();
        let err = prune_report(&store).unwrap_err();
        assert!(matches!(err, CoreError::Busy(_)), "{err}");
        assert!(containers.join("orphan.tar").is_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
