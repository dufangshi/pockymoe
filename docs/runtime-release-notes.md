Claude turn billing now includes native subagents. Usage is read incrementally
from the owning session's child logs, with old history excluded and repeated
native request snapshots counted once. Parent and child requests use their own
models and preserve cache-write durations; mixed-model totals survive database
reload and custom price changes. Children do not change the parent model,
context occupancy or completion lifecycle. A late ACP summary cannot replace
the complete native total.

Validated with focused native-usage and pricing unit tests plus database
integration checks for repeated updates, mixed models, final-summary fallback
and history reload. This release also includes 0.12.90's intact Claude messages
and subscription-panel visual fixes. Existing turn totals are not rewritten.
Windows Device Manager is unchanged; the current device is not force-updated.
