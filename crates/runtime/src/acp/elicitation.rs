//! Translate Codex native questions and ACP form elicitations without losing
//! question ids, options, free-text answers, or multi-question submissions.
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

fn choice_schema(field: &Value) -> &Value {
    if field["type"] == "array" {
        &field["items"]
    } else {
        field
    }
}

fn choices(field: &Value) -> Option<&Vec<Value>> {
    let schema = choice_schema(field);
    schema["oneOf"]
        .as_array()
        .or_else(|| schema["anyOf"].as_array())
}

/// Claude's AskUserQuestion bridge uses question_<n>_custom without metadata.
/// Recognize only its companion shape; unrelated free-text fields stay intact.
fn custom_question_id<'a>(
    id: &'a str,
    field: &'a Value,
    props: &Map<String, Value>,
) -> Option<&'a str> {
    if field.pointer("/_meta/codex/isOtherAnswer") == Some(&Value::Bool(true)) {
        return field
            .pointer("/_meta/codex/questionId")
            .and_then(Value::as_str);
    }
    let question_id = id.strip_suffix("_custom")?;
    let index = question_id.strip_prefix("question_")?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) || field["type"] != "string" {
        return None;
    }
    let question = props.get(question_id)?;
    (choices(question).is_some() && choices(field).is_none() && !field["enum"].is_array())
        .then_some(question_id)
}

fn custom_field<'a>(params: &'a Value, question_id: &str) -> Option<(&'a String, &'a Value)> {
    let props = params.pointer("/requestedSchema/properties")?.as_object()?;
    props
        .iter()
        .find(|(id, field)| custom_question_id(id, field, props) == Some(question_id))
}

pub(crate) fn questions(params: &Value, native: bool) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    if native {
        for q in params["questions"].as_array().into_iter().flatten() {
            out.push(serde_json::from_value(json!({
                "id":q["id"], "header":q.get("header").and_then(Value::as_str).unwrap_or("Question"),
                "question":q["question"],"isOther":q.get("isOther").and_then(Value::as_bool).unwrap_or(true),
                "isSecret":q.get("isSecret").and_then(Value::as_bool).unwrap_or(false),
                "multiSelect":q.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
                "options":q.get("options").cloned().unwrap_or(Value::Null)
            }))?);
        }
    } else {
        if params["mode"].as_str().unwrap_or("form") != "form" {
            bail!("Only form elicitation is supported");
        }
        let properties = params
            .pointer("/requestedSchema/properties")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("Elicitation form has no properties"))?;
        for (id, field) in properties {
            if custom_question_id(id, field, properties).is_some() {
                continue;
            }
            let schema = choice_schema(field);
            let options: Vec<Value> = if let Some(choices) = choices(field) {
                choices.iter().map(|choice| json!({"label":choice["const"],"description":choice.get("description").and_then(Value::as_str).unwrap_or("")})).collect()
            } else {
                schema["enum"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|v| json!({"label":v,"description":""}))
                    .collect()
            };
            out.push(serde_json::from_value(json!({
                "id":id,"header":field.get("title").and_then(Value::as_str).unwrap_or("Question"),
                "question":field.get("description").or_else(||params.get("message")).or_else(||field.get("title")).and_then(Value::as_str).unwrap_or(id),
                "isOther":custom_field(params, id).is_some() || field.pointer("/_meta/codex/isOther").and_then(Value::as_bool).unwrap_or(options.is_empty()),
                "isSecret":field.pointer("/_meta/codex/isSecret").and_then(Value::as_bool).unwrap_or(false),
                "multiSelect":field["type"] == "array", "options":if options.is_empty() {Value::Null} else {json!(options)}
            }))?);
        }
    }
    if out.is_empty() {
        bail!("User input request contains no questions");
    }
    Ok(out)
}

pub(crate) fn response(
    params: &Value,
    native: bool,
    allow: bool,
    answers: Option<&str>,
) -> Result<Value> {
    if !allow {
        return Ok(if native {
            json!({"answers":{}})
        } else {
            json!({"action":"cancel"})
        });
    }
    let answers: Value = serde_json::from_str(answers.unwrap_or("{}"))?;
    let mut content = Map::new();
    let mut normalized = Map::new();
    for q in questions(params, native)? {
        let id = q["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing question id"))?;
        let mut values: Vec<String> = answers[id]["answers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string)
            .collect();
        let companion = (!native).then(|| custom_field(params, id)).flatten();
        if let Some((other_id, _)) = companion {
            values.extend(
                answers[other_id]["answers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|v| !v.trim().is_empty())
                    .map(str::to_string),
            );
        }
        if values.is_empty() {
            bail!("Answer required for {}", q["header"]);
        }
        normalized.insert(id.to_string(), json!({"answers":values}));
        let value = if q["multiSelect"].as_bool().unwrap_or(false) {
            json!(values)
        } else {
            json!(values[0])
        };
        let custom = q["options"].as_array().is_some_and(|opts| {
            values
                .iter()
                .any(|v| !opts.iter().any(|o| o["label"] == *v))
        });
        if custom && !q["isOther"].as_bool().unwrap_or(false) {
            bail!("Invalid option for {}", q["header"]);
        }
        if let Some((other_id, field)) = companion {
            let (picks, text): (Vec<_>, Vec<_>) = values.iter().partition(|v| {
                q["options"]
                    .as_array()
                    .is_some_and(|opts| opts.iter().any(|o| o["label"] == **v))
            });
            if !picks.is_empty() {
                content.insert(
                    id.to_string(),
                    if q["multiSelect"] == true {
                        json!(picks)
                    } else {
                        json!(picks[0])
                    },
                );
            }
            if !text.is_empty() {
                content.insert(
                    other_id.clone(),
                    if field["type"] == "array" {
                        json!(text)
                    } else {
                        json!(text
                            .into_iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                            .join(", "))
                    },
                );
            }
            continue;
        }
        content.insert(id.to_string(), value);
    }
    Ok(if native {
        json!({"answers":normalized})
    } else {
        json!({"action":"accept","content":content})
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Matches the unmarked companion fields emitted by claude-agent-acp's
    // askUserQuestionsToCreateRequest (including anyOf for multi-select).
    fn claude_form(multi: bool) -> Value {
        let choices = json!([
            {"const":"Raise limit","title":"Raise limit","description":"Use 55k"},
            {"const":"Keep limit","title":"Keep limit","description":"Use 50k"}
        ]);
        let selection = if multi {
            json!({"type":"array","title":"Limit","items":{"anyOf":choices}})
        } else {
            json!({"type":"string","title":"Limit","oneOf":choices})
        };
        json!({
            "mode":"form", "sessionId":"claude-session", "toolCallId":"ask-1",
            "message":"What should the limit be?",
            "requestedSchema":{"type":"object","properties":{
                "question_0":selection,
                "question_0_custom":{"type":"string","title":"Other",
                    "description":"Type your own answer, or add a note to the option you chose above (optional)."}
            }}
        })
    }

    fn answer(params: &Value, values: Value) -> Value {
        response(params, false, true, Some(&values.to_string())).unwrap()
    }

    #[test]
    fn claude_elicitation_groups_optional_custom_field_with_choice() {
        let form = claude_form(false);
        let qs = questions(&form, false).unwrap();
        assert_eq!(
            qs.len(),
            1,
            "optional text is not a second required question"
        );
        assert_eq!(qs[0]["id"], "question_0");
        assert_eq!(qs[0]["isOther"], true);
        assert_eq!(
            answer(&form, json!({"question_0":{"answers":["Raise limit"]}})),
            json!({"action":"accept","content":{"question_0":"Raise limit"}})
        );
    }

    #[test]
    fn claude_elicitation_accepts_custom_text_without_selecting_an_option() {
        assert_eq!(
            answer(
                &claude_form(false),
                json!({"question_0":{"answers":["No limit"]}})
            ),
            json!({"action":"accept","content":{"question_0_custom":"No limit"}})
        );
    }

    #[test]
    fn claude_elicitation_multiselect_retains_choices_and_custom_text() {
        let form = claude_form(true);
        let qs = questions(&form, false).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0]["multiSelect"], true);
        assert_eq!(qs[0]["options"].as_array().unwrap().len(), 2);
        assert_eq!(
            answer(
                &form,
                json!({"question_0":{"answers":["Raise limit","Keep limit","Only for tests"]}})
            ),
            json!({"action":"accept","content":{"question_0":["Raise limit","Keep limit"],"question_0_custom":"Only for tests"}})
        );
    }

    #[test]
    fn claude_elicitation_rejects_empty_answer_and_preserves_companion_notes() {
        let form = claude_form(false);
        assert!(response(&form, false, true, Some("{}")).is_err());
        assert!(response(
            &form,
            false,
            true,
            Some(r#"{"question_0":{"answers":["  "]}}"#)
        )
        .is_err());
        assert_eq!(
            answer(
                &form,
                json!({
                    "question_0":{"answers":["Keep limit"]},
                    "question_0_custom":{"answers":["Except fixtures"]}
                })
            ),
            json!({"action":"accept","content":{"question_0":"Keep limit","question_0_custom":"Except fixtures"}})
        );
    }

    #[test]
    fn elicitation_preserves_native_and_metadata_companion_answers() {
        let native = json!({"questions":[{"id":"q","header":"Q","question":"Choose",
            "isOther":true,"options":[{"label":"Yes","description":""}]}]});
        let input = json!({"q":{"answers":["Yes"]}}).to_string();
        assert_eq!(
            response(&native, true, true, Some(&input)).unwrap(),
            json!({"answers":{"q":{"answers":["Yes"]}}})
        );
        assert_eq!(
            response(&native, true, false, None).unwrap(),
            json!({"answers":{}})
        );

        let form = json!({"requestedSchema":{"properties":{
            "q":{"type":"string","enum":["Yes","No"],"_meta":{"codex":{"isOther":true}}},
            "notes":{"type":"string","_meta":{"codex":{"isOtherAnswer":true,"questionId":"q"}}}
        }}});
        assert_eq!(questions(&form, false).unwrap().len(), 1);
        assert_eq!(
            answer(&form, json!({"q":{"answers":["Maybe"]}})),
            json!({"action":"accept","content":{"notes":"Maybe"}})
        );
        assert_eq!(
            response(&form, false, false, None).unwrap(),
            json!({"action":"cancel"})
        );
    }

    #[test]
    fn claude_elicitation_keeps_multiple_questions_and_unrelated_text_separate() {
        let mut form = claude_form(false);
        let props = form["requestedSchema"]["properties"]
            .as_object_mut()
            .unwrap();
        props.insert(
            "question_1".into(),
            json!({"type":"string","oneOf":[{"const":"Yes"}]}),
        );
        props.insert(
            "question_1_custom".into(),
            json!({"type":"string","title":"Other"}),
        );
        assert_eq!(questions(&form, false).unwrap().len(), 2);
        assert_eq!(
            answer(
                &form,
                json!({"question_0":{"answers":["Keep limit"]},
            "question_1":{"answers":["Custom reply"]}})
            ),
            json!({"action":"accept","content":{"question_0":"Keep limit","question_1_custom":"Custom reply"}})
        );
        form["requestedSchema"]["properties"]["comments_custom"] =
            json!({"type":"string","title":"Notes"});
        assert_eq!(questions(&form, false).unwrap().len(), 3);
    }
}
