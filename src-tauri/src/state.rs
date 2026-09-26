use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deploy_core::store::TaskLock;
use deploy_core::{DeployEngine, Result, Store};
use serde::Serialize;
use tauri::{AppHandle, Manager};

/// 已登记的仓库文件监听：记录被监听目录，仓库路径变化时可替换 watcher。
pub struct WatchedRepo {
    pub path: PathBuf,
    /// 仅靠持有保活，drop 时自动停止监听，不需要读取。
    #[allow(dead_code)]
    pub watcher: notify::RecommendedWatcher,
}

/// 正在进行的部署（用于应用退出时清理远端脚本）。
#[derive(Clone)]
pub struct ActiveDeploy {
    pub server_id: String,
    pub target_dir: String,
    /// 部署任务的终止句柄，退出清理前先中止任务，避免它再启动新脚本。
    pub abort: tokio::task::AbortHandle,
}

/// 正在进行的备份（用于应用退出时清理远端脚本）。
#[derive(Clone)]
pub struct ActiveBackup {
    pub server_id: String,
    pub record_id: String,
    /// 备份任务的终止句柄，退出清理前先中止任务，避免它再启动新脚本。
    pub abort: tokio::task::AbortHandle,
}

/// 正在进行的容器备份 / 迁移。
///
/// 一台任务同时碰两台服务器（来源打包、目标恢复），所以清理要按服务器列表逐个做。
#[derive(Clone)]
pub struct ActiveContainer {
    pub server_ids: Vec<String>,
    pub record_id: String,
    pub abort: tokio::task::AbortHandle,
}

/// 关机计划的来源。
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShutdownSource {
    /// 「每天定时关机」到点排的那一次。
    Scheduled,
    /// 用户在设置页手动按下的倒计时。
    Manual,
}

/// 已排定的关机计划：仍在应用内倒计时阶段，尚未下发系统关机请求。
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingShutdown {
    /// 下发系统关机请求的时刻（epoch 毫秒）。前端按秒自己走表，不占 IPC。
    pub at_ms: i64,
    pub source: ShutdownSource,
}

/// 可抢占的任务类型。
#[derive(Clone, Copy)]
pub enum ClaimKind {
    Deploy,
    Backup,
    Pages,
    Nginx,
    Container,
}

impl ClaimKind {
    fn try_claim(self, state: &AppState) -> Result<bool> {
        match self {
            ClaimKind::Deploy => state.try_claim_deploy(),
            ClaimKind::Backup => state.try_claim_backup(),
            ClaimKind::Pages => state.try_claim_pages(),
            ClaimKind::Nginx => state.try_claim_nginx(),
            ClaimKind::Container => state.try_claim_container(),
        }
    }

    fn release(self, state: &AppState) {
        match self {
            ClaimKind::Deploy => state.release_deploy_claim(),
            ClaimKind::Backup => state.release_backup_claim(),
            ClaimKind::Pages => state.release_pages_claim(),
            ClaimKind::Nginx => state.release_nginx_claim(),
            ClaimKind::Container => state.release_container_claim(),
        }
    }
}

/// 任务抢占守卫：无论 prepare 失败、任务 panic 还是正常结束，都会释放抢占标记。
/// 
/// RAII 模式保证：**任何退出路径**（成功/取消/panic）都能触发 Drop，自动释放锁
pub struct ClaimGuard {
    app: AppHandle,
    kind: ClaimKind,
}

impl ClaimGuard {
    /// 尝试抢占指定任务；已有同类任务在进行时返回 `Ok(None)`。
    /// 
    /// 返回值设计：
    /// - `Ok(Some(guard))` → 抢占成功，调用方持有 guard，Drop 时自动释放
    /// - `Ok(None)` → 抢占失败，返回 None，**不需要 Drop**
    /// - 调用方只需 `guard?`，无论成功失败都会释放
    pub fn acquire(app: &AppHandle, kind: ClaimKind) -> Result<Option<Self>> {
        let state = app.state::<AppState>();
        if !kind.try_claim(&state)? {
            return Ok(None);
        }
        Ok(Some(Self {
            app: app.clone(),
            kind,
        }))
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        let state = self.app.state::<AppState>();
        self.kind.release(&state); // 确保独占标记被释放
    }
}

/// 部署任务守卫：事件转发结束（含任务 panic / 取消）后释放抢占标记并清理登记项。
pub struct DeployTaskGuard {
    app: AppHandle,
    record_id: String,
    _claim: ClaimGuard,
}

impl DeployTaskGuard {
    pub fn new(app: AppHandle, record_id: String, claim: ClaimGuard) -> Self {
        Self {
            app,
            record_id,
            _claim: claim,
        }
    }
}

impl Drop for DeployTaskGuard {
    fn drop(&mut self) {
        self.app.state::<AppState>().untrack_deploy(&self.record_id);
    }
}

/// 批量部署的登记守卫：整批共用一次抢占，登记项随每台开始而推进。
///
/// 取消时命令层按「当前这一台」的 recordId 取句柄，句柄是整个批次任务的，
/// abort 会让循环连同本守卫一起析构，因此剩余登记不会残留。
pub struct DeployBatchTracking {
    app: AppHandle,
    abort: tokio::task::AbortHandle,
    current: Option<String>,
}

impl DeployBatchTracking {
    pub fn new(app: AppHandle, abort: tokio::task::AbortHandle) -> Self {
        Self {
            app,
            abort,
            current: None,
        }
    }

    /// 开始一台：先摘掉上一台的登记，再按批次任务的句柄登记这一台。
    pub fn begin(&mut self, record: &deploy_core::models::DeployRecord) {
        if let Some(previous) = self.current.take() {
            self.app.state::<AppState>().untrack_deploy(&previous);
        }
        self.app.state::<AppState>().track_deploy(
            &record.id,
            ActiveDeploy {
                server_id: record.server_id.clone(),
                target_dir: record.target_dir.clone(),
                abort: self.abort.clone(),
            },
        );
        self.current = Some(record.id.clone());
    }
}

impl Drop for DeployBatchTracking {
    fn drop(&mut self) {
        if let Some(current) = self.current.take() {
            self.app.state::<AppState>().untrack_deploy(&current);
        }
    }
}

/// 备份任务守卫：事件转发结束（含任务 panic / 取消）后释放抢占标记并清理登记项。
pub struct BackupTaskGuard {
    app: AppHandle,
    record_id: String,
    _claim: ClaimGuard,
}

impl BackupTaskGuard {
    pub fn new(app: AppHandle, record_id: String, claim: ClaimGuard) -> Self {
        Self {
            app,
            record_id,
            _claim: claim,
        }
    }
}

impl Drop for BackupTaskGuard {
    fn drop(&mut self) {
        self.app.state::<AppState>().untrack_backup(&self.record_id);
    }
}

/// Pages 部署任务守卫：事件转发结束后释放 Pages 抢占标记。
pub struct PagesTaskGuard {
    _claim: ClaimGuard,
}

/// 容器任务守卫：事件转发结束后释放抢占标记并清理登记项。
pub struct ContainerTaskGuard {
    app: AppHandle,
    record_id: String,
    _claim: ClaimGuard,
}

impl ContainerTaskGuard {
    pub fn new(app: AppHandle, record_id: String, claim: ClaimGuard) -> Self {
        Self {
            app,
            record_id,
            _claim: claim,
        }
    }
}

impl Drop for ContainerTaskGuard {
    fn drop(&mut self) {
        self.app
            .state::<AppState>()
            .untrack_container(&self.record_id);
    }
}

impl PagesTaskGuard {
    pub fn new(claim: ClaimGuard) -> Self {
        Self { _claim: claim }
    }
}

/// 全局应用状态：所有命令通过它访问配置与部署引擎。
pub struct AppState {
    pub store: Arc<Store>,
    /// 正在监听的文件系统 watcher（按仓库 id 保存），用于 GUI 实时刷新。
    pub watchers: Mutex<HashMap<String, WatchedRepo>>,
    /// 进行中的部署（record_id -> 服务器/目录/任务句柄），退出时用于终止远端脚本。
    pub active_deploys: Mutex<HashMap<String, ActiveDeploy>>,
    /// 进行中的备份（record_id -> 服务器/记录/任务句柄），退出时用于终止远端脚本。
    pub active_backups: Mutex<HashMap<String, ActiveBackup>>,
    /// 进行中的容器备份 / 迁移（record_id -> 涉及的服务器/任务句柄）。
    pub active_containers: Mutex<HashMap<String, ActiveContainer>>,
    /// 取消部署后正在做远端清理的任务；此时任务守卫已摘除 active_deploys，
    /// 单独登记保证清理期间退出应用仍会终止远端脚本。
    pub pending_cleanups: Mutex<Vec<(String, ActiveDeploy)>>,
    /// 取消容器任务后正在做远端清理的那些：`take_container` 已经把登记项摘掉，
    /// 不另记一份的话清理期间退出应用会把备份包留在来源与目标两台服务器上。
    pub pending_container_cleanups: Mutex<Vec<(String, ActiveContainer)>>,
    /// 排定中的关机计划（倒计时阶段）；到点由调度器取出并下发系统关机请求。
    pending_shutdown: Mutex<Option<PendingShutdown>>,
    /// 部署抢占标记：在 prepare 之前原子占位，避免两个并发命令同时通过检查。
    deploy_claim: AtomicBool,
    /// 备份抢占标记：同一时间只允许一个数据库备份。
    backup_claim: AtomicBool,
    /// Pages 部署抢占标记：同一时间只允许一个 Pages 部署。
    pages_claim: AtomicBool,
    /// Nginx 操作抢占标记：同一时间只允许一个 Nginx 操作（避免并发修改配置）。
    nginx_claim: AtomicBool,
    /// 容器任务抢占标记：同一时间只允许一个容器备份 / 迁移。
    container_claim: AtomicBool,
    /// 部署跨进程任务锁（持有时禁止 CLI 等其他进程执行部署）。
    deploy_lock: Mutex<Option<TaskLock>>,
    /// 备份跨进程任务锁。
    backup_lock: Mutex<Option<TaskLock>>,
    /// Pages 跨进程任务锁。
    pages_lock: Mutex<Option<TaskLock>>,
    /// Nginx 跨进程任务锁。
    nginx_lock: Mutex<Option<TaskLock>>,
    /// 容器任务跨进程锁。
    container_lock: Mutex<Option<TaskLock>>,
}

impl AppState {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            watchers: Mutex::new(HashMap::new()),
            active_deploys: Mutex::new(HashMap::new()),
            active_backups: Mutex::new(HashMap::new()),
            active_containers: Mutex::new(HashMap::new()),
            pending_cleanups: Mutex::new(Vec::new()),
            pending_container_cleanups: Mutex::new(Vec::new()),
            pending_shutdown: Mutex::new(None),
            deploy_claim: AtomicBool::new(false),
            backup_claim: AtomicBool::new(false),
            pages_claim: AtomicBool::new(false),
            nginx_claim: AtomicBool::new(false),
            container_claim: AtomicBool::new(false),
            deploy_lock: Mutex::new(None),
            backup_lock: Mutex::new(None),
            pages_lock: Mutex::new(None),
            nginx_lock: Mutex::new(None),
            container_lock: Mutex::new(None),
        }
    }

    /// 尝试占用任务名额：**双重保护机制**
    /// 
    /// 1️⃣ **进程内互斥**: `AtomicBool.compare_exchange` 确保同一进程只允许一个任务抢占成功
    /// 2️⃣ **跨进程互斥**: `try_task_lock` 文件锁确保不同进程（GUI/CLI）不会同时执行同类任务
    /// 
    /// ⚠️ 注意：两步操作不是原子的，但设计如此——先快速失败（原子标记），再精确控制（文件锁）
    /// 如果原子标记成功但文件锁失败，说明有其他进程抢到了锁，此时必须释放原子标记让出机会
    fn try_claim(
        &self,
        claimed: &AtomicBool,
        held: &Mutex<Option<TaskLock>>,
        name: &str,
    ) -> Result<bool> {
        if claimed
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(false); // 进程内已有任务在进行，快速失败
        }
        match self.store.try_task_lock(name) {
            Ok(Some(lock)) => {
                // Mutex poison 处理：持有锁的线程 panic 时返回 poisoned，新线程接管锁并清理
                let mut slot = held
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                *slot = Some(lock);
                Ok(true)
            }
            // 其他进程（如 CLI）正在执行同类任务，释放原子标记让下次重试。
            Ok(None) => {
                claimed.store(false, Ordering::SeqCst);
                Ok(false)
            }
            Err(err) => {
                claimed.store(false, Ordering::SeqCst);
                Err(err)
            }
        }
    }

    fn release_claim(&self, claimed: &AtomicBool, held: &Mutex<Option<TaskLock>>) {
        // 先取出并释放文件锁，再复位原子标记，避免新任务抢到名额却拿不到锁。
        let lock = held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        drop(lock);
        claimed.store(false, Ordering::SeqCst);
    }

    /// 尝试占用部署名额；失败说明已有部署在进行。
    pub fn try_claim_deploy(&self) -> Result<bool> {
        self.try_claim(&self.deploy_claim, &self.deploy_lock, "deploy")
    }

    pub fn release_deploy_claim(&self) {
        self.release_claim(&self.deploy_claim, &self.deploy_lock);
    }

    /// 尝试占用备份名额；失败说明已有备份在进行。
    pub fn try_claim_backup(&self) -> Result<bool> {
        self.try_claim(&self.backup_claim, &self.backup_lock, "backup")
    }

    pub fn release_backup_claim(&self) {
        self.release_claim(&self.backup_claim, &self.backup_lock);
    }

    /// 是否有备份任务正在进行（定时调度器判断占用用）。
    pub fn is_backup_active(&self) -> bool {
        self.backup_claim.load(Ordering::SeqCst)
    }

    /// 尝试占用 Pages 部署名额。
    pub fn try_claim_pages(&self) -> Result<bool> {
        self.try_claim(&self.pages_claim, &self.pages_lock, "pages")
    }

    pub fn release_pages_claim(&self) {
        self.release_claim(&self.pages_claim, &self.pages_lock);
    }

    /// 尝试占用 Nginx 操作名额。
    pub fn try_claim_nginx(&self) -> Result<bool> {
        self.try_claim(&self.nginx_claim, &self.nginx_lock, "nginx")
    }

    /// 尝试占用容器任务名额（备份 / 迁移 / 恢复共用一个）。
    pub fn try_claim_container(&self) -> Result<bool> {
        self.try_claim(&self.container_claim, &self.container_lock, "container")
    }

    pub fn release_container_claim(&self) {
        self.release_claim(&self.container_claim, &self.container_lock);
    }

    /// 容器任务是否仍在进行（退出时用于判断是否需要推迟退出）。
    pub fn has_active_container(&self) -> bool {
        self.container_claim.load(Ordering::SeqCst)
    }

    pub fn release_nginx_claim(&self) {
        self.release_claim(&self.nginx_claim, &self.nginx_lock);
    }

    /// Nginx 任务是否仍在进行（退出时用于判断是否需要推迟退出并清理子进程）。
    pub fn has_active_nginx(&self) -> bool {
        self.nginx_claim.load(Ordering::SeqCst)
    }

    /// Pages 任务是否仍在进行（退出时用于判断是否需要推迟退出并清理子进程）。
    pub fn has_active_pages(&self) -> bool {
        self.pages_claim.load(Ordering::SeqCst)
    }

    fn shutdown_slot(&self) -> std::sync::MutexGuard<'_, Option<PendingShutdown>> {
        self.pending_shutdown
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 当前排定的关机计划（仍在可取消的倒计时阶段）。
    pub fn pending_shutdown(&self) -> Option<PendingShutdown> {
        *self.shutdown_slot()
    }

    /// 排定关机计划；已有计划时覆盖（用户重新选时长就是要改时间）。
    pub fn arm_shutdown(&self, plan: PendingShutdown) {
        *self.shutdown_slot() = Some(plan);
    }

    /// 取消尚未下发的关机计划。
    pub fn cancel_shutdown(&self) -> Option<PendingShutdown> {
        self.shutdown_slot().take()
    }

    /// 只取消「每天定时关机」排的那一次：关掉开关后不该还留着一次待执行的关机，
    /// 但用户刚手动按下的倒计时不算被这次开关管着。
    pub fn cancel_scheduled_shutdown(&self) -> Option<PendingShutdown> {
        let mut slot = self.shutdown_slot();
        let scheduled = matches!(*slot, Some(plan) if plan.source == ShutdownSource::Scheduled);
        if scheduled {
            slot.take()
        } else {
            None
        }
    }

    /// 取出已到点的关机计划。与「取消」共用一把锁：用户同一刻点取消时，
    /// 要么取消成功（计划已被摘走，不会再下发），要么关机已经排上，不会两头落空。
    pub fn take_due_shutdown(&self, now_ms: i64) -> Option<PendingShutdown> {
        let mut slot = self.shutdown_slot();
        let due = matches!(*slot, Some(plan) if plan.at_ms <= now_ms);
        if due {
            slot.take()
        } else {
            None
        }
    }

    pub fn engine(&self) -> DeployEngine {
        DeployEngine::new(self.store.clone())
    }

    pub fn track_deploy(&self, record_id: &str, deploy: ActiveDeploy) {
        // Mutex poison 处理：同 try_claim，poison 表示持有锁的线程 panic，新线程接管清理
        self.active_deploys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(record_id.to_string(), deploy);
    }

    pub fn untrack_deploy(&self, record_id: &str) {
        self.active_deploys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(record_id);
    }

    /// 取出指定记录的部署登记项（取消部署时使用；取出后退出清理不会再处理它）。
    pub fn take_deploy(&self, record_id: &str) -> Option<ActiveDeploy> {
        self.active_deploys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(record_id)
    }

    /// 取出并清空当前进行中的部署列表。
    pub fn take_deploys(&self) -> Vec<(String, ActiveDeploy)> {
        self.active_deploys
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
            .collect()
    }

    /// 登记一条「取消后正在远端清理」的部署；重复登记同一记录时保留先登记的项。
    pub fn add_pending_cleanup(&self, record_id: &str, deploy: ActiveDeploy) {
        let mut pending = self
            .pending_cleanups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !pending.iter().any(|(id, _)| id == record_id) {
            pending.push((record_id.to_string(), deploy));
        }
    }

    /// 取消流程自身的远端清理结束后移除登记；若已被退出流程取走则为空操作。
    pub fn remove_pending_cleanup(&self, record_id: &str) {
        self.pending_cleanups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|(id, _)| id != record_id);
    }

    /// 取出并清空「取消清理中」的部署列表（退出时与 active_deploys 一起清理）。
    pub fn take_pending_cleanups(&self) -> Vec<(String, ActiveDeploy)> {
        // Mutex poison 处理：同 track_deploy，poison 表示持有锁的线程 panic，新线程接管清理
        self.pending_cleanups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect()
    }

    /// 登记一条「取消后正在远端清理」的容器任务，重复登记时保留先登记的项。
    pub fn add_pending_container_cleanup(&self, record_id: &str, task: ActiveContainer) {
        let mut pending = self
            .pending_container_cleanups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !pending.iter().any(|(id, _)| id == record_id) {
            pending.push((record_id.to_string(), task));
        }
    }

    /// 取消流程自身的远端清理结束后移除登记；若已被退出流程取走则为空操作。
    pub fn remove_pending_container_cleanup(&self, record_id: &str) {
        self.pending_container_cleanups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|(id, _)| id != record_id);
    }

    /// 取出并清空「取消清理中」的容器任务列表（退出时与 active_containers 一起清理）。
    pub fn take_pending_container_cleanups(&self) -> Vec<(String, ActiveContainer)> {
        self.pending_container_cleanups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain(..)
            .collect()
    }

    pub fn track_backup(&self, record_id: &str, backup: ActiveBackup) {
        self.active_backups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(record_id.to_string(), backup);
    }

    pub fn untrack_backup(&self, record_id: &str) {
        self.active_backups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(record_id);
    }

    /// 取出并清空当前进行中的备份列表。
    pub fn take_backups(&self) -> Vec<(String, ActiveBackup)> {
        self.active_backups
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
            .collect()
    }

    pub fn track_container(&self, record_id: &str, task: ActiveContainer) {
        self.active_containers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(record_id.to_string(), task);
    }

    pub fn untrack_container(&self, record_id: &str) {
        self.active_containers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(record_id);
    }

    /// 取出指定记录的容器登记项（取消时取出后退出清理不会再处理它）。
    pub fn take_container(&self, record_id: &str) -> Option<ActiveContainer> {
        self.active_containers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(record_id)
    }

    /// 取出并清空当前进行中的容器任务列表。
    pub fn take_containers(&self) -> Vec<(String, ActiveContainer)> {
        self.active_containers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .drain()
            .collect()
    }
}
