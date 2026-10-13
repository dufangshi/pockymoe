//! The Supervisor's descendant processes, grouped by loaded harness, with
//! their memory (docs/harness-lifecycle.md). Linux reports PSS from
//! `smaps_rollup`; other platforms report RSS from `sysinfo`.
use std::collections::{HashMap, HashSet};

use pockymoe_protocol::{HarnessProcessDto, HarnessProcessRole, ProcessMemoryMetric};

#[derive(Debug, Clone)]
pub(crate) struct ProcessEntry {
    pub pid: u32,
    pub ppid: Option<u32>,
    /// Used for classification only; never returned.
    pub argv: Vec<String>,
    pub bytes: Option<u64>,
}

pub(crate) struct ProcessSample {
    pub metric: ProcessMemoryMetric,
    pub supervisor_pid: u32,
    entries: HashMap<u32, ProcessEntry>,
    children: HashMap<u32, Vec<u32>>,
}

impl ProcessSample {
    /// Blocking: reads the process table. Memory is read only when asked.
    pub fn collect(memory: bool) -> Self {
        let supervisor_pid = std::process::id();
        let (entries, metric) = platform::collect(supervisor_pid, memory);
        Self::from_entries(supervisor_pid, entries, metric)
    }

    pub fn from_entries(
        supervisor_pid: u32,
        entries: Vec<ProcessEntry>,
        metric: ProcessMemoryMetric,
    ) -> Self {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for entry in &entries {
            if let Some(parent) = entry.ppid.filter(|parent| *parent != entry.pid) {
                children.entry(parent).or_default().push(entry.pid);
            }
        }
        for list in children.values_mut() {
            list.sort_unstable();
        }
        Self {
            metric,
            supervisor_pid,
            entries: entries
                .into_iter()
                .map(|entry| (entry.pid, entry))
                .collect(),
            children,
        }
    }

    /// `root` first, then its descendants breadth-first.
    pub fn descendants(&self, root: u32) -> Vec<u32> {
        if !self.entries.contains_key(&root) {
            return Vec::new();
        }
        let mut out = vec![root];
        let mut seen = HashSet::from([root]);
        let mut index = 0;
        while index < out.len() {
            for child in self.children.get(&out[index]).into_iter().flatten() {
                if seen.insert(*child) {
                    out.push(*child);
                }
            }
            index += 1;
        }
        out
    }

    pub fn bytes(&self, pid: u32) -> Option<u64> {
        self.entries.get(&pid).and_then(|entry| entry.bytes)
    }

    /// Sum over `pids`; None when any of them could not be measured.
    pub fn total(&self, pids: impl IntoIterator<Item = u32>) -> Option<u64> {
        pids.into_iter()
            .try_fold(0u64, |sum, pid| Some(sum + self.bytes(pid)?))
    }

    /// The harness rooted at `root` (the adapter we spawned), classified.
    pub fn harness_tree(&self, root: u32) -> Vec<HarnessProcessDto> {
        let mut roles: HashMap<u32, HarnessProcessRole> = HashMap::new();
        self.descendants(root)
            .into_iter()
            .filter_map(|pid| {
                let entry = self.entries.get(&pid)?;
                let parent = entry.ppid.and_then(|ppid| roles.get(&ppid).copied());
                let role = if pid == root {
                    HarnessProcessRole::Adapter
                } else {
                    role_of(&entry.argv, parent)
                };
                roles.insert(pid, role);
                Some(HarnessProcessDto {
                    pid,
                    role,
                    name: name_of(&entry.argv),
                    bytes: entry.bytes,
                })
            })
            .collect()
    }

    /// Processes the agent started with its tools, which a release would end.
    pub fn runs_tools(&self, root: u32) -> bool {
        self.harness_tree(root)
            .iter()
            .any(|process| process.role == HarnessProcessRole::Tool)
    }

    /// Supervisor descendants outside `attributed` (probes, shells, terminals).
    pub fn unattributed(&self, attributed: &HashSet<u32>) -> Vec<HarnessProcessDto> {
        self.descendants(self.supervisor_pid)
            .into_iter()
            .skip(1)
            .filter(|pid| !attributed.contains(pid))
            .filter_map(|pid| {
                let entry = self.entries.get(&pid)?;
                let role = match role_of(&entry.argv, None) {
                    role @ (HarnessProcessRole::Mcp | HarnessProcessRole::Adapter) => role,
                    _ => HarnessProcessRole::Other,
                };
                Some(HarnessProcessDto {
                    pid,
                    role,
                    name: name_of(&entry.argv),
                    bytes: entry.bytes,
                })
            })
            .collect()
    }
}

/// `npm` rewrites its title into one argv string; split it back into words.
fn words(argv: &[String]) -> Vec<String> {
    match argv {
        [single] if single.contains(' ') => single.split_whitespace().map(str::to_owned).collect(),
        _ => argv.to_vec(),
    }
}

fn basename(value: &str) -> String {
    let base = value.rsplit(['/', '\\']).next().unwrap_or(value);
    base.strip_suffix(".exe")
        .or_else(|| base.strip_suffix(".EXE"))
        .unwrap_or(base)
        .to_string()
}

/// Programs whose first argument names what actually runs.
fn interpreter(program: &str) -> bool {
    let program = program.to_ascii_lowercase();
    matches!(
        program.as_str(),
        "node"
            | "nodejs"
            | "bun"
            | "deno"
            | "python"
            | "python3"
            | "ruby"
            | "perl"
            | "sh"
            | "bash"
            | "zsh"
            | "dash"
            | "fish"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "npm"
            | "npx"
            | "pnpm"
            | "yarn"
            | "bunx"
            | "uv"
            | "uvx"
            | "pipx"
    ) || program.starts_with("python3.")
}

/// Rejects anything that could be a credential or a long value: only short
/// words with separators, never a long run of letters and digits.
fn safe_word(word: &str) -> bool {
    let mut run = 0;
    let mut longest = 0;
    for ch in word.chars() {
        if ch.is_ascii_alphanumeric() {
            run += 1;
            longest = longest.max(run);
        } else if matches!(ch, '.' | '_' | '-' | '@' | '+') {
            run = 0;
        } else {
            return false;
        }
    }
    !word.is_empty() && word.len() <= 48 && longest <= 12
}

/// The script or package an interpreter or runner executes.
fn script_of(words: &[String]) -> Option<String> {
    words
        .iter()
        .skip(1)
        .filter(|word| !word.starts_with('-'))
        .find(|word| !matches!(word.as_str(), "exec" | "run" | "x" | "dlx" | "tool"))
        .and_then(|word| word.split_whitespace().next())
        .map(basename)
        .filter(|word| safe_word(word))
}

/// A subcommand right after the program (`codex app-server`, `grok agent`).
fn subcommand_of(words: &[String]) -> Option<String> {
    words
        .get(1)
        .filter(|word| {
            word.len() <= 24
                && word.chars().all(|ch| ch.is_ascii_lowercase() || ch == '-')
                && !word.starts_with('-')
        })
        .cloned()
}

/// Executable basename plus one distinguishing argument, never the full
/// command line (it may carry secrets).
pub(crate) fn name_of(argv: &[String]) -> String {
    let words = words(argv);
    let Some(program) = words.first().map(|word| basename(word)) else {
        return "?".into();
    };
    let argument = if interpreter(&program) {
        script_of(&words)
    } else {
        subcommand_of(&words)
    };
    match argument {
        Some(argument) if argument != program => format!("{program} {argument}"),
        _ => program,
    }
}

pub(crate) fn role_of(argv: &[String], parent: Option<HarnessProcessRole>) -> HarnessProcessRole {
    if parent == Some(HarnessProcessRole::Mcp) {
        return HarnessProcessRole::Mcp;
    }
    let words = words(argv);
    if words
        .iter()
        .any(|word| word.to_ascii_lowercase().contains("mcp"))
    {
        return HarnessProcessRole::Mcp;
    }
    let Some(program) = words
        .first()
        .map(|word| basename(word).to_ascii_lowercase())
    else {
        return HarnessProcessRole::Tool;
    };
    let script = interpreter(&program)
        .then(|| script_of(&words).map(|script| script.to_ascii_lowercase()))
        .flatten();
    let app_server = words.iter().any(|word| word == "app-server");
    match (program.as_str(), script.as_deref()) {
        (_, Some("codex-acp" | "claude-agent-acp")) | ("codex-acp" | "claude-agent-acp", _) => {
            HarnessProcessRole::Adapter
        }
        ("grok", _) => HarnessProcessRole::Adapter,
        // The npm `codex` launcher in front of the native app-server.
        (_, Some("codex")) => HarnessProcessRole::Bridge,
        ("claude" | "codex", _) => HarnessProcessRole::Agent,
        ("codex-linux-sandbox", _) => HarnessProcessRole::Tool,
        (program, _) if program.starts_with("codex-") => HarnessProcessRole::Agent,
        (program, _)
            if app_server
                && (program.starts_with("remote-codex") || program.starts_with("pockymoe")) =>
        {
            HarnessProcessRole::Bridge
        }
        _ => HarnessProcessRole::Tool,
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    fn read(pid: u32, file: &str) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/{file}")).ok()
    }

    /// The parent PID; `comm` may contain spaces and parentheses.
    fn parent(stat: &str) -> Option<u32> {
        stat.rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    }

    fn argv(pid: u32) -> Vec<String> {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|raw| {
                raw.split(|byte| *byte == 0)
                    .filter(|part| !part.is_empty())
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `(rss, pss)` in bytes.
    pub(super) fn smaps_rollup(text: &str) -> (Option<u64>, Option<u64>) {
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name))
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|kb| kb.parse::<u64>().ok())
                .map(|kb| kb * 1024)
        };
        (field("Rss:"), field("Pss:"))
    }

    fn statm_rss(pid: u32) -> Option<u64> {
        let pages: u64 = read(pid, "statm")?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()?;
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Some(
            pages
                * u64::try_from(page)
                    .ok()
                    .filter(|page| *page > 0)
                    .unwrap_or(4096),
        )
    }

    pub(super) fn collect(
        supervisor_pid: u32,
        memory: bool,
    ) -> (Vec<ProcessEntry>, ProcessMemoryMetric) {
        let mut entries: Vec<ProcessEntry> = std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(|pid| {
                Some(ProcessEntry {
                    pid,
                    ppid: parent(&read(pid, "stat")?),
                    argv: Vec::new(),
                    bytes: None,
                })
            })
            .collect();
        let tree: HashSet<u32> =
            ProcessSample::from_entries(supervisor_pid, entries.clone(), ProcessMemoryMetric::Pss)
                .descendants(supervisor_pid)
                .into_iter()
                .collect();
        entries.retain(|entry| tree.contains(&entry.pid));
        let mut all_pss = true;
        let mut rss = HashMap::new();
        for entry in &mut entries {
            entry.argv = argv(entry.pid);
            if !memory {
                continue;
            }
            let (rollup_rss, pss) = read(entry.pid, "smaps_rollup")
                .map(|text| smaps_rollup(&text))
                .unwrap_or_default();
            all_pss &= pss.is_some();
            entry.bytes = pss;
            rss.insert(entry.pid, rollup_rss.or_else(|| statm_rss(entry.pid)));
        }
        if !memory || all_pss {
            return (entries, ProcessMemoryMetric::Pss);
        }
        // One unreadable PSS would make the sum meaningless: use RSS throughout.
        for entry in &mut entries {
            entry.bytes = rss.get(&entry.pid).copied().flatten();
        }
        (entries, ProcessMemoryMetric::Rss)
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::*;
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    pub(super) fn collect(
        _supervisor_pid: u32,
        memory: bool,
    ) -> (Vec<ProcessEntry>, ProcessMemoryMetric) {
        let mut system = System::new();
        let mut kind = ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cmd(UpdateKind::Always);
        if memory {
            kind = kind.with_memory();
        }
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        let entries = system
            .processes()
            .values()
            .filter(|process| process.thread_kind().is_none())
            .map(|process| ProcessEntry {
                pid: process.pid().as_u32(),
                ppid: process.parent().map(|parent| parent.as_u32()),
                argv: process
                    .cmd()
                    .iter()
                    .map(|part| part.to_string_lossy().into_owned())
                    .collect(),
                bytes: memory.then(|| process.memory()),
            })
            .collect();
        (entries, ProcessMemoryMetric::Rss)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, ppid: u32, argv: &str, bytes: u64) -> ProcessEntry {
        ProcessEntry {
            pid,
            ppid: Some(ppid),
            argv: argv.split(' ').map(str::to_owned).collect(),
            bytes: Some(bytes),
        }
    }

    /// The trees measured on a live device: a Claude thread with its MCP
    /// server, a Codex chain and a Claude thread running a background shell.
    fn sample() -> ProcessSample {
        ProcessSample::from_entries(
            1,
            vec![
                entry(
                    1,
                    0,
                    "remote-codex device-run --config /x/relay-supervisor.json",
                    80,
                ),
                entry(10, 1, "node /a/bin/claude-agent-acp", 40),
                entry(
                    11,
                    10,
                    "/a/sdk/claude --output-format stream-json --resume=80c21430-58fe",
                    200,
                ),
                ProcessEntry {
                    argv: vec!["npm exec @bitget-ai/bitget-agent-mcp".into()],
                    ..entry(12, 11, "", 30)
                },
                entry(13, 12, "sh -c bitget-agent-mcp", 1),
                entry(14, 13, "node /home/u/.local/bin/bitget-agent-mcp", 35),
                entry(20, 1, "node /a/bin/codex-acp", 30),
                entry(21, 20, "/opt/remote-codex-linux-x64-gnu app-server", 5),
                entry(22, 21, "node /home/u/.local/bin/codex app-server", 10),
                entry(23, 22, "/v/bin/codex app-server", 120),
                entry(24, 23, "/v/bin/codex-code-mode-host", 17),
                entry(30, 1, "node /a/bin/claude-agent-acp", 40),
                entry(31, 30, "/a/sdk/claude --output-format stream-json", 200),
                entry(
                    32,
                    31,
                    "bash -c npm run dev -- --token=sk-proj-AbC123dEf456GhI789",
                    3,
                ),
                entry(33, 32, "node /w/node_modules/.bin/vite", 60),
                entry(40, 1, "bash -l", 2),
            ],
            ProcessMemoryMetric::Pss,
        )
    }

    #[test]
    fn processes_are_classified_by_executable_and_mcp_descendants() {
        let sample = sample();
        let claude = sample.harness_tree(10);
        let roles: Vec<_> = claude.iter().map(|p| (p.pid, p.role)).collect();
        assert_eq!(
            roles,
            [
                (10, HarnessProcessRole::Adapter),
                (11, HarnessProcessRole::Agent),
                (12, HarnessProcessRole::Mcp),
                (13, HarnessProcessRole::Mcp),
                (14, HarnessProcessRole::Mcp),
            ]
        );
        assert!(!sample.runs_tools(10));
        let codex: Vec<_> = sample.harness_tree(20).iter().map(|p| p.role).collect();
        assert_eq!(
            codex,
            [
                HarnessProcessRole::Adapter,
                HarnessProcessRole::Bridge,
                HarnessProcessRole::Bridge,
                HarnessProcessRole::Agent,
                HarnessProcessRole::Agent,
            ]
        );
        assert!(!sample.runs_tools(20));
        assert!(sample.runs_tools(30), "a dev server keeps the harness busy");
        assert_eq!(sample.total(sample.descendants(20)), Some(182));
        let other = sample.unattributed(
            &sample
                .descendants(10)
                .into_iter()
                .chain(sample.descendants(20))
                .chain(sample.descendants(30))
                .collect(),
        );
        assert_eq!(other.len(), 1);
        assert_eq!(
            (other[0].pid, other[0].role),
            (40, HarnessProcessRole::Other)
        );
    }

    #[test]
    fn process_names_never_include_secrets_or_flag_values() {
        let sample = sample();
        let names: HashMap<u32, String> = [10, 20, 30]
            .into_iter()
            .flat_map(|root| sample.harness_tree(root))
            .map(|p| (p.pid, p.name))
            .collect();
        assert_eq!(names[&10], "node claude-agent-acp");
        assert_eq!(names[&11], "claude");
        assert_eq!(names[&12], "npm bitget-agent-mcp");
        assert_eq!(names[&13], "sh bitget-agent-mcp");
        assert_eq!(names[&21], "remote-codex-linux-x64-gnu app-server");
        assert_eq!(names[&22], "node codex");
        assert_eq!(names[&23], "codex app-server");
        assert_eq!(names[&24], "codex-code-mode-host");
        assert_eq!(names[&32], "bash npm");
        assert_eq!(names[&33], "node vite");
        for argv in [
            "node sk-proj-AbC123dEf456GhI789",
            "tool AbC123dEf456GhI789",
            "python3 -c import os;os.system('x')",
            "env API_KEY=secret node",
        ] {
            let name = name_of(&argv.split(' ').map(str::to_owned).collect::<Vec<_>>());
            assert!(
                !name.contains("AbC123") && !name.contains("secret") && !name.contains('('),
                "{name}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_processes_sample_reads_pss_for_the_current_process() {
        let (rss, pss) =
            platform::smaps_rollup("Rss:   100 kB\nPss:    60 kB\nShared_Clean: 1 kB\n");
        assert_eq!((rss, pss), (Some(102_400), Some(61_440)));
        let sample = ProcessSample::collect(true);
        assert_eq!(sample.supervisor_pid, std::process::id());
        assert!(sample
            .bytes(std::process::id())
            .is_some_and(|bytes| bytes > 0));
        assert!(matches!(
            sample.metric,
            ProcessMemoryMetric::Pss | ProcessMemoryMetric::Rss
        ));
    }
}
