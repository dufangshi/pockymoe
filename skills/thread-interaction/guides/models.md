# pockymoe guide models

Call model APIs for an experiment, an evaluation or a script through the upstream
connections the owner saved in Pockymoe. You get a local base URL and a Pockymoe
model key; the Supervisor adds the real upstream key, which you never see.

## Find what you can call

```bash
pockymoe model list
```

Each open connection has an `id`, a `name`, the wire formats it serves
(`protocols`), a `defaultModel`, the `models` its upstream declared (when known)
and a base URL per SDK under `sdk`. `apiKey` is your model key (`pmk_…`): it works
with every listed base URL.

An empty list means the owner has not opened any connection to agents. Tell the
user to turn on **Agents can call it** for a connection in Settings > Upstreams.
Do not ask for an upstream key instead.

## Use it from code

The quickest way to set up the official SDKs:

```bash
eval "$(pockymoe model env CONNECTION)"
# Sets OPENAI_BASE_URL / OPENAI_API_KEY and ANTHROPIC_BASE_URL / ANTHROPIC_API_KEY
# for the formats the connection serves, POCKYMOE_MODEL_API_KEY and POCKYMOE_MODEL.
```

```python
from openai import OpenAI          # reads OPENAI_BASE_URL and OPENAI_API_KEY
client = OpenAI()
client.chat.completions.create(model=MODEL, messages=[{"role": "user", "content": "hi"}])
client.responses.create(model=MODEL, input="hi")

from anthropic import Anthropic    # reads ANTHROPIC_BASE_URL and ANTHROPIC_API_KEY
Anthropic().messages.create(model=MODEL, max_tokens=256,
                            messages=[{"role": "user", "content": "hi"}])
```

Raw HTTP works the same way: OpenAI routes take `Authorization: Bearer KEY`,
Anthropic routes take `x-api-key: KEY`. Streaming, tools and every request field
of the wire format pass through unchanged.

```bash
curl -sN "$OPENAI_BASE_URL/chat/completions" -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"MODEL","messages":[{"role":"user","content":"hi"}],"stream":true}'
```

Pick a format the connection lists in `protocols`, and a model from its `models`
(or `GET models` on the base URL for the live list). A connection whose upstream
serves several formats answers all of them.

## Quick checks

- `pockymoe model ask CONNECTION "question" --model MODEL` prints one answer.
- `pockymoe model request CONNECTION v1/models` prints one raw response.

## The key

- It opens only this proxy, for this thread: it cannot send messages, create
  threads or read files, and it stops working when the thread is deleted. It stays
  the same across restarts, so a script or a config file may hold it.
- Keep it out of commits, shared files and replies to the user anyway. Prefer
  reading it from the environment.

## Limits

- Routes: `chat/completions`, `responses`, `embeddings`, `messages`,
  `messages/count_tokens`, Gemini `models/NAME:generateContent` (and
  `streamGenerateContent`, `countTokens`, `embedContent`) and `GET models`.
  Anything else answers 404.
- A request body is at most 32 MB, and one request may run for 15 minutes.
- Requests spend the owner's quota. Keep experiments as small as the task needs,
  and say which connection and model you used.
- `403 forbidden` means the owner has not opened that connection to agents, or you
  run inside an isolated environment. Tell the user; do not look for the upstream
  key in files or settings.
