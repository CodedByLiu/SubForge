use std::fs;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};

use super::paths::{secrets_path, vault_key_path};

const NONCE_LEN: usize = 12;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SecretPayload {
    #[serde(default)]
    pub llm_api_key: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct EncFile {
    nonce: String,
    payload: String,
}

fn load_or_create_vault_key(app_dir: &Path) -> Result<[u8; 32]> {
    let p = vault_key_path(app_dir);
    if p.exists() {
        let bytes = fs::read(&p).with_context(|| format!("读取密钥文件: {}", p.display()))?;
        if bytes.len() != 32 {
            return Err(anyhow!("vault.key 长度无效"));
        }
        let mut k = [0u8; 32];
        k.copy_from_slice(&bytes);
        return Ok(k);
    }
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut k = [0u8; 32];
    getrandom::fill(&mut k).map_err(|e| anyhow!("生成随机密钥失败: {e}"))?;
    fs::write(&p, k).with_context(|| format!("写入密钥文件: {}", p.display()))?;
    Ok(k)
}

pub fn load_secrets(app_dir: &Path) -> Result<SecretPayload> {
    let path = secrets_path(app_dir);
    if !path.exists() {
        return Ok(SecretPayload::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("读取 {}", path.display()))?;
    let enc: EncFile = serde_json::from_str(&raw).context("解析 secrets.enc.json")?;
    let nonce_bytes = B64
        .decode(enc.nonce.trim())
        .map_err(|e| anyhow!("nonce 解码失败: {e}"))?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(anyhow!("nonce 长度无效"));
    }
    let cipher_bytes = B64
        .decode(enc.payload.trim())
        .map_err(|e| anyhow!("密文解码失败: {e}"))?;
    let key = load_or_create_vault_key(app_dir)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("{e}"))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plain = cipher
        .decrypt(nonce, cipher_bytes.as_ref())
        .map_err(|_| anyhow!("解密失败，密钥或文件可能已损坏"))?;
    let s: SecretPayload =
        serde_json::from_slice(&plain).context("解密后的密钥 JSON 无效")?;
    Ok(s)
}

pub fn save_secrets(app_dir: &Path, secrets: &SecretPayload) -> Result<()> {
    let path = secrets_path(app_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let key = load_or_create_vault_key(app_dir)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("{e}"))?;
    let mut nonce_raw = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_raw).map_err(|e| anyhow!("生成 nonce 失败: {e}"))?;
    let nonce = Nonce::from_slice(&nonce_raw);
    let plain = serde_json::to_vec(secrets).context("序列化密钥明文")?;
    let ciphertext = cipher
        .encrypt(nonce, plain.as_ref())
        .map_err(|e| anyhow!("加密失败: {e}"))?;
    let enc = EncFile {
        nonce: B64.encode(nonce_raw),
        payload: B64.encode(ciphertext),
    };
    let json = serde_json::to_string_pretty(&enc).context("序列化密文包装")?;
    fs::write(&path, json).with_context(|| format!("写入 {}", path.display()))?;
    Ok(())
}

pub fn clear_llm_api_key(app_dir: &Path) -> Result<()> {
    let mut s = load_secrets(app_dir)?;
    s.llm_api_key.clear();
    save_secrets(app_dir, &s)
}
