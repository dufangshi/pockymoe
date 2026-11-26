//! Separate credential audience and bounded, transactional persistence.
use anyhow::{Context, Result};
use pockymoe_runtime::Supervisor;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}
pub fn secret() -> String {
    format!(
        "pmcp_{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
pub fn transaction<T>(state: &Supervisor, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let path = state.config.database_url.with_extension("mcp.sqlite");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path.as_path())?;
    let mut db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS entries(kind TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, expires INTEGER NOT NULL, PRIMARY KEY(kind,key));")?;
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute(
        "DELETE FROM entries WHERE expires < ? AND kind != 'refresh'",
        [now()],
    )?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}
pub fn get(db: &Connection, kind: &str, key: &str) -> Result<Value> {
    let text: Option<String> = db
        .query_row(
            "SELECT value FROM entries WHERE kind=? AND key=?",
            params![kind, key],
            |r| r.get(0),
        )
        .optional()?;
    serde_json::from_str(&text.context("not found or expired")?).map_err(Into::into)
}
pub fn put(db: &Connection, kind: &str, key: &str, value: &Value, seconds: i64) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO entries VALUES(?,?,?,?)",
        params![kind, key, value.to_string(), now() + seconds],
    )?;
    Ok(())
}
pub fn delete(db: &Connection, kind: &str, key: &str) -> Result<()> {
    db.execute(
        "DELETE FROM entries WHERE kind=? AND key=?",
        params![kind, key],
    )?;
    Ok(())
}
pub fn list(db: &Connection, kind: &str) -> Result<Vec<Value>> {
    let mut stmt =
        db.prepare("SELECT value FROM entries WHERE kind=? ORDER BY rowid DESC LIMIT 1000")?;
    let rows = stmt.query_map([kind], |r| r.get::<_, String>(0))?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}
pub fn rate(db: &Connection, bucket: &str, limit: u64) -> Result<()> {
    let n = get(db, "rate", bucket)
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    anyhow::ensure!(n < limit, "rate limit exceeded");
    put(db, "rate", bucket, &Value::from(n + 1), 60)
}
