# The prompt editor and keyboards

The prompt is a `contenteditable="plaintext-only"` element
(`composer/ComposerPromptEditor.tsx` in the shared UI), not a `textarea`,
because attachments and quotes sit in the text as tiles. The browser and the
keyboard own its content while someone types; React state follows it.

## Rules

A phone keyboard, and a voice keyboard most of all, edits in sequences: it
writes a rough transcript, then deletes it backwards and inserts the tidied
text. A third-party keyboard on iOS can do nothing else: it sees the text before
the caret, deletes backwards and inserts at the caret. If the content or the
caret changes under it between two steps, its next step lands in the wrong
place: the rough text stays and the tidied text is added again.

- **The editor's content is read, not written, while it is the source.** An
  input event serializes the editor into the draft (`handlePromptInput`), and
  the editor remembers that text as its own (`editorTextRef`). The DOM is rebuilt
  from state only for a change that did not come from the editor: a sent prompt
  clearing the draft, an attachment or quote inserted, a queued prompt taken back.
- **State that is behind the editor never overwrites it.** When the state still
  holds the text the editor reported and the focused editor already differs, a
  keyboard is mid-sequence. The sync is skipped and the editor's text is taken
  into the state on the next frame (`useComposerPromptDomSync`).
- **A composition is left alone.** Between `compositionstart` and
  `compositionend` the editor is neither read nor written.
- **A rebuild never leaves the caret at the start.** With a known position it is
  restored; without one the caret goes to the end.
- **Nothing is laid out next to the prompt while it is being typed in.** The
  folded composer measures a hidden copy of the prompt to decide its overflow
  mark; that runs only while the composer is folded
  (`useCompactComposer`).
- A host that controls the draft gets it 180 ms after the last change; its echo
  is an acknowledgement and never rolls newer text back (`useComposerDraft`).

## Recording what a keyboard does

Input problems often happen on one phone with one keyboard and cannot be
reproduced elsewhere. Opening a thread with `?inputtrace=1` (kept for the
browser session; `?inputtrace=0` turns it off) shows a strip above the prompt
with a count, **Copy** and **Clear**. It records the editor's `beforeinput`,
`input`, composition, focus, selection and editing-key events and every rebuild
or skipped rebuild of the prompt, each with the time, the text length, the caret
position and the number of child nodes. It never records text: only lengths and
positions (`composer/inputTrace.ts`). The copied report is one event per line.

## Tests

The maintained tests are the unit tests beside the hooks (`useComposerPromptDomSync.test.tsx`,
`inputTrace.test.ts`, `useComposerDraft` and `useCompactComposer` tests). A
keyboard's sequence can be replayed in Chromium through the DevTools protocol
(`Input.imeSetComposition` with a replacement range, `Input.insertText`), and in
WebKit with the editing commands a keyboard ends up calling
(`document.execCommand('insertText' | 'delete')`). Neither reproduces an iOS
third-party keyboard exactly; the recording above is how to see one.
