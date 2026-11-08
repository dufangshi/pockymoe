Files up to 1 GB can now be dragged into a conversation, and the Relay never
stores them.

- **Large attachments**: each file can be up to 1 GB, 10 to a prompt. Files over
  16 MB in total go to the device in small encrypted chunks before the prompt is
  sent. The composer shows progress and a cancel button.
- **Interrupted uploads continue**: a dropped connection retries by itself, and a
  slow link gets smaller chunks. After leaving the conversation, sending the same
  draft again continues from where it stopped. An unfinished upload is removed
  after 24 hours.
- **Worktree conversations**: attachments are now saved in the conversation's own
  worktree, where its agent looks for them, instead of the main workspace.
- **Kept out of git**: `.temp/threads` ignores itself, so `git add` no longer picks
  up attachments.

A device must run this version to receive files over 25 MB. An older one still
takes smaller files, and the Web asks for the update otherwise.
