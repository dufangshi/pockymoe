//! Reader-oriented replies use the same public chronology as the Text Context API.
use super::*;

const REDACTED: &str = "[credential omitted]";

#[derive(Debug)]
pub(super) struct ContextBudgetExceeded;
impl std::fmt::Display for ContextBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("reply exceeds context character budget after credential redaction")
    }
}
impl std::error::Error for ContextBudgetExceeded {}

pub(super) fn request(
    s: &Supervisor,
    thread: &str,
    turn: &str,
    item: &str,
    status: &str,
    settings: &Settings,
    credential: &str,
) -> Result<Value> {
    let (mut summary, target) = s.thread_summary_for_reply(
        thread,
        turn,
        item,
        settings.reply_context_turns,
        settings.reply_context_max_chars,
    )?;
    let mut secrets = vec![credential.to_string()];
    let directory = crate::upstreams::directory(&s.config.database_url);
    // A historical user message may contain a different connection's credential.
    // Read only local connection keys; never put configuration files in context.
    for metadata in crate::upstreams::connections::metadata(&directory)?
        .as_array()
        .context("invalid saved connection metadata")?
    {
        let id = metadata["id"].as_str().context("missing connection id")?;
        let connection = crate::upstreams::connections::get(&directory, id)?;
        secrets.push(connection.api_key);
        for (name, value) in connection.headers {
            let name = name.to_ascii_lowercase();
            if ["authorization", "key", "token", "secret"]
                .iter()
                .any(|s| name.contains(s))
            {
                secrets.push(value.clone());
                if let Some(token) = value.strip_prefix("Bearer ") {
                    secrets.push(token.into());
                }
            }
        }
    }
    // Longest first avoids leaving a suffix when credentials share a prefix.
    secrets.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    secrets.dedup();
    for message in &mut summary.messages {
        message.text = redact(&message.text, &secrets);
    }
    // A short saved credential can expand into a longer redaction marker.
    // Reapply the budget to the actual outgoing text, preserving target identity.
    let (messages, target) =
        fit_redacted(summary.messages, target, settings.reply_context_max_chars)?;
    Ok(json!({
        "preferences": {
            "language": settings.language,
            "preset": settings.preset,
            "detail": settings.detail,
            "readerBackground": redact(&settings.reader_background, &secrets),
        },
        "turnStatus": status,
        "messages": messages,
        "target": target,
    }))
}

fn fit_redacted(
    messages: Vec<pockymoe_protocol::ThreadSummaryMessage>,
    target: usize,
    max_chars: u64,
) -> Result<(Vec<pockymoe_protocol::ThreadSummaryMessage>, usize)> {
    let target_chars = messages[target].text.chars().count() as u64;
    if target_chars > max_chars {
        return Err(ContextBudgetExceeded.into());
    }
    let mut remaining = max_chars - target_chars;
    let mut retained = Vec::with_capacity(messages.len());
    for (index, mut message) in messages.into_iter().enumerate().rev() {
        if index != target {
            let chars = message.text.chars().count() as u64;
            let take = chars.min(remaining);
            if take == 0 {
                continue;
            }
            remaining -= take;
            if take < chars {
                message.text = message.text.chars().skip((chars - take) as usize).collect();
                message.truncated = true;
            }
        }
        retained.push((index, message));
    }
    retained.reverse();
    let target = retained
        .iter()
        .position(|(index, _)| *index == target)
        .context("reply target missing after redaction")?;
    Ok((
        retained.into_iter().map(|(_, message)| message).collect(),
        target,
    ))
}

fn redact(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_string();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        text = text.replace(secret, REDACTED);
    }
    // Also cover recognizable old keys no longer present in the local registry.
    // This is a narrow credential filter, not an attempt to anonymize prose.
    let mut result = String::with_capacity(text.len());
    let mut token = String::new();
    let flush = |token: &mut String, result: &mut String| {
        let secret = token.len() >= 24
            && ["sk-", "AIza", "ghp_", "github_pat_", "xoxb-", "xoxp-"]
                .iter()
                .any(|prefix| token.starts_with(prefix));
        result.push_str(if secret { REDACTED } else { token.as_str() });
        token.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
            token.push(c);
        } else {
            flush(&mut token, &mut result);
            result.push(c);
        }
    }
    flush(&mut token, &mut result);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_known_and_recognizable_historical_keys_without_changing_normal_prose() {
        let known = "private-fixture-credential";
        let old = "sk-oldFixtureCredential0123456789";
        let text = format!("中文 API: {known}; old={old}; `cargo check` https://example.test");
        let result = redact(&text, &[known.into()]);
        assert!(!result.contains(known));
        assert!(!result.contains(old));
        assert_eq!(result.matches(REDACTED).count(), 2);
        assert!(result.contains("中文 API"));
        assert!(result.contains("`cargo check` https://example.test"));
    }

    #[test]
    fn redaction_growth_keeps_the_exact_target_and_stays_within_the_budget() {
        use pockymoe_protocol::{ThreadSummaryMessage, ThreadSummaryRole};
        let message = |text: &str| ThreadSummaryMessage {
            time: None,
            role: ThreadSummaryRole::Assistant,
            text: text.into(),
            truncated: false,
        };
        // Redacting an unusually short credential expands a context message.
        // The duplicate newer text must not replace the middle target.
        let messages = vec![
            message(&redact("K🙂", &["K".into()])),
            message("目标🙂"),
            message("目标🙂"),
        ];
        let (messages, target) = fit_redacted(messages, 1, 7).unwrap();
        assert_eq!(target, 1);
        assert_eq!(messages[target].text, "目标🙂");
        assert!(!messages[target].truncated);
        assert_eq!(messages[0].text, "🙂");
        assert!(messages[0].truncated);
        assert_eq!(
            messages
                .iter()
                .map(|m| m.text.chars().count())
                .sum::<usize>(),
            7
        );
        let error = fit_redacted(vec![message(REDACTED)], 0, 1).unwrap_err();
        assert!(error.is::<ContextBudgetExceeded>());
    }
}
