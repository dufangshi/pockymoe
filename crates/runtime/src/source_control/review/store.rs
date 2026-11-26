//! Review state kept on the Supervisor, so a phone and a desktop see the same
//! drafts, viewed files, guides and saved views. Host data is never cached here.
use super::*;
use crate::db::Database;
use rusqlite::{params, OptionalExtension};

pub(crate) fn migrate(c: &rusqlite::Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS pull_request_drafts (
            id TEXT PRIMARY KEY,
            pr_key TEXT NOT NULL,
            author TEXT NOT NULL,
            kind TEXT NOT NULL,
            path TEXT,
            side TEXT,
            line INTEGER,
            start_line INTEGER,
            body TEXT NOT NULL,
            head_sha TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS pull_request_drafts_pr ON pull_request_drafts(pr_key, author);
        CREATE TABLE IF NOT EXISTS pull_request_viewed (
            pr_key TEXT NOT NULL,
            path TEXT NOT NULL,
            revision TEXT NOT NULL,
            viewed_at TEXT NOT NULL,
            PRIMARY KEY(pr_key, path)
        );
        CREATE TABLE IF NOT EXISTS pull_request_guides (
            pr_key TEXT PRIMARY KEY,
            head_sha TEXT NOT NULL,
            guide_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );",
    )?;
    Ok(())
}

pub const MAX_DRAFTS: usize = 300;
pub const MAX_BODY: usize = 65_536;

fn row(r: &rusqlite::Row) -> rusqlite::Result<Draft> {
    Ok(Draft {
        id: r.get(0)?,
        kind: r.get(1)?,
        path: r.get(2)?,
        side: r.get(3)?,
        line: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
        start_line: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
        body: r.get(6)?,
        head_sha: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

pub fn drafts(db: &Database, key: &str, author: &str) -> Result<Vec<Draft>> {
    db.with(|c| {
        let mut q = c.prepare("SELECT id,kind,path,side,line,start_line,body,head_sha,created_at,updated_at FROM pull_request_drafts WHERE pr_key=?1 AND author=?2 ORDER BY created_at, id")?;
        let rows = q
            .query_map(params![key, author], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Create or edit one draft. A `summary` draft is unique per PR and author.
pub fn save_draft(db: &Database, key: &str, author: &str, input: &Value) -> Result<Draft> {
    let kind = input["kind"].as_str().unwrap_or("line");
    ensure!(
        matches!(kind, "line" | "file" | "summary"),
        "Draft kind must be line, file or summary"
    );
    let body = input["body"].as_str().unwrap_or("");
    ensure!(body.len() <= MAX_BODY, "Comment too long");
    let path = input["path"].as_str().filter(|p| !p.is_empty());
    let side = input["side"].as_str().unwrap_or("right");
    ensure!(
        matches!(side, "left" | "right"),
        "Side must be left or right"
    );
    let line = input["line"].as_u64();
    let start = input["startLine"].as_u64().filter(|s| Some(*s) != line);
    if kind != "summary" {
        ensure!(!body.trim().is_empty(), "Comment required");
        let path = path.context("Path required")?;
        ensure!(
            path.len() <= 4096 && !path.contains('\0') && !path.starts_with('/'),
            "Invalid path"
        );
    }
    if kind == "line" {
        let line = line.filter(|l| *l > 0).context("Line required")?;
        ensure!(
            start.is_none_or(|s| s > 0 && s < line),
            "Range must start before its last line"
        );
    }
    let head = input["headSha"].as_str().filter(|h| h.len() <= 64);
    let now = pockymoe_protocol::now_rfc3339();
    let existing = if kind == "summary" {
        drafts(db, key, author)?
            .into_iter()
            .find(|d| d.kind == "summary")
            .map(|d| d.id)
    } else {
        input["id"].as_str().map(str::to_owned)
    };
    let id = match existing {
        Some(id) => {
            let changed = db.with(|c| {
                Ok(c.execute(
                    "UPDATE pull_request_drafts SET body=?4, path=?5, side=?6, line=?7, start_line=?8, head_sha=COALESCE(?9, head_sha), updated_at=?10 WHERE id=?1 AND pr_key=?2 AND author=?3",
                    params![id, key, author, body, path, (kind == "line").then_some(side), line.filter(|_| kind == "line").map(|l| l as i64), start.filter(|_| kind == "line").map(|l| l as i64), head, now],
                )?)
            })?;
            ensure!(changed == 1, "Draft not found");
            id
        }
        None => {
            ensure!(
                drafts(db, key, author)?.len() < MAX_DRAFTS,
                "Too many draft comments on this PR"
            );
            let id = uuid::Uuid::new_v4().to_string();
            db.with(|c| {
                c.execute(
                    "INSERT INTO pull_request_drafts(id,pr_key,author,kind,path,side,line,start_line,body,head_sha,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11)",
                    params![id, key, author, kind, path.filter(|_| kind != "summary"), (kind == "line").then_some(side), line.filter(|_| kind == "line").map(|l| l as i64), start.filter(|_| kind == "line").map(|l| l as i64), body, head, now],
                )?;
                Ok(())
            })?;
            id
        }
    };
    drafts(db, key, author)?
        .into_iter()
        .find(|d| d.id == id)
        .context("Draft not saved")
}

pub fn delete_drafts(db: &Database, key: &str, author: &str, ids: &[String]) -> Result<usize> {
    db.with(|c| {
        let mut count = 0;
        for id in ids {
            count += c.execute(
                "DELETE FROM pull_request_drafts WHERE id=?1 AND pr_key=?2 AND author=?3",
                params![id, key, author],
            )?;
        }
        Ok(count)
    })
}

pub fn delete_all_drafts(db: &Database, key: &str, author: &str) -> Result<usize> {
    db.with(|c| {
        Ok(c.execute(
            "DELETE FROM pull_request_drafts WHERE pr_key=?1 AND author=?2 AND kind!='summary'",
            params![key, author],
        )?)
    })
}

/// After a successful submit: drop what was sent, keep anything edited since.
pub fn clear_submitted(db: &Database, key: &str, author: &str, sent: &[Draft]) -> Result<()> {
    db.with(|c| {
        for draft in sent {
            c.execute(
                "DELETE FROM pull_request_drafts WHERE id=?1 AND pr_key=?2 AND author=?3 AND updated_at=?4",
                params![draft.id, key, author, draft.updated_at],
            )?;
        }
        Ok(())
    })
}

/// Viewed marks for hosts without native state. A mark whose revision differs
/// from the file's current one reads as `changed`.
pub fn apply_viewed(db: &Database, key: &str, files: &mut [PullFile]) -> Result<()> {
    let marks: Vec<(String, String)> = db.with(|c| {
        let mut q = c.prepare("SELECT path, revision FROM pull_request_viewed WHERE pr_key=?1")?;
        let rows = q
            .query_map([key], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })?;
    for file in files {
        if let Some((_, revision)) = marks.iter().find(|(p, _)| p == &file.path) {
            file.viewed = if revision == &file.digest {
                "viewed"
            } else {
                "changed"
            }
            .into();
        }
    }
    Ok(())
}

pub fn set_viewed(db: &Database, key: &str, path: &str, revision: Option<&str>) -> Result<()> {
    let now = pockymoe_protocol::now_rfc3339();
    db.with(|c| {
        match revision {
            Some(revision) => c.execute(
                "INSERT INTO pull_request_viewed(pr_key,path,revision,viewed_at) VALUES(?1,?2,?3,?4) ON CONFLICT(pr_key,path) DO UPDATE SET revision=excluded.revision, viewed_at=excluded.viewed_at",
                params![key, path, revision, now],
            )?,
            None => c.execute(
                "DELETE FROM pull_request_viewed WHERE pr_key=?1 AND path=?2",
                params![key, path],
            )?,
        };
        Ok(())
    })
}

pub fn guide(db: &Database, key: &str) -> Result<Option<Value>> {
    db.with(|c| {
        let raw: Option<String> = c
            .query_row(
                "SELECT guide_json FROM pull_request_guides WHERE pr_key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
    })
}

pub fn save_guide(db: &Database, key: &str, head: &str, guide: &Value) -> Result<()> {
    let now = pockymoe_protocol::now_rfc3339();
    db.with(|c| {
        c.execute(
            "INSERT INTO pull_request_guides(pr_key,head_sha,guide_json,created_at) VALUES(?1,?2,?3,?4) ON CONFLICT(pr_key) DO UPDATE SET head_sha=excluded.head_sha, guide_json=excluded.guide_json, created_at=excluded.created_at",
            params![key, head, guide.to_string(), now],
        )?;
        Ok(())
    })
}

const VIEWS_KEY: &str = "pr-saved-views";

pub fn views(db: &Database) -> Result<Value> {
    Ok(db
        .get_kv(VIEWS_KEY)?
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_else(|| json!([])))
}

/// Replace the saved views. Each is `{id, name, query}`; the query is the page's
/// own filter object, stored as given within size limits.
pub fn save_views(db: &Database, input: &Value) -> Result<Value> {
    let list = input.as_array().context("Views must be a list")?;
    ensure!(list.len() <= 50, "At most 50 saved views");
    let mut clean = Vec::new();
    for view in list {
        let id = view["id"]
            .as_str()
            .filter(|i| !i.is_empty() && i.len() <= 64)
            .context("View id required")?;
        let name = view["name"]
            .as_str()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .context("View name required")?;
        ensure!(name.chars().count() <= 80, "View name too long");
        ensure!(view["query"].is_object(), "View query required");
        ensure!(
            view["query"].to_string().len() <= 4096,
            "View query too large"
        );
        clean.push(json!({"id":id,"name":name,"query":view["query"]}));
    }
    let value = Value::Array(clean);
    db.set_kv(VIEWS_KEY, &value.to_string())?;
    Ok(value)
}
