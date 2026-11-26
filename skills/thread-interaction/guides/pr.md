# Pull requests and issues

Use `pockymoe pr --help` and `pockymoe issue --help` for flags. Commands use your
thread's working copy and the owner's saved host sign-in. Local thread credentials
can act only on their own thread; peers and shares cannot use host credentials.
For a PR in another repository, link its URL first, then pass
`--item-url HTTPS_URL` to select that linked item explicitly.

- `pockymoe pr list`, `show NUMBER`, `comments NUMBER`, `reviews NUMBER`, `files NUMBER`, `commits NUMBER`
- `pockymoe pr create --head BRANCH --base main --title TITLE --body BODY --draft`
- `pockymoe pr update NUMBER --title TITLE --body BODY`
- `pockymoe pr comment NUMBER --body BODY`
- `pockymoe pr checks NUMBER` resolves the latest head automatically.
- `pockymoe pr merge NUMBER --expected-head SHA --method squash` refuses if the head changed.
- `pockymoe pr watch NUMBER`, `pockymoe pr unwatch` (root threads only).
- `pockymoe pr link HTTPS_URL`, `pockymoe pr unlink HTTPS_URL`
- `pockymoe pr work-on NUMBER` fetches the PR ref, creates a managed worktree and a
  new thread there. Continue using the returned thread and path explicitly.
- Review: `pockymoe pr review show NUMBER` returns the conversation, reviews,
  review threads (with ids), checks (with rerun tokens), the head SHA and your
  drafts. `review files NUMBER` lists changed files with patches and viewed state.
- `pockymoe pr review comment NUMBER --path P --line N [--start-line M] [--side left]
  --body TEXT` adds a draft line comment (`--file` comments on the whole file).
  Drafts stay on this device until you submit; they are yours only, never the
  owner's. `review drafts`, `review discard --id ID | --all`.
- `pockymoe pr review submit NUMBER --event comment|approve|request-changes
  --body TEXT --expected-head SHA` sends your drafts as one review and refuses
  if the head moved: run `review show` again and recheck your lines.
- `review reply NUMBER --thread ID --body TEXT`, `review resolve NUMBER --thread ID
  [--unresolve]`, `review viewed NUMBER PATH…`, `review reviewers NUMBER --add LOGIN`,
  `review state NUMBER close|reopen|draft|ready`, `review rerun NUMBER --token T`,
  `review revert NUMBER` (GitHub), `review guide NUMBER [--show]`.
- `pockymoe pr review merge NUMBER --expected-head SHA --method squash
  --remove-agent-credits` merges without agent co-author trailers; human
  co-authors stay.
- `pockymoe issue list --search WORDS`, `show NUMBER`, `create --title TITLE`,
  `comment NUMBER --body BODY`, `link HTTPS_URL` (GitHub and GitLab).

Host tokens are configured by the owner in Settings → Source control. Tokens are
never returned. Git push/clone use Git transport credentials independently.
