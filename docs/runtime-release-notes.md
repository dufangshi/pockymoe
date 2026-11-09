DeepSeek-V4.1-Flash sees attached pictures in DeepSeek Harness conversations.

- **Pictures for DeepSeek-V4.1-Flash**: the DeepSeek Harness decides at start
  whether it takes images, from a built-in default model id that no longer names
  the vision model, so pictures were refused. Pockymoe now starts it with the
  conversation's model, and a model that reads images gets the picture inline.
- **After a model switch**: until the harness restarts, pictures go as files the
  agent can open, never inline to a model that cannot read them, which would
  break every later turn of the conversation.
