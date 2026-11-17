//! Periodic disk cleanup of native releases, update staging directories,
//! the retired npm launcher's binary cache and adapter installations
//! (docs/github-runtime.md, "Disk cleanup").
use super::{releases, service, *};
use pockymoe_runtime::{
    disk_cleanup::{self, LiveProcesses},
    Supervisor,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    sync::Arc,
    time::{Duration, SystemTime},
};

/// After startup, once a just-updated Supervisor has been verified.
const FIRST_RUN: Duration = Duration::from_secs(5 * 60);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Interrupted release stages and finished update staging directories.
const STAGE_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// Binaries of the retired npm launcher.
const LEGACY_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Run disk cleanup shortly after startup and then daily. Only a Supervisor
/// started from an installed native release cleans: a source or npm build
/// does not own `native/`.
pub fn spawn(state: Arc<Supervisor>) {
    if !pockymoe_runtime::config::installed_release() {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_RUN).await;
        loop {
            // An update in progress postpones the run, not the whole day.
            let wait = if run(&state).await {
                INTERVAL
            } else {
                FIRST_RUN
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// False when an update was in progress and nothing ran.
async fn run(state: &Supervisor) -> bool {
    let Some(job) = super::update::idle_job(state) else {
        tracing::debug!("native update in progress; disk cleanup postponed");
        return false;
    };
    let updates = super::update::updates_dir(state);
    let finished = tokio::task::spawn_blocking(move || {
        let live = LiveProcesses::collect();
        let now = SystemTime::now();
        clean_releases(
            &install_root(),
            &job,
            live.as_ref(),
            &service::definition_files(),
        );
        if let Some(updates) = updates {
            clean_staging(&updates, &job, now);
        }
        if let Some(live) = &live {
            clean_legacy_binaries(&home().join(".remote-codex/bin"), live, now);
        }
    })
    .await;
    if let Err(error) = finished {
        tracing::warn!(%error, "disk cleanup task failed");
    }
    pockymoe_runtime::acp::cleanup_adapter_installations().await;
    true
}

fn version(name: &str) -> Option<(u32, u32, u32)> {
    let normalized = releases::stable_version(name).ok()?;
    if normalized != name {
        return None;
    }
    let mut parts = name.split('.').map(|part| part.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Release directory names to remove. Keeps `current`, the newest version
/// below it (the rollback target), every newer version (an update may be
/// installing it), everything in `keep`, and names that are not versions;
/// interrupted `stage-*` directories go once `stale` says they are old.
fn select_releases(
    entries: &[(String, bool)],
    current: &str,
    keep: &BTreeSet<String>,
) -> Vec<String> {
    let Some(current) = version(current) else {
        return Vec::new();
    };
    let previous = entries
        .iter()
        .filter_map(|(name, _)| version(name))
        .filter(|candidate| *candidate < current)
        .max();
    entries
        .iter()
        .filter(|(name, stale)| match version(name) {
            Some(candidate) => {
                candidate < current && Some(candidate) != previous && !keep.contains(name)
            }
            None => name.starts_with("stage-") && *stale,
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// The release directory under `releases` that `path` is inside, if any.
fn release_of(releases: &Path, path: &Path) -> Option<String> {
    let canonical = std::fs::canonicalize(releases).ok();
    let resolved = std::fs::canonicalize(path).ok();
    [
        (Some(releases.to_path_buf()), Some(path.to_path_buf())),
        (canonical, resolved),
    ]
    .into_iter()
    .find_map(|(root, path)| {
        let relative = path?.strip_prefix(root?).ok()?.to_path_buf();
        match relative.components().next()? {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        }
    })
}

fn clean_releases(root: &Path, job: &Value, live: Option<&LiveProcesses>, units: &[PathBuf]) {
    let releases = root.join("releases");
    disk_cleanup::sweep_trash(&releases);
    let Some(current) = read::<releases::Installed>(&root.join("current.json"))
        .ok()
        .map(|installed| installed.version)
    else {
        return;
    };
    let mut keep = BTreeSet::from([current.clone()]);
    // `current` follows current.json, but keep its target if they disagree.
    if let Some(target) = std::fs::read_link(root.join("current"))
        .ok()
        .and_then(|target| release_of(&releases, &target))
    {
        keep.insert(target);
    }
    if let Some(running) = std::env::current_exe()
        .ok()
        .and_then(|exe| release_of(&releases, &exe))
    {
        keep.insert(running);
    }
    for key in ["targetVersion", "runningVersion"] {
        if let Some(version) = job[key].as_str() {
            keep.insert(version.to_string());
        }
    }
    let unit_texts: Vec<String> = units
        .iter()
        .filter_map(|file| std::fs::read_to_string(file).ok())
        .collect();
    let now = SystemTime::now();
    let entries: Vec<(String, bool)> = disk_cleanup::child_dirs(&releases)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let named = unit_texts.iter().any(|text| {
                text.contains(&format!("releases/{name}/"))
                    || text.contains(&format!("releases\\{name}\\"))
            });
            if named || live.is_some_and(|live| live.uses(&path)) {
                keep.insert(name.clone());
            }
            Some((name, disk_cleanup::older_than(&path, STAGE_AGE, now)))
        })
        .collect();
    let remove: Vec<PathBuf> = select_releases(&entries, &current, &keep)
        .into_iter()
        .map(|name| releases.join(name))
        .collect();
    disk_cleanup::remove_all(&releases, &remove, "native release");
}

/// `supervisor-<uuid>` directories of finished updates, except the one the
/// last job's log is in.
fn select_staging(entries: &[(String, bool)], in_use: Option<&str>) -> Vec<String> {
    entries
        .iter()
        .filter(|(name, stale)| {
            *stale
                && Some(name.as_str()) != in_use
                && name
                    .strip_prefix("supervisor-")
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn clean_staging(updates: &Path, job: &Value, now: SystemTime) {
    disk_cleanup::sweep_trash(updates);
    let in_use = job["logPath"]
        .as_str()
        .map(Path::new)
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned());
    let entries: Vec<(String, bool)> = disk_cleanup::child_dirs(updates)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some((name, disk_cleanup::older_than(&path, STAGE_AGE, now)))
        })
        .collect();
    let remove: Vec<PathBuf> = select_staging(&entries, in_use.as_deref())
        .into_iter()
        .map(|name| updates.join(name))
        .collect();
    disk_cleanup::remove_all(updates, &remove, "update staging directory");
}

/// Version directories of the retired npm launcher's binary cache
/// (`~/.remote-codex/bin/<version>/<platform>/`), except the newest. Only that
/// launcher reads them, and it downloads a missing binary again.
fn select_legacy(entries: &[(String, bool)], in_use: impl Fn(&str) -> bool) -> Vec<String> {
    let newest = entries.iter().filter_map(|(name, _)| version(name)).max();
    entries
        .iter()
        .filter(|(name, stale)| {
            version(name).is_some_and(|candidate| Some(candidate) != newest)
                && *stale
                && !in_use(name)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn clean_legacy_binaries(bin: &Path, live: &LiveProcesses, now: SystemTime) {
    disk_cleanup::sweep_trash(bin);
    let entries: Vec<(String, bool)> = disk_cleanup::child_dirs(bin)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            Some((name, disk_cleanup::older_than(&path, LEGACY_AGE, now)))
        })
        .collect();
    let remove: Vec<PathBuf> = select_legacy(&entries, |name| live.uses(&bin.join(name)))
        .into_iter()
        .map(|name| bin.join(name))
        .collect();
    disk_cleanup::remove_all(bin, &remove, "npm launcher binary");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(list: &[&str], stale: bool) -> Vec<(String, bool)> {
        list.iter().map(|name| (name.to_string(), stale)).collect()
    }
    fn sorted(mut list: Vec<String>) -> Vec<String> {
        list.sort();
        list
    }

    #[test]
    fn releases_keep_current_previous_newer_pinned_and_unknown_names() {
        let mut entries = names(
            &[
                "0.13.9", "0.13.10", "0.13.40", "0.13.60", "0.13.61", "0.13.62", "0.14.0", "notes",
            ],
            true,
        );
        entries.push(("stage-old".into(), true));
        entries.push(("stage-new".into(), false));
        let keep = BTreeSet::from(["0.13.40".to_string()]);
        assert_eq!(
            sorted(select_releases(&entries, "0.13.62", &keep)),
            ["0.13.10", "0.13.60", "0.13.9", "stage-old"]
        );
        // Without a readable current version nothing is removed.
        assert!(select_releases(&entries, "", &keep).is_empty());
        assert!(select_releases(&entries, "garbage", &keep).is_empty());
        // Numeric, not lexical, order: 0.13.10 is newer than 0.13.9.
        assert_eq!(
            select_releases(
                &names(&["0.13.9", "0.13.10", "0.13.11"], true),
                "0.13.11",
                &BTreeSet::new()
            ),
            ["0.13.9"]
        );
    }

    #[test]
    fn release_cleanup_on_disk_keeps_units_job_and_live_processes() {
        let root = tempfile::tempdir().unwrap();
        let releases = root.path().join("releases");
        for version in [
            "0.13.1", "0.13.2", "0.13.3", "0.13.4", "0.13.5", "0.13.6", "0.13.7",
        ] {
            std::fs::create_dir_all(releases.join(version).join("web")).unwrap();
        }
        let outside = root.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, releases.join("0.13.0")).unwrap();
        write(
            &root.path().join("current.json"),
            &releases::Installed {
                version: "0.13.7".into(),
                executable: releases.join("0.13.7").join(binary_name()),
                web_dist: releases.join("0.13.7/web"),
            },
        )
        .unwrap();
        let unit = root.path().join("unit.service");
        std::fs::write(
            &unit,
            format!(
                "ExecStart=\"{}\" device-run\n",
                releases.join("0.13.3").join(binary_name()).display()
            ),
        )
        .unwrap();
        let live = LiveProcesses::from_texts([releases
            .join("0.13.2")
            .join(binary_name())
            .to_string_lossy()
            .into_owned()]);
        clean_releases(
            root.path(),
            &json!({"phase":"completed","targetVersion":"0.13.4"}),
            Some(&live),
            &[unit],
        );
        let left: Vec<String> = sorted(
            std::fs::read_dir(&releases)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect(),
        );
        let mut expected = vec!["0.13.2", "0.13.3", "0.13.4", "0.13.6", "0.13.7"];
        if cfg!(unix) {
            expected.insert(0, "0.13.0");
        }
        assert_eq!(left, expected);
        assert!(outside.is_dir());
    }

    #[test]
    fn staging_keeps_the_last_job_young_and_unrelated_entries() {
        let old = format!("supervisor-{}", uuid::Uuid::new_v4());
        let last = format!("supervisor-{}", uuid::Uuid::new_v4());
        let young = format!("supervisor-{}", uuid::Uuid::new_v4());
        let mut entries = names(&[&old, &last, "supervisor-update.lock", "other"], true);
        entries.push((young, false));
        assert_eq!(select_staging(&entries, Some(&last)), [old]);
    }

    #[test]
    fn staging_cleanup_on_disk_removes_only_old_finished_directories() {
        let updates = tempfile::tempdir().unwrap();
        let old = format!("supervisor-{}", uuid::Uuid::new_v4());
        let last = format!("supervisor-{}", uuid::Uuid::new_v4());
        for name in [&old, &last] {
            std::fs::create_dir_all(updates.path().join(name)).unwrap();
            std::fs::write(updates.path().join(name).join(binary_name()), "x").unwrap();
        }
        std::fs::create_dir(updates.path().join("supervisor-update.lock")).unwrap();
        let job =
            json!({"phase":"completed","logPath":updates.path().join(&last).join("update.log")});
        clean_staging(updates.path(), &job, SystemTime::now());
        assert!(updates.path().join(&old).is_dir());
        clean_staging(updates.path(), &job, SystemTime::now() + STAGE_AGE * 2);
        assert!(!updates.path().join(&old).exists());
        assert!(updates.path().join(&last).is_dir());
        assert!(updates.path().join("supervisor-update.lock").is_dir());
    }

    #[test]
    fn legacy_binaries_keep_the_newest_recent_and_running_versions() {
        let mut entries = names(&["0.12.35", "0.12.67", "0.12.77", "logs"], true);
        entries.push(("0.12.50".into(), false));
        assert_eq!(
            select_legacy(&entries, |name| name == "0.12.67"),
            ["0.12.35"]
        );
    }

    #[test]
    fn release_of_finds_the_first_component_only_inside_the_root() {
        let root = Path::new("/n/releases");
        assert_eq!(
            release_of(root, Path::new("/n/releases/0.13.1/remote-codex")).as_deref(),
            Some("0.13.1")
        );
        assert_eq!(
            release_of(root, Path::new("/n/other/0.13.1/remote-codex")),
            None
        );
    }
}
