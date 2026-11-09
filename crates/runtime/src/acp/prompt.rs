use std::path::Path;

use anyhow::{bail, Result};
use base64::Engine;
use serde_json::{json, Value};

use crate::actor::PromptImage;
use crate::files::assert_within;

pub fn build_prompt_blocks(
    prompt: &str,
    cwd: &Path,
    image_capable: bool,
    extra_images: &[PromptImage],
) -> Result<Vec<Value>> {
    let mut blocks = expand_attachment_tokens(prompt, cwd, image_capable)?;
    if !image_capable && !extra_images.is_empty() {
        // These have no file to link to instead.
        bail!("This agent does not accept images in a prompt.");
    }
    for image in extra_images {
        blocks.push(json!({
            "type": "image",
            "mimeType": image.mime_type,
            "data": image.data
        }));
    }
    if blocks.is_empty() && !prompt.is_empty() {
        blocks.push(json!({ "type": "text", "text": prompt }));
    }
    Ok(blocks)
}

fn expand_attachment_tokens(prompt: &str, cwd: &Path, image_capable: bool) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    // `cursor` marks text not yet emitted; `search` only advances the scan. Moving
    // `cursor` past an ordinary bracket used to drop everything before it.
    let mut cursor = 0;
    let mut search = 0;
    while search < prompt.len() {
        let rest = &prompt[search..];
        let Some(start_rel) = rest.find('[') else {
            break;
        };
        let start = search + start_rel;
        let after = &prompt[start + 1..];
        let (kind, rest_after_kind) = if after.starts_with("PHOTO ") {
            ("PHOTO", &after[6..])
        } else if after.starts_with("FILE ") {
            ("FILE", &after[5..])
        } else {
            search = start + 1;
            continue;
        };
        let Some(end_rel) = rest_after_kind.find(']') else {
            break;
        };
        let requested = rest_after_kind[..end_rel].trim();
        let end = start + 1 + (after.len() - rest_after_kind.len()) + end_rel + 1;
        let preceding = &prompt[cursor..start];
        if !preceding.is_empty() {
            blocks.push(json!({ "type": "text", "text": preceding }));
        }
        let abs = assert_within(cwd, Path::new(requested))?;
        let uri = url::Url::from_file_path(&abs)
            .map_err(|_| anyhow::anyhow!("cannot convert attachment path to a file URL"))?
            .to_string();
        if kind == "PHOTO" && image_capable {
            let file_bytes = std::fs::read(&abs)?;
            if file_bytes.len() > 20 * 1024 * 1024 {
                bail!("ACP image attachment is missing or exceeds 20 MiB.");
            }
            let data = base64::engine::general_purpose::STANDARD.encode(file_bytes);
            blocks.push(json!({
                "type": "image",
                "mimeType": mime_for(&abs),
                "data": data,
                "uri": uri
            }));
        } else {
            let name = abs
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| requested.to_string());
            // An agent that does not take images gets the picture as a file it can open.
            let mut link = json!({
                "type": "resource_link",
                "name": name,
                "uri": uri
            });
            if kind == "PHOTO" {
                link["mimeType"] = json!(mime_for(&abs));
            }
            blocks.push(link);
        }
        cursor = end;
        search = end;
    }
    if cursor == 0 {
        if !prompt.is_empty() {
            blocks.push(json!({ "type": "text", "text": prompt }));
        }
        return Ok(blocks);
    }
    let trailing = &prompt[cursor..];
    if !trailing.is_empty() {
        blocks.push(json!({ "type": "text", "text": trailing }));
    }
    Ok(blocks)
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(blocks: &[Value]) -> String {
        blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<String>()
    }

    #[test]
    fn ordinary_brackets_keep_all_text() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "[remoteCodex: pointer]\n\n[Pockymoe role: reviewer]\nbody\n\n[Pockymoe task from thread X | s]\nfix arr[0] and see [docs](http://x)";
        let blocks = build_prompt_blocks(prompt, dir.path(), true, &[]).unwrap();
        assert_eq!(texts(&blocks), prompt);
    }

    #[test]
    fn attachments_split_text_without_losing_brackets_around_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let prompt = "[ctx] read [FILE notes.txt] then a[1]";
        let blocks = build_prompt_blocks(prompt, dir.path(), true, &[]).unwrap();
        assert_eq!(blocks.len(), 3, "{blocks:?}");
        assert_eq!(blocks[0]["text"], "[ctx] read ");
        assert_eq!(blocks[1]["type"], "resource_link");
        assert_eq!(blocks[2]["text"], " then a[1]");
    }

    #[test]
    fn a_photo_is_a_file_link_for_an_agent_without_images() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("red.png"), b"png").unwrap();
        let blocks = build_prompt_blocks("see [PHOTO red.png]", dir.path(), false, &[]).unwrap();
        assert_eq!(blocks[1]["type"], "resource_link");
        assert_eq!(blocks[1]["name"], "red.png");
        assert_eq!(blocks[1]["mimeType"], "image/png");
        let inline = [PromptImage {
            mime_type: "image/png".into(),
            data: "AA==".into(),
        }];
        let err = build_prompt_blocks("x", dir.path(), false, &inline).unwrap_err();
        assert!(err.to_string().contains("does not accept images"));
        let blocks = build_prompt_blocks("[PHOTO red.png]", dir.path(), true, &inline).unwrap();
        assert_eq!(blocks.iter().filter(|b| b["type"] == "image").count(), 2);
    }
}
