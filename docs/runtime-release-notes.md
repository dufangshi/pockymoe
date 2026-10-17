On a shared device you can now see who sent each message. A prompt from anyone
but the device owner shows a small initial on the corner of its bubble, including
while it waits in the queue; tap it for a compact card with the sender's account
name and ID. Your own prompts stay unmarked.

A device bound to your account key takes the sender from the recipient's signed
browser and the name from the grant you signed, so the relay cannot change it.
An unbound device uses the account the relay authorized. Messages sent before
this update are not marked.

Validated with the Supervisor, runtime and relay suites, new sender tests for
signed recipients, forged relay headers, unbound devices and the relay's
headers, and the Web and shared UI tests. The relay is redeployed for the new
headers. Windows Device Manager is unchanged. Installing this release is a
separate device update; publication does not restart an existing Supervisor.
