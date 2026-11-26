# Pull requests and issues

Use `pockymoe pr --help` and `pockymoe issue --help` for flags. Commands use your
thread's working copy and the owner's saved host sign-in. Local thread credentials
can act only on their own thread; peers and shares cannot use host credentials.

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
- `pockymoe issue list --search WORDS`, `show NUMBER`, `create --title TITLE`,
  `comment NUMBER --body BODY`, `link HTTPS_URL` (GitHub and GitLab).

Host tokens are configured by the owner in Settings → Source control. Tokens are
never returned. Git push/clone use Git transport credentials independently.
