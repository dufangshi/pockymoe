use anyhow::{bail, Result};
use pockymoe_protocol::{ThreadHistoryItemDto, ThreadTurnDto, WorkspaceDto};
use pockymoe_runtime::service::{SelectedTurns, TurnSelection};

const MAX_ITEM_CHARS: usize = 12_000;
#[derive(Debug, Clone)]
pub struct TranscriptExportOptions {
    pub profile: String,
    pub include_token_and_price: bool,
    pub include_command_output: bool,
    pub include_absolute_paths: bool,
}

impl Default for TranscriptExportOptions {
    fn default() -> Self {
        Self {
            profile: "review".into(),
            include_token_and_price: true,
            include_command_output: false,
            include_absolute_paths: false,
        }
    }
}

/// Validates an export request before any history loads.
pub fn turn_selection<'a>(
    mode: &str,
    limit: Option<usize>,
    turn_ids: &'a [String],
) -> Result<TurnSelection<'a>> {
    if mode == "selected" {
        if turn_ids.is_empty() {
            bail!("Select at least one turn to export.");
        }
        if turn_ids.len() > 100 {
            bail!("An export can include at most 100 turns.");
        }
        return Ok(TurnSelection::Ids(turn_ids));
    }
    let limit = limit.unwrap_or(10).clamp(1, 100);
    Ok(TurnSelection::Latest(limit as u32))
}

pub fn html_transcript(
    selected: &SelectedTurns,
    options: &TranscriptExportOptions,
) -> Result<String> {
    let title = escape_html(&selected.thread.title);
    let workspace = escape_html(&selected.workspace.label);
    let mut body = String::new();
    let switch_html = |after: Option<&str>| -> String {
        selected
            .harness_switches
            .iter()
            .filter(|entry| entry.extra.get("afterTurnId").and_then(|v| v.as_str()) == after)
            .map(|entry| {
                let change = &entry.extra["harnessSwitch"];
                format!(
                    "<p class=\"harness-switch\">{} · {} → {} · {} · {} turns handed over</p>",
                    escape_html(change["fromAgentId"].as_str().unwrap_or("")),
                    escape_html(change["fromModel"].as_str().unwrap_or("")),
                    escape_html(change["agentId"].as_str().unwrap_or("")),
                    escape_html(change["model"].as_str().unwrap_or("")),
                    change["turns"].as_u64().unwrap_or(0)
                )
            })
            .collect()
    };
    body.push_str(&switch_html(None));
    for numbered in &selected.turns {
        let (number, turn) = (numbered.number, &numbered.turn);
        let turn_meta = turn_meta(turn, options);
        body.push_str(&format!(
            "<section class=\"turn\"><header><span>Turn {number}</span><span class=\"status\">{}</span></header>",
            escape_html(&turn_meta)
        ));
        for item in visible_items(turn, options) {
            let label = item_label(item.kind.as_str());
            let text = escape_html(&export_item_text(item, &selected.workspace, options));
            if matches!(item.kind.as_str(), "userMessage" | "agentMessage") {
                let role = if item.kind == "userMessage" {
                    "user"
                } else {
                    "agent"
                };
                body.push_str(&format!(
                    "<article class=\"message {role}\"><div class=\"label\">{label}</div><div class=\"content\">{text}</div></article>"
                ));
            } else {
                let status = item
                    .status
                    .as_deref()
                    .map(|value| format!("<span class=\"status\">{}</span>", escape_html(value)))
                    .unwrap_or_default();
                body.push_str(&format!(
                    "<details class=\"event\"><summary><span>{label}</span>{status}</summary><pre>{text}</pre></details>"
                ));
            }
        }
        body.push_str("</section>");
        body.push_str(&switch_html(Some(&turn.id)));
    }

    Ok(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="light dark"><title>{title}</title>
<style>
:root{{color-scheme:light dark;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI","Noto Sans",sans-serif;background:#f5f5f1;color:#20201d}}*{{box-sizing:border-box}}body{{margin:0;background:#f5f5f1;color:#20201d}}main{{width:min(860px,calc(100% - 32px));margin:0 auto;padding:56px 0 80px}}.document-header{{padding-bottom:28px;border-bottom:1px solid #cecec5}}h1{{margin:0;font-size:30px;line-height:1.2;letter-spacing:0}}.meta{{margin:10px 0 0;color:#66665f;font-size:14px}}.turn{{padding:28px 0;border-bottom:1px solid #d9d9d1}}.turn>header{{display:flex;justify-content:space-between;gap:16px;margin-bottom:20px;color:#66665f;font-size:13px;font-weight:600}}.message{{margin:0 0 22px}}.label{{margin-bottom:7px;color:#66665f;font-size:12px;font-weight:700;text-transform:uppercase}}.content,pre{{margin:0;white-space:pre-wrap;overflow-wrap:anywhere;font:inherit;line-height:1.65}}.message.user .content{{font-weight:500}}.event{{margin:12px 0;border:1px solid #d6d6ce;border-radius:6px;background:#ecece6}}.event summary{{display:flex;justify-content:space-between;gap:16px;padding:10px 12px;cursor:pointer;font-size:13px}}.event pre{{padding:0 12px 12px;color:#52524c;font-family:ui-monospace,SFMono-Regular,Consolas,monospace;font-size:12px}}.status{{color:#77776f;font-weight:500}}
@media(prefers-color-scheme:dark){{:root,body{{background:#171713;color:#efefe8}}.document-header,.turn{{border-color:#37372f}}.meta,.turn>header,.label,.status{{color:#a4a49a}}.event{{border-color:#3b3b33;background:#22221d}}.event pre{{color:#c9c9bf}}}}@media(max-width:560px){{main{{width:min(100% - 24px,860px);padding-top:32px}}h1{{font-size:24px}}}}@media print{{:root,body{{background:white;color:#20201d}}main{{width:100%;padding:0}}.turn{{break-inside:avoid}}}}
</style></head><body><main><header class="document-header"><h1>{title}</h1><p class="meta">{workspace} · {} turns exported</p></header>{body}</main></body></html>
"#,
        selected.turns.len()
    ))
}

fn turn_meta(turn: &ThreadTurnDto, options: &TranscriptExportOptions) -> String {
    if !options.include_token_and_price {
        return turn.status.to_string();
    }
    let total = turn.token_usage.as_ref().and_then(|usage| {
        usage
            .get("totalTokens")
            .or_else(|| usage.get("total_tokens"))
            .and_then(serde_json::Value::as_u64)
    });
    match total {
        Some(total) => format!("{} · {total} tokens", turn.status),
        None => turn.status.to_string(),
    }
}

fn visible_items<'a>(
    turn: &'a ThreadTurnDto,
    options: &TranscriptExportOptions,
) -> Vec<&'a ThreadHistoryItemDto> {
    turn.items
        .iter()
        .filter(|item| {
            options.profile == "technical"
                || matches!(item.kind.as_str(), "userMessage" | "agentMessage" | "plan")
        })
        .collect()
}

fn export_item_text(
    item: &ThreadHistoryItemDto,
    workspace: &WorkspaceDto,
    options: &TranscriptExportOptions,
) -> String {
    let source = item.preview_text.as_deref().unwrap_or(&item.text);
    let source = if item.kind == "commandExecution" && !options.include_command_output {
        source.lines().next().unwrap_or(source)
    } else {
        source
    };
    let mut text = truncate_chars(source, MAX_ITEM_CHARS);
    if !options.include_absolute_paths && !workspace.abs_path.is_empty() {
        text = text.replace(&workspace.abs_path, "{workspace}");
    }
    text
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

fn item_label(kind: &str) -> &'static str {
    match kind {
        "userMessage" => "User",
        "agentMessage" => "Agent",
        "reasoning" => "Reasoning",
        "plan" => "Plan",
        "commandExecution" => "Command",
        "fileChange" => "File changes",
        "fileRead" => "File read",
        "webSearch" => "Web search",
        "contextCompaction" => "Context",
        "image" => "Image",
        "skillToolCall" => "Skill",
        "agentToolCall" => "Agent tool",
        "toolCall" => "Tool",
        _ => "Event",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
