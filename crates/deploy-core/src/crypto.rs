//! 已移除：这里曾是一个 AES-256-GCM + HKDF 的「敏感数据加密」模块。
//!
//! 它从上线起就没有接进任何读写路径 —— `Store::save_config` / `load_config` 一直是明文
//! 存取，配套的 `encrypt_sensitive_fields` / `decrypt_sensitive_fields` /
//! `set_master_password` / `verify_master_password` 全仓零调用方。留下它的唯一效果，是
//! 让读到本文件的人以为「密码是加密存的」，而磁盘上的字段其实是明文。
//!
//! 所以整块删掉了（模块声明也从 `lib.rs` 移除），并把只被它使用的
//! `aes-gcm` / `hkdf` / `base64` / `rand` / `zeroize` 五个依赖一并摘掉。
//!
//! 当前口径：**凭据在本机 `config.json` 中按明文存储**，这是 README「数据存储」一节
//! 记录的已知取舍，见 `store/mod.rs` 顶部注释。
//!
//! 若将来真要接通加密，不要照抄旧实现 —— 它有几处是错的：固定 salt 的 HKDF 不是口令
//! KDF（应当用 Argon2 / scrypt）、口令校验用无盐 SHA-256、靠「长度 > 20 且全是字母数字」
//! 猜一个字段是不是密文、解密失败静默保留原值。正确做法是带版本号的配置格式 + 显式
//! 迁移路径，而不是在 `load_config` 里就地猜。
