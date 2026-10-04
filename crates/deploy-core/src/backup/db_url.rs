//! 备份目标连接串的校验、解析与脱敏。

use crate::error::{CoreError, Result};

pub(super) fn validate_target_url(url: &str) -> Result<String> {
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Ok(url.to_string())
    } else {
        Err(CoreError::config(
            "备份目标连接串必须以 postgres:// 或 postgresql:// 开头",
        ))
    }
}

/// 定位连接串 authority 的结束位置，以及 userinfo 分界 `@` 的位置（如果有）。
///
/// 常规情况：authority 截止到第一个 `/`、`?`、`#`。
/// 密码里可能出现未编码的 `/`（如 `user:p/ss@host`），此时第一个 `/` 之前没有 `@`，
/// 但查询 / 片段之前存在 `@`。
///
/// `assume_late_at_is_userinfo` 决定这种歧义输入的取舍：
/// - `true`（脱敏展示）：一律当作 userinfo，宁可多遮蔽也不能漏出密码明文；
///   代价是 `host:5432/db@name` 会被显示成 `host:***@name`。
/// - `false`（连接串解析）：只有第一个 `/` 前的部分不像 `host[:port]` 时才当作 userinfo，
///   且取第一个 `@`（避免密码含 `/` 且路径也含 `@` 时把路径的 `@` 当成分界）。
///   歧义输入保持原样交给 psql，宁可报错也不要连到错误的主机 / 库。
fn locate_authority(rest: &str, assume_late_at_is_userinfo: bool) -> (usize, Option<usize>) {
    let first_sep = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host_end_after = |at: usize| {
        rest[at + 1..]
            .find(['/', '?', '#'])
            .map(|offset| at + 1 + offset)
            .unwrap_or(rest.len())
    };

    if let Some(at) = rest[..first_sep].rfind('@') {
        return (host_end_after(at), Some(at));
    }

    let query_limit = rest.find(['?', '#']).unwrap_or(rest.len());
    let late = if assume_late_at_is_userinfo {
        rest[..query_limit].rfind('@')
    } else {
        rest[first_sep..query_limit]
            .find('@')
            .map(|offset| first_sep + offset)
    };
    match late {
        Some(at) if assume_late_at_is_userinfo || !looks_like_host_port(&rest[..first_sep]) => {
            (host_end_after(at), Some(at))
        }
        _ => (first_sep, None),
    }
}

/// 判断 authority 片段是否像 `host[:port]`（含 IPv6）：端口必须是纯数字。
fn looks_like_host_port(prefix: &str) -> bool {
    if let Some(rest) = prefix.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return false;
        };
        let tail = &rest[end + 1..];
        return tail.is_empty()
            || tail
                .strip_prefix(':')
                .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()));
    }
    match prefix.split_once(':') {
        Some((_, port)) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
        None => true,
    }
}

/// 隐藏连接串中的密码（userinfo 与查询参数），用于日志与记录展示。
pub fn mask_database_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..scheme_end];
    let rest = &url[scheme_end + 3..];

    // 脱敏按「宁可多遮蔽」处理：不能因为无法区分而漏出密码明文。
    let (authority_end, at) = locate_authority(rest, true);
    let authority = match at {
        Some(at) => {
            let creds = &rest[..at];
            let user = creds.split(':').next().unwrap_or(creds);
            let host = &rest[at + 1..authority_end];
            if creds.contains(':') {
                format!("{user}:***@{host}")
            } else {
                format!("{user}@{host}")
            }
        }
        None => rest[..authority_end].to_string(),
    };

    format!(
        "{scheme}://{authority}{}",
        mask_query_password(&rest[authority_end..])
    )
}

/// 把查询串中的 `password=...` 值替换为 `***`，保留其他参数。
fn mask_query_password(remainder: &str) -> String {
    let Some(question) = remainder.find('?') else {
        return remainder.to_string();
    };
    let hash = remainder.find('#').unwrap_or(remainder.len());
    if question > hash {
        return remainder.to_string();
    }
    let prefix = &remainder[..question];
    let query = &remainder[question + 1..hash];
    let fragment = &remainder[hash..];
    let masked = query
        .split('&')
        .map(|param| {
            let key = param.split('=').next().unwrap_or(param);
            if key.eq_ignore_ascii_case("password") && param.contains('=') {
                format!("{key}=***")
            } else {
                param.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{prefix}?{masked}{fragment}")
}

/// 从连接串中拆分出「去掉密码的连接串」与「密码」。
///
/// 密码可能来自 userinfo（`postgres://user:pass@host/db`）或查询参数
/// （`...?password=pass`）。psql 通过 PGPASSWORD 读取原始密码，
/// 因此去掉密码的连接串可安全地放进命令行/进程环境，不再暴露给 `ps`。
pub fn split_database_url(url: &str) -> Result<(String, Option<String>)> {
    let scheme_end = url
        .find("://")
        .ok_or_else(|| CoreError::config("连接串格式无效"))?;
    let scheme = &url[..scheme_end];
    if scheme != "postgres" && scheme != "postgresql" {
        return Err(CoreError::config(
            "备份目标连接串必须以 postgres:// 或 postgresql:// 开头",
        ));
    }
    let rest = &url[scheme_end + 3..];
    // 歧义输入（如密码含未编码 '/'）保守处理：拿不准就不剥离密码，让 psql 报错而不是连错库。
    let (authority_end, _) = locate_authority(rest, false);
    let authority = &rest[..authority_end];
    let remainder = &rest[authority_end..];

    // 查询参数里的 password 先取出并从连接串移除；userinfo 中的密码优先级更高。
    let (remainder, query_password) = strip_query_password(remainder);

    let mut password = query_password;
    let authority = match authority.rfind('@') {
        Some(at) => {
            let userinfo = &authority[..at];
            let host = &authority[at + 1..];
            match userinfo.find(':') {
                Some(colon) => {
                    password = Some(percent_decode(&userinfo[colon + 1..]));
                    format!("{}@{host}", &userinfo[..colon])
                }
                None => authority.to_string(),
            }
        }
        None => authority.to_string(),
    };

    Ok((format!("{scheme}://{authority}{remainder}"), password))
}

/// 移除查询串中的 `password` 参数，返回新的 remainder 与解码后的密码。
fn strip_query_password(remainder: &str) -> (String, Option<String>) {
    let Some(question) = remainder.find('?') else {
        return (remainder.to_string(), None);
    };
    let hash = remainder.find('#').unwrap_or(remainder.len());
    if question > hash {
        return (remainder.to_string(), None);
    }
    let prefix = &remainder[..question];
    let query = &remainder[question + 1..hash];
    let fragment = &remainder[hash..];

    let mut password = None;
    let mut kept: Vec<&str> = Vec::new();
    for param in query.split('&') {
        let key = param.split('=').next().unwrap_or(param);
        if key.eq_ignore_ascii_case("password") {
            if password.is_none() {
                password = param
                    .split_once('=')
                    .map(|(_, value)| percent_decode(value));
            }
        } else {
            kept.push(param);
        }
    }

    let mut rebuilt = prefix.to_string();
    if !kept.is_empty() {
        rebuilt.push('?');
        rebuilt.push_str(&kept.join("&"));
    }
    rebuilt.push_str(fragment);
    (rebuilt, password)
}

/// 百分号解码（`%XX` 十六进制）；非 UTF-8 字节按 lossy 处理。
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_database_url_hides_password() {
        assert_eq!(
            mask_database_url("postgresql://postgres.abc:secret@aws-0.pooler.supabase.com:5432/postgres"),
            "postgresql://postgres.abc:***@aws-0.pooler.supabase.com:5432/postgres"
        );
        assert_eq!(
            mask_database_url("postgresql://postgres:p@ss@host:5432/db"),
            "postgresql://postgres:***@host:5432/db"
        );
        assert_eq!(
            mask_database_url("postgresql://postgres@host:5432/db"),
            "postgresql://postgres@host:5432/db"
        );
        // 查询参数里的密码同样要隐藏。
        assert_eq!(
            mask_database_url("postgresql://u@host:5432/db?password=secret&sslmode=require"),
            "postgresql://u@host:5432/db?password=***&sslmode=require"
        );
        assert_eq!(
            mask_database_url("postgresql://u@host:5432/db?a=1&Password=secret#frag"),
            "postgresql://u@host:5432/db?a=1&Password=***#frag"
        );
        // 密码含未编码 '/' 时也必须遮蔽（不能按第一个 '/' 截断 authority）。
        assert_eq!(
            mask_database_url("postgresql://user:p/ss@host:5432/db"),
            "postgresql://user:***@host:5432/db"
        );
        // 密码前段是纯数字时同样必须遮蔽（宁可把路径里的 '@' 一起遮掉，也不能漏明文）。
        assert_eq!(
            mask_database_url("postgresql://admin:888888/password@host/db"),
            "postgresql://admin:***@host/db"
        );
        assert_eq!(
            mask_database_url("postgresql://host:5432/db@name"),
            "postgresql://host:***@name"
        );
        // 查询参数里的 '@' 不应被当作 host 分界。
        assert_eq!(
            mask_database_url("postgresql://host/db?user=a@b&password=s@cret"),
            "postgresql://host/db?user=a@b&password=***"
        );
        assert_eq!(mask_database_url("not-a-url"), "not-a-url");
    }

    #[test]
    fn split_database_url_extracts_password() {
        let (url, password) = split_database_url("postgresql://u:secret@h:5432/db").unwrap();
        assert_eq!(url, "postgresql://u@h:5432/db");
        assert_eq!(password.as_deref(), Some("secret"));
        assert!(!url.contains("secret"));

        // 密码中的特殊字符先做百分号解码，交给 PGPASSWORD 的是原始值。
        let (url, password) = split_database_url("postgres://u:p%40ss%3Aword@h/db").unwrap();
        assert_eq!(url, "postgres://u@h/db");
        assert_eq!(password.as_deref(), Some("p@ss:word"));

        // 密码含未编码 '/' 时不能按第一个 '/' 截断（与 mask_database_url 行为一致）。
        let (url, password) = split_database_url("postgresql://user:p/ss@host:5432/db").unwrap();
        assert_eq!(url, "postgresql://user@host:5432/db");
        assert_eq!(password.as_deref(), Some("p/ss"));

        // 路径里的 '@'（前缀是 host:port）不能被当成 userinfo。
        let (url, password) = split_database_url("postgresql://host:5432/db@name").unwrap();
        assert_eq!(url, "postgresql://host:5432/db@name");
        assert!(password.is_none());

        // 密码含 '/' 且路径也含 '@'：取第一个 '@' 做分界，不能把路径的 '@' 当分界。
        let (url, password) =
            split_database_url("postgresql://user:p/ss@host:5432/db@name").unwrap();
        assert_eq!(url, "postgresql://user@host:5432/db@name");
        assert_eq!(password.as_deref(), Some("p/ss"));

        // 无法区分「密码含 '/'」与「host:port + 路径」时保守处理：不剥离密码，交给 psql 报错。
        let (url, password) = split_database_url("postgresql://admin:888888/password@host/db").unwrap();
        assert_eq!(url, "postgresql://admin:888888/password@host/db");
        assert!(password.is_none());

        // 查询参数中的 password 也会被提取并从连接串移除。
        let (url, password) =
            split_database_url("postgresql://u@h:5432/db?password=se%20cret&sslmode=require")
                .unwrap();
        assert_eq!(url, "postgresql://u@h:5432/db?sslmode=require");
        assert_eq!(password.as_deref(), Some("se cret"));

        // 唯一的参数被移除后不残留 '?'。
        let (url, password) = split_database_url("postgresql://u@h/db?Password=secret").unwrap();
        assert_eq!(url, "postgresql://u@h/db");
        assert_eq!(password.as_deref(), Some("secret"));

        // 中间参数被移除后其余参数与 fragment 正常拼接。
        let (url, _) = split_database_url("postgresql://u@h/db?a=1&password=x&b=2#frag").unwrap();
        assert_eq!(url, "postgresql://u@h/db?a=1&b=2#frag");

        // userinfo 中的密码优先于查询参数。
        let (url, password) =
            split_database_url("postgresql://u:first@h/db?password=second").unwrap();
        assert_eq!(url, "postgresql://u@h/db");
        assert_eq!(password.as_deref(), Some("first"));

        // 没有密码时保持原样。
        let (url, password) = split_database_url("postgresql://u@h:5432/db").unwrap();
        assert_eq!(url, "postgresql://u@h:5432/db");
        assert!(password.is_none());

        assert!(split_database_url("mysql://u:p@h/db").is_err());
        assert!(split_database_url("not-a-url").is_err());
    }
}
