Switching a Codex thread's model works again.

- **"Unsupported format of modelId … Expected: modelId[effort]"**: since 0.13.62,
  changing the model of a Codex thread (or starting one on a model other than
  Codex's default) failed with this error, or the turn quietly ran on the
  default model. The model is now sent with its reasoning effort, and a refused
  catalog refresh no longer stops the switch.
