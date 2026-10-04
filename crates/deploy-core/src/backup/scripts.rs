//! 远端备份脚本模板与渲染。

use crate::models::{BackupRecord, DbBackupSource};
use crate::process::shell_quote;

// ---------------------------------------------------------------------------
// 远端脚本
// ---------------------------------------------------------------------------

/// 备份脚本模板：来源导出 -> 校验 -> 清空目标 schema -> 导入。
const BACKUP_TEMPLATE: &str = r##"#!/usr/bin/env bash
set -euo pipefail
umask 077

OUT="/tmp/deploycode-backup-__ID__.sql.gz"
PID_FILE="/tmp/deploycode-backup-__ID__.pid"
export TARGET_URL=__TARGET_URL__
export TARGET_PASSWORD=__TARGET_PASSWORD__
MODE=__MODE__
CONTAINER=__CONTAINER__
DB_NAME=__DB_NAME__
DB_USER=__DB_USER__
DB_PASSWORD=__DB_PASSWORD__
SCHEMA=__SCHEMA__
KEEP=__KEEP__

# KEEP=1 时把导出留给调用方下载（下载完由调用方删），否则照旧一走了之。
cleanup() {
  if [ "$KEEP" = "1" ]; then
    rm -f "$PID_FILE" "$0"
  else
    rm -f "$OUT" "$PID_FILE" "$0"
  fi
}
trap cleanup EXIT
# 信号那一路必须带着 exit，而且导出文件照删。只 trap cleanup INT TERM 时 bash 跑完 handler
# 会接着往下执行（本机实测：cleanup 之后下一行照样打出来），而到这一步 pidfile 与脚本自身都已删掉，
# 后面那句 DROP SCHEMA + 导入照跑 —— 本地却早已把这次算成失败，用户点重试就是两个脚本并发写同一个
# 目标 schema。被信号打断时不会有谁来下载，所以这里连 KEEP=1 也删掉导出。
# HUP 也要接：任务被 abort 时本地会话先没，远端收到的是 SIGHUP 而不是 TERM（与容器脚本同一口径）。
trap 'rm -f "$OUT" "$PID_FILE" "$0"; exit 1' INT TERM HUP
echo $$ > "$PID_FILE"

# Supabase 等托管服务在公网访问，需要 psql 客户端：优先用服务器本机的，退回数据库容器自带的。
if command -v psql >/dev/null 2>&1; then
  USE_DOCKER_PSQL=0
elif [ "$MODE" = "docker" ] && docker exec "$CONTAINER" sh -c 'command -v psql >/dev/null 2>&1'; then
  USE_DOCKER_PSQL=1
else
  echo '目标端缺少 psql 客户端：请安装 postgresql-client，或使用 docker 模式（PG 镜像自带 psql）' >&2
  exit 1
fi

run_psql() {
  if [ "$USE_DOCKER_PSQL" = "1" ]; then
    docker exec -i -e TARGET_URL -e PGPASSWORD "$CONTAINER" sh -c 'psql "$TARGET_URL" "$@"' psql "$@"
  else
    psql "$TARGET_URL" "$@"
  fi
}

echo '###STAGE:1:正在导出数据库' "$DB_NAME" '...'
export PGPASSWORD="$DB_PASSWORD"
if [ "$MODE" = "docker" ]; then
  docker exec -e PGPASSWORD "$CONTAINER" pg_dump -U "$DB_USER" -d "$DB_NAME" -n "$SCHEMA" --no-owner --no-acl
else
  pg_dump -U "$DB_USER" -d "$DB_NAME" -n "$SCHEMA" --no-owner --no-acl
fi | gzip > "$OUT"
unset PGPASSWORD

export PGPASSWORD="$TARGET_PASSWORD"
SIZE=$(stat -c%s "$OUT" 2>/dev/null || wc -c < "$OUT")
printf '###SIZE:%s\n' "$SIZE"
echo '###STAGE:2:正在校验备份文件 ...'
gzip -t "$OUT"

echo '###STAGE:3:正在清空目标 schema ...'
run_psql -v ON_ERROR_STOP=1 -q <<'DEPLOYCODE_SQL'
__RESET_SQL__
DEPLOYCODE_SQL

echo '###STAGE:4:正在导入到目标数据库 ...'
# pg_dump -n 在较新版本（PG15+，或 public 的属主/注释被改过）会输出 CREATE SCHEMA public;
# 目标 schema 已由上面的重置步骤创建并授权，直接导入会因 "schema already exists" 中断（ON_ERROR_STOP）。
# 这里只过滤「紧跟 Schema TOC 注释的那条 CREATE SCHEMA」，避免误删数据里同名的文本行；
# COPY 数据段原样放行，避免数据里恰好出现形似 TOC 注释的行时被误删。
gunzip -c "$OUT" | awk '
  in_copy && /^\\\.$/ { in_copy = 0; print; next }
  in_copy { print; next }
  /^COPY .* FROM stdin;$/ { in_copy = 1; print; next }
  /^-- Name: / { schema_toc = ($0 ~ /; Type: SCHEMA;/) }
  schema_toc && /^CREATE SCHEMA / { schema_toc = 0; next }
  schema_toc && /^[^-[:space:]]/ { schema_toc = 0 }
  { print }
' | run_psql -v ON_ERROR_STOP=1 -q

echo '###DONE'
"##;

/// 环境检查脚本：pg_dump 版本 + Supabase 连接测试。
const TEST_TEMPLATE: &str = r##"#!/usr/bin/env bash
set -euo pipefail

export TARGET_URL=__TARGET_URL__
export TARGET_PASSWORD=__TARGET_PASSWORD__
export PGPASSWORD="$TARGET_PASSWORD"
MODE=__MODE__
CONTAINER=__CONTAINER__
trap 'rm -f "$0"' EXIT INT TERM

echo -n "pg_dump: "
if [ "$MODE" = "docker" ]; then
  docker exec "$CONTAINER" pg_dump --version
else
  pg_dump --version
fi

echo -n "目标数据库: "
if command -v psql >/dev/null 2>&1; then
  psql "$TARGET_URL" -Atc 'select current_database() || chr(64) || current_user'
elif docker exec "$CONTAINER" sh -c 'command -v psql >/dev/null 2>&1'; then
  docker exec -i -e TARGET_URL -e PGPASSWORD "$CONTAINER" sh -c 'psql "$TARGET_URL" -Atc "select current_database() || chr(64) || current_user"'
else
  echo '缺少 psql 客户端' >&2
  exit 1
fi
"##;

/// 远端导出文件路径。脚本模板里是字面量，这里拼一份给下载与善后用 —— 两处必须同规则。
pub(super) fn remote_dump_path(record_id: &str) -> String {
    format!("/tmp/deploycode-backup-{record_id}.sql.gz")
}

pub(super) fn build_backup_script(
    record: &BackupRecord,
    source: &DbBackupSource,
    target_url: &str,
    target_password: Option<&str>,
    keep: bool,
) -> String {
    render_template(
        BACKUP_TEMPLATE,
        &[
            ("__ID__", record.id.clone()),
            ("__KEEP__", if keep { "1" } else { "0" }.to_string()),
            ("__TARGET_URL__", shell_quote(target_url)),
            // 目标密码可选：配置中未设置时为空字符串（不影响执行）
            (
                "__TARGET_PASSWORD__",
                shell_quote(target_password.unwrap_or("")),
            ),
            ("__MODE__", shell_quote(&source.mode)),
            ("__CONTAINER__", shell_quote(&source.container)),
            ("__DB_NAME__", shell_quote(&source.database)),
            ("__DB_USER__", shell_quote(&source.username)),
            ("__DB_PASSWORD__", shell_quote(&source.password)),
            ("__SCHEMA__", shell_quote(&source.schema)),
            ("__RESET_SQL__", reset_schema_sql(&source.schema)),
        ],
    )
}

pub(super) fn build_test_script(
    source: &DbBackupSource,
    target_url: &str,
    target_password: Option<&str>,
) -> String {
    render_template(
        TEST_TEMPLATE,
        &[
            ("__TARGET_URL__", shell_quote(target_url)),
            // 目标密码可选：配置中未设置时为空字符串（不影响执行）
            (
                "__TARGET_PASSWORD__",
                shell_quote(target_password.unwrap_or("")),
            ),
            ("__MODE__", shell_quote(&source.mode)),
            ("__CONTAINER__", shell_quote(&source.container)),
        ],
    )
}

/// 单遍渲染模板占位符：替换结果不再被扫描，避免值中的 `__KEY__` 触发二次替换。
pub(crate) fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("__") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("__") else {
            output.push_str(&rest[start..]);
            return output;
        };
        // 占位符整体（含两侧下划线）与 values 中的键匹配。
        let placeholder = &rest[start..start + 2 + end + 2];
        match values.iter().find(|(name, _)| *name == placeholder) {
            Some((_, value)) => output.push_str(value),
            None => output.push_str(placeholder),
        }
        rest = &after[end + 2..];
    }
    output.push_str(rest);
    output
}

/// 生成「清空并重建 schema」SQL；重建后恢复 Supabase 默认角色授权。
fn reset_schema_sql(schema: &str) -> String {
    let ident = sql_ident(schema);
    let base = format!(
        "DROP SCHEMA IF EXISTS {ident} CASCADE;\n\
         CREATE SCHEMA {ident};\n\
         GRANT ALL ON SCHEMA {ident} TO PUBLIC;"
    );
    // 标识符内嵌到 EXECUTE 的字符串字面量时，只需转义单引号（双引号是标识符本身的定界符）。
    let literal = ident.replace('\'', "''");
    format!(
        "{base}\n\
         DO $do$\n\
         BEGIN\n\
         \x20 IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'anon') THEN\n\
         \x20   EXECUTE 'GRANT USAGE ON SCHEMA {literal} TO anon, authenticated, service_role';\n\
         \x20   EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA {literal} GRANT ALL ON TABLES TO anon, authenticated, service_role';\n\
         \x20   EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA {literal} GRANT ALL ON SEQUENCES TO anon, authenticated, service_role';\n\
         \x20   EXECUTE 'ALTER DEFAULT PRIVILEGES IN SCHEMA {literal} GRANT ALL ON FUNCTIONS TO anon, authenticated, service_role';\n\
         \x20 END IF;\n\
         END\n\
         $do$;"
    )
}

fn sql_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::testutil::{record, source};

    #[test]
    fn script_quotes_credentials_and_keeps_stage_markers() {
        let script = build_backup_script(
            &record(),
            &source(),
            "postgresql://u@h:5432/db",
            Some("secret"),
            false,
        );
        assert!(script.contains("###STAGE:1"));
        assert!(script.contains("###STAGE:4"));
        assert!(script.contains("###DONE"));
        assert!(
            script.contains("DB_PASSWORD='p'\\''ass'"),
            "password not quoted: {script}"
        );
        assert!(script.contains(
            "pg_dump -U \"$DB_USER\" -d \"$DB_NAME\" -n \"$SCHEMA\" --no-owner --no-acl"
        ));
        assert!(script.contains("docker exec -e PGPASSWORD"));
        // 查询数据库后必须先撤下来源库密码，再为导入阶段设置目标库密码。
        assert!(script.contains("unset PGPASSWORD"));
        assert!(script.contains("export PGPASSWORD=\"$TARGET_PASSWORD\""));
        assert!(script.contains("docker exec -i -e TARGET_URL -e PGPASSWORD"));
        // dump 文件位于 /tmp，需要收紧权限。
        assert!(script.contains("umask 077"));
        // 目标密码不再出现在连接串里，只通过环境变量传递。
        let target_line = script
            .lines()
            .find(|line| line.starts_with("export TARGET_URL="))
            .unwrap();
        assert!(target_line.contains("u@h:5432/db"));
        assert!(
            !target_line.contains("secret"),
            "password leaked into url: {target_line}"
        );
        assert!(script.contains("export TARGET_PASSWORD=secret"));
        // 数据库名不得插入单引号字符串内部（防止引号错位/命令注入）。
        assert!(!script.contains("__DB_NAME__"));
        assert!(script.contains("echo '###STAGE:1:正在导出数据库' \"$DB_NAME\" '...'"));
        // 超时后可通过 pidfile 终止远端脚本。
        assert!(script.contains("PID_FILE=\"/tmp/deploycode-backup-abc.pid\""));
        assert!(script.contains("echo $$ > \"$PID_FILE\""));
    }

    #[test]
    fn retention_flag_decides_whether_the_dump_survives_the_script() {
        let url = "postgresql://u@h:5432/db";
        let keep = build_backup_script(&record(), &source(), url, None, true);
        assert!(keep.contains("KEEP=1"));
        // 留存模式下 cleanup 放过 $OUT，本机才有东西可下载；pidfile 与脚本仍旧照删。
        assert!(keep.contains("if [ \"$KEEP\" = \"1\" ]"));
        assert!(keep.contains("rm -f \"$PID_FILE\" \"$0\""));

        let discard = build_backup_script(&record(), &source(), url, None, false);
        assert!(discard.contains("KEEP=0"));
        assert!(discard.contains("rm -f \"$OUT\" \"$PID_FILE\" \"$0\""));
    }

    /// 信号那一路必须既删导出又停下：handler 不含 exit 时 bash 跑完清理还会继续往下执行，
    /// 那时 pidfile 与脚本自身都已删掉，而 DROP SCHEMA + 导入照跑 —— 本地早已把这次算成失败，
    /// 用户点重试就是两个脚本并发写同一个目标 schema。被信号打断时没人来下载，导出也一并删。
    #[test]
    fn signal_trap_deletes_the_dump_and_stops_the_script() {
        let script = build_backup_script(&record(), &source(), "postgresql://u@h:5432/db", None, true);
        assert!(script.contains("trap 'rm -f \"$OUT\" \"$PID_FILE\" \"$0\"; exit 1' INT TERM HUP"));
        // 老写法把 EXIT 与信号合用一个不含 exit 的 handler，等于「清完照跑」。
        assert!(!script.contains("trap cleanup EXIT INT TERM"));
    }

    #[test]
    fn template_values_with_reset_sql_placeholder_are_not_rescanned() {
        let mut src = source();
        src.database = "x__RESET_SQL__y$(touch /tmp/pwned)".to_string();
        src.schema = "s__RESET_SQL__$(id)".to_string();
        let script = build_backup_script(
            &record(),
            &src,
            "postgresql://u@h:5432/db",
            Some("secret"),
            false,
        );
        // 值整体留在单引号内，内部的 __RESET_SQL__ 不会被二次替换成裸 SQL。
        assert!(
            script.contains("DB_NAME='x__RESET_SQL__y$(touch /tmp/pwned)'"),
            "database value was rescanned: {script}"
        );
        assert!(script.contains("SCHEMA='s__RESET_SQL__$(id)'"));
        // 重置 SQL 只出现一次（仅来自模板自身的 __RESET_SQL__ 占位符）。
        assert_eq!(script.matches("DROP SCHEMA IF EXISTS").count(), 1);
    }

    #[test]
    fn reset_sql_drops_and_recreates_schema_with_grants() {
        let sql = reset_schema_sql("public");
        assert!(sql.contains("DROP SCHEMA IF EXISTS \"public\" CASCADE;"));
        assert!(sql.contains("CREATE SCHEMA \"public\";"));
        assert!(sql.contains("anon, authenticated, service_role"));
        assert!(sql.contains("$do$"));
        // EXECUTE 字符串里的标识符必须是 "public"，不能是 '"public"'（会语法错误）。
        assert!(sql.contains("EXECUTE 'GRANT USAGE ON SCHEMA \"public\" TO"));
        assert!(!sql.contains("'\"public\"'"));
        // schema 名含单引号时字面量转义，不破坏 SQL。
        let tricky = reset_schema_sql("it's");
        assert!(tricky.contains("EXECUTE 'GRANT USAGE ON SCHEMA \"it''s\" TO"));
    }

    #[test]
    fn import_filters_dump_created_schema_statement() {
        let script = build_backup_script(
            &record(),
            &source(),
            "postgresql://u@h:5432/db",
            Some("secret"),
            false,
        );
        // 导入时必须过滤 pg_dump 自带的 CREATE SCHEMA，否则目标 schema 已存在会中断导入。
        assert!(
            script.contains("gunzip -c \"$OUT\" | awk"),
            "import pipeline missing schema filter: {script}"
        );
        assert!(script.contains("schema_toc"));
        // COPY 数据段必须原样放行，避免数据行恰好形似 TOC 注释时被误删。
        assert!(script.contains("in_copy"));
        // 过滤后仍以 ON_ERROR_STOP 严格导入。
        assert!(script.contains("| run_psql -v ON_ERROR_STOP=1 -q"));
    }
}
