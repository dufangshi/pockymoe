Pockymoe inside Treer can now update itself from Settings, and switching
threads no longer empties the tab strip.

- **Update the hub from Settings in Treer**: Settings in a Treer Agent tab gain
  a Device tab with the hub's running and latest version, Check updates, Update
  and Restart Supervisor. It uses the same management API as Treer setup; the
  hub exits and Treer relaunches it, and the page waits for it to return. After
  this release, updating no longer needs a Treer Agent running setup. Harnesses,
  device templates and the local passcode stay out of it.
- **Steady tab strip**: switching to another thread keeps every tab in the top
  strip and only moves the highlight. Before, the strip showed just the target
  thread for up to a second while the page reloaded the thread list.

Validated with the Treer crate and Supervisor Treer tests, Web tests including
a new thread-switch regression, and a browser check of the Treer tunnel: the
Device tab loads only the Supervisor status. The tab strip was traced on a
snapshot of a real database. Windows Device Manager is unchanged. Installing this
release is a separate device update; publication does not restart an existing
Supervisor.
