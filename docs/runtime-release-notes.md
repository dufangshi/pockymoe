Isolation now only closes the way in, sign-in always says why it did not
happen, threads can be shared from their menu, and prompts can quote the
conversation.

- **Isolation is inbound only**: an isolated device, or one with peer access
  off, refuses requests from your other devices but its own agents can still
  reach them.
- **Sign-in explains itself**: a Google or GitHub account with no Pockymoe
  account goes on to registration, or to Create account asking for the invite
  code when one is needed. A registration waiting for approval, a rejected one,
  a disabled account, closed registration or a cancelled sign-in each show
  their reason.
- **Admin badges**: waiting registrations show a red count on Users and on its
  Registrations tab in the relay admin console.
- **Share thread**: the thread menu in the sidebar, recent conversations, tabs
  and the phone thread panel opens the thread's share window.
- **Quotes**: select part of the conversation and press Quote to add it to the
  prompt as a numbered quote.
- **Session IDs and transcripts**: Copy Pockymoe session ID copies a labelled
  line an agent can act on, and `pockymoe transcript ID --view summary` shows
  each user message and its reply by turn.
- **Conversation**: a finished turn shows its answer, folded replies end on a
  clean break, finished commands fold while running ones stay out, and
  registered wakes are a small bell above the composer.
- **Port previews**: `pockymoe preview create --origin preserve` forwards the
  browser's own Origin for apps that check it.

The relay and Web parts (sign-in outcomes, admin badges, Share thread) go live
with this release; the isolation change, quotes, transcript summaries and
preview origins need the device update. Validated with the relay suite, the
Web and shared UI unit tests, type checks and screenshots in every theme.
Installing this release is a separate device update.
