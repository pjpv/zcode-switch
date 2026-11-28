use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use pbkdf2::pbkdf2_hmac;
use serde_json::{json, Value};
use sha2::Sha256;

pub const FORMAT_BUNDLE: &str = "zsw-accounts-bundle";
pub const KDF_ITERS: u32 = 100_000;
pub const KDF_ITERS_V2: u32 = 600_000;
const MAX_PLAIN_BYTES: usize = 64 * 1024 * 1024;

fn derive_key(password: &str, salt: &[u8], iters: u32) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iters, &mut key);
    key
}

pub fn seal(payload: &Value, password: &str, format: &str) -> Result<Value, String> {
    if password.trim().is_empty() {
        return Err(crate::i18n::tr("err.cipher.pw_empty"));
    }
    let mut salt = [0u8; 16];
    let mut nonce_b = [0u8; 12];
    aes_gcm::aead::OsRng.fill_bytes(&mut salt);
    aes_gcm::aead::OsRng.fill_bytes(&mut nonce_b);

    let plain = serde_json::to_vec(payload)
        .map_err(|e| crate::i18n::trf("err.serialize", &[("e", &e.to_string())]))?;
    if plain.len() > MAX_PLAIN_BYTES {
        return Err(crate::i18n::tr("err.cipher.bad_plain"));
    }
    let key = derive_key(password, &salt, KDF_ITERS_V2);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| crate::i18n::trf("err.cipher.key", &[("e", &e.to_string())]))?;
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce_b), plain.as_slice())
        .map_err(|e| crate::i18n::trf("err.cipher.encrypt", &[("e", &e.to_string())]))?;
    let (data, tag) = ct.split_at(ct.len() - 16);

    Ok(json!({
        "format": format,
        "version": 2,
        "kdf": { "algo": "pbkdf2-hmac-sha256", "iters": KDF_ITERS_V2, "salt": B64.encode(salt) },
        "cipher": { "algo": "aes-256-gcm", "nonce": B64.encode(nonce_b), "tag": B64.encode(tag), "data": B64.encode(data) },
    }))
}

pub fn open(envelope: &Value, password: &str) -> Result<Value, String> {
    if password.trim().is_empty() {
        return Err(crate::i18n::tr("err.cipher.pw_empty"));
    }
    let version = envelope
        .get("version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if version != 1 && version != 2 {
        return Err(crate::i18n::tr("err.cipher.bad_plain"));
    }
    let kdf = envelope
        .get("kdf")
        .ok_or_else(|| crate::i18n::tr("err.cipher.no_kdf"))?;
    let c = envelope
        .get("cipher")
        .ok_or_else(|| crate::i18n::tr("err.cipher.no_cipher"))?;
    if kdf.get("algo").and_then(|v| v.as_str()) != Some("pbkdf2-hmac-sha256") {
        return Err(crate::i18n::tr("err.cipher.no_kdf"));
    }
    if c.get("algo").and_then(|v| v.as_str()) != Some("aes-256-gcm") {
        return Err(crate::i18n::tr("err.cipher.no_cipher"));
    }
    let iters = kdf
        .get("iters")
        .and_then(|v| v.as_u64())
        .unwrap_or(if version == 1 { KDF_ITERS as u64 } else { 0 });
    if !(1..=600_000).contains(&iters) {
        return Err(crate::i18n::tr("err.cipher.bad_plain"));
    }
    let iters = iters as u32;
    let salt = B64
        .decode(kdf.get("salt").and_then(|v| v.as_str()).unwrap_or_default())
        .map_err(|_| crate::i18n::tr("err.cipher.bad_salt"))?;
    let nonce_b = B64
        .decode(c.get("nonce").and_then(|v| v.as_str()).unwrap_or_default())
        .map_err(|_| crate::i18n::tr("err.cipher.bad_nonce"))?;
    let tag = B64
        .decode(c.get("tag").and_then(|v| v.as_str()).unwrap_or_default())
        .map_err(|_| crate::i18n::tr("err.cipher.bad_tag"))?;
    let data = B64
        .decode(c.get("data").and_then(|v| v.as_str()).unwrap_or_default())
        .map_err(|_| crate::i18n::tr("err.cipher.bad_data"))?;
    if nonce_b.len() != 12 {
        return Err(crate::i18n::tr("err.cipher.nonce_len"));
    }
    if salt.len() != 16 {
        return Err(crate::i18n::tr("err.cipher.bad_salt"));
    }
    if tag.len() != 16 {
        return Err(crate::i18n::tr("err.cipher.bad_tag"));
    }
    if data.len() > MAX_PLAIN_BYTES {
        return Err(crate::i18n::tr("err.cipher.bad_plain"));
    }

    let key = derive_key(password, &salt, iters);
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| crate::i18n::trf("err.cipher.key", &[("e", &e.to_string())]))?;
    let mut buf = data.clone();
    buf.extend_from_slice(&tag);
    let plain = cipher
        .decrypt(Nonce::from_slice(&nonce_b), buf.as_slice())
        .map_err(|_| crate::i18n::coded("wrong_password", "err.cipher.wrong_pw", &[]))?;
    serde_json::from_slice(&plain)
        .map_err(|e| crate::i18n::trf("err.cipher.bad_plain", &[("e", &e.to_string())]))
}

pub fn is_sealed(v: &Value) -> bool {
    v.get("kdf").is_some() && v.get("cipher").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open_round_trip() {
        let payload = json!({"accounts": [{"name": "test"}]});
        let envelope = seal(&payload, "correct horse", FORMAT_BUNDLE).unwrap();
        assert_eq!(envelope["version"], 2);
        assert_eq!(open(&envelope, "correct horse").unwrap(), payload);
    }

    #[test]
    fn rejects_wrong_password() {
        let envelope = seal(&json!({"x": 1}), "right", FORMAT_BUNDLE).unwrap();
        assert!(open(&envelope, "wrong").is_err());
    }

    #[test]
    fn rejects_wrong_tag_length() {
        let mut envelope = seal(&json!({"x": 1}), "right", FORMAT_BUNDLE).unwrap();
        envelope["cipher"]["tag"] = json!(B64.encode([0u8; 15]));
        assert!(open(&envelope, "right").is_err());
    }
}
