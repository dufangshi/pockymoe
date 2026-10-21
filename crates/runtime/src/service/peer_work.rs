//! Read projection for the work a delegate message caused. Execution turns and
//! billing stay separate; the transcript can nest them under the original task.
use super::*;

/// Every wake header starts with this; the rest of the line names the reason.
pub(crate) const WAKE_PREFIX: &str = "[Pockymoe wake: ";
pub(crate) const WAKE_SETTLED: &str =
    "[Pockymoe wake: the delegates you registered a wake for have settled]\n";
pub(crate) const WAKE_BLOCKED: &str =
    "[Pockymoe wake: a delegate you registered a wake for is blocked]\n";
/// A wake reports on delegated work, so it can join the caller's running turn.
pub(crate) const WAKE_NOTE: &str =
    "This continues the task you delegated from: it adds information, not a new instruction.\n";

/// The text after a wake header line, whatever its reason.
pub(crate) fn wake_body(prompt: &str) -> Option<&str> {
    let (header, body) = prompt.strip_prefix(WAKE_PREFIX)?.split_once('\n')?;
    header.ends_with(']').then_some(body)
}

pub(crate) fn root_turn(conn: &Connection, thread: &str, turn: &str) -> Result<String> {
    if let Some(raw) = conn
        .query_row(
            "SELECT value FROM kv WHERE key=?1",
            [format!("cli:peer-work:turn:{turn}")],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        if let Some(root) = serde_json::from_str::<Value>(&raw)?
            .get("taskTurnId")
            .and_then(Value::as_str)
        {
            return Ok(root.to_owned());
        }
    }
    Ok(conn.query_row(
        "SELECT id FROM thread_turns WHERE thread_id=?1 AND ordinal <= (SELECT ordinal FROM thread_turns WHERE id=?2 AND thread_id=?1)
         AND (display_prompt IS NULL OR substr(display_prompt,1,?3)!=?4) ORDER BY ordinal DESC LIMIT 1",
        params![thread,turn,WAKE_PREFIX.chars().count(),WAKE_PREFIX], |r| r.get(0),
    ).optional()?.unwrap_or_else(|| turn.to_owned()))
}

pub(crate) fn context(
    conn: &Connection,
    thread: &str,
    task: &str,
    prompt: &str,
) -> Result<Option<Value>> {
    let Some(body) = wake_body(prompt) else {
        return Ok(None);
    };
    if !body.contains("Verify results before acting on them. Full transcripts:") {
        return Ok(None);
    }
    let mut messages = Vec::new();
    let mut statement = conn.prepare(
        "SELECT value FROM kv WHERE key GLOB ?1 AND json_extract(value,'$.kind')='question'",
    )?;
    let mail = statement
        .query_map([format!("cli:inbox:{thread}:*")], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for raw in mail {
        let message: Value = serde_json::from_str(&raw)?;
        let Some(id) = message["id"].as_str() else {
            continue;
        };
        if !body.contains(&format!("question {id} to {thread}:")) {
            continue;
        }
        let name: Option<String> = message["fromThreadId"]
            .as_str()
            .map(|from| {
                conn.query_row(
                    "SELECT COALESCE(agent_name,title) FROM threads WHERE id=?1",
                    [from],
                    |r| r.get(0),
                )
                .optional()
            })
            .transpose()?
            .flatten();
        messages.push(json!({"id":id,"fromThreadId":message["fromThreadId"],"fromDeviceId":message["fromDeviceId"],"name":name,"kind":"question","subject":message["subject"],"text":message["text"],"createdAt":message["createdAt"]}));
    }
    if messages.is_empty() {
        // Older wakes predate explicit causal metadata. Accept only actual local
        // descendants from the system envelope, not arbitrary quoted user text.
        for line in body.lines().filter_map(|line| line.strip_prefix("- ")) {
            let Some((name, rest)) = line.split_once(" (") else {
                continue;
            };
            let Some((id, state)) = rest.split_once("): ") else {
                continue;
            };
            if Uuid::parse_str(id).is_err() {
                continue;
            }
            let descendant: bool = conn.query_row("WITH RECURSIVE family(id) AS (SELECT id FROM threads WHERE parent_thread_id=?1 UNION ALL SELECT t.id FROM threads t JOIN family f ON t.parent_thread_id=f.id) SELECT EXISTS(SELECT 1 FROM family WHERE id=?2)", params![thread,id], |r| r.get(0))?;
            if !descendant {
                continue;
            }
            messages.push(json!({"id":format!("delegate:{id}"),"fromThreadId":id,"name":name,"kind":"result","subject":state,"text":null}));
        }
    }
    if messages.is_empty() {
        return Ok(None);
    }
    Ok(Some(json!({"taskTurnId":task,"messages":messages})))
}

pub(super) fn annotate(
    conn: &Connection,
    thread: &str,
    turn: &str,
    items: &mut [ThreadHistoryItemDto],
    detail: bool,
) -> Result<()> {
    let Some(user) = items.iter_mut().find(|item| item.kind == "userMessage") else {
        return Ok(());
    };
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE key=?1",
            [format!("cli:peer-work:turn:{turn}")],
            |r| r.get(0),
        )
        .optional()?;
    let context = match raw {
        Some(raw) => Some(serde_json::from_str::<Value>(&raw)?),
        None => context(conn, thread, &root_turn(conn, thread, turn)?, &user.text)?,
    };
    if let Some(mut context) = context {
        if !detail {
            if let Some(messages) = context["messages"].as_array_mut() {
                for message in messages {
                    message.as_object_mut().unwrap().remove("text");
                }
            }
        }
        user.extra.insert("peerWork".into(), context);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE kv(key TEXT PRIMARY KEY,value TEXT); CREATE TABLE threads(id TEXT PRIMARY KEY,parent_thread_id TEXT,agent_name TEXT,title TEXT); CREATE TABLE thread_turns(id TEXT PRIMARY KEY,thread_id TEXT,ordinal INTEGER,display_prompt TEXT);
         INSERT INTO threads VALUES('parent',NULL,NULL,'Lead');
         INSERT INTO threads VALUES('12345678-1234-1234-1234-123456789012','parent','trust-web','Web');
         INSERT INTO thread_turns VALUES('task','parent',1,'Implement pairing');").unwrap();
        conn.execute("INSERT INTO kv VALUES(?1,?2)",params!["cli:inbox:parent:question-one",json!({"id":"question-one","fromThreadId":"12345678-1234-1234-1234-123456789012","kind":"question","subject":"Session lifetime","text":"Can a device register this session tomorrow?","createdAt":"2026-10-12T00:23:00Z","acknowledgedAt":null}).to_string()]).unwrap();
        conn
    }
    fn prompt() -> String {
        format!("{WAKE_SETTLED}- trust-web (12345678-1234-1234-1234-123456789012): blocked, waiting on: question question-one to parent: Session lifetime\n\nVerify results before acting on them. Full transcripts: pockymoe transcript ID --limit 1")
    }
    #[test]
    fn peer_work_projects_the_full_question_only_on_detail_without_acknowledging_it() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO thread_turns VALUES('wake','parent',2,?1)",
            [prompt()],
        )
        .unwrap();
        let item: ThreadHistoryItemDto =
            serde_json::from_value(json!({"id":"wake:user","kind":"userMessage","text":prompt()}))
                .unwrap();
        let mut items = vec![item];
        annotate(&conn, "parent", "wake", &mut items, false).unwrap();
        assert_eq!(items[0].extra["peerWork"]["taskTurnId"], "task");
        assert_eq!(
            items[0].extra["peerWork"]["messages"][0]["name"],
            "trust-web"
        );
        assert!(items[0].extra["peerWork"]["messages"][0]
            .get("text")
            .is_none());
        annotate(&conn, "parent", "wake", &mut items, true).unwrap();
        assert_eq!(
            items[0].extra["peerWork"]["messages"][0]["text"],
            "Can a device register this session tomorrow?"
        );
        let raw: String = conn
            .query_row(
                "SELECT value FROM kv WHERE key='cli:inbox:parent:question-one'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(serde_json::from_str::<Value>(&raw).unwrap()["acknowledgedAt"].is_null());
    }
    #[test]
    fn peer_work_repeated_wakes_retain_the_original_task_and_new_user_tasks_are_separate() {
        let conn = fixture();
        for (id, ordinal, text) in [
            ("first", 2, prompt()),
            ("second", 3, prompt()),
            ("new-task", 4, "New user request".into()),
            ("third", 5, prompt()),
        ] {
            conn.execute(
                "INSERT INTO thread_turns VALUES(?1,'parent',?2,?3)",
                params![id, ordinal, text],
            )
            .unwrap();
        }
        assert_eq!(root_turn(&conn, "parent", "second").unwrap(), "task");
        assert_eq!(root_turn(&conn, "parent", "third").unwrap(), "new-task");
    }
    #[test]
    fn peer_work_ignores_ordinary_prompts_and_foreign_mail() {
        let conn = fixture();
        assert!(context(&conn, "parent", "task", "What is a question?")
            .unwrap()
            .is_none());
        assert!(context(&conn, "someone-else", "task", &prompt())
            .unwrap()
            .is_none());
    }
}
