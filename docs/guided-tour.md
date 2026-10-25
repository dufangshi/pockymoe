# Guided tour and its sample device

The tour (Tutorial in the rail, Settings → Preferences, or the first-visit card)
walks chapters of real controls: devices and workspaces, threads, terminal,
split view, files, agent teamwork, sharing, and automation and notifications.
Every account sees the same steps, whether or not it has devices or threads,
because the tour runs on a **sample device** answered inside the page.

## The sample device

- Its id is `pockymoe-tour`. All of its pages live under
  `/devices/pockymoe-tour/...`, in both local and Relay mode. On those pages,
  `lib/tourSandbox/` answers what a Supervisor would, from sample data kept in
  memory:
  - the REST API, the `/ws` socket for thread events and terminals, and the
    relay calls the pages make about their device (`relay/portal`,
    `relay/access`, `relay/shares`, `relay/grants`, `relay/thread-links`);
  - the hooks are in `lib/api.ts`: `apiPath`, `fetchApi`, `openSocket` and
    the media URLs. Signing in (`api/auth/*`, `relay/auth/*`) stays real.
- The sample is two workspaces (`pockymoe-demo`, `docs-site`) and three
  threads:
  - a main thread whose agent refactors a file and starts a helper with
    `pockymoe thread create ... --wake`. The helper's report wakes the main
    thread and folds under the original task.
  - the helper thread, a child of the main one. Its first message is a
    `[Pockymoe task from thread ...]` envelope.
  - a docs thread, for the split view.
  The sample text follows the Web language.
- A sent prompt gets a scripted reply of about ten seconds: progress, one
  command, a streamed answer. That leaves time to try Stop. No model runs.
- The terminal is a small interpreter over the sample files (`help` lists
  its commands).
- Files can be browsed, created, edited and saved. A save checks the hash it
  was read at, like the device's safe save.
- Shares and grants made on the sample device are rows in memory. They are not
  signed and nothing is delivered; sharing controls work there in local mode
  too.

## What stays apart from the reader's own state

- `setSelectedRelayDeviceId` never stores the sample device.
- Recents, shortcuts, open tabs and workspace order for the sample device use
  their own browser keys (suffix `:pockymoe-tour`), never the relay workbench.
- On sample pages `relay/portal` is the sample device's, so the account's
  owned-device record is not rewritten.
- The account's device list (`/relay-devices`) never lists the sample device.

## Starting and closing

- Starting the tour from any page other than a sample page:
  - remembers that page in session storage;
  - starts the sample over (`resetTourSandbox`), clearing the sample's own
    browser keys.
- Each step opens its page once. A thread step names the sample thread it
  needs (`thread: 'main' | 'helper'`).
- The open chapter and step are kept in session storage, so a reload or a
  remount continues the tour.
- Closing the tour on a sample page returns to the remembered page.
- In Relay mode the devices chapter starts on the account's own device list
  (add a device, the token form, the list). Its list step is optional, so an
  empty list reads as "no devices yet". The chapter then continues on the
  sample device.

Step definitions are in `components/onboarding/tourChapters.ts`. Their wording
is `tour.*` in the shared UI's `i18n/tour.*.ts`.
