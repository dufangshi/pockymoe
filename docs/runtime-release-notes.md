Applying an upstream to a harness works when its configuration file is shared
into a container.

- **Apply failed with "Device or resource busy (os error 16)"** on a device that
  runs in a container where the harness's configuration file (for example
  `~/.codex/config.toml` or `auth.json`) is mounted on its own from the host.
  Such a file cannot be replaced by renaming, which is how Pockymoe writes it
  safely. It is now rewritten in place when that happens; everywhere else the
  safe replacement is unchanged.
- Note that a file shared this way is the same file on both machines: applying
  an upstream on one changes the other too.
