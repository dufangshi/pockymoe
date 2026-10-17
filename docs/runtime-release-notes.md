An agent thread opened on its own now says where it came from. A slim bar
pinned above the conversation reads "子线程 · 由 <parent>" (with the parent's
workspace when it differs) and offers a button back to the parent thread; on a
phone the button is an icon. A task message from another thread names that
thread instead of an 8-character id.

Agent threads that work in another workspace than their conversation now count
there: the thread map gives them their own tile in that workspace ("由 … 创建"),
and `pockymoe thread list --workspace` lists them without `--all`. The sidebar
shows each thread's own workspace instead of the open thread's.

Validated with the Supervisor suite including a new listing test, the shared UI
and Web tests, and screenshots in every theme and pack on desktop and phone.
Windows Device Manager is unchanged. Installing this release is a separate
device update; publication does not restart an existing Supervisor.
