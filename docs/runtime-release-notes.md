The "Unlock this browser" prompt no longer reappears on every tap. It only asks
for the password when it can help: when this browser has no device key yet or
was removed. A device that refuses this browser (for example a shared device)
now gets one silent retry and then fails that request, instead of opening the
prompt again and again.

Every unlock prompt now says why it opened: no browser key here, a removed
browser, the action that needs your account key, or which device refused which
request with which code. "Copy details for support" copies these details.

The Web also stops polling devices it cannot reach. An offline device, or one
refusing this browser, is skipped for 30 seconds doubling to 10 minutes; a
refused browser registration is not retried for a minute doubling to thirty; a
closed split stops polling its reference thread; and subscription usage waits
while the tab is hidden.

A device now replaces a recipient's least recently seen browser when that
recipient already has 32 registered, instead of refusing new ones, so private
windows or cleared storage can no longer lock someone out of a shared device.
The relay applies the same rule to account browsers.

Validated with the Supervisor and Web suites, including new tests for the
recipient limit, the registration backoff, the device backoff and the prompt's
cause, and the prompt in every theme on phone and desktop. Windows Device Manager
is unchanged. Installing this release is a separate device update; publication
does not restart an existing Supervisor.
