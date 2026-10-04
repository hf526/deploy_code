//! 四类任务记录（部署 / 备份 / Pages / 容器）的 CRUD、中断收敛与孤儿备份包。
//!
//! 四类记录共用同一种「JSON 列表 + upsert 截断」的落盘方式：泛型层（`TaskRecord` trait
//! 与 `load_records` / `upsert_record` 等）只此一份，四组公开方法各自绑定类型与文件路径。

use std::path::{Path, PathBuf};

use super::{atomic_write, Store};
use crate::error::{CoreError, Result};
use crate::models::{
    BackupRecord, ContainerRecord, DeployRecord, DeployStatus, PagesDeployRecord,
    now_string,
};

/// 一个「盘上有文件、记录里已经没有指向它」的本地备份包。见 [`Store::orphan_bundles`]。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanBundle {
    /// `"database"` 或 `"container"`：两个目录分开列，用户才知道删的是哪一类。
    pub kind: String,
    pub file_name: String,
    pub path: String,
    pub size_bytes: u64,
}

impl Store {
    pub fn load_history(&self) -> Result<Vec<DeployRecord>> {
        load_records(&self.history_path(), HISTORY_LABEL)
    }

    /// 新增或更新一条部署记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_history(&self, record: &DeployRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.history_path(), record, limit, HISTORY_LABEL)
    }

    pub fn remove_history(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<DeployRecord>(&self.history_path(), id, HISTORY_LABEL)
    }

    pub fn remove_history_many(&self, ids: &[String]) -> Result<usize> {
        let _guard = self.write_guard()?;
        remove_records_by_id::<DeployRecord>(&self.history_path(), ids, HISTORY_LABEL)
    }

    pub fn clear_history(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<DeployRecord>(&self.history_path(), &[])
    }

    pub fn find_record(&self, id: &str) -> Result<DeployRecord> {
        load_records::<DeployRecord>(&self.history_path(), HISTORY_LABEL)?
            .into_iter()
            .find(|r| r.id == id)
            .ok_or_else(|| CoreError::not_found(format!("部署记录不存在: {id}")))
    }

    pub fn load_backups(&self) -> Result<Vec<BackupRecord>> {
        load_records(&self.backups_path(), BACKUP_LABEL)
    }

    /// 新增或更新一条备份记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_backup(&self, record: &BackupRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.backups_path(), record, limit, BACKUP_LABEL)
    }

    pub fn find_backup(&self, id: &str) -> Result<BackupRecord> {
        find_record_by_prefix(&self.backups_path(), id, BACKUP_LABEL, BACKUP_LABEL)
    }

    pub fn remove_backup(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<BackupRecord>(&self.backups_path(), id, BACKUP_LABEL)
    }

    pub fn remove_backups_many(&self, ids: &[String]) -> Result<usize> {
        let _guard = self.write_guard()?;
        remove_records_by_id::<BackupRecord>(&self.backups_path(), ids, BACKUP_LABEL)
    }

    pub fn clear_backups(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<BackupRecord>(&self.backups_path(), &[])
    }

    pub fn load_pages_records(&self) -> Result<Vec<PagesDeployRecord>> {
        load_records(&self.pages_path(), PAGES_LABEL)
    }

    /// 新增或更新一条 Pages 部署记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_pages_record(&self, record: &PagesDeployRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.pages_path(), record, limit, PAGES_LABEL)
    }

    pub fn find_pages_record(&self, id: &str) -> Result<PagesDeployRecord> {
        find_record_by_prefix(&self.pages_path(), id, PAGES_LABEL, PAGES_MISSING_LABEL)
    }

    pub fn remove_pages_record(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<PagesDeployRecord>(&self.pages_path(), id, PAGES_LABEL)
    }

    pub fn remove_pages_records_many(&self, ids: &[String]) -> Result<usize> {
        let _guard = self.write_guard()?;
        remove_records_by_id::<PagesDeployRecord>(&self.pages_path(), ids, PAGES_LABEL)
    }

    pub fn clear_pages_records(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<PagesDeployRecord>(&self.pages_path(), &[])
    }

    pub fn load_containers(&self) -> Result<Vec<ContainerRecord>> {
        load_records(&self.containers_path(), CONTAINER_LABEL)
    }

    /// 新增或更新一条容器任务记录（按 id 匹配），并自动裁剪历史长度。
    pub fn upsert_container(&self, record: &ContainerRecord, limit: usize) -> Result<()> {
        let _guard = self.write_guard()?;
        upsert_record(&self.containers_path(), record, limit, CONTAINER_LABEL)
    }

    pub fn find_container_record(&self, id: &str) -> Result<ContainerRecord> {
        find_record_by_prefix(&self.containers_path(), id, CONTAINER_LABEL, CONTAINER_LABEL)
    }

    pub fn remove_container_record(&self, id: &str) -> Result<bool> {
        let _guard = self.write_guard()?;
        remove_record::<ContainerRecord>(&self.containers_path(), id, CONTAINER_LABEL)
    }

    /// 清空记录只删本地的任务历史，备份包文件留在原处（删记录不该带走数据）。
    pub fn clear_container_records(&self) -> Result<()> {
        let _guard = self.write_guard()?;
        write_records::<ContainerRecord>(&self.containers_path(), &[])
    }

    /// 列出两个备份包目录里「盘上有文件、记录中已经没有指向它」的包，按大小从大到小。
    ///
    /// 出现的原因有两个，都不是 bug：记录列表按 `*_history_limit` 裁剪（[`upsert_record`]），
    /// 以及界面上的删除记录 / 清空刻意不带走备份包。问题是轮转的候选集完全来自记录，
    /// 所以这些包从此再没有代码路径会去删 —— 这个视图把它们显出来，交给用户手动清。
    /// 正在写的半截包（`*.part`）不算，那属于进行中的任务。
    pub fn orphan_bundles(&self) -> Result<Vec<OrphanBundle>> {
        let referenced = self.referenced_bundle_keys()?;
        let mut found = Vec::new();
        for (kind, dir) in [
            ("database", self.db_bundle_dir()),
            ("container", self.container_bundle_dir()),
        ] {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().into_owned();
                if !path.is_file() || file_name.ends_with(".part") {
                    continue;
                }
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                if referenced.0.contains(&canonical) || referenced.1.contains(&file_name) {
                    continue;
                }
                found.push(OrphanBundle {
                    kind: kind.to_string(),
                    file_name,
                    path: path.display().to_string(),
                    size_bytes: path.metadata().map(|m| m.len()).unwrap_or(0),
                });
            }
        }
        found.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
        Ok(found)
    }

    /// 删掉这些孤儿包，返回删掉的个数与释放的字节数。
    ///
    /// 三条硬边界：① 只认这两个目录的**直接子文件**，路径不对就整批拒绝（不静默跳过，
    /// 免得界面以为清干净了）；② 删之前重新核一遍记录，已经被人补回去的包不许删；
    /// ③ 备份或容器任务在跑时拒绝执行 —— 刚生成的包还没写进记录，此刻会被当成孤儿删掉。
    pub fn delete_orphan_bundles(&self, paths: &[String]) -> Result<(usize, u64)> {
        let _backup = self
            .try_task_lock("backup")?
            .ok_or_else(|| CoreError::busy("数据库备份正在进行，请等它结束后再清理备份包"))?;
        let _container = self
            .try_task_lock("container")?
            .ok_or_else(|| CoreError::busy("容器备份 / 迁移正在进行，请等它结束后再清理备份包"))?;
        let dirs = [self.db_bundle_dir(), self.container_bundle_dir()];
        let referenced = self.referenced_bundle_keys()?;
        let mut deleted = 0usize;
        let mut freed = 0u64;
        for raw in paths {
            let path = PathBuf::from(raw.trim());
            let parent = match path.parent() {
                Some(parent) => parent.to_path_buf(),
                None => {
                    return Err(CoreError::config(format!(
                        "只能删除备份包目录里的文件: {}",
                        path.display()
                    )))
                }
            };
            if !dirs.iter().any(|dir| dir.as_path() == parent.as_path()) {
                return Err(CoreError::config(format!(
                    "只能删除备份包目录里的文件: {}",
                    path.display()
                )));
            }
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if referenced.0.contains(&canonical) || referenced.1.contains(&file_name) {
                return Err(CoreError::config(format!(
                    "「{file_name}」还挂在某条记录上，不能删。请先确认那条记录是否还在。"
                )));
            }
            let size = path.metadata().map(|m| m.len()).unwrap_or(0);
            match std::fs::remove_file(&path) {
                Ok(_) => {
                    deleted += 1;
                    freed += size;
                }
                // 文件已经不在了（用户自己删了、或另一个进程收掉了）：不算失败，也不算释放。
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(CoreError::io_path(&path, err)),
            }
        }
        Ok((deleted, freed))
    }

    /// 现有记录指向的包：一份 canonical 路径集合 + 一份文件名集合。
    ///
    /// 两道判据是为了安全方向上保守：路径写法不一致（`/` 与 `\`、映射盘符）时按文件名也能认出
    /// 「这不是孤儿」，宁可漏报孤儿，也不能把一个还有效的包摆到清理列表里。
    fn referenced_bundle_keys(&self) -> Result<(std::collections::HashSet<PathBuf>, std::collections::HashSet<String>)> {
        let mut paths = std::collections::HashSet::new();
        let mut names = std::collections::HashSet::new();
        let mut collect = |raw: &str| {
            if raw.trim().is_empty() {
                return;
            }
            let path = PathBuf::from(raw);
            if let Some(name) = path.file_name() {
                names.insert(name.to_string_lossy().into_owned());
            }
            paths.insert(path.canonicalize().unwrap_or_else(|_| path.clone()));
        };
        for record in self.load_backups()? {
            collect(&record.bundle_path);
        }
        for record in self.load_containers()? {
            collect(&record.bundle_path);
        }
        Ok((paths, names))
    }


    /// 启动时把上次异常退出（崩溃/强杀）遗留的 Running 记录收敛为失败，
    /// 避免历史里永远显示"进行中"且无法重新部署。
    /// 全程持有写守卫，防止与并发写（如 CLI 完成任务）互相覆盖。
    pub fn mark_interrupted(&self) -> Result<usize> {
        let _guard = self.write_guard()?;
        let message = "任务被中断（应用退出或崩溃）";
        let mut converted = mark_running_as_interrupted::<DeployRecord>(
            &self.history_path(),
            HISTORY_LABEL,
            message,
        )?;
        converted += mark_running_as_interrupted::<BackupRecord>(
            &self.backups_path(),
            BACKUP_LABEL,
            message,
        )?;
        converted += mark_running_as_interrupted::<PagesDeployRecord>(
            &self.pages_path(),
            PAGES_LABEL,
            message,
        )?;
        converted += mark_running_as_interrupted::<ContainerRecord>(
            &self.containers_path(),
            CONTAINER_LABEL,
            message,
        )?;
        Ok(converted)
    }

    /// 仅当没有其他进程正在执行部署 / 备份 / Pages / 容器任务时，才收敛遗留的 Running 记录。
    /// GUI 启动和 CLI 启动共用，避免误伤正在运行的任务。
    pub fn reconcile_interrupted(&self) -> Result<usize> {
        let deploy = self.try_task_lock("deploy")?;
        let backup = self.try_task_lock("backup")?;
        let pages = self.try_task_lock("pages")?;
        let container = self.try_task_lock("container")?;
        if deploy.is_none() || backup.is_none() || pages.is_none() || container.is_none() {
            return Ok(0);
        }
        self.mark_interrupted()
    }

    /// 半路放弃一条数据库备份时，把本机（控制机）那条 Running 收成失败。
    ///
    /// 不能留给 [`reconcile_interrupted`]：它要求四把任务锁都没人持有，而放弃发生的这一刻
    /// 我们正持有自己那一把 —— 等下去就是记录永远挂着「进行中」。
    pub fn abandon_backup_record(&self, record_id: &str, reason: &str) -> Result<()> {
        let _guard = self.write_guard()?;
        mark_one_interrupted::<BackupRecord>(&self.backups_path(), BACKUP_LABEL, record_id, reason)
    }

    /// 同 [`Store::abandon_backup_record`]，容器备份 / 迁移那一条。
    pub fn abandon_container_record(&self, record_id: &str, reason: &str) -> Result<()> {
        let _guard = self.write_guard()?;
        mark_one_interrupted::<ContainerRecord>(
            &self.containers_path(),
            CONTAINER_LABEL,
            record_id,
            reason,
        )
    }
}

/// 三类任务记录共有的字段访问，供通用列表 CRUD 复用。
trait TaskRecord: Clone {
    fn id(&self) -> &str;
    fn status_mut(&mut self) -> &mut DeployStatus;
    fn error_mut(&mut self) -> &mut Option<String>;
    fn finished_at_mut(&mut self) -> &mut Option<String>;
}

impl TaskRecord for DeployRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

impl TaskRecord for BackupRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

impl TaskRecord for PagesDeployRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

impl TaskRecord for ContainerRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn status_mut(&mut self) -> &mut DeployStatus {
        &mut self.status
    }

    fn error_mut(&mut self) -> &mut Option<String> {
        &mut self.error
    }

    fn finished_at_mut(&mut self) -> &mut Option<String> {
        &mut self.finished_at
    }
}

const HISTORY_LABEL: &str = "部署记录";
const BACKUP_LABEL: &str = "备份记录";
const PAGES_LABEL: &str = "Pages 记录";
const PAGES_MISSING_LABEL: &str = "Pages 部署记录";
const CONTAINER_LABEL: &str = "容器记录";

fn load_records<T: serde::de::DeserializeOwned>(path: &Path, label: &str) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| CoreError::io_path(path, e))?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&text)
        .map_err(|e| CoreError::config(format!("{label}解析失败 {}: {e}", path.display())))
}

fn write_records<T: serde::Serialize>(path: &Path, records: &[T]) -> Result<()> {
    let text = serde_json::to_string_pretty(records)?;
    atomic_write(path, &text)
}

/// 新增或更新一条记录（按 id 匹配），并按 `limit` 裁剪最早的多余记录。
fn upsert_record<T>(path: &Path, record: &T, limit: usize, label: &str) -> Result<()>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    let mut records = load_records::<T>(path, label)?;
    match records.iter_mut().find(|r| r.id() == record.id()) {
        Some(existing) => *existing = record.clone(),
        None => records.push(record.clone()),
    }
    if limit > 0 && records.len() > limit {
        let overflow = records.len() - limit;
        records.drain(0..overflow);
    }
    write_records(path, &records)
}

/// 按 id 查找记录：精确命中优先，其次允许唯一前缀匹配。
fn find_record_by_prefix<T>(
    path: &Path,
    id: &str,
    parse_label: &str,
    missing_label: &str,
) -> Result<T>
where
    T: TaskRecord + serde::de::DeserializeOwned,
{
    let records = load_records::<T>(path, parse_label)?;
    if let Some(record) = records.iter().find(|r| r.id() == id) {
        return Ok(record.clone());
    }
    let matches: Vec<_> = records.iter().filter(|r| r.id().starts_with(id)).collect();
    match matches.len() {
        0 => Err(CoreError::not_found(format!("{missing_label}不存在: {id}"))),
        1 => Ok(matches[0].clone()),
        _ => Err(CoreError::config("记录 ID 不唯一，请输入完整 ID")),
    }
}

fn remove_record<T>(path: &Path, id: &str, label: &str) -> Result<bool>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    Ok(remove_records_by_id::<T>(path, &[id.to_string()], label)? > 0)
}

/// 批量删除记录：一次加锁、一次写回，比逐条删除少掉 N-1 次读写。返回真正删掉的条数。
fn remove_records_by_id<T>(path: &Path, ids: &[String], label: &str) -> Result<usize>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    if ids.is_empty() {
        return Ok(0);
    }
    let mut records = load_records::<T>(path, label)?;
    let before = records.len();
    records.retain(|r| !ids.iter().any(|id| id.as_str() == r.id()));
    let removed = before - records.len();
    if removed > 0 {
        write_records(path, &records)?;
    }
    Ok(removed)
}

/// 把列表中遗留的 Running 记录收敛为失败；返回转换数量。
///
/// 崩溃后无法得知真实结束时间，`duration_ms` 保持 0（界面显示为未知），
/// 不能用「下次启动时刻」计算，否则会把停机时间也算进耗时。
fn mark_running_as_interrupted<T>(path: &Path, label: &str, message: &str) -> Result<usize>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    let mut records = load_records::<T>(path, label)?;
    let mut converted = 0usize;
    for record in records.iter_mut() {
        if *record.status_mut() == DeployStatus::Running {
            *record.status_mut() = DeployStatus::Failed;
            *record.error_mut() = Some(message.to_string());
            *record.finished_at_mut() = Some(now_string());
            converted += 1;
        }
    }
    if converted > 0 {
        write_records(path, &records)?;
    }
    Ok(converted)
}

/// 只收敛指定那一条 Running 记录（整批收敛见 [`mark_running_as_interrupted`]）。
fn mark_one_interrupted<T>(path: &Path, label: &str, id: &str, message: &str) -> Result<()>
where
    T: TaskRecord + serde::Serialize + serde::de::DeserializeOwned,
{
    let mut records = load_records::<T>(path, label)?;
    let mut changed = false;
    for record in records.iter_mut() {
        // 已经不是 Running 的那条不动：引擎可能赶在这之前自己写完了。
        if record.id() != id || *record.status_mut() != DeployStatus::Running {
            continue;
        }
        *record.status_mut() = DeployStatus::Failed;
        *record.error_mut() = Some(message.to_string());
        *record.finished_at_mut() = Some(now_string());
        changed = true;
    }
    if changed {
        write_records(path, &records)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::testutil::{backup_record, history_record, pages_record, temp_store};

    #[test]
    fn remove_history_many_drops_only_selected_ids() {
        let (store, dir) = temp_store();
        for id in ["a", "b", "c"] {
            let record: DeployRecord = serde_json::from_str(&format!(
                r#"{{
                    "id":"{id}","repoId":"repo","repoName":"demo","rev":"main","branch":"main",
                    "commit":"abc","commitShort":"abc","commitSubject":"init","serverId":"srv",
                    "serverName":"prod","targetDir":"/srv/app","scriptDir":"docker","scripts":[],
                    "runScripts":false,"envFiles":[],"status":"success","error":null,"log":"",
                    "startedAt":"2026-01-01 00:00:00","finishedAt":null,"durationMs":0
                }}"#
            ))
            .unwrap();
            store.upsert_history(&record, 0).unwrap();
        }

        // 混入不存在的 id：返回值只计真正删掉的条数。
        let removed = store
            .remove_history_many(&["a".to_string(), "c".to_string(), "gone".to_string()])
            .unwrap();
        assert_eq!(removed, 2);
        let left = store.load_history().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, "b");
        assert_eq!(store.remove_history_many(&[]).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reconcile_interrupted_skips_when_task_lock_held() {
        let dir = std::env::temp_dir()
            .join(format!("deploycode-store-reconcile-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let record: DeployRecord = serde_json::from_str(
            r#"{
                "id":"r1","repoId":"repo","repoName":"demo","rev":"main","branch":"main",
                "commit":"abc","commitShort":"abc","commitSubject":"init","serverId":"srv",
                "serverName":"prod","targetDir":"/srv/app","scriptDir":"docker","scripts":[],
                "runScripts":false,"envFiles":[],"status":"running","error":null,"log":"",
                "startedAt":"2026-01-01 00:00:00","finishedAt":null,"durationMs":0
            }"#,
        )
        .unwrap();
        store.upsert_history(&record, 0).unwrap();

        // 其他进程 / 任务仍在运行（持有任务锁）时不得收敛。
        let held = store.try_task_lock("deploy").unwrap().unwrap();
        assert_eq!(store.reconcile_interrupted().unwrap(), 0);
        assert_eq!(
            store.load_history().unwrap()[0].status,
            DeployStatus::Running
        );
        drop(held);

        // 四类任务锁（部署 / 备份 / Pages / 容器）都空闲时，才把遗留的 Running 记录收敛为失败。
        assert_eq!(store.reconcile_interrupted().unwrap(), 1);
        assert_eq!(
            store.load_history().unwrap()[0].status,
            DeployStatus::Failed
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 容器记录走同一套 CRUD：按 id 覆盖、超限裁剪，崩溃后一起收敛为失败。
    #[test]
    fn container_records_upsert_trim_and_mark_interrupted() {
        let dir = std::env::temp_dir()
            .join(format!("deploycode-store-containers-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let record = |id: &str, status: DeployStatus| ContainerRecord {
            id: id.to_string(),
            kind: crate::models::ContainerRecordKind::Backup,
            project: "blog".to_string(),
            server_id: "s1".to_string(),
            server_name: "prod".to_string(),
            target_server_id: String::new(),
            target_server_name: String::new(),
            target_dir: String::new(),
            bundle_path: format!("/tmp/{id}.tar"),
            bundle_size: 10,
            services: vec!["web".to_string()],
            volumes: vec!["blog_data".to_string()],
            images: Vec::new(),
            include_volumes: true,
            include_images: true,
            status,
            error: None,
            log: String::new(),
            started_at: "2026-01-01 00:00:00".to_string(),
            finished_at: None,
            duration_ms: 0,
        };

        store
            .upsert_container(&record("c1", DeployStatus::Running), 5)
            .unwrap();
        store
            .upsert_container(&record("c2", DeployStatus::Success), 5)
            .unwrap();
        // 同 id 再写一次是覆盖而不是追加。
        store
            .upsert_container(&record("c1", DeployStatus::Success), 5)
            .unwrap();
        assert_eq!(store.load_containers().unwrap().len(), 2);
        assert_eq!(
            store.find_container_record("c1").unwrap().status,
            DeployStatus::Success
        );

        // 超出保留条数时丢最旧的。
        store
            .upsert_container(&record("c3", DeployStatus::Success), 2)
            .unwrap();
        assert_eq!(store.load_containers().unwrap().len(), 2);

        store
            .upsert_container(&record("c4", DeployStatus::Running), 5)
            .unwrap();
        assert_eq!(store.mark_interrupted().unwrap(), 1);
        let running_left = store
            .load_containers()
            .unwrap()
            .into_iter()
            .filter(|item| item.status == DeployStatus::Running)
            .count();
        assert_eq!(running_left, 0);

        store.remove_container_record("c3").unwrap();
        assert!(store.find_container_record("c3").is_err());
        store.clear_container_records().unwrap();
        assert!(store.load_containers().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_crud_keeps_upsert_limit_and_prefix_semantics() {
        let dir =
            std::env::temp_dir().join(format!("deploycode-store-records-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);

        // upsert 按 id 更新而不是追加，limit 只裁剪最早的多余记录。
        store
            .upsert_history(&history_record("h1", DeployStatus::Success), 2)
            .unwrap();
        store
            .upsert_history(&history_record("h2", DeployStatus::Success), 2)
            .unwrap();
        store
            .upsert_history(&history_record("h1", DeployStatus::Failed), 2)
            .unwrap();
        let history = store.load_history().unwrap();
        assert_eq!(history.len(), 2);
        // 更新是原位替换，不改变记录顺序。
        assert_eq!(history[0].id, "h1");
        assert_eq!(history[1].id, "h2");
        assert_eq!(history[0].status, DeployStatus::Failed);
        store
            .upsert_history(&history_record("h3", DeployStatus::Success), 2)
            .unwrap();
        let ids: Vec<String> = store
            .load_history()
            .unwrap()
            .iter()
            .map(|record| record.id.clone())
            .collect();
        assert_eq!(ids, vec!["h2".to_string(), "h3".to_string()]);

        // limit 为 0 表示不裁剪（CLI / 迁移场景）。
        store
            .upsert_history(&history_record("h4", DeployStatus::Success), 0)
            .unwrap();
        assert_eq!(store.load_history().unwrap().len(), 3);

        assert!(store.remove_history("h2").unwrap());
        assert!(!store.remove_history("h2").unwrap());
        store.clear_history().unwrap();
        assert!(store.load_history().unwrap().is_empty());

        // 查找：精确命中优先，唯一前缀可命中，多义前缀报错。
        store
            .upsert_backup(&backup_record("aaaa-1111", DeployStatus::Success), 0)
            .unwrap();
        store
            .upsert_backup(&backup_record("bbbb-2222", DeployStatus::Success), 0)
            .unwrap();
        assert_eq!(store.find_backup("aaaa-1111").unwrap().id, "aaaa-1111");
        assert_eq!(store.find_backup("aaaa").unwrap().id, "aaaa-1111");
        assert!(matches!(
            store.find_backup("nope"),
            Err(CoreError::NotFound(_))
        ));
        store
            .upsert_backup(&backup_record("aaaa-3333", DeployStatus::Success), 0)
            .unwrap();
        assert!(matches!(
            store.find_backup("aaaa"),
            Err(CoreError::Config(_))
        ));
        assert_eq!(store.find_backup("aaaa-3333").unwrap().id, "aaaa-3333");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_interrupted_converts_running_records_across_kinds() {
        let dir = std::env::temp_dir()
            .join(format!("deploycode-store-interrupted-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        store
            .upsert_history(&history_record("h1", DeployStatus::Running), 0)
            .unwrap();
        store
            .upsert_history(&history_record("h2", DeployStatus::Success), 0)
            .unwrap();
        store
            .upsert_backup(&backup_record("b1", DeployStatus::Running), 0)
            .unwrap();
        store
            .upsert_pages_record(&pages_record("p1", DeployStatus::Running), 0)
            .unwrap();

        assert_eq!(store.mark_interrupted().unwrap(), 3);
        let converted = store.find_record("h1").unwrap();
        assert_eq!(converted.status, DeployStatus::Failed);
        assert_eq!(
            converted.error.as_deref(),
            Some("任务被中断（应用退出或崩溃）")
        );
        assert!(converted.finished_at.is_some());
        // 崩溃后无法得知真实结束时间，耗时保持 0（界面显示为未知）。
        assert_eq!(converted.duration_ms, 0);
        assert_eq!(
            store.find_record("h2").unwrap().status,
            DeployStatus::Success
        );
        assert_eq!(
            store.find_backup("b1").unwrap().status,
            DeployStatus::Failed
        );
        assert_eq!(
            store.find_pages_record("p1").unwrap().status,
            DeployStatus::Failed
        );
        // 收敛过之后再次调用不应产生变更。
        assert_eq!(store.mark_interrupted().unwrap(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 记录被裁剪或手动删掉之后留在盘上的包要能看见、能清，同时越界路径与有记录的包一律不许删。
    #[test]
    fn orphan_bundles_lists_files_without_records_and_protects_the_rest() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-store-orphans-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::new(&dir);
        let db_dir = store.db_bundle_dir();
        let container_dir = store.container_bundle_dir();
        std::fs::create_dir_all(&db_dir).unwrap();
        std::fs::create_dir_all(&container_dir).unwrap();

        let kept = db_dir.join("app-keep.sql.gz");
        std::fs::write(&kept, "x".repeat(10)).unwrap();
        let orphan = db_dir.join("app-orphan.sql.gz");
        std::fs::write(&orphan, "y".repeat(20)).unwrap();
        // 进行中的半截包属于那次任务，不该被当成孤儿。
        std::fs::write(db_dir.join("app-part.sql.gz.part"), "z").unwrap();
        let tar = container_dir.join("blog-orphan.tar");
        std::fs::write(&tar, "w".repeat(5)).unwrap();

        let mut record = backup_record("b1", DeployStatus::Success);
        record.bundle_path = kept.display().to_string();
        store.upsert_backup(&record, 0).unwrap();

        let orphans = store.orphan_bundles().unwrap();
        let names: Vec<&str> = orphans
            .iter()
            .map(|item| item.file_name.as_str())
            .collect();
        assert!(names.contains(&"app-orphan.sql.gz"), "{names:?}");
        assert!(names.contains(&"blog-orphan.tar"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.contains("keep")),
            "有记录指向的包不该出现：{names:?}"
        );
        assert!(
            !names.iter().any(|name| name.ends_with(".part")),
            "半截包不该进清理列表：{names:?}"
        );
        assert_eq!(orphans[0].size_bytes, 20, "按大小从大到小排");
        assert_eq!(orphans[0].kind, "database");

        // 目录外的路径整批拒绝，而且一个都不删。
        let outside = dir.join("config.json");
        let err = store
            .delete_orphan_bundles(&[outside.display().to_string()])
            .expect_err("越界路径必须拒绝");
        assert!(err.to_string().contains("只能删除备份包目录"), "{err}");
        assert!(orphan.is_file());
        // 还挂在记录上的不许删。
        assert!(store
            .delete_orphan_bundles(&[kept.display().to_string()])
            .is_err());
        assert!(kept.is_file());

        // 有任务在跑时拒绝：那次的包还没写进记录，此刻会被当成孤儿删掉。
        let held = store.try_task_lock("backup").unwrap();
        assert!(store
            .delete_orphan_bundles(&[orphan.display().to_string()])
            .is_err());
        assert!(orphan.is_file());
        drop(held);

        let (count, freed) = store
            .delete_orphan_bundles(&[orphan.display().to_string(), tar.display().to_string()])
            .unwrap();
        assert_eq!((count, freed), (2, 25));
        assert!(!orphan.exists() && !tar.exists());
        assert!(kept.is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

}
