//! 容器备份包的格式：包内清单 `manifest.json` + 打包 / 恢复两份远端 bash 脚本。
//!
//! 备份包是一次快照的全部产物（项目目录、命名卷 tar、`docker save` 出的镜像 tar 与清单），
//! 恢复阶段只读包内清单，不再回源机查询。`tar -cf` 的归档成员固定为
//! `manifest.json project volumes images`，所以本机也能用 `tar -xOf` 直接取清单。

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::discover::ComposeStackDetail;
use super::{relative_to, validate_project, validate_remote_dir, volume_short, ContainerPlan};
use crate::backup::render_template;
use crate::error::{CoreError, Result};
use crate::models::{now_string, ContainerTarget, ServerConfig};
use crate::process::shell_quote;

/// 备份包内的 `manifest.json`：恢复阶段只需要它，不再回源机查询。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub version: u32,
    pub created_at: String,
    pub project: String,
    pub source_host: String,
    /// 源机上的项目目录；恢复时未指定目录就用它。
    pub working_dir: String,
    /// 源机使用的 compose 命令（`docker compose` 或 `docker-compose`）。
    pub compose_command: Vec<String>,
    /// 相对 `project/` 的配置文件路径，恢复后相对新目录传给 `-f`。
    pub config_files: Vec<String>,
    pub services: Vec<String>,
    pub volumes: Vec<BundleVolume>,
    pub images: Vec<String>,
    pub volume_bytes: u64,
    pub image_bytes: u64,
    pub project_bytes: u64,
}

/// 一个命名卷及其恢复方式。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleVolume {
    /// 卷的完整名字（`<项目>_<短名>`），恢复时按原名创建。
    pub name: String,
    /// compose 文件里的短名，写回 `com.docker.compose.volume` 标签用。
    pub short: String,
    /// 宿主机目录不可写时用来挂载该卷做回灌的镜像。
    pub image: String,
}

// ---------------------------------------------------------------------------
// 远端脚本
// ---------------------------------------------------------------------------

/// 快照脚本：预检 -> （暂停源服务）-> 复制项目目录 -> 导出卷 -> 恢复源服务 -> 保存镜像 -> 打包。
pub(crate) const SNAPSHOT_TEMPLATE: &str = r##"#!/usr/bin/env bash
set -euo pipefail
umask 077

ID=__ID__
ROOT=__ROOT__
PROJECT=__PROJECT__
WORKING_DIR=__WORKING_DIR__
COMPOSE_V2=__COMPOSE_V2__
PAUSE=__PAUSE__
INCLUDE_VOLUMES=__INCLUDE_VOLUMES__
INCLUDE_IMAGES=__INCLUDE_IMAGES__
NEED_MB=__NEED_MB__

RUN="$ROOT/$ID"
STAGE="$RUN/bundle"
OUT="$ROOT/$ID.tar"
PID_FILE="$RUN/snapshot.pid"
SCRIPT="$0"
SUCCESS=0

cleanup() {
  rm -f "$PID_FILE" "$SCRIPT"
  if [ "$SUCCESS" = "1" ]; then
    rm -rf "$STAGE"
  else
    rm -rf "$RUN" "$OUT"
  fi
}
trap cleanup EXIT INT TERM

compose() {
  if [ "$COMPOSE_V2" = "1" ]; then docker compose "$@"; else docker-compose "$@"; fi
}

mkdir -p "$STAGE/project" "$STAGE/volumes" "$STAGE/images" || { echo "无法创建远端工作目录 $STAGE" >&2; exit 1; }
echo $$ > "$PID_FILE"

cd "$WORKING_DIR" || { echo "项目目录不存在: $WORKING_DIR" >&2; exit 1; }
CF=()
while IFS= read -r f; do
  [ -n "$f" ] && CF+=(-f "$WORKING_DIR/$f")
done <<'DEPLOYCODE_LIST'
__CONFIG_LIST__
DEPLOYCODE_LIST
if [ ${#CF[@]} -eq 0 ]; then
  echo "备份包清单里没有 compose 配置文件" >&2
  exit 1
fi

echo '###STAGE:1:预检磁盘空间 ...'
FREE_MB=$(df -Pm "$ROOT" | awk 'NR==2 {print $4}')
[ -n "$FREE_MB" ] || FREE_MB=0
if [ "$FREE_MB" -lt "$NEED_MB" ]; then
  echo "空间不足：打包约需 ${NEED_MB}MB，$ROOT 所在分区只剩 ${FREE_MB}MB" >&2
  exit 1
fi

if [ "$PAUSE" = "1" ]; then
  echo '###STAGE:2:暂停来源服务（卷导出完成后立即恢复）...'
  compose -p "$PROJECT" "${CF[@]}" stop --timeout 30
fi

echo '###STAGE:3:复制项目目录 ...'
cp -a "$WORKING_DIR/." "$STAGE/project/"

if [ "$INCLUDE_VOLUMES" = "1" ]; then
  echo '###STAGE:4:导出数据卷 ...'
  while IFS=$'\t' read -r name image; do
    [ -n "$name" ] || continue
    printf '###VOL:%s
' "$name"
    mp=$(docker volume inspect -f '{{.Mountpoint}}' "$name" 2>/dev/null || true)
    if [ -n "$mp" ] && [ -d "$mp" ] && tar -C "$mp" -cf "$STAGE/volumes/$name.tar" . 2>/dev/null; then
      continue
    fi
    if [ -z "$image" ]; then
      echo "数据卷 $name 无法从宿主机读取，且没有可用于容器内导出的镜像" >&2
      exit 1
    fi
    docker run --rm -v "$name":/from:ro -v "$STAGE/volumes":/to "$image" \
      sh -c 'tar -C /from -cf "/to/$1.tar" .' sh "$name" \
      || { echo "数据卷 $name 导出失败（镜像 $image 内需要有 tar 命令）" >&2; exit 1; }
  done <<'DEPLOYCODE_LIST'
__VOLUME_LIST__
DEPLOYCODE_LIST
fi

if [ "$PAUSE" = "1" ]; then
  echo '###STAGE:5:恢复来源服务 ...'
  compose -p "$PROJECT" "${CF[@]}" start
fi

if [ "$INCLUDE_IMAGES" = "1" ]; then
  echo '###STAGE:6:保存镜像（docker save）...'
  # 按序号命名：镜像名里的 `/` `:` 不需要转义，`docker load` 认的是包内自带的 tag。
  n=0
  while IFS= read -r img; do
    [ -n "$img" ] || continue
    n=$((n + 1))
    printf '###IMAGE:%s
' "$img"
    docker save -o "$STAGE/images/$n.tar" "$img"
  done <<'DEPLOYCODE_LIST'
__IMAGE_LIST__
DEPLOYCODE_LIST
fi

echo '###STAGE:7:写入清单并打包 ...'
printf %s __MANIFEST__ > "$STAGE/manifest.json"
tar -cf "$OUT" -C "$STAGE" manifest.json project volumes images
SIZE=$(stat -c%s "$OUT" 2>/dev/null || wc -c < "$OUT")
printf '###SIZE:%s\n' "$SIZE"
echo '###DONE'
SUCCESS=1
"##;

/// 恢复脚本：校验包 -> 载入镜像 -> 建卷并回灌 -> 释放项目文件 -> 启动 -> 校验状态。
pub(crate) const RESTORE_TEMPLATE: &str = r##"#!/usr/bin/env bash
set -euo pipefail
umask 077

ID=__ID__
ROOT=__ROOT__
PROJECT=__PROJECT__
TARGET_DIR=__TARGET_DIR__
COMPOSE_V2=__COMPOSE_V2__
START=__START__

RUN="$ROOT/$ID"
STAGE="$RUN/bundle"
IN="$RUN/bundle.tar"
PID_FILE="$RUN/restore.pid"
SCRIPT="$0"

cleanup() {
  rm -rf "$RUN"
  rm -f "$PID_FILE" "$SCRIPT"
}
trap cleanup EXIT INT TERM

compose() {
  if [ "$COMPOSE_V2" = "1" ]; then docker compose "$@"; else docker-compose "$@"; fi
}

mkdir -p "$RUN" || { echo "无法创建远端工作目录 $RUN" >&2; exit 1; }
echo $$ > "$PID_FILE"

echo '###STAGE:1:校验备份包 ...'
[ -f "$IN" ] || { echo "备份包缺失: $IN" >&2; exit 1; }
tar -tf "$IN" >/dev/null
rm -rf "$STAGE"
mkdir -p "$STAGE"
tar -xf "$IN" -C "$STAGE"

echo '###STAGE:2:载入镜像（docker load）...'
for f in "$STAGE"/images/*.tar; do
  [ -f "$f" ] || continue
  docker load -i "$f"
done

echo '###STAGE:3:恢复数据卷 ...'
while IFS=$'\t' read -r name short image; do
  [ -n "$name" ] || continue
  printf '###VOL:%s
' "$name"
  docker volume create --label "com.docker.compose.project=$PROJECT" \
    --label "com.docker.compose.volume=$short" "$name" >/dev/null
  mp=$(docker volume inspect -f '{{.Mountpoint}}' "$name" 2>/dev/null || true)
  if [ -n "$mp" ] && [ -d "$mp" ] && tar -C "$mp" -xf "$STAGE/volumes/$name.tar" 2>/dev/null; then
    continue
  fi
  if [ -z "$image" ]; then
    echo "数据卷 $name 无法写入宿主机目录，且备份包里没有可用的镜像" >&2
    exit 1
  fi
  docker run --rm -v "$name":/to -v "$STAGE/volumes":/from:ro "$image" \
    sh -c 'tar -C /to -xf "/from/$1.tar"' sh "$name" \
    || { echo "数据卷 $name 恢复失败（镜像 $image 内需要有 tar 命令）" >&2; exit 1; }
done <<'DEPLOYCODE_LIST'
__VOLUME_LIST__
DEPLOYCODE_LIST

echo '###STAGE:4:释放项目文件 ...'
mkdir -p "$TARGET_DIR"
cp -a "$STAGE/project/." "$TARGET_DIR/"

if [ "$START" != "1" ]; then
  echo '###STAGE:5:按设置跳过启动'
  echo '###DONE'
  exit 0
fi

cd "$TARGET_DIR"
CF=()
while IFS= read -r f; do
  [ -n "$f" ] && CF+=(-f "$TARGET_DIR/$f")
done <<'DEPLOYCODE_LIST'
__CONFIG_LIST__
DEPLOYCODE_LIST
# bash 4.2 下 `set -u` 展开空数组会直接报 unbound variable，先给出能看懂的错。
if [ ${#CF[@]} -eq 0 ]; then
  echo "备份包清单里没有 compose 配置文件，无法启动项目" >&2
  exit 1
fi

echo '###STAGE:5:启动服务（compose up -d）...'
compose -p "$PROJECT" "${CF[@]}" up -d --no-build

echo '###STAGE:6:校验容器状态 ...'
running=0
while IFS= read -r s; do
  [ -n "$s" ] || continue
  id=$(docker ps -q --filter "label=com.docker.compose.project=$PROJECT" \
    --filter "label=com.docker.compose.service=$s" --filter status=running | head -n1)
  if [ -n "$id" ]; then running=$((running + 1)); printf '###SERVICE:%s:running
' "$s"; else printf '###SERVICE:%s:not-running
' "$s"; fi
done <<'DEPLOYCODE_LIST'
__SERVICE_LIST__
DEPLOYCODE_LIST
EXPECTED=__EXPECTED__
# 全部服务都没起来才算失败：一次性任务本来就会退出，不能因此判定迁移没成功。
if [ "$EXPECTED" -gt 0 ] && [ "$running" -eq 0 ]; then
  echo "启动后没有任何服务进入 running，请到目标服务器上查看 compose 日志" >&2
  exit 1
fi
echo '###DONE'
"##;

/// 由快照前的探测结果生成备份包清单。
pub(crate) fn build_manifest(
    plan: &ContainerPlan,
    detail: &ComposeStackDetail,
    compose: &[String],
    source: &ServerConfig,
) -> BundleManifest {
    BundleManifest {
        version: 1,
        created_at: now_string(),
        project: plan.project.clone(),
        source_host: format!("{}:{}", source.host, source.port),
        working_dir: detail.stack.working_dir.clone(),
        compose_command: compose.to_vec(),
        config_files: detail
            .stack
            .files
            .iter()
            .map(|file| relative_to(&detail.stack.working_dir, file))
            .collect(),
        services: detail.services.iter().map(|s| s.name.clone()).collect(),
        volumes: if plan.include_volumes {
            detail
                .volumes
                .iter()
                .map(|volume| BundleVolume {
                    name: volume.name.clone(),
                    short: volume_short(&plan.project, &volume.name),
                    image: volume.image.clone(),
                })
                .collect()
        } else {
            Vec::new()
        },
        images: if plan.include_images {
            detail.images.clone()
        } else {
            Vec::new()
        },
        volume_bytes: if plan.include_volumes { detail.volume_bytes } else { 0 },
        image_bytes: if plan.include_images { detail.image_bytes } else { 0 },
        project_bytes: detail.project_bytes,
    }
}

pub(crate) fn build_snapshot_script(
    record_id: &str,
    root: &str,
    plan: &ContainerPlan,
    manifest: &BundleManifest,
) -> String {
    render_template(
        SNAPSHOT_TEMPLATE,
        &[
            ("__ID__", shell_quote(record_id)),
            ("__ROOT__", shell_quote(root)),
            ("__PROJECT__", shell_quote(&manifest.project)),
            ("__WORKING_DIR__", shell_quote(&manifest.working_dir)),
            (
                "__COMPOSE_V2__",
                shell_quote(&if is_compose_v2(&manifest.compose_command) { "1" } else { "0" }),
            ),
            ("__PAUSE__", shell_quote(&if plan.pause_source { "1" } else { "0" })),
            (
                "__INCLUDE_VOLUMES__",
                shell_quote(&if !manifest.volumes.is_empty() { "1" } else { "0" }),
            ),
            (
                "__INCLUDE_IMAGES__",
                shell_quote(&if !manifest.images.is_empty() { "1" } else { "0" }),
            ),
            ("__NEED_MB__", shell_quote(&estimate_mb(manifest).to_string())),
            ("__CONFIG_LIST__", list_text(&manifest.config_files)),
            ("__VOLUME_LIST__", snapshot_volume_list(&manifest.volumes)),
            ("__IMAGE_LIST__", list_text(&manifest.images)),
            (
                "__MANIFEST__",
                manifest_literal(manifest),
            ),
        ],
    )
}

pub(crate) fn build_restore_script(
    record_id: &str,
    root: String,
    target_dir: &str,
    start_services: bool,
    manifest: &BundleManifest,
    compose: &[String],
) -> String {
    render_template(
        RESTORE_TEMPLATE,
        &[
            ("__ID__", shell_quote(record_id)),
            ("__ROOT__", shell_quote(&root)),
            ("__PROJECT__", shell_quote(&manifest.project)),
            ("__TARGET_DIR__", shell_quote(target_dir)),
            (
                "__COMPOSE_V2__",
                shell_quote(&if is_compose_v2(compose) { "1" } else { "0" }),
            ),
            ("__START__", shell_quote(&if start_services { "1" } else { "0" })),
            ("__VOLUME_LIST__", restore_volume_list(&manifest.volumes)),
            ("__CONFIG_LIST__", list_text(&manifest.config_files)),
            ("__SERVICE_LIST__", list_text(&manifest.services)),
            ("__EXPECTED__", shell_quote(&manifest.services.len().to_string())),
        ],
    )
}

/// 清单以单引号字面量落进脚本，内容原样写盘，不经 shell 解释。
pub(crate) fn manifest_literal(manifest: &BundleManifest) -> String {
    let json = serde_json::to_string(manifest).unwrap_or_else(|_| "{}".to_string());
    shell_quote(&json)
}

/// 两份脚本里 here-doc 的终止符。
const LIST_TERMINATOR: &str = "DEPLOYCODE_LIST";

/// 条目能否安全写进 here-doc。
///
/// 配置文件名 / 卷名 / 镜像名来自来源服务器，属于外部输入：整行等于终止符时会提前结束
/// here-doc，剩下的内容就被当成脚本文本执行，这类条目直接丢掉（宁可少带一个文件）。
fn listable(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty() && value != LIST_TERMINATOR && !value.chars().any(char::is_control)
}

/// 列表写进 here-doc：每行一项。
pub(crate) fn list_text(values: &[String]) -> String {
    rows(values
        .iter()
        .map(|value| vec![value.trim().to_string()])
        .filter(|row| listable(&row[0]))
        .collect())
}

/// 快照脚本的卷清单：`名字<TAB>镜像`。
pub(crate) fn snapshot_volume_list(volumes: &[BundleVolume]) -> String {
    rows(volumes
        .iter()
        .filter(|volume| listable(&volume.name))
        .map(|volume| vec![volume.name.trim().to_string(), image_column(volume)])
        .collect())
}

/// 恢复脚本的卷清单：`名字<TAB>短名<TAB>镜像`，短名写回 `com.docker.compose.volume` 标签。
pub(crate) fn restore_volume_list(volumes: &[BundleVolume]) -> String {
    rows(volumes
        .iter()
        .filter(|volume| listable(&volume.name) && listable(&volume.short))
        .map(|volume| {
            vec![
                volume.name.trim().to_string(),
                volume.short.trim().to_string(),
                image_column(volume),
            ]
        })
        .collect())
}

/// 镜像名不合规则留空：脚本会给出「没有可用镜像」的明确报错，而不是拼出空参数的 docker run。
fn image_column(volume: &BundleVolume) -> String {
    if listable(&volume.image) {
        volume.image.trim().to_string()
    } else {
        String::new()
    }
}

fn rows(items: Vec<Vec<String>>) -> String {
    items
        .into_iter()
        .map(|columns| columns.join("\t"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn is_compose_v2(compose: &[String]) -> bool {
    compose.len() == 2 && compose[0] == "docker" && compose[1] == "compose"
}

/// 需要的空间 = 项目目录 + 卷 + 镜像，再加 30% 余量（tar 打包时源文件与包并存）。
pub(crate) fn estimate_mb(manifest: &BundleManifest) -> u64 {
    let bytes = manifest.project_bytes + manifest.volume_bytes + manifest.image_bytes;
    (bytes + bytes / 3) / (1024 * 1024) + 64
}

/// 目标目录：显式给的优先，否则沿用备份包里记录的原目录。
pub(crate) fn target_dir_or(manifest: &BundleManifest, target: &ContainerTarget) -> Result<String> {
    if target.target_dir.trim().is_empty() {
        validate_remote_dir(&manifest.working_dir)
    } else {
        validate_remote_dir(&target.target_dir)
    }
}

/// 读取备份包内的 `manifest.json`（本机 tar 命令；Windows 10+ 自带 bsdtar）。
pub fn read_bundle_manifest(bundle: &Path) -> Result<BundleManifest> {
    let output = crate::process::run_timeout(
        "tar",
        &[
            "-xOf".to_string(),
            bundle.to_string_lossy().into_owned(),
            "manifest.json".to_string(),
        ],
        None,
        Duration::from_secs(120),
    )
    .map_err(|err| CoreError::backup(format!("读取备份包清单失败: {err}")))?;
    if !output.success() {
        return Err(CoreError::backup(format!(
            "读取备份包清单失败（请确认备份包未损坏、本机 tar 命令可用）: {}",
            output.combined()
        )));
    }
    let manifest: BundleManifest = serde_json::from_str(&output.stdout).map_err(|err| {
        CoreError::backup(format!(
            "备份包清单解析失败（{}）: {err}",
            bundle.display()
        ))
    })?;
    if manifest.version != 1 {
        return Err(CoreError::backup(format!(
            "不支持的备份包版本：{}（本程序只认 1）",
            manifest.version
        )));
    }
    if validate_project(&manifest.project).is_err() || manifest.working_dir.trim().is_empty() {
        return Err(CoreError::backup("备份包清单缺少项目名或原目录，无法恢复"));
    }
    Ok(manifest)
}
