//! What a share or grant may store (docs/account-key-trust.md#grant-values).
//!
//! A field the owner leaves out takes its default; a value outside the shared
//! sets in `pockymoe_protocol::access` is refused with 400 rather than read as
//! another permission. Expiry times are stored in the exact UTC millisecond form
//! of `now_rfc3339()`, because every access query compares them as text.
use super::*;
use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use pockymoe_protocol::access;

/// A refused value: 400 `bad_request` with what the field accepts.
pub(super) struct Invalid(&'static str);

impl IntoResponse for Invalid {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiError::new("bad_request", self.0)),
        )
            .into_response()
    }
}

fn bad_request(message: &'static str) -> Invalid {
    Invalid(message)
}

/// A missing `threadAccess` keeps the long-standing default, `control`.
pub(super) fn thread_access(value: Option<&str>) -> Result<&'static str, Invalid> {
    value.map_or(Ok("control"), |value| {
        access::thread_access_value(value)
            .ok_or_else(|| bad_request("threadAccess must be read or control"))
    })
}

pub(super) fn workspace_access(value: Option<&str>) -> Result<&'static str, Invalid> {
    value.map_or(Ok("none"), |value| {
        access::workspace_access_value(value)
            .ok_or_else(|| bad_request("workspaceAccess must be none, read or write"))
    })
}

pub(super) fn scope(value: Option<&str>) -> Result<&'static str, Invalid> {
    value.map_or(Ok("thread"), |value| {
        access::grant_scope_value(value)
            .ok_or_else(|| bad_request("scope must be thread, workspace or device"))
    })
}

pub(super) fn workspace_scope(value: Option<&str>) -> Result<&'static str, Invalid> {
    value.map_or(Ok("all"), |value| {
        access::workspace_scope_value(value)
            .ok_or_else(|| bad_request("workspaceScope must be all or selected"))
    })
}

pub(super) fn grant_shape(scope: &str, workspace_scope: &str) -> Result<(), Invalid> {
    if access::supported_grant_shape(scope, workspace_scope) {
        Ok(())
    } else {
        Err(bad_request(
            "A device grant covers all workspaces; choose a workspace grant for selected workspaces",
        ))
    }
}

/// An RFC 3339 time with any offset, in the stored form; `None` if it is not one.
pub(super) fn normalize_expiry(value: &str) -> Option<String> {
    let at = DateTime::parse_from_rfc3339(value.trim())
        .ok()?
        .with_timezone(&Utc);
    // Four-digit years keep the text order equal to the time order.
    (1970..=9999)
        .contains(&at.year())
        .then(|| at.to_rfc3339_opts(SecondsFormat::Millis, true))
}

pub(super) fn expiry(value: Option<&str>) -> Result<Option<String>, Invalid> {
    value.map_or(Ok(None), |value| {
        normalize_expiry(value)
            .map(Some)
            .ok_or_else(|| bad_request("expiresAt must be an RFC 3339 date and time"))
    })
}

/// One edit of an owner's share or grant, applied as a single UPDATE so a save
/// changes every named column or none. Columns come from the handlers, never
/// from the request.
pub(super) struct Patch(Vec<(&'static str, rusqlite::types::Value)>);

impl Patch {
    pub(super) fn new() -> Self {
        Self(Vec::new())
    }

    pub(super) fn set(&mut self, column: &'static str, value: impl Into<rusqlite::types::Value>) {
        self.0.push((column, value.into()));
    }

    /// `Ok(false)` when no live row of this owner has the ID.
    pub(super) fn apply(
        self,
        conn: &Connection,
        table: &str,
        id: &str,
        owner: &str,
    ) -> rusqlite::Result<bool> {
        if self.0.is_empty() {
            return conn
                .query_row(
                    &format!(
                        "SELECT 1 FROM {table} WHERE id=?1 AND owner_user_id=?2 AND revoked_at IS NULL"
                    ),
                    params![id, owner],
                    |_| Ok(()),
                )
                .optional()
                .map(|found| found.is_some());
        }
        let count = self.0.len();
        let sets = self
            .0
            .iter()
            .enumerate()
            .map(|(index, (column, _))| format!("{column}=?{}", index + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE {table} SET {sets} WHERE id=?{} AND owner_user_id=?{} AND revoked_at IS NULL",
            count + 1,
            count + 2
        );
        let values = self
            .0
            .into_iter()
            .map(|(_, value)| value)
            .chain([id.to_owned().into(), owner.to_owned().into()]);
        conn.execute(&sql, rusqlite::params_from_iter(values))
            .map(|changed| changed == 1)
    }
}

/// The answer to an edit: success only after the row changed as asked.
pub(super) fn patch_response(
    applied: rusqlite::Result<bool>,
    id: &str,
    not_found: &str,
) -> Response {
    match applied {
        Ok(true) => Json(json!({ "id": id, "ok": true })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ApiError::new("not_found", not_found)),
        )
            .into_response(),
        Err(error) => {
            tracing::warn!(%error, id, "Shared access edit failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError::new("internal", "The change could not be saved")),
            )
                .into_response()
        }
    }
}

/// What a stored row that cannot be read as a time becomes: already expired.
/// The relay cannot tell what such a value meant, so it gives no access.
pub(super) const UNREADABLE_EXPIRY: &str = "1970-01-01T00:00:00.000Z";

/// Rewrites stored expiry times into the stored form, once per start. Rows
/// written before validation may hold offsets, other precisions or text.
pub(super) fn ensure_schema(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for table in ["relay_shares", "relay_access_grants"] {
        let rows: Vec<(String, String)> = tx
            .prepare(&format!(
                "SELECT id,expires_at FROM {table} WHERE expires_at IS NOT NULL"
            ))?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (id, stored) in rows {
            let normalized = normalize_expiry(&stored);
            let value = normalized.as_deref().unwrap_or(UNREADABLE_EXPIRY);
            if value == stored {
                continue;
            }
            if normalized.is_none() {
                tracing::warn!(
                    table,
                    id,
                    "Unreadable share expiry; the row now counts as expired"
                );
            }
            tx.execute(
                &format!("UPDATE {table} SET expires_at=?1 WHERE id=?2"),
                params![value, id],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_times_are_stored_in_the_form_they_are_compared_in() {
        assert_eq!(
            normalize_expiry("2026-11-19T10:30:00+02:00").as_deref(),
            Some("2026-11-19T08:30:00.000Z")
        );
        assert_eq!(
            normalize_expiry("2026-11-19T08:30:00Z").as_deref(),
            Some("2026-11-19T08:30:00.000Z")
        );
        assert_eq!(
            normalize_expiry("2026-11-19T08:30:00.123456-00:00").as_deref(),
            Some("2026-11-19T08:30:00.123Z")
        );
        // 10:30+02:00 is earlier than 09:00Z, which text order got wrong.
        let offset = normalize_expiry("2026-11-19T10:30:00+02:00").unwrap();
        let utc = normalize_expiry("2026-11-19T09:00:00Z").unwrap();
        assert!("2026-11-19T10:30:00+02:00" > "2026-11-19T09:00:00Z");
        assert!(offset < utc);
        for bad in ["", "tomorrow", "2026-11-19", "2026-11-19 08:30", "never"] {
            assert_eq!(normalize_expiry(bad), None, "{bad}");
        }
    }
}
