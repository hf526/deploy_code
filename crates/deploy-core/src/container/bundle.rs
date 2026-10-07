//! 本机备份包管理：落地路径、`.part` 收尾守卫与成功后的轮转。
//!
//! `bundle_path` 的语义是「文件在这里」：`prepare` 在任务开始前就按计划写好了这个路径，
//! 之后的失败 / 取消 / 崩溃都可能让它指向一个从未落地的文件，[`clear_missing_bundle_path`]
//! 负责在收尾时抹空——界面「恢复到目标服务器」与轮转的保留窗口判的都是它。

use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};
use crate::models::{ContainerRecord, ContainerRecordKind};

use super::ContainerEngine;

impl ContainerEngine {
    /// 备份包落地位置：`<数据目录>/containers/<项目>-<时间>-<记录前缀>.tar`，免设置。
    pub fn bundle_path(&self, project: &str, record_id: &str) -> PathBuf {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let name = format!(
            "{}-{}-{}.tar",
            safe_component(project),
            stamp,
            short_id(record_id)
        );
        self.store.container_bundle_dir().join(name)
    }

    /// 备份包存放目录（界面展示与「打开目录」用）。
    pub fn bundle_dir(&self) -> PathBuf {
        self.store.container_bundle_dir()
    }

    /// 备份包轮转：同一个（服务器 + compose 项目）连本次成功任务在内只保留 `keep` 个包，
    /// 窗口之外的从磁盘删掉，并把那条记录里的 `bundle_path` 抹空——
    /// 界面的「恢复到目标服务器」入口就是按它判断有没有包可用，留着悬空路径只会让人点了才报错。
    ///
    /// 定时备份一晚落一个 GB 级文件，没有这一步会把系统盘写满。`keep` 为 0 表示完全不轮转。
    /// 只认 `<数据目录>/containers/` 下的直接子文件：老记录可能指向别的机器上的路径，
    /// 也可能被用户自己挪走了，那些一律跳过。
    pub(super) fn prune_bundles(
        &self,
        record: &ContainerRecord,
        keep: usize,
        limit: usize,
    ) -> Result<Vec<(String, u64)>> {
        if keep == 0 || record.server_id.is_empty() || record.project.is_empty() {
            return Ok(Vec::new());
        }
        let bundle_dir = self.store.container_bundle_dir();
        let mut records = self.store.load_containers()?;
        let mut stale: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.id != record.id
                    && item.kind != ContainerRecordKind::Restore
                    && item.server_id == record.server_id
                    && item.project == record.project
                    && !item.bundle_path.is_empty()
            })
            .map(|(index, _)| index)
            .collect();
        if stale.len() < keep {
            return Ok(Vec::new());
        }
        // started_at 是 "%Y-%m-%d %H:%M:%S"，字典序就是时间序；本次任务自己不在候选里，
        // 所以窗口按 keep - 1 算，新下来的这个包永远不会被自己挤掉。
        stale.sort_by(|a, b| records[*b].started_at.cmp(&records[*a].started_at));
        let mut touched: Vec<(ContainerRecord, PathBuf, String, u64)> = Vec::new();
        for index in stale.into_iter().skip(keep - 1) {
            let path = PathBuf::from(&records[index].bundle_path);
            if path.parent().map(|parent| parent != bundle_dir).unwrap_or(true) {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size = records[index].bundle_size;
            records[index].bundle_path = String::new();
            touched.push((records[index].clone(), path, name, size));
        }
        if touched.is_empty() {
            return Ok(Vec::new());
        }
        // 先把记录里的路径抹空，再删文件。反过来做的话，删完文件到写回记录之间崩了，盘上就留下一条
        // 指向不存在文件的 bundle_path —— 而界面「恢复到目标服务器」判的正是它，用户点了才报错。
        // 现在这个顺序最坏只是文件晚一步删除：它变成孤儿包，界面上的孤儿统计能看见它。
        for (record, _, _, _) in &touched {
            self.store.upsert_container(record, limit)?;
        }
        let mut removed = Vec::new();
        for (_, path, name, size) in touched {
            if !path.is_file() {
                continue;
            }
            // 删不掉（被别的进程占着）就留下这个文件：记录里的路径已经抹空，轮转不会再回头看它，
            // 但它会出现在设置页的「未认领的备份包」里，由用户手动清。
            if std::fs::remove_file(&path).is_err() {
                continue;
            }
            removed.push((name, size));
        }
        Ok(removed)
    }
}

pub(super) fn validate_bundle_path(value: &str) -> Result<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::config("请选择本机上的容器备份包"));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_file() {
        return Err(CoreError::not_found(format!(
            "备份包不存在: {}",
            path.display()
        )));
    }
    Ok(crate::process::canonicalize_path(&path))
}

pub(crate) fn bundle_part_path(bundle: &Path) -> PathBuf {
    let name = bundle
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("container.tar");
    bundle.with_file_name(format!("{name}.part"))
}

/// 收尾时用：记录里的备份包路径没有对应文件就抹空。
///
/// `prepare` 在任务开始前就按计划写好了这个路径，之后的失败 / 取消 / 崩溃都可能让它
/// 指向一个从未落地的文件。界面「恢复到目标服务器」与轮转的保留窗口判的都是它：
/// 留着悬空路径，用户点了才报错，轮转还会把它当成一个真实包占掉保留席位，
/// 于是本该留下的旧包被删掉。所以路径的语义就是「文件在这里」。
pub fn clear_missing_bundle_path(record: &mut ContainerRecord) {
    if !record.bundle_path.is_empty() && !Path::new(&record.bundle_path).is_file() {
        record.bundle_path.clear();
    }
}

/// `.part` 的收尾守卫：半途而废的下载文件必须跟着任务一起消失。
///
/// 只在 `Err` 分支里手动删不够 —— 取消是直接把整条 future drop 掉，那一段代码根本轮不到执行，
/// 半截包就一直堆在磁盘水位闸要保的那块盘上。改名到位之后 `disarm`。
#[derive(Debug)]
pub(crate) struct PartGuard {
    path: PathBuf,
    armed: bool,
}

impl PartGuard {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    /// 正式包已经改名出来了，这个 `.part` 路径不再代表半成品。
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PartGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn short_id(record_id: &str) -> String {
    record_id.chars().take(8).collect()
}

pub(crate) fn safe_component(value: &str) -> String {
    let text: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let text = text.trim_matches('.').to_string();
    if text.is_empty() {
        "stack".to_string()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AppConfig, DeployStatus, ServerConfig};
    use crate::store::Store;
    use std::sync::Arc;

    fn server(name: &str) -> ServerConfig {
        ServerConfig::new(
            name.to_string(),
            "10.0.0.1".to_string(),
            "root".to_string(),
            crate::models::SshAuth::Password {
                password: "x".to_string(),
            },
        )
    }

    /// 被取消是整条 future 被 drop，`Err` 分支那句删不到 —— 只能靠 Drop 顶上去。
    #[test]
    fn part_guard_removes_the_partial_when_dropped() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-container-part-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("bundle.tar.part");

        std::fs::write(&part, b"partial").unwrap();
        drop(PartGuard::new(part.clone()));
        assert!(!part.exists(), "放弃的任务不该在盘上留半截包");

        // 改名到位之后不能再回头删：那时这个路径已经是正式备份包。
        std::fs::write(&part, b"complete").unwrap();
        let mut guard = PartGuard::new(part.clone());
        guard.disarm();
        drop(guard);
        assert!(part.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 失败 / 取消收尾要把「计划路径但没落文件」的悬空路径抹空：
    /// 界面按它显示恢复入口，轮转也会把它当成真实包占掉一个保留席位。
    #[test]
    fn missing_bundle_file_is_cleared_from_the_record() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-container-bundle-path-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("done.tar");
        std::fs::write(&existing, b"tar").unwrap();

        let mut record = ContainerRecord {
            kind: ContainerRecordKind::Backup,
            id: "r1".to_string(),
            project: "blog".to_string(),
            server_id: "s1".to_string(),
            server_name: "prod".to_string(),
            target_server_id: String::new(),
            target_server_name: String::new(),
            target_dir: String::new(),
            bundle_path: dir.join("never-written.tar").to_string_lossy().into_owned(),
            bundle_size: 0,
            services: Vec::new(),
            volumes: Vec::new(),
            images: Vec::new(),
            include_volumes: true,
            include_images: true,
            status: DeployStatus::Running,
            error: None,
            log: String::new(),
            started_at: String::new(),
            finished_at: None,
            duration_ms: 0,
        };
        clear_missing_bundle_path(&mut record);
        assert_eq!(record.bundle_path, "", "没落地的计划路径要抹空");

        record.bundle_path = existing.to_string_lossy().into_owned();
        clear_missing_bundle_path(&mut record);
        assert_eq!(
            record.bundle_path,
            existing.to_string_lossy(),
            "文件真在的路径不许动"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 备份包轮转：同一个（服务器 + 项目）只留最近 keep 个，
    /// 多出来的删文件并把记录里的 `bundle_path` 抹空；目录外的路径与别的项目一律不动。
    #[test]
    fn prune_bundles_keeps_the_newest_and_blanks_the_rest() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-container-prune-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Arc::new(Store::new(&dir));
        let engine = ContainerEngine::new(store.clone());
        let bundles = store.container_bundle_dir();
        std::fs::create_dir_all(&bundles).unwrap();
        // 故意放在备份目录之外：轮转不许删用户自己挪走的包。
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let prod = server("prod");
        let prod_id = prod.id.clone();
        let mut config = AppConfig::default();
        config.servers.push(prod);
        store.save_config(&config).unwrap();

        let bundle = |dir: &Path, name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, b"tar").unwrap();
            path
        };
        let record = |id: &str, project: &str, started_at: &str, path: &Path| ContainerRecord {
            kind: ContainerRecordKind::Backup,
            id: id.to_string(),
            project: project.to_string(),
            server_id: prod_id.clone(),
            server_name: "prod".to_string(),
            target_server_id: String::new(),
            target_server_name: String::new(),
            target_dir: String::new(),
            bundle_path: path.to_string_lossy().into_owned(),
            bundle_size: 3,
            services: Vec::new(),
            volumes: Vec::new(),
            images: Vec::new(),
            include_volumes: true,
            include_images: true,
            status: DeployStatus::Success,
            error: None,
            log: String::new(),
            started_at: started_at.to_string(),
            finished_at: None,
            duration_ms: 0,
        };

        let moved = bundle(&elsewhere, "blog-moved.tar");
        let oldest = bundle(&bundles, "blog-0.tar");
        let older = bundle(&bundles, "blog-1.tar");
        let mid = bundle(&bundles, "blog-2.tar");
        let newest_old = bundle(&bundles, "blog-3.tar");
        let other_project = bundle(&bundles, "api-0.tar");
        let current = bundle(&bundles, "blog-cur.tar");

        let history = vec![
            record("moved", "blog", "2026-09-20 03:30:00", &moved),
            record("oldest", "blog", "2026-09-21 03:30:00", &oldest),
            record("older", "blog", "2026-09-22 03:30:00", &older),
            record("mid", "blog", "2026-09-23 03:30:00", &mid),
            record("newest", "blog", "2026-09-24 03:30:00", &newest_old),
            record("api", "api", "2026-09-25 03:30:00", &other_project),
        ];
        for item in &history {
            store.upsert_container(item, 100).unwrap();
        }
        let running = record("cur", "blog", "2026-09-27 03:30:00", &current);

        // keep = 0 表示关掉轮转：一个都不许动。
        assert!(engine
            .prune_bundles(&running, 0, 100)
            .unwrap()
            .is_empty());
        assert!(oldest.is_file() && moved.is_file());

        // keep = 2：本次这个占一席，历史里只留最新的一个。
        let removed = engine.prune_bundles(&running, 2, 100).unwrap();
        let names: Vec<String> = removed.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(names, vec!["blog-2.tar", "blog-1.tar", "blog-0.tar"]);
        assert!(!oldest.is_file() && !older.is_file() && !mid.is_file());
        assert!(newest_old.is_file(), "窗口内的最新历史包要留下");
        assert!(current.is_file(), "刚做出来的包不该被自己挤掉");
        assert!(other_project.is_file(), "别的项目的包不在轮转范围");
        assert!(moved.is_file(), "备份目录外的文件一律不碰");

        let stored = store.load_containers().unwrap();
        let path_of = |id: &str| -> String {
            stored
                .iter()
                .find(|item| item.id == id)
                .expect("记录应还在")
                .bundle_path
                .clone()
        };
        // 记录留在列表里（历史与日志还有用），只是不再指向一个已经不存在的文件。
        assert_eq!(path_of("oldest"), "");
        assert_eq!(path_of("older"), "");
        assert_eq!(path_of("mid"), "");
        assert_eq!(path_of("newest"), newest_old.to_string_lossy());
        assert_eq!(path_of("moved"), moved.to_string_lossy());
        assert_eq!(path_of("api"), other_project.to_string_lossy());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
