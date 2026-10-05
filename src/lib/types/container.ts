import type { DeployStatus, LiveTask, LogLevel, RunLocation } from "./common";

// ---------------------------------------------------------------------------
// 容器备份与迁移（镜像 deploy-core src/container.rs）
// ---------------------------------------------------------------------------

/** 服务器上发现到的一个 docker-compose 项目。 */
export interface ComposeStack {
  /** compose 项目名（`-p` 参数，也是数据卷名前缀）。 */
  name: string;
  /** 服务数量。 */
  services: number;
  /** 当前处于运行中的容器数。 */
  running: number;
  /** compose 给出的状态摘要，如 `running (3)`。 */
  status: string;
  /** 项目目录下找到的 compose 配置文件（绝对路径）。 */
  files: string[];
  /** 项目工作目录。 */
  workingDir: string;
}

/** 项目挂载的一个数据卷。 */
export interface ComposeVolume {
  name: string;
  /** 宿主机上的卷目录（非 root 连接时可能读不到）。 */
  mountpoint: string;
  sizeBytes: number;
  /** 导出失败时用来挂载卷的镜像（取当前使用该卷的容器镜像，本机已有）。 */
  image: string;
  /** 宿主机路径可直接读取。 */
  readable: boolean;
}

/** 项目里的一个服务（按容器解析）。 */
export interface ComposeService {
  name: string;
  container: string;
  image: string;
  state: string;
  /** 已发布的端口映射，如 `0.0.0.0:80->80/tcp`。 */
  ports: string;
}

/** 快照前的项目详情与预检结果。 */
export interface ComposeStackDetail {
  stack: ComposeStack;
  services: ComposeService[];
  volumes: ComposeVolume[];
  /** 需要 `docker save` 的镜像（已去重）。 */
  images: string[];
  /** 项目目录下一起带走的 env 文件。 */
  envFiles: string[];
  /** 目标机上使用的 compose 命令（`docker compose` 或 `docker-compose`）。 */
  composeCommand: string;
  volumeBytes: number;
  imageBytes: number;
  /** 项目目录（compose 文件与 env）体积：打包时总会带上，不受卷/镜像开关影响。 */
  projectBytes: number;
  /** 预检提示（不阻断，界面上原样展示）。 */
  warnings: string[];
}

/** 迁移目标。 */
export interface ContainerTarget {
  serverId: string;
  /** 目标服务器上的项目目录。 */
  targetDir: string;
  /** 恢复后自动 `docker compose up -d`。 */
  startServices: boolean;
}

/** 发起一次容器快照 / 迁移的参数。 */
export interface ContainerRequest {
  serverId: string;
  project: string;
  /** 打包数据卷前先 `compose stop`、结束后 `compose start`，保证数据一致。 */
  pauseSource: boolean;
  includeVolumes: boolean;
  includeImages: boolean;
  /** 为空表示只备份到本机。 */
  target: ContainerTarget | null;
}

/** 从本机已有备份包再恢复一次到服务器。 */
export interface ContainerRestoreRequest {
  bundlePath: string;
  target: ContainerTarget;
}

/** 保存的容器备份配置：一次快照 / 迁移的全部参数，可复用、可定时。 */
export interface ContainerConfig {
  id: string;
  name: string;
  serverId: string;
  project: string;
  pauseSource: boolean;
  includeVolumes: boolean;
  includeImages: boolean;
  /** 迁移目标；null 表示只备份到本机。 */
  target: ContainerTarget | null;
  createdAt: string;
  /** 执行位：本机定时，或交给控制机上的 agent。 */
  runLocation: RunLocation;
}

export type ContainerRecordKind = "backup" | "migrate" | "restore";

/** 一次容器备份 / 迁移记录。 */
export interface ContainerRecord {
  id: string;
  kind: ContainerRecordKind;
  project: string;
  serverId: string;
  serverName: string;
  targetServerId: string;
  targetServerName: string;
  targetDir: string;
  /** 本机备份包路径；`restore` 记录里是被恢复的来源包。 */
  bundlePath: string;
  bundleSize: number;
  services: string[];
  volumes: string[];
  images: string[];
  includeVolumes: boolean;
  includeImages: boolean;
  status: DeployStatus;
  error: string | null;
  log: string;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number;
}

export type ContainerEvent =
  | { type: "started"; recordId: string }
  | { type: "log"; level: LogLevel; message: string }
  | { type: "progress"; percent: number; message: string }
  | { type: "finished"; record: ContainerRecord };

export type LiveContainer = LiveTask<ContainerRecord>;
