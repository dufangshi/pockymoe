Composer menus pick what you tap again; each model offers its own reasoning
efforts, and a model switched during a turn is visible; scrollbars follow the
theme; the web terminal handles Delete with zsh on macOS.

- **Composer menus**: picking from the slash list or the model menu ran the last
  entry instead (choosing /debug ran /mcp, and a model could not be chosen). The
  keys' enlarged touch area covered every menu row; it now belongs to the keys
  only. The menus size to their content: no sideways scrolling, and the model
  list no longer scrolls inside a small box.
- **Reasoning efforts per model**: the list was the current model's for every
  model, so a session on Sonnet 4.6 hid xhigh everywhere. Each model now shows
  its own levels (xhigh for Opus 4.7 and later, Sonnet 5 and Fable; Sonnet 4.6
  and Opus 4.6 stop at max; Haiku has none), learned from the harness without
  sending a prompt.
- **Switching during a turn**: Settings → Session accepts model, effort and
  permission changes while a turn runs, like the composer. Claude takes a new
  model or effort from the turn's next model call; other harnesses from the next
  turn. The model menu and Settings say which applies.
- **Which models a turn called**: a turn whose model was switched heads with the
  models in the order they ran, such as "opus 4.7 → sonnet 4.6 · max"; tapping it
  lists the calls each made. An effort changed mid-turn becomes the turn's effort.
- **Themed scrollbars**: every scrolling box draws the theme's own scrollbar
  instead of the system's (Classic a thin pill, Plum Pocket a capsule on a dashed
  rail, Terminal a square block on a hairline rail). Touch-only devices keep the
  platform's overlay indicator.
- **Web terminal on macOS**: a Supervisor started by launchd gave shells no TERM,
  so zsh redrew Delete as spaces. Shells now run as xterm-256color with a UTF-8
  locale when none is set.

Validated with the runtime, supervisor and protocol Rust tests and the Claude
usage and turn integration tests, the Web and shared UI tests, browser checks in
an isolated Supervisor (every composer menu row hit-tested on desktop and phone,
the model menu, Settings and turn head during a running turn, themes light and
dark, the terminal environment), the per-model efforts read from the installed
Claude harness, and the zsh redraw reproduced with and without TERM. Windows
Device Manager is unchanged. Installing this release is a separate device
update; publication does not restart an existing Supervisor.
