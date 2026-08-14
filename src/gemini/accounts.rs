use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::{
    config::Config,
    error::{GatewayError, Result},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountCredentials {
    pub id: String,
    #[serde(default)]
    pub auth_user: usize,
    pub secure_1psid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secure_1psidts: Option<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default = "default_weight")]
    pub weight: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountsDocument {
    version: u32,
    accounts: Vec<AccountCredentials>,
}

fn enabled_by_default() -> bool {
    true
}

fn default_weight() -> usize {
    1
}

pub fn validate_account_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(GatewayError::InvalidRequest(
            "account_id must contain 1..64 letters, digits, '.', '_' or '-'".into(),
        ));
    }
    Ok(())
}

pub fn load(config: &Config) -> Result<Vec<AccountCredentials>> {
    let backup = backup_path(&config.accounts_file);
    let has_persisted_file = config.accounts_file.is_file() || backup.is_file();
    let mut accounts = if config.accounts_file.is_file() {
        read_document(&config.accounts_file)?
    } else {
        if backup.is_file() {
            tracing::warn!(path = %backup.display(), "recovering account credentials from backup");
            read_document(&backup)?
        } else {
            Vec::new()
        }
    };

    if !has_persisted_file && !config.secure_1psid.trim().is_empty() {
        accounts.push(AccountCredentials {
            id: "default".into(),
            auth_user: config.auth_user,
            secure_1psid: config.secure_1psid.clone(),
            secure_1psidts: config.secure_1psidts.clone(),
            enabled: true,
            weight: 1,
        });
    }
    if !has_persisted_file && accounts.is_empty() && config.cookie_sync_enabled {
        accounts.push(AccountCredentials {
            id: "default".into(),
            auth_user: config.auth_user,
            secure_1psid: String::new(),
            secure_1psidts: None,
            enabled: true,
            weight: 1,
        });
    }
    for account in &accounts {
        validate_account_id(&account.id)?;
        if !(1..=100).contains(&account.weight) {
            return Err(GatewayError::InvalidRequest(format!(
                "account '{}' weight must be between 1 and 100",
                account.id
            )));
        }
    }
    accounts.sort_by(|left, right| left.id.cmp(&right.id));
    accounts.dedup_by(|left, right| left.id == right.id);
    Ok(accounts)
}

fn read_document(path: &Path) -> Result<Vec<AccountCredentials>> {
    let bytes = std::fs::read(path)?;
    let mut document: AccountsDocument = serde_json::from_slice(&bytes)?;
    if !matches!(document.version, 1 | 2) {
        return Err(GatewayError::InvalidRequest(format!(
            "unsupported accounts file version: {}",
            document.version
        )));
    }
    for account in &mut document.accounts {
        account.secure_1psid = unprotect_secret(&account.secure_1psid)?;
        account.secure_1psidts = account
            .secure_1psidts
            .as_deref()
            .map(unprotect_secret)
            .transpose()?;
    }
    Ok(document.accounts)
}

pub async fn persist(path: &Path, accounts: &[AccountCredentials]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut protected = accounts.to_vec();
    for account in &mut protected {
        account.secure_1psid = protect_secret(&account.secure_1psid)?;
        account.secure_1psidts = account
            .secure_1psidts
            .as_deref()
            .map(protect_secret)
            .transpose()?;
    }
    let document = AccountsDocument {
        version: 2,
        accounts: protected,
    };
    let bytes = serde_json::to_vec_pretty(&document)?;
    let temporary = temporary_path(path);
    let backup = backup_path(path);
    fs::write(&temporary, bytes).await?;
    restrict_file(&temporary).await?;

    if path.is_file() {
        if backup.is_file() {
            fs::remove_file(&backup).await?;
        }
        fs::rename(path, &backup).await?;
    }
    if let Err(error) = fs::rename(&temporary, path).await {
        if backup.is_file() && !path.is_file() {
            let _ = fs::rename(&backup, path).await;
        }
        return Err(error.into());
    }
    restrict_file(path).await?;
    if backup.is_file() {
        fs::remove_file(backup).await?;
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.new", path.display()))
}

fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.bak", path.display()))
}

pub fn protection_backend() -> &'static str {
    if cfg!(windows) {
        "windows_dpapi"
    } else {
        "file_permissions_0600"
    }
}

#[cfg(not(windows))]
fn protect_secret(value: &str) -> Result<String> {
    Ok(value.to_owned())
}

#[cfg(not(windows))]
fn unprotect_secret(value: &str) -> Result<String> {
    Ok(value.to_owned())
}

#[cfg(windows)]
fn protect_secret(value: &str) -> Result<String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData},
    };

    if value.is_empty() {
        return Ok(String::new());
    }
    let bytes = value.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let success = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if success == 0 {
        return Err(GatewayError::Internal(anyhow::anyhow!(
            "Windows DPAPI encryption failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    let protected = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let encoded = hex_encode(protected);
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(format!("dpapi:{encoded}"))
}

#[cfg(windows)]
fn unprotect_secret(value: &str) -> Result<String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
        },
    };

    let Some(encoded) = value.strip_prefix("dpapi:") else {
        return Ok(value.to_owned());
    };
    let mut encrypted = hex_decode(encoded)?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: encrypted.len() as u32,
        pbData: encrypted.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let success = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if success == 0 {
        return Err(GatewayError::Internal(anyhow::anyhow!(
            "Windows DPAPI decryption failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let decoded = String::from_utf8(bytes.to_vec()).map_err(|error| {
        GatewayError::Internal(anyhow::anyhow!("DPAPI value is not UTF-8: {error}"))
    })?;
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(decoded)
}

#[cfg(windows)]
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(windows)]
fn hex_decode(value: &str) -> Result<Vec<u8>> {
    if value.len() % 2 != 0 {
        return Err(GatewayError::InvalidRequest(
            "invalid DPAPI value length".into(),
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

#[cfg(windows)]
fn hex_digit(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(GatewayError::InvalidRequest(
            "invalid DPAPI hex value".into(),
        )),
    }
}

#[cfg(unix)]
async fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn restrict_file(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_stable_account_ids() {
        assert!(validate_account_id("work.u-1").is_ok());
        assert!(validate_account_id("").is_err());
        assert!(validate_account_id("bad/account").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_hides_plaintext() {
        let protected = protect_secret("cookie-secret-value").unwrap();
        assert!(protected.starts_with("dpapi:"));
        assert!(!protected.contains("cookie-secret-value"));
        assert_eq!(unprotect_secret(&protected).unwrap(), "cookie-secret-value");
    }
}
