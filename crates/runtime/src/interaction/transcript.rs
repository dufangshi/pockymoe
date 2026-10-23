use crate::service::thread_summary::{SummaryPageOptions, SummaryTurn};
use crate::Supervisor;
use anyhow::{ensure, Context, Result};
use pockymoe_protocol::{now_rfc3339, ThreadSummaryMessage};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub use pockymoe_protocol::ThreadTranscriptQuery as TranscriptQuery;

/// Summary view bounds: turns per page, and Unicode characters per page and message.
const SUMMARY_TURNS: u32 = 10;
const SUMMARY_MAX_TURNS: u32 = 50;
const SUMMARY_CHARS: u64 = 16_000;
const SUMMARY_MESSAGE_CHARS: u64 = 1_500;

impl Supervisor {
    pub fn transcript(&self, id: &str, q: &TranscriptQuery) -> Result<Value> {
        self.get_thread(id)?;
        if q.view.as_deref() == Some("summary") {
            return self.transcript_summary(id, q);
        }
        let limit = q.limit.unwrap_or(3);
        ensure!((1..=20).contains(&limit), "limit must be between 1 and 20");
        ensure!(
            q.item_id.is_none() || q.turn_id.is_some(),
            "itemId requires turnId"
        );
        ensure!(!q.raw || q.item_id.is_some(), "raw requires itemId");
        ensure!(
            q.before_turn_id.is_none() || q.turn_id.is_none(),
            "beforeTurnId conflicts with turnId"
        );
        let view = q.view.as_deref().unwrap_or(if q.turn_id.is_some() {
            "turn"
        } else {
            "overview"
        });
        ensure!(
            matches!(view, "overview" | "turn"),
            "view must be overview, turn or summary"
        );
        ensure!(
            view != "turn" || q.turn_id.is_some(),
            "turn view requires turnId"
        );
        self.db.with(|c| {
            if let Some(item) = &q.item_id {
                let offset = q.text_offset.unwrap_or(0);
                let expr = if q.raw { "item_json" } else { "COALESCE(json_extract(item_json,'$.detailText'),json_extract(item_json,'$.text'),'')" };
                let sql = format!("SELECT substr({expr},?4+1,8192),length({expr}),updated_at FROM thread_history_items WHERE thread_id=?1 AND turn_id=?2 AND item_id=?3");
                let (text,length,updated): (String,u32,String) = c.query_row(&sql,params![id,q.turn_id,item,offset],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.context("history item not found")?;
                ensure!(offset<=length,"textOffset is beyond the content");
                let end=offset+text.chars().count() as u32;
                return Ok(json!({"threadId":id,"turnId":q.turn_id,"itemId":item,"raw":q.raw,"text":text,"textOffset":offset,"textLength":length,"nextTextOffset":(end<length).then_some(end),"updatedAt":updated,"observedAt":now_rfc3339(),"next":(end<length).then(||format!("pockymoe transcript {id} --turn {} --item {} {} --text-offset {end}",q.turn_id.as_deref().unwrap(),shell_words::quote(item),if q.raw {"--raw"} else {""}))}));
            }
            let before: Option<i64> = if let Some(turn) = &q.before_turn_id { Some(c.query_row("SELECT ordinal FROM thread_turns WHERE thread_id=?1 AND id=?2",params![id,turn],|r|r.get(0)).optional()?.context("before turn not found")?) } else { None };
            let mut stmt = c.prepare("SELECT id,status,started_at,completed_at,ordinal FROM thread_turns WHERE thread_id=?1 AND (?2 IS NULL OR ordinal<?2) AND (?3 IS NULL OR id=?3) ORDER BY ordinal DESC LIMIT ?4")?;
            let mut turns = stmt.query_map(params![id,before,q.turn_id,limit],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,i64>(4)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            ensure!(q.turn_id.is_none() || !turns.is_empty(),"turn not found");
            turns.reverse();
            let mut result=vec![];
            // Bound the number AND text size of entries across the page.
            let entries_per_turn = (36 / turns.len().max(1)).min(12);
            for (turn,status,started,completed,_) in &turns {
                let filter=if view=="overview" {"AND json_extract(item_json,'$.kind') IN ('userMessage','agentMessage')"} else {""};
                let offset=q.offset.unwrap_or(0);
                let count_sql=format!("SELECT count(*) FROM thread_history_items WHERE thread_id=?1 AND turn_id=?2 {filter}");
                let count:u32=c.query_row(&count_sql,params![id,turn],|r|r.get(0))?;
                let total:u32=c.query_row("SELECT count(*) FROM thread_history_items WHERE thread_id=?1 AND turn_id=?2",params![id,turn],|r|r.get(0))?;
                let text_expr=if view=="overview" {"COALESCE(json_extract(item_json,'$.text'),'')"} else {"COALESCE(json_extract(item_json,'$.previewText'),json_extract(item_json,'$.text'),'')"};
                let cap=if view=="overview" {(16_000 / entries_per_turn / turns.len().max(1)).min(2048) as u32} else {256};
                let sql=format!("SELECT item_id,json_extract(item_json,'$.kind'),json_extract(item_json,'$.createdAt'),substr({text_expr},1,{cap}),length({text_expr}),json_extract(item_json,'$.status') FROM thread_history_items WHERE thread_id=?1 AND turn_id=?2 {filter} ORDER BY created_at,rowid LIMIT ?3 OFFSET ?4");
                let mut items_stmt=c.prepare(&sql)?;
                let items=items_stmt.query_map(params![id,turn,entries_per_turn as u32,offset],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,r.get::<_,u32>(4)?,r.get::<_,Option<String>>(5)?)))?.collect::<std::result::Result<Vec<_>,_>>()?.into_iter().map(|(item,kind,created,text,len,status)|json!({"id":item,"kind":kind,"createdAt":created,"text":text,"truncated":len>cap,"status":status,"detail":format!("pockymoe transcript {id} --turn {turn} --item {}",shell_words::quote(&item))})).collect::<Vec<_>>();
                let end=offset+items.len() as u32;
                result.push(json!({"id":turn,"status":status,"startedAt":started,"completedAt":completed,"items":items,"itemCount":count,"hiddenItemCount":total-count,"offset":offset,"nextOffset":(end<count).then_some(end),"next":(end<count).then(||format!("pockymoe transcript {id} --turn {turn} --view {view} --offset {end}")),"expand":format!("pockymoe transcript {id} --turn {turn}")}));
            }
            let earlier = if let Some(first)=turns.first() { c.query_row("SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND ordinal<?2)",params![id,first.4],|r|r.get::<_,bool>(0))? } else {false};
            let next=turns.first().filter(|_|earlier && q.turn_id.is_none()).map(|t|t.0.clone());
            Ok(json!({"threadId":id,"view":view,"turns":result,"nextBeforeTurnId":next,"next":next.as_ref().map(|turn|format!("pockymoe transcript {id} --before-turn {turn} --limit {limit}")),"observedAt":now_rfc3339()}))
        })
    }

    /// The public conversation of the newest turns: each user message and the
    /// reply to it, grouped by turn with the command that reads that turn.
    fn transcript_summary(&self, id: &str, q: &TranscriptQuery) -> Result<Value> {
        ensure!(
            q.turn_id.is_none() && q.item_id.is_none() && !q.raw,
            "summary view reads whole turns; read one with --turn TURN_ID --view overview"
        );
        ensure!(
            q.offset.unwrap_or(0) == 0 && q.text_offset.unwrap_or(0) == 0,
            "summary view pages with beforeTurnId, not offsets"
        );
        let limit = q.limit.unwrap_or(SUMMARY_TURNS);
        ensure!(
            (1..=SUMMARY_MAX_TURNS).contains(&limit),
            "limit must be between 1 and {SUMMARY_MAX_TURNS} for the summary view"
        );
        let page = self.thread_summary_page(
            id,
            limit,
            SUMMARY_CHARS,
            &SummaryPageOptions {
                before_turn: q.before_turn_id.as_deref(),
                final_replies: true,
                message_chars: Some(SUMMARY_MESSAGE_CHARS),
            },
        )?;
        fn of<'a>(
            messages: &'a [ThreadSummaryMessage],
            turn: &'a SummaryTurn,
        ) -> impl Iterator<Item = &'a ThreadSummaryMessage> {
            messages
                .iter()
                .filter(move |m| m.turn_id.as_deref() == Some(turn.id.as_str()))
        }
        let messages = &page.summary.messages;
        let mut turns = page.turns;
        turns.reverse();
        // When the character budget ran out, the page starts at the oldest turn
        // it shows; that turn can lack its earlier messages.
        if page.budget_exhausted {
            if let Some(first) = turns.iter().position(|t| of(messages, t).next().is_some()) {
                turns.drain(..first);
            }
        }
        let shown = turns
            .iter()
            .map(|turn| {
                let messages = of(messages, turn)
                    .map(|m| {
                        let mut message = json!({"role":m.role,"text":m.text});
                        if let Some(time) = &m.time {
                            message["time"] = json!(time);
                        }
                        if m.truncated {
                            message["truncated"] = json!(true);
                        }
                        message
                    })
                    .collect::<Vec<_>>();
                json!({"id":turn.id,"ordinal":turn.ordinal,"status":turn.status,"messages":messages,"overview":format!("pockymoe transcript {id} --turn {} --view overview",turn.id)})
            })
            .collect::<Vec<_>>();
        let earlier = match turns.first() {
            Some(first) => self.db.with(|c| {
                Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM thread_turns WHERE thread_id=?1 AND ordinal<?2)",
                    params![id, first.ordinal],
                    |r| r.get::<_, bool>(0),
                )?)
            })?,
            None => false,
        };
        let next = turns.first().filter(|_| earlier).map(|t| t.id.clone());
        Ok(json!({
            "threadId": id,
            "view": "summary",
            "turns": shown,
            "truncated": page.summary.truncated,
            "nextBeforeTurnId": next,
            "next": next.as_ref().map(|turn| format!("pockymoe transcript {id} --view summary --before-turn {turn} --limit {limit}")),
            "observedAt": now_rfc3339(),
        }))
    }
}
