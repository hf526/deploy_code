/** 盘上有文件、记录里已没有指向它的本地备份包（Rust: deploy_core::store::OrphanBundle）。 */
export interface OrphanBundle {
  /** "database" 走 backups 目录，"container" 走 containers 目录。 */
  kind: "database" | "container";
  fileName: string;
  path: string;
  sizeBytes: number;
}

/** 一次孤儿包清理的结果（Rust: commands::backup::OrphanCleanup）。 */
export interface OrphanCleanup {
  deleted: number;
  freedBytes: number;
}
