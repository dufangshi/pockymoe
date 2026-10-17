Content Quality now limits polishing context to the most recent 5 execution turns
and 10,000 Unicode characters by default. Both limits apply together, include the
target turn and complete source reply, and can be changed in device plugin
settings or overridden for one thread. Older saved profiles acquire these defaults
when running this version. The public Text Context `/summary` API keeps its
existing all-history default and query semantics.

The plugin preserves the exact target message, keeps the newest remaining public
text within the budget and retains chronological order. A reply that alone exceeds
the configured budget is skipped with a readable explanation rather than cut in
half. Credential redaction is also checked against the outgoing text budget.
The model still chooses the rewrite's length and layout freely.

The settings form detects device support so an older Supervisor cannot appear to
have limits it does not enforce. The original reply-length trigger remains a
separate setting. Update the device through Settings to apply the new defaults;
publishing the Web UI does not restart an installed Supervisor.

Verified with focused Content Quality and summary regressions, shared UI component
checks and an authorized current-thread upstream request with actual token usage.
Windows Device Manager is unchanged.
