Share a whole workspace, not only one conversation or a whole device. Shares
never reach a terminal, conversation and workspace shares stay inside their
workspace's files, and the model is told when a prompt comes from someone you
shared with.

- **Workspace shares**: the share dialog offers This thread, This workspace and
  Whole device. A workspace share covers every conversation in that workspace
  (and the agent threads they start) with the same view/collaborate and file
  access choices as the others; **Can create new threads** lets the recipient
  start conversations there, and only there. Recipients find the workspace
  under the device in **Devices with me**. Port previews of that workspace's
  conversations open for them too.
- **No terminal for shares**: whatever a share allows, its recipient never gets
  the terminal: no shell panel or shell toggle, no terminal plugin, no one-off
  shell commands, no script automations. This also applies to existing shares
  that could use it before.
- **Files stay in the workspace**: for conversation and workspace shares, the
  file browser and attachments stay inside the shared workspace. Whole-device
  shares keep their rules.
- **Prompts from recipients**: a prompt, queued prompt, interjection or goal from
  someone you shared with reaches the model with one line saying who sent it,
  that they are not the device's owner, and that sensitive requests (private
  files, deleting data, credentials or system settings) should be refused. The
  conversation shows the prompt as they typed it.

Validated with the access-rule, relay, Supervisor grant and runtime tests (a
workspace grant reaches only that workspace's conversations, terminal routes
and messages are refused for every share, attachments stay in the workspace,
the guardrail reaches the model but not the transcript), the Web and shared UI
tests, an end-to-end workspace share on a protected device, and the share
dialog and recipient views in every theme, light and dark, on phone and
desktop. Workspace shares and the no-terminal rule need the relay (already
updated) and, for protected devices and the guardrail, this device update.
Installing this release is a separate device update.
