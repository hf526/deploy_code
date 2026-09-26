//! 敏感数据加密模块
//! 
//! 使用 AES-256-GCM 加密 + HKDF 主密码派生密钥
//!
//! ⚠️ 目前未接入任何读写路径：`Store::save_config` / `load_config` 仍按明文存取 `config.json`
//! （README 与 AGENTS.md 记录的就是明文），`Store` 里的 encrypt_/decrypt_sensitive_fields
//! 与 set/verify_master_password 没有调用方。接通前不要按「密文」理解磁盘上的字段。

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use hkdf::Hkdf;
use rand::Rng; // for try_fill
use sha2::Sha256;
use zeroize::Zeroize;

/// 加密错误类型
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("加密失败：{0}")]
    Encryption(String),
    #[error("解密失败：{0}")]
    Decryption(String),
    #[error("密钥派生失败")]
    KeyDerivation(String),
}

pub type Result<T> = std::result::Result<T, CryptoError>;

/// 从主密码派生加密密钥（32 字节用于 AES-256）
fn derive_key_from_master_password(master_password: &str) -> Result<[u8; 32]> {
    // 使用固定盐（实际生产环境应该存储盐值）
    // 这里简化处理，使用固定盐便于迁移
    let salt = b"deploycode-v1-salt";
    
    let hk = Hkdf::<Sha256>::new(Some(salt), master_password.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(&[], &mut key)
        .map_err(|e| CryptoError::KeyDerivation(e.to_string()))?;
    Ok(key)
}

/// 加密字符串（返回 base64 编码的密文）
pub fn encrypt_string(plaintext: &str, master_password: &str) -> Result<String> {
    let key = derive_key_from_master_password(master_password)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| CryptoError::Encryption(e.to_string()))?;

    // 生成随机 nonce (12 bytes for GCM)
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().try_fill(&mut nonce_bytes).ok().unwrap();
    let nonce = Nonce::from_slice(&nonce_bytes);

    // 加密
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| CryptoError::Encryption(e.to_string()))?;

    // 拼接：nonce(12 字节) + ciphertext
    let mut data = Vec::with_capacity(12 + ciphertext.len());
    data.extend_from_slice(&nonce);
    data.extend_from_slice(&ciphertext);

    // base64 编码
    Ok(base64::encode(&data))
}

/// 解密字符串
pub fn decrypt_string(ciphertext_b64: &str, master_password: &str) -> Result<String> {
    let key = derive_key_from_master_password(master_password)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| CryptoError::Decryption(e.to_string()))?;

    // base64 解码
    let data = base64::decode(ciphertext_b64)
        .map_err(|e| CryptoError::Decryption(format!("base64 解码失败：{}", e)))?;

    if data.len() < 12 {
        return Err(CryptoError::Decryption("密文长度不足".to_string()));
    }

    // 分离 nonce 和密文
    let (nonce_bytes, encrypted_data) = data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    // 解密
    let plaintext = cipher
        .decrypt(nonce, encrypted_data)
        .map_err(|e| CryptoError::Decryption(e.to_string()))?;

    String::from_utf8(plaintext)
        .map_err(|e| CryptoError::Decryption(format!("UTF8 解码失败：{}", e)))
}

/// 标记需要加密的字段
pub trait EncryptedField {
    fn encrypt(self, master_password: &str) -> Result<String>;
    fn decrypt(encrypted: String, master_password: &str) -> Result<Self>
    where
        Self: Sized;
}

impl EncryptedField for String {
    fn encrypt(self, master_password: &str) -> Result<String> {
        encrypt_string(&self, master_password)
    }

    fn decrypt(encrypted: String, master_password: &str) -> Result<Self>
    where
        Self: Sized,
    {
        decrypt_string(&encrypted, master_password)
    }
}

/// 安全地清零内存中的敏感数据
pub fn secure_zero(mut string: String) {
    let bytes = unsafe { string.as_mut_vec() };
    bytes.zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let password = "test-password-123";
        let plaintext = "my-secret-password";

        let encrypted = encrypt_string(plaintext, password).unwrap();
        let decrypted = decrypt_string(&encrypted, password).unwrap();

        assert_eq!(decrypted, plaintext);
        assert_ne!(encrypted, plaintext);
        assert!(!encrypted.contains(plaintext));
    }

    #[test]
    fn test_wrong_password_fails() {
        let correct_pw = "correct";
        let wrong_pw = "wrong";
        let plaintext = "secret";

        let encrypted = encrypt_string(plaintext, correct_pw).unwrap();
        let result = decrypt_string(&encrypted, wrong_pw);

        assert!(result.is_err(), "错误密码应该导致解密失败");
    }

    #[test]
    fn test_special_characters_in_password() {
        let password = "p@ssw0rd!#$%^&*()";
        let plaintext = "sëcrët-pässwörd";

        let encrypted = encrypt_string(plaintext, password).unwrap();
        let decrypted = decrypt_string(&encrypted, password).unwrap();

        assert_eq!(decrypted, plaintext);
    }
}
