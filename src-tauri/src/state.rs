use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use deploy_core::store::TaskLock;
use deploy_core::{DeployEngine, Result, Store};
use tauri::{AppHandle, Manager};

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

/// 可抢占的任务类型。
#[derive(Clone, Copy)]
pub enum ClaimKind {
    Deploy,
    Backup,
    Pages,
}

impl ClaimKind {
    fn try_claim(self, state: &AppState) -> Result<bool> {
        match self {
            ClaimKind::Deploy => state.try_claim_deploy(),
            ClaimKind::Backup => state.try_claim_backup(),
            ClaimKind::Pages => state.try_claim_pages(),
        }
    }

    fn release(self, state: &AppState) {
        match self {
            ClaimKind::Deploy => state.release_deploy_claim(),
            ClaimKind::Backup => state.release_backup_claim(),
            ClaimKind::Pages => state.release_pages_claim(),
        }
    }
}

/// 任务抢占守卫：无论 prepare 失败、任务 panic 还是正常结束，都会释放抢占标记。
pub struct ClaimGuard {
    app: AppHandle,
    kind: ClaimKind,
}

impl ClaimGuard {
    /// 尝试抢占指定任务；已有同类任务在进行时返回 `Ok(None)`。
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
        self.kind.release(&state);
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

impl PagesTaskGuard {
    pub fn new(claim: ClaimGuard) -> Self {
        Self { _claim: claim }
    }
}

/// 全局应用状态：所有命令通过它访问配置与部署引擎。
pub struct AppState {
    pub store: Arc<Store>,
    /// 正在监听的文件系统 watcher（按仓库 id 保存），用于 GUI 实时刷新。
    pub watchers: Mutex<HashMap<String, notify::RecommendedWatcher>>,
    /// 进行中的部署（record_id -> 服务器/目录/任务句柄），退出时用于终止远端脚本。
    pub active_deploys: Mutex<HashMap<String, ActiveDeploy>>,
    /// 进行中的备份（record_id -> 服务器/记录/任务句柄），退出时用于终止远端脚本。
    pub active_backups: Mutex<HashMap<String, ActiveBackup>>,
    /// 部署抢占标记：在 prepare 之前原子占位，避免两个并发命令同时通过检查。
    deploy_claim: AtomicBool,
    /// 备份抢占标记：同一时间只允许一个数据库备份。
    backup_claim: AtomicBool,
    /// Pages 部署抢占标记：同一时间只允许一个 Pages 部署。
    pages_claim: AtomicBool,
    /// 部署跨进程任务锁（持有时禁止 CLI 等其他进程执行部署）。
    deploy_lock: Mutex<Option<TaskLock>>,
    /// 备份跨进程任务锁。
    backup_lock: Mutex<Option<TaskLock>>,
    /// Pages 跨进程任务锁。
    pages_lock: Mutex<Option<TaskLock>>,
}

impl AppState {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            watchers: Mutex::new(HashMap::new()),
            active_deploys: Mutex::new(HashMap::new()),
            active_backups: Mutex::new(HashMap::new()),
            deploy_claim: AtomicBool::new(false),
            backup_claim: AtomicBool::new(false),
            pages_claim: AtomicBool::new(false),
            deploy_lock: Mutex::new(None),
            backup_lock: Mutex::new(None),
            pages_lock: Mutex::new(None),
        }
    }

    /// 尝试占用任务名额：先赢下进程内原子标记，再尝试获取跨进程文件锁。
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
            return Ok(false);
        }
        match self.store.try_task_lock(name) {
            Ok(Some(lock)) => {
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

    /// 尝试占用 Pages 部署名额。
    pub fn try_claim_pages(&self) -> Result<bool> {
        self.try_claim(&self.pages_claim, &self.pages_lock, "pages")
    }

    pub fn release_pages_claim(&self) {
        self.release_claim(&self.pages_claim, &self.pages_lock);
    }

    pub fn engine(&self) -> DeployEngine {
        DeployEngine::new(self.store.clone())
    }

    pub fn track_deploy(&self, record_id: &str, deploy: ActiveDeploy) {
        if let Ok(mut deploys) = self.active_deploys.lock() {
            deploys.insert(record_id.to_string(), deploy);
        }
    }

    pub fn untrack_deploy(&self, record_id: &str) {
        if let Ok(mut deploys) = self.active_deploys.lock() {
            deploys.remove(record_id);
        }
    }

    /// 取出并清空当前进行中的部署列表。
    pub fn take_deploys(&self) -> Vec<(String, ActiveDeploy)> {
        match self.active_deploys.lock() {
            Ok(mut deploys) => deploys.drain().collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn track_backup(&self, record_id: &str, backup: ActiveBackup) {
        if let Ok(mut backups) = self.active_backups.lock() {
            backups.insert(record_id.to_string(), backup);
        }
    }

    pub fn untrack_backup(&self, record_id: &str) {
        if let Ok(mut backups) = self.active_backups.lock() {
            backups.remove(record_id);
        }
    }

    /// 取出并清空当前进行中的备份列表。
    pub fn take_backups(&self) -> Vec<(String, ActiveBackup)> {
        match self.active_backups.lock() {
            Ok(mut backups) => backups.drain().collect(),
            Err(_) => Vec::new(),
        }
    }
}
