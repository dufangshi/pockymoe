use super::store;
use anyhow::Result;
use pockymoe_runtime::Supervisor;
use serde_json::{json, Value};
use std::sync::Arc;
/// Dropping a cancelled call still leaves an outcome, without storing its input.
pub struct CallAudit {
    state: Arc<Supervisor>,
    id: String,
    record: Value,
    finished: bool,
}
impl CallAudit {
    pub fn begin(state: &Arc<Supervisor>, client: &str, tool: &str, args: &Value) -> Result<Self> {
        let audit = Self {
            state: state.clone(),
            id: uuid::Uuid::new_v4().to_string(),
            record: json!({"client":client,"tool":tool.chars().take(100).collect::<String>(),"target":args["threadId"].as_str().or(args["workspaceId"].as_str()).map(|s|s.chars().take(100).collect::<String>()),"time":store::now(),"result":"running"}),
            finished: false,
        };
        audit.save()?;
        Ok(audit)
    }
    fn save(&self) -> Result<()> {
        store::transaction(&self.state, |db| {
            store::put(db, "audit", &self.id, &self.record, 30 * 86400)?;
            db.execute("DELETE FROM entries WHERE kind='audit' AND rowid NOT IN (SELECT rowid FROM entries WHERE kind='audit' ORDER BY rowid DESC LIMIT 1000)",[])?;
            Ok(())
        })
    }
    pub fn finish(&mut self, success: bool) -> Result<()> {
        self.record["result"] = json!(if success { "ok" } else { "error" });
        self.finished = true;
        self.save()
    }
}
impl Drop for CallAudit {
    fn drop(&mut self) {
        if !self.finished {
            self.record["result"] = json!("cancelled");
            let _ = self.save();
        }
    }
}
