Codex and Claude threads can switch agents again, the effort slider shows the level in colour, and Forgejo pull requests show their review state.

**Fixes**

- **Switching agents on Codex and Claude threads.** The model picker offered the agent rail only to threads run through the ACP agent. Native Codex and Claude threads, and every thread from before 0.14, were stored on their own provider and never showed it, so a Claude thread could only change Claude models. These threads now list the device's agents and can switch to Codex or any other one. The switch record names the agent the thread came from instead of "unknown".
- **Forgejo review state.** The Forgejo pull request list never read reviews, so an approved PR still said "Review required". The list now reads each open PR's reviews, and a later plain comment no longer hides a reviewer's approval or change request.

**Changed**

- **Coloured effort slider.** The thumb and the level name take the level's colour, the same scale as the composer's effort label, and the fill runs through each level's colour, so the slider warms as the effort rises. The Terminal pack's blocks take their own level's colours; Plum Pocket keeps its plum-to-rose scale.

Update a device from Settings → Check/Update.
