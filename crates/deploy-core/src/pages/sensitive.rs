//! Pages 产物敏感文件扫描 —— 提交拦截那条红线在 Pages 上的补位。
//!
//! Cloudflare 是把整个目录推上去、gh-pages 带着 `--dotfiles` 直接建提交推到发布分支
//! （多半是公开站点），两条都不过 `Git::commit_all`，所以 `.env`、`*.pem` 这类文件
//! 原本可以一声不吭地被发布到公网。宁可拒绝这一次发布，让用户把它们从产物里排掉。

use std::path::Path;

use crate::error::{CoreError, Result};

/// 读产物目录失败：红线判定跑不完就不能当它是干净的。
fn unreadable_output_dir(dir: &Path, err: std::io::Error) -> CoreError {
    CoreError::pages(format!(
        "无法读取产物目录 {}：{err}（敏感文件判定跑不完，就不能当它是干净的）",
        dir.display()
    ))
}

/// 产物目录里的疑似敏感文件（相对路径，最多列 20 条）。
fn sensitive_files_in(dir: &Path) -> Result<Vec<String>> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current)
            .map_err(|err| unreadable_output_dir(&current, err))?;
        for entry in entries {
            let entry = entry.map_err(|err| unreadable_output_dir(&current, err))?;
            let path = entry.path();
            // 按 `file_type()` 判目录：它不跟随符号链接，而 `path.is_dir()` 会 ——
            // 产物里一个指回祖先的软链（`latest -> .` 这类）就能让这趟遍历没完没了。
            // 不跟随也不漏：两条发布路径都不搬链接指向的内容（gh-pages 走 git，
            // 符号链接以 120000 模式原样入库；wrangler 传的是目录里的普通文件），
            // 要拦的是这个链接自己的名字，所以下面仍按文件判它一次。
            let is_dir = entry
                .file_type()
                .map(|kind| kind.is_dir())
                .unwrap_or(false);
            if is_dir {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            // 提交拦截那条判据里有 `contains("credentials")` / `starts_with("secrets")` 这种**按名字**
            // 的模糊规则，对源码提交合适（那边还有 `--allow-sensitive` 出口），套到打包产物上就是死墙：
            // chunk 名跟着源文件走（`Credentials.tsx` → `credentials-3f8a.js`），而这类文件本来就要发给
            // 浏览器，名字撞上判据不等于带了凭据，Pages 发布又没有「确认继续」那条路。
            // 所以只按扩展名放过前端资源；真凭据文件（.env / *.pem / *.key / credentials.json）不带这些后缀。
            if crate::git::is_sensitive_path(&relative)
                && !is_bundled_asset(&relative)
                && found.len() < 20
            {
                found.push(relative);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// 打包产物里本来就要交给浏览器的那类文件（脚本 / 样式 / sourcemap / 图片 / 字体）。
///
/// 豁免只按扩展名放行，是因为真凭据文件实践里不带这些后缀；代价是一个刻意命名为
/// `id_rsa.js` 的文件会放过 —— 相比「正常站点永久发不出去」，这个取舍可以接受。
fn is_bundled_asset(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let tail = match name.rfind('.') {
        Some(index) => &name[index + 1..],
        None => return false,
    };
    matches!(
        tail,
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "css" | "scss" | "less" | "map" | "html"
            | "htm" | "svg" | "woff" | "woff2" | "ttf" | "eot" | "otf" | "png" | "jpg" | "jpeg"
            | "gif" | "webp" | "avif" | "ico" | "mp4" | "webm"
    )
}

pub(super) fn assert_output_has_no_secrets(output_path: &Path) -> Result<()> {
    let blocked = sensitive_files_in(output_path)?;
    if blocked.is_empty() {
        return Ok(());
    }
    Err(CoreError::pages(format!(
        "产物目录里有疑似敏感文件，已拒绝发布（发出去就是公开的）：\n  {}\n请把它们从构建产物里排除，或换一个输出目录。",
        blocked.join("\n  ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pages 两条发布路径都不过提交拦截，产物里混进 .env / 私钥必须被拦在发布之前。
    #[test]
    fn sensitive_files_in_flags_dotenv_and_keys_but_not_normal_assets() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-pages-secrets-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), "<html></html>").unwrap();
        std::fs::write(dir.join("assets").join("app.js"), "console.log(1)").unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        std::fs::write(dir.join("assets").join("id_rsa"), "x").unwrap();
        std::fs::write(dir.join("config.pem"), "x").unwrap();
        // 前端 chunk 名跟着源文件走，名字撞上「模糊凭据」判据不算带了凭据：
        // 这类文件本来就要发给浏览器，而 Pages 发布没有确认继续的出口，拦住就是永久发不出去。
        std::fs::write(dir.join("assets").join("credentials-3f8a.js"), "x").unwrap();
        std::fs::write(dir.join("assets").join("secrets-Bq2.js.map"), "x").unwrap();
        // 同名的配置文件（不带前端扩展名）仍然要拦。
        std::fs::write(dir.join("credentials.json"), "{}").unwrap();
        // 示例 / 模板文件不算敏感，别把正常的 .env.example 一起拦了。
        std::fs::write(dir.join(".env.example"), "SAMPLE=1").unwrap();

        let found = sensitive_files_in(&dir).unwrap();
        assert!(found.contains(&".env".to_string()), "{found:?}");
        assert!(found.contains(&"assets/id_rsa".to_string()), "{found:?}");
        assert!(found.contains(&"config.pem".to_string()), "{found:?}");
        assert!(
            found.contains(&"credentials.json".to_string()),
            "{found:?}"
        );
        assert!(
            !found.iter().any(|item| item.ends_with(".js") || item.ends_with(".map")),
            "前端资源不该被模糊名字判据钉住：{found:?}"
        );
        assert!(
            !found.iter().any(|item| item.contains("example")),
            "示例文件不该被当成凭据：{found:?}"
        );
        assert!(
            !found.iter().any(|item| item.contains("index.html")),
            "正常产物不该被拦：{found:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 扫不动就拒绝发布：把「读不到」当成「没有敏感文件」，红线就成了摆设。
    #[test]
    fn unreadable_output_dir_is_an_error_not_a_clean_bill() {
        let dir = std::env::temp_dir().join(format!(
            "deploycode-pages-unreadable-{}",
            uuid::Uuid::new_v4()
        ));
        // 指着一个**文件**要它列目录：跨平台都能稳定读不出来。
        std::fs::write(&dir, "not a directory").unwrap();
        let err = sensitive_files_in(&dir).unwrap_err();
        assert!(err.to_string().contains("无法读取产物目录"), "实际: {err}");
        assert!(matches!(err, CoreError::Pages(_)), "实际: {err:?}");
        // 同一条要从 `assert_output_has_no_secrets` 出头，才是发布真正走的那条路。
        assert!(assert_output_has_no_secrets(&dir).is_err());
        let _ = std::fs::remove_file(&dir);
    }

    /// 产物里指回祖先的软链不能把这次遍历拖成死循环（`latest -> .` 这类真会出现在产物里）。
    #[cfg(unix)]
    #[test]
    fn symlinked_directories_are_not_followed() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!(
            "deploycode-pages-symlink-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("dist").join(".env"), "SECRET=1").unwrap();
        // 一条指回自己所在目录（环），一条指到目录之外。
        symlink(dir.join("dist"), dir.join("dist").join("latest")).unwrap();
        symlink("/etc", dir.join("dist").join("etc")).unwrap();

        let found = sensitive_files_in(&dir).unwrap();
        assert_eq!(found, vec!["dist/.env".to_string()], "{found:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
