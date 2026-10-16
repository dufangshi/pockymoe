//! Only connection discovery metadata is persisted. Credentials stay on devices.
use super::*;

pub(super) fn ensure_schema(c: &Connection) -> Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS relay_upstream_catalog(device_id TEXT NOT NULL,connection_id TEXT NOT NULL,name TEXT NOT NULL,protocol TEXT NOT NULL,revision TEXT NOT NULL,PRIMARY KEY(device_id,connection_id));")?;
    Ok(())
}
pub(super) fn replace(c: &Connection, device: &str, input: &Value) -> Result<()> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Row {
        id: String,
        name: String,
        protocol: String,
        revision: String,
    }
    let rows: Vec<Row> =
        serde_json::from_value(input.clone()).context("Invalid upstream catalog")?;
    anyhow::ensure!(rows.len() <= 1000, "Too many upstream connections");
    let mut seen = std::collections::HashSet::new();
    for r in &rows {
        Uuid::parse_str(&r.id)?;
        Uuid::parse_str(&r.revision)?;
        anyhow::ensure!(
            seen.insert(&r.id)
                && !r.name.trim().is_empty()
                && r.name.len() <= 120
                && !r.name.chars().any(char::is_control)
                && ["openai_chat", "openai_responses", "anthropic", "gemini"]
                    .contains(&r.protocol.as_str()),
            "Invalid upstream metadata"
        );
    }
    let tx = c.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM relay_upstream_catalog WHERE device_id=?1",
        [device],
    )?;
    for r in rows {
        tx.execute("INSERT INTO relay_upstream_catalog(device_id,connection_id,name,protocol,revision) VALUES(?1,?2,?3,?4,?5)",params![device,r.id,r.name,r.protocol,r.revision])?;
    }
    tx.commit()?;
    Ok(())
}
pub(super) async fn directory(state: &AppState, device: &str, connection: Uuid, frame: Value) {
    let Some(id) = frame["requestId"].as_str() else {
        return;
    };
    let rows = {
        let c = state.store.conn.lock().await;
        (|| -> Result<Vec<Value>> {
            let owner:String=c.query_row("SELECT d.owner_user_id FROM relay_devices d JOIN relay_users u ON u.id=d.owner_user_id WHERE d.id=?1 AND u.enabled=1",[device],|r|r.get(0))?;
            let mut stmt=c.prepare("SELECT m.connection_id,m.name,m.protocol,m.revision,d.id,d.name FROM relay_upstream_catalog m JOIN relay_devices d ON d.id=m.device_id WHERE d.owner_user_id=?1 ORDER BY m.connection_id,d.id")?;
            let rows=stmt.query_map([owner],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"protocol":r.get::<_,String>(2)?,"revision":r.get::<_,String>(3)?,"deviceId":r.get::<_,String>(4)?,"deviceName":r.get::<_,String>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })()
    };
    let frame = match rows {
        Ok(mut rows) => {
            let sockets = state.sockets.read().await;
            for r in &mut rows {
                r["online"] = json!(sockets.contains_key(r["deviceId"].as_str().unwrap()));
            }
            json!({"type":"upstream.directory.result","requestId":id,"connections":rows})
        }
        Err(_) => {
            json!({"type":"peer.response","requestId":id,"payload":{"statusCode":403,"body":"{\"code\":\"catalog_forbidden\",\"message\":\"Catalog unavailable\"}"}})
        }
    };
    let sockets = state.sockets.read().await;
    if let Some(s) = sockets
        .get(device)
        .filter(|s| s.connection_id == connection)
    {
        let _ = s.tx.try_send(frame.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_metadata_only_validated_and_atomically_replaced() {
        let c = Connection::open_in_memory().unwrap();
        ensure_schema(&c).unwrap();
        let row = json!({"id":Uuid::new_v4().to_string(),"name":"My upstream","protocol":"gemini","revision":Uuid::new_v4().to_string()});
        replace(&c, "A", &json!([row.clone()])).unwrap();
        replace(&c, "B", &json!([row.clone()])).unwrap();
        let mut secret = row.clone();
        secret["apiKey"] = json!("fixture-secret");
        assert!(replace(&c, "A", &json!([secret])).is_err());
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM relay_upstream_catalog", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert!(replace(&c, "A", &json!([row.clone(), row])).is_err());
        replace(&c, "A", &json!([])).unwrap();
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM relay_upstream_catalog WHERE device_id='B'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
}
