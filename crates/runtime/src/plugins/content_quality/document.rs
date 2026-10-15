use super::*;

pub(super) fn validate_relative_path(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && path.len() <= 4096,
        "invalid document path"
    );
    let path = Path::new(path);
    ensure!(
        !path.is_absolute() && path.components().all(|c| matches!(c, Component::Normal(_))),
        "document path must be workspace-relative without traversal"
    );
    for part in path.components() {
        let name = part.as_os_str().to_string_lossy().to_ascii_lowercase();
        ensure!(
            !name.starts_with('.')
                && ![
                    "vendor",
                    "vendors",
                    "node_modules",
                    "target",
                    "dist",
                    "build",
                    "generated",
                    "__generated__",
                    "coverage"
                ]
                .contains(&name.as_str()),
            "generated, private or vendor documents are excluded"
        );
        ensure!(
            !["agents.md", "claude.md", "skill.md"].contains(&name.as_str()),
            "agent instruction documents are excluded"
        );
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    ensure!(
        ["md", "mdx", "rst", "txt"].contains(&ext.as_str()),
        "unsupported document extension"
    );
    Ok(())
}
pub(super) fn resolve(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative_path(relative)?;
    let root = root
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("cannot resolve workspace"))?;
    let mut path = root.clone();
    for part in Path::new(relative).components() {
        path.push(part);
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|_| anyhow::anyhow!("document path not found"))?;
        ensure!(
            !meta.file_type().is_symlink(),
            "symlink document paths are excluded"
        );
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("cannot resolve document"))?;
    ensure!(
        canonical.starts_with(&root) && canonical.is_file(),
        "document must be inside the workspace"
    );
    Ok(canonical)
}
pub(super) fn read(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|_| anyhow::anyhow!("cannot open document"))?;
    ensure!(file.metadata()?.is_file(), "document is not a regular file");
    let mut bytes = Vec::new();
    file.take(SOURCE_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("cannot read document"))?;
    ensure!(bytes.len() <= SOURCE_LIMIT, "document exceeds 64 KiB");
    let text =
        String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("document must be strict UTF-8"))?;
    ensure!(!text.contains('\0'), "binary documents are excluded");
    let start = text
        .chars()
        .take(2048)
        .collect::<String>()
        .to_ascii_lowercase();
    ensure!(
        ![
            "@generated",
            "automatically generated",
            "auto-generated",
            "do not edit"
        ]
        .iter()
        .any(|s| start.contains(s)),
        "generated documents are excluded"
    );
    Ok(text)
}
pub(super) fn unsupported(source: &str, path: &str) -> bool {
    // JSX/expressions require an MDX parser. RST directives/literal blocks also
    // have semantic indentation that a prose-only editor must not rewrite.
    path.to_ascii_lowercase().ends_with(".mdx")
        || source.contains('<')
        || source.contains('{')
        || source.contains('}')
        || (path.to_ascii_lowercase().ends_with(".rst")
            && (source.contains("::") || source.lines().any(|l| l.starts_with(".. "))))
}

pub(super) struct Protected {
    pub masked: String,
    parts: Vec<String>,
    prefix: String,
    brackets: (usize, usize),
}
impl Protected {
    pub fn new(source: &str) -> Result<Self> {
        let prefix = format!("POCKYMOE_PROTECTED_{}_", &hash(source)[..16]);
        ensure!(
            !source.contains(&prefix),
            "source collides with protection markers"
        );
        let mut result = Self {
            masked: String::new(),
            parts: Vec::new(),
            prefix,
            brackets: bracket_counts(source),
        };
        let mut offset = 0;
        if source.starts_with("---\n")
            || source.starts_with("---\r\n")
            || source.starts_with("+++\n")
            || source.starts_with("+++\r\n")
        {
            let delimiter = &source[..3];
            let mut end = None;
            for line in source.split_inclusive('\n').skip(1) {
                offset += line.len();
                if line.trim_end() == delimiter {
                    end = Some(source.split_inclusive('\n').next().unwrap().len() + offset);
                    break;
                }
            }
            let end = end
                .context("unclosed document frontmatter")?
                .min(source.len());
            result.push(&source[..end]);
            offset = end;
        }
        while offset < source.len() {
            let rest = &source[offset..];
            let start_line = offset == 0 || source.as_bytes()[offset - 1] == b'\n';
            if start_line {
                let leading = rest.len() - rest.trim_start_matches(' ').len();
                let line = rest.trim_start_matches(' ');
                if line.starts_with("```") || line.starts_with("~~~") {
                    let fence = line.as_bytes()[0];
                    let count = line.bytes().take_while(|b| *b == fence).count();
                    let first = rest.find('\n').map_or(rest.len(), |v| v + 1);
                    let mut end = first;
                    let mut closed = false;
                    for next in rest[first..].split_inclusive('\n') {
                        end += next.len();
                        let next = next.trim();
                        if next.bytes().take_while(|b| *b == fence).count() >= count
                            && next.bytes().all(|b| b == fence)
                        {
                            closed = true;
                            break;
                        }
                    }
                    ensure!(leading <= 3 && closed, "unsupported or unclosed code fence");
                    result.push(&rest[..end]);
                    offset += end;
                    continue;
                }
                // Preserve Markdown indented code, RST indentation and tables.
                if rest.starts_with("    ") || rest.starts_with('\t') || rest.starts_with('|') {
                    let end = rest.find('\n').map_or(rest.len(), |v| v + 1);
                    result.push(&rest[..end]);
                    offset += end;
                    continue;
                }
            }
            if rest.starts_with('`') {
                let count = rest.bytes().take_while(|b| *b == b'`').count();
                let delimiter = "`".repeat(count);
                let end = rest[count..]
                    .find(&delimiter)
                    .map(|n| count + n + count)
                    .context("unclosed inline code")?;
                result.push(&rest[..end]);
                offset += end;
                continue;
            }
            if rest.starts_with('[') || rest.starts_with("![") {
                if let Some(end) = inline_link(rest)? {
                    ensure!(
                        offset == 0 || source.as_bytes()[offset - 1] != b'\\',
                        "escaped Markdown links are unsupported"
                    );
                    result.push(&rest[..end]);
                    offset += end;
                    continue;
                }
            }
            ensure!(!rest.starts_with("]("), "malformed Markdown link");
            if rest.starts_with("https://")
                || rest.starts_with("http://")
                || rest.starts_with("mailto:")
            {
                let end = rest
                    .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | ')' | ']' | '"'))
                    .unwrap_or(rest.len());
                result.push(&rest[..end]);
                offset += end;
                continue;
            }
            let c = rest.chars().next().unwrap();
            result.masked.push(c);
            offset += c.len_utf8();
        }
        Ok(result)
    }
    pub(super) fn marker(&self, index: usize) -> String {
        format!("{}{}__", self.prefix, index)
    }
    fn push(&mut self, source: &str) {
        self.masked.push_str(&self.marker(self.parts.len()));
        self.parts.push(source.into());
    }
    pub fn restore(&self, revised: &str) -> Result<String> {
        let mut cursor = 0;
        for i in 0..self.parts.len() {
            let marker = self.marker(i);
            ensure!(
                revised.matches(&marker).count() == 1,
                "protected content missing or duplicated"
            );
            let at = revised.find(&marker).unwrap();
            ensure!(at >= cursor, "protected content reordered");
            cursor = at + marker.len();
        }
        ensure!(
            revised.matches(&self.prefix).count() == self.parts.len(),
            "unknown protection marker"
        );
        let mut restored = revised.to_string();
        for (i, part) in self.parts.iter().enumerate() {
            restored = restored.replace(&self.marker(i), part);
        }
        ensure!(
            bracket_counts(&restored) == self.brackets,
            "editorial result changed Markdown brackets"
        );
        Ok(restored)
    }
}

fn bracket_counts(text: &str) -> (usize, usize) {
    (text.matches('[').count(), text.matches(']').count())
}
fn inline_link(text: &str) -> Result<Option<usize>> {
    let start = usize::from(text.starts_with('!'));
    let mut depth = 1;
    let mut nested = false;
    let mut escaped = false;
    let mut closing = None;
    for (i, b) in text.bytes().enumerate().skip(start + 1) {
        if b == b'\\' {
            escaped = true;
        }
        if b == b'[' {
            depth += 1;
            nested = true;
        }
        if b == b']' {
            depth -= 1;
            if depth == 0 {
                closing = Some(i);
                break;
            }
        }
    }
    let Some(closing) = closing else {
        return Ok(None);
    };
    if text.as_bytes().get(closing + 1) != Some(&b'(') {
        return Ok(None);
    }
    ensure!(
        !nested && !escaped,
        "nested or escaped Markdown link labels are unsupported"
    );
    depth = 1;
    for (i, b) in text.bytes().enumerate().skip(closing + 2) {
        ensure!(
            !matches!(b, b'\\' | b'\n' | b'\r' | b'\"' | b'\''),
            "escaped or multiline Markdown link targets are unsupported"
        );
        if b == b'(' {
            depth += 1;
        }
        if b == b')' {
            depth -= 1;
            if depth == 0 {
                return Ok(Some(i + 1));
            }
        }
    }
    bail!("unclosed Markdown link")
}

fn facts(text: &str) -> BTreeMap<String, usize> {
    let mut facts = BTreeMap::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || "_-./:+@".contains(c))) {
        let word = word.trim_matches(|c| ".,:/".contains(c));
        let technical = word.chars().any(|c| c.is_ascii_digit())
            || word.contains(['_', '/', '@', '+'])
            || word.contains("::")
            || word.chars().all(|c| !c.is_ascii_lowercase()) && word.len() >= 2
            || word
                .as_bytes()
                .windows(2)
                .any(|w| w[0].is_ascii_lowercase() && w[1].is_ascii_uppercase())
            || word.contains('.') && word.chars().any(|c| c.is_ascii_alphabetic());
        if !word.is_empty() && technical {
            *facts.entry(word.into()).or_insert(0) += 1;
        }
    }
    facts
}
pub(super) fn preserve_facts(source: &str, revised: &str) -> Result<()> {
    ensure!(
        facts(source) == facts(revised),
        "editorial result changed numeric facts or technical tokens"
    );
    Ok(())
}
pub(super) fn atomic_write(path: &Path, source_hash: &str, revised: &str) -> Result<()> {
    let parent = path.parent().context("document has no parent")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| anyhow::anyhow!("cannot stage document rewrite"))?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| anyhow::anyhow!("document disappeared before write"))?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "document changed type before write"
    );
    temp.as_file().set_permissions(metadata.permissions())?;
    temp.write_all(revised.as_bytes())
        .map_err(|_| anyhow::anyhow!("cannot stage document rewrite"))?;
    temp.as_file()
        .sync_all()
        .map_err(|_| anyhow::anyhow!("cannot flush document rewrite"))?;
    ensure!(
        hash(&read(path)?) == source_hash,
        "document changed during review; rewrite cancelled"
    );
    temp.persist(path)
        .map_err(|_| anyhow::anyhow!("cannot atomically replace document"))?;
    Ok(())
}
