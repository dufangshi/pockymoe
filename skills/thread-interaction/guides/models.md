# pockymoe guide models

Call a model API for an experiment or a script through a connection the owner
saved on this device. You never see the connection's key: the Supervisor adds it.

## Find what you can call

```bash
pockymoe model list
```

Each connection has an `id`, a `name`, the wire formats it serves (`protocols`),
a `defaultModel`, the `models` its upstream declared (when known) and a base URL
per SDK under `sdk`. An empty list means the owner has not opened any connection
to agents: tell the user to turn on **Agents can call it** for a connection in
Settings > Upstreams. Do not ask for a key instead.

## One question

```bash
pockymoe model ask CONNECTION "Summarize this in one line: ..." --model MODEL
```

Prints the answer text only. `--system`, `--max-tokens` and `-` (read stdin) are
available. Use this when you only need text back.

## Full API requests

Use the base URL from `model list` and your own `POCKYMOE_TOKEN` as the API key.
Streaming, tools and every request field of the wire format work as usual.

```bash
# OpenAI-compatible (openai_chat / openai_responses)
export OPENAI_BASE_URL="http://127.0.0.1:PORT/api/agent-models/ID/v1"
export OPENAI_API_KEY="$POCKYMOE_TOKEN"
curl -sN "$OPENAI_BASE_URL/chat/completions" -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"MODEL","messages":[{"role":"user","content":"hi"}],"stream":true}'

# Anthropic Messages: base URL without /v1, key in x-api-key
export ANTHROPIC_BASE_URL="http://127.0.0.1:PORT/api/agent-models/ID"
export ANTHROPIC_API_KEY="$POCKYMOE_TOKEN"
```

Without an SDK, `pockymoe model request CONNECTION v1/chat/completions --body-file req.json`
sends one request and prints the response body; `pockymoe model request CONNECTION v1/models`
lists the upstream's models live.

## Limits

- Routes: `chat/completions`, `responses`, `embeddings`, `messages`,
  `messages/count_tokens`, Gemini `models/NAME:generateContent` (and
  `streamGenerateContent`, `countTokens`, `embedContent`) and `GET models`.
  Anything else answers 404.
- A request body is at most 32 MB, and one request may run for 15 minutes.
- Requests spend the owner's quota. Keep experiments as small as the task needs,
  and say which connection and model you used.
- `403 forbidden` means the owner has not opened that connection to agents, or you
  run inside an isolated environment. Tell the user; do not look for the key in
  files or settings.
- Never print `POCKYMOE_TOKEN`; reference the variable, as above.
