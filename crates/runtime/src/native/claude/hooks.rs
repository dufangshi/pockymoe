//! SDK callback hooks the native runtime registers in `initialize`, as the
//! Agent SDK does for claude-agent-acp. They observe; they never block, change
//! or approve a tool. Claude's own configured hooks keep running unchanged.
use serde_json::{json, Value};
use std::path::Path;

/// PreToolUse for Write: read the file's current contents so the transcript
/// diff is the real overwrite (claude-agent-acp's file-change preview hook).
pub(super) const WRITE_PREVIEW: &str = "pockymoe-write-preview";
const PREVIEW_LIMIT: u64 = 1024 * 1024;

pub(super) fn registration() -> Value {
    json!({"PreToolUse":[{"matcher":"Write","hookCallbackIds":[WRITE_PREVIEW]}]})
}

/// What `path` holds before the Write runs: `""` for a file that does not exist
/// yet, `None` when it cannot be read faithfully (too large, binary, outside the
/// environment's workspace), in which case no diff is shown.
pub(super) fn previous_text(cwd: &str, path: &str) -> Option<String> {
    if crate::execution::current().is_some() {
        // Inside an environment the file lives in the container's view; read it
        // through the worker, confined to the working directory.
        let response =
            crate::execution::file_request("readText", Path::new(cwd), path, json!({})).ok()??;
        let text = response["content"].as_str()?;
        return (text.len() as u64 <= PREVIEW_LIMIT).then(|| text.to_owned());
    }
    let target = Path::new(cwd).join(path);
    match std::fs::metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(String::new()),
        Err(_) => None,
        Ok(meta) if !meta.is_file() || meta.len() > PREVIEW_LIMIT => None,
        Ok(_) => String::from_utf8(std::fs::read(&target).ok()?).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_read_existing_new_and_refuse_unfaithful_files() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap();
        std::fs::write(dir.path().join("old.txt"), "before\n").unwrap();
        assert_eq!(previous_text(cwd, "old.txt").as_deref(), Some("before\n"));
        let absolute = dir.path().join("old.txt");
        assert_eq!(
            previous_text(cwd, absolute.to_str().unwrap()).as_deref(),
            Some("before\n")
        );
        assert_eq!(previous_text(cwd, "new.txt").as_deref(), Some(""));
        std::fs::write(dir.path().join("binary"), [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(previous_text(cwd, "binary"), None);
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        assert_eq!(previous_text(cwd, "folder"), None);
        assert_eq!(
            registration()["PreToolUse"][0]["hookCallbackIds"][0],
            WRITE_PREVIEW
        );
    }
}
