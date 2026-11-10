# Prompt attachments

A file dragged, pasted or picked in the composer goes to the device and is named in
the prompt as `[FILE ./.temp/threads/<thread>/<name>]` or `[PHOTO ...]`. Each file
can be up to 1 GiB, and a prompt can carry 10.

## How an agent receives them

`[FILE]` reaches an ACP agent as a `resource_link` to the file. `[PHOTO]` is an
inline image block when the agent advertises `promptCapabilities.image`, or its
adapter says it takes images anyway (Grok, `accepts_unadvertised_images` in
`crates/runtime/src/acp/adapter.rs`). Any other agent gets the picture as a
`resource_link` with its image type, which it can open with its tools.

The DeepSeek Harness advertises images from the model its ACP is configured with
when it starts, so Pockymoe starts it with the thread's model (see
[dsh-adapter.md](dsh-adapter.md#图片)). After a model switch, until the process
restarts, pictures go as file links: an inline image in DSH's history fails
every later turn of a model that cannot read images.

## Two paths

| Files of one prompt | Path |
| --- | --- |
| 16 MiB or less in total | With the prompt, in one multipart request |
| More | In chunks first, then the prompt as ordinary JSON |

The chunked path needs a Supervisor from 0.13.53 or later. An older device answers
404: the Web sends files of up to 25 MiB (30 MiB in total) the old way and otherwise
asks for the device to be updated.

## Chunked routes

All are under `/api/threads/{thread}` and require control of the conversation.

```text
POST   /uploads                     {name,size,kind} -> {uploadId,received,size,chunkSize}
PUT    /uploads/{id}?offset=N       raw bytes        -> {received}
GET    /uploads/{id}                                 -> {received,size,token}
POST   /uploads/{id}/complete                        -> {token,size}
DELETE /uploads/{id}
```

- A chunk is at most 4 MiB. A slow link's client halves it, down to 512 KiB, when
  requests time out.
- `offset` must be where the file ends. Otherwise the answer is `409 offset_conflict`
  with `received`, and the client continues from there. Repeating a chunk that landed
  is accepted, because the answer to the first copy may have been lost.
- `complete` moves the file into place and may be repeated. A photo over 20 MiB is
  named as `[FILE]`, because `[PHOTO]` is read whole into the prompt.
- `507 insufficient_storage` refuses a file that would leave less than 256 MiB free.

The Web sends one chunk of one file at a time. Leaving the conversation stops the
upload and keeps what arrived: sending the same draft again continues it. The composer's
cancel button also removes the partial file.

## Where the bytes are

**Relay.** Only forwards. Each chunk is held in memory until the device answers, then
dropped, and nothing is written to disk. The reverse proxy in front of it must not write
request bodies to disk either: for nginx, set `proxy_request_buffering off` on the
Relay's server block. See [self-host-relay.md](self-host-relay.md).

**Device.** The partial file is `.temp/threads/<thread>/.uploads/<id>.part` under the
directory the harness runs in, and its record is in `thread-uploads/` beside the
database. Environment threads write inside their container through the `appendChunk`
worker operation.

`.temp/threads/.gitignore` keeps all of it out of `git add`.

## Cleanup

An unfinished upload is removed 24 hours after it started, by a sweep at startup, every
hour and on each new upload. Finished files stay in `.temp/threads/<thread>`, like every
attachment before them, and count toward the thread's workspace size.
