//! Compatibility projection for already-saved replies; never rewrite history.
use super::*;
use sha2::{Digest, Sha256};
use std::time::SystemTime;

type Stamp = (PathBuf, u64, SystemTime);
type Phases = HashMap<String, Value>;
#[derive(Default)]
pub(super) struct ReplyPhaseCache(Mutex<HashMap<String, (Stamp, Phases)>>);

fn needs_phases(turn: &ThreadTurnDto) -> bool {
    turn.status == "completed"
        && turn.items.iter().any(|item| {
            matches!(
                item.extra.get("origin").and_then(Value::as_str),
                Some("nativeBackgroundWait" | "nativeTaskNotification")
            )
        })
        && turn
            .items
            .iter()
            .any(|item| item.kind == "agentMessage" && !has_phase(item))
}

fn has_phase(item: &ThreadHistoryItemDto) -> bool {
    matches!(
        item.extra.get("responsePhase").and_then(Value::as_str),
        Some("final" | "commentary")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn legacy_final_report_survives_summary_and_detail_without_rewriting_history() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::from_env().unwrap();
        config.database_url = dir.path().join("isolated.sqlite");
        config.workspace_root = dir.path().join("workspaces");
        config.fake_runtime = true;
        config.relay_server_url = None;
        config.relay_agent_token = None;
        let homes = LocalSessionHomes {
            claude_home: dir.path().join("claude"),
            codex_home: dir.path().join("codex"),
            grok_home: dir.path().join("grok"),
        };
        let supervisor = Supervisor::new(
            config,
            Database::open(&dir.path().join("isolated.sqlite")).unwrap(),
            vec![Arc::new(crate::fake::FakeRuntime::new(Provider::Claude))],
        )
        .with_local_session_homes(homes.clone());
        let workspace = supervisor
            .create_workspace(CreateWorkspaceInput {
                abs_path: Some(dir.path().to_string_lossy().into_owned()),
                git_url: None,
                label: None,
            })
            .unwrap();
        let thread = supervisor
            .create_thread(CreateThreadInput {
                workspace_id: workspace.id,
                title: None,
                provider: Some(Provider::Claude),
                agent_id: None,
                model: "default".into(),
                reasoning_effort: None,
                approval_mode: "yolo".into(),
                parent_thread_id: None,
            })
            .await
            .unwrap();
        let session = "788305ef-f282-4ded-8ca1-ef24a6333de0";
        let native = include_str!("../../tests/fixtures/claude_legacy_reply_phases.jsonl");
        let project = homes.claude_home.join("projects/test");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join(format!("{session}.jsonl"));
        std::fs::write(&path, native).unwrap();
        supervisor.db.with(|conn| {
            conn.execute("UPDATE threads SET provider_session_id=?1 WHERE id=?2",params![format!("claude::{session}"),thread.id])?;
            for (id,start,end,text) in [
                ("waiting","2026-10-10T21:13:55.220Z","2026-10-10T21:38:06.786Z","Still waiting on the other platforms"),
                ("report","2026-10-11T02:34:06.916Z","2026-10-11T02:38:14.653Z","Repositories renamed."),
            ] {
                conn.execute("INSERT INTO thread_turns(id,thread_id,status,started_at,completed_at,ordinal) VALUES(?1,?2,'completed',?3,?4,?5)",params![id,thread.id,start,end,if id=="waiting" {1}else{2}])?;
                for item in [json!({"id":format!("{id}:wake"),"kind":"generic","text":"Build finished","origin":"nativeTaskNotification","createdAt":start}),json!({"id":format!("{id}:reply"),"kind":"agentMessage","text":text,"createdAt":end})] {
                    conn.execute("INSERT INTO thread_history_items(thread_id,turn_id,item_id,item_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",params![thread.id,id,item["id"].as_str().unwrap(),item.to_string(),item["createdAt"].as_str().unwrap()])?;
                }
            }
            Ok(())
        }).unwrap();
        for _ in 0..2 {
            let summary = supervisor
                .get_thread_detail_view(&thread.id, Some(3), true)
                .await
                .unwrap();
            assert_eq!(
                summary.turns[0].items.last().unwrap().extra["responsePhase"],
                "commentary"
            );
            assert_eq!(
                summary.turns[1].items.last().unwrap().extra["responsePhase"],
                "final"
            );
            let detail = supervisor
                .get_thread_turn_detail(&thread.id, "report")
                .await
                .unwrap();
            assert_eq!(detail.items.last().unwrap().extra["responsePhase"], "final");
        }
        let raw = supervisor
            .load_items_for_turn(&thread.id, "report")
            .unwrap();
        assert!(
            !raw.last().unwrap().extra.contains_key("responsePhase"),
            "compatibility must not rewrite saved records"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), native);
    }
}

impl Supervisor {
    pub(super) async fn hydrate_legacy_claude_reply_phases(
        &self,
        thread: &ThreadDto,
        turns: &mut [ThreadTurnDto],
    ) {
        if !(thread.provider == Provider::Claude || thread.agent_id.as_deref() == Some("claude"))
            || !turns.iter().any(needs_phases)
        {
            return;
        }
        let Some(session) = thread.provider_session_id.as_deref() else {
            return;
        };
        let session = parse_session_ref(session).raw_id;
        if Uuid::parse_str(&session).is_err() {
            return;
        }
        let home = self.local_session_homes.claude_home.join("projects");
        let name = format!("{session}.jsonl");
        let stamp = tokio::task::spawn_blocking(move || {
            let path = walkdir::WalkDir::new(home)
                .max_depth(2)
                .into_iter()
                .filter_map(|e| e.ok())
                .find(|e| e.file_type().is_file() && e.file_name() == name.as_str())?
                .into_path();
            let meta = std::fs::metadata(&path).ok()?;
            Some((path, meta.len(), meta.modified().ok()?))
        })
        .await
        .ok()
        .flatten();
        let Some(stamp) = stamp else { return };
        let mut cache = self.claude_reply_phases.0.lock().await;
        let mut missing = Vec::new();
        let mut keys = HashMap::new();
        for turn in turns.iter().filter(|t| needs_phases(t)) {
            let mut digest = Sha256::new();
            for item in &turn.items {
                if item.kind == "agentMessage" {
                    digest.update(item.id.as_bytes());
                    digest.update(item.text.as_bytes());
                }
            }
            let key = format!(
                "{session}:{}:{:?}:{:?}:{}",
                turn.id,
                turn.started_at,
                turn.completed_at,
                hex::encode(digest.finalize())
            );
            if !cache.get(&key).is_some_and(|(old, _)| old == &stamp) {
                let mut candidate = turn.clone();
                candidate.items.retain(|item| {
                    item.kind == "agentMessage" || item.extra.contains_key("origin")
                });
                missing.push(candidate);
            }
            keys.insert(turn.id.clone(), key);
        }
        if !missing.is_empty() {
            let path = stamp.0.clone();
            let read = tokio::task::spawn_blocking(move || {
                crate::acp::legacy_background_reply_phases(&path, &session, &mut missing).ok()?;
                Some(missing)
            })
            .await
            .ok()
            .flatten();
            let Some(read) = read else { return };
            if cache.len() >= 64 {
                cache.clear();
            }
            for turn in read {
                let phases = turn
                    .items
                    .into_iter()
                    .filter_map(|item| {
                        item.extra
                            .get("responsePhase")
                            .cloned()
                            .map(|phase| (item.id, phase))
                    })
                    .collect();
                cache.insert(keys[&turn.id].clone(), (stamp.clone(), phases));
            }
        }
        for turn in turns.iter_mut().filter(|t| needs_phases(t)) {
            if let Some((_, phases)) = cache.get(&keys[&turn.id]) {
                for item in &mut turn.items {
                    if !has_phase(item) {
                        if let Some(phase) = phases.get(&item.id) {
                            item.extra.insert("responsePhase".into(), phase.clone());
                        }
                    }
                }
            }
        }
    }
}
