Claude can now receive a Steer message while waiting for a native background task.

- When Claude has no foreground response running, ACP asks the host to submit a normal prompt. Pockymoe now does so immediately instead of reporting `promptRequired / noRunningTurn`. The existing turn retains ownership of the response, tool events, usage and cancellation; the original background task can still wake it later.
- Input delivery returns after the prompt is written, without waiting for the model to finish. Unrelated steering errors are not retried, and cancelled or retired turns reject new input.
- The workbench always has one split button. Its menu offers closing the active split or restoring the previous split, alongside selecting another conversation. Entering or leaving split mode no longer adds a separate restore button to the toolbar.

Validation includes durable-history protocol regressions, an isolated real Claude waiting/steering/wake cycle, and desktop/mobile split-menu checks in the classic and Plum Pocket themes. This runtime release includes the current main-branch Web UI. Windows Device Manager remains independently released.
