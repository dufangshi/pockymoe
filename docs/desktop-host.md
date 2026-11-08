# Desktop Web host

The Electron client supplies `window.pockymoeHost` only to the configured Web
origin's top frame. The page checks `capabilities()` and feature flags before
using optional services. Ordinary browsers, mobile hosts and older desktop
clients keep their existing navigation when a service is absent.

The Web-side types live in `apps/supervisor-web/src/desktop/host.ts`; the native
source of truth is `desktop/src/shared/contract.ts` in
[the client repository](https://github.com/dufangshi/pockymoe-app).

## Client updates

`features.updates === true` and the optional `updates` service advertise native
client updates. The page subscribes with `updates.onChange(listener)` before
requesting `updates.state()`, and unsubscribes when the shell leaves. A delayed
initial snapshot cannot overwrite a later change. The state has a `status` of
`idle`, `downloading`, `ready`, `installing` or `error`, with an optional `version`,
localized `error` and `releaseUrl`.

Only `ready`, `installing` and install-error states with a version show a rail
button. The native client downloads and verifies files before exposing `ready`;
the Web never handles downloaded files, signatures or file paths. Clicking
“Update to {version}” calls `updates.install()` without arguments and immediately
shows the existing busy spinner. The native client asks every trusted window to
prepare through the `prepareUpdate` host command. Approval returns
`{ ok: true, data: { prepared: true } }`; native requires that marker so older
pages that acknowledge unknown commands cannot bypass the guard. A window with unsent composer
drafts or unsaved workspace documents uses the existing confirmation dialog to
name every affected conversation and show both counts, including attachment-only drafts. The primary confirmation is “Update and restart?” with Update / Cancel; its lead message names the unsent content that will be lost. Long title lists scroll inside the existing dialog. Cancellation aborts the
update. An empty window needs no confirmation. The native client owns installation
and relaunch, and rejects requests without an already-verified update.

The control sits in the rail's bottom host-action slot immediately above the
account avatar. Narrow desktop windows expose the same host action in the navigation
drawer. Its 40px target contains a static 32px accent circle and a 16px ArrowUp;
all colours come from the current theme. A failed installation retains the
button and uses `ErrorMark` for the reason, Retry and Go to download. Download
recovery accepts only the client's canonical GitHub release-tag page.

A missing service, an unadvertised feature, an idle check or an ongoing background
download never adds an icon. Clients without a valid signed release can offer
manual downloads through their own Preferences; the page does not invent a
ready state. Unsigned or invalidly signed offers do not create a link or an icon.
`retryAllowed: false` keeps the icon and Retry disabled after the native attempt
limit; the verified release page remains available for manual recovery.

`prepareUpdate { cancel: true }` revokes earlier consent, including dismissing an
open confirmation if another window refused. Consent covers only the draft and
document revisions shown in that window. Synthetic native unload preflight does
not consume it; a real browser unload does. New edits veto closing again. Native
installation starts only after all windows have actually closed.

## Composer drafts

`useThreadDrafts` preserves prompt text and selected `File` attachments in a
module-local map while navigating or remounting conversations in the same page.
It deliberately does not persist draft bodies or files to browser storage.
Consequently a full page reload or native app quit/relaunch loses unsent composer
drafts. Running agent work belongs to Supervisors and is independent of this
local draft lifetime.

## File downloads

A workspace file is read from the device in pieces (1 MiB chunks over the
encrypted relay), and its Download button shows a bar with bytes received, the
total and the percentage. A failed download shows its reason after that button;
the toolbar and menu downloads report it in the workbench's message.

The desktop client's `files.saveAs` takes a whole file in one message of at most
64 MB. A larger file goes through the client's download manager instead, which
asks where to save it and streams it to disk (`desktop/download.ts`).
