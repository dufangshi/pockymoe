//! Optional passcode for a Supervisor in local mode. Without one the local Web UI
//! and API stay open to whoever can reach the port, as before; with one, each
//! browser signs in once with the passcode. Stored in `kv`, never in plain text.

use anyhow::{ensure, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use crate::db::Database;

const KEY: &str = "local_access";
pub const MIN_PASSCODE_CHARS: usize = 4;
const MAX_PASSCODE_BYTES: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccess {
    salt: String,
    hash: String,
    /// Signs browser sessions. A new passcode gets a new secret, which signs out
    /// every other browser.
    pub session_secret: String,
}

fn random() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn derive(passcode: &str, salt: &str) -> Result<String> {
    let params = scrypt::Params::new(14, 8, 1, 32)?;
    let mut output = [0_u8; 32];
    scrypt::scrypt(passcode.as_bytes(), salt.as_bytes(), &params, &mut output)?;
    Ok(URL_SAFE_NO_PAD.encode(output))
}

impl LocalAccess {
    pub fn new(passcode: &str) -> Result<Self> {
        ensure!(
            passcode.chars().count() >= MIN_PASSCODE_CHARS,
            "The passcode needs at least {MIN_PASSCODE_CHARS} characters"
        );
        ensure!(
            passcode.len() <= MAX_PASSCODE_BYTES,
            "The passcode is too long"
        );
        let salt = random();
        Ok(Self {
            hash: derive(passcode, &salt)?,
            salt,
            session_secret: random(),
        })
    }

    pub fn verify(&self, passcode: &str) -> bool {
        passcode.len() <= MAX_PASSCODE_BYTES
            && derive(passcode, &self.salt)
                .is_ok_and(|hash| hash.as_bytes().ct_eq(self.hash.as_bytes()).into())
    }
}

pub fn load(db: &Database) -> Result<Option<LocalAccess>> {
    db.get_kv(KEY)?
        .map(|value| serde_json::from_str(&value).context("Invalid stored passcode"))
        .transpose()
}

pub fn store(db: &Database, access: Option<&LocalAccess>) -> Result<()> {
    match access {
        Some(access) => db.set_kv(KEY, &serde_json::to_string(access)?),
        None => db.with(|conn| {
            conn.execute("DELETE FROM kv WHERE key=?1", [KEY])?;
            Ok(())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passcodes_verify_and_persist_without_the_plain_text() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("access.sqlite")).unwrap();
        assert!(LocalAccess::new("abc").is_err());
        let access = LocalAccess::new("open sesame").unwrap();
        assert!(access.verify("open sesame") && !access.verify("open sesam"));
        store(&db, Some(&access)).unwrap();
        let stored = db.get_kv(KEY).unwrap().unwrap();
        assert!(!stored.contains("open sesame"));
        let loaded = load(&db).unwrap().unwrap();
        assert!(loaded.verify("open sesame"));
        assert_ne!(
            LocalAccess::new("open sesame").unwrap().session_secret,
            loaded.session_secret
        );
        store(&db, None).unwrap();
        assert!(load(&db).unwrap().is_none());
    }
}
