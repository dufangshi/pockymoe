Upstream settings now start with saved API connections. Add, edit, test and sync a
connection without choosing or installing a harness. A separate section lets each
harness use a compatible local connection through an explicit Apply action.
Saving or synchronizing a connection never applies it to a harness. Plugins use
the same saved connections and shared Add form.

Content Quality settings no longer show a legacy connection-file path. Existing
private-file settings remain compatible in the backend. The reply-length field
now explains that automatic polishing starts when the final reply's prose reaches
the configured number of characters; code does not count. A disabled plugin's
settings show that it must also be enabled in the Plugins list.

Connection checks allow a reasoning model enough output budget to finish its
brief response instead of misreporting a small token limit as a broken connection.
Incomplete responses are still rejected. Original agent replies remain available
when automatic polishing succeeds or fails.

Validated with focused runtime and Web checks, a live saved-connection editorial
request, and desktop/mobile settings interactions. Windows Device Manager is
unchanged. Installing the runtime is a separate device update; publication does
not restart an existing Supervisor.
