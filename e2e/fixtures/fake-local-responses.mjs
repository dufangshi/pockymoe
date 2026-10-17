// A fake local inference server speaking the OpenAI Responses API with known timing:
// prefill at PREFILL tok/s over the uncached input, then reasoning and answer tokens
// streamed one per delta at DECODE tok/s. Each request is logged with its ground truth.
import { createServer } from 'node:http';
import { appendFileSync } from 'node:fs';

const PORT = Number(process.env.PORT ?? 18941);
const PREFILL = Number(process.env.PREFILL ?? 3000);
const DECODE = Number(process.env.DECODE ?? 40);
const REASONING = Number(process.env.REASONING ?? 120);
const ANSWER = Number(process.env.ANSWER ?? 160);
const CACHED = Number(process.env.CACHED ?? 0.5); // share of input served from the prefix cache
const LOG = process.env.LOG ?? 'fake-local-truth.jsonl';
const sleep = ms => new Promise(done => setTimeout(done, ms));
let seq = 0;

createServer(async (req, res) => {
  let body = '';
  for await (const chunk of req) body += chunk;
  if (req.method === 'GET') { res.writeHead(200, { 'content-type': 'application/json' }); res.end(JSON.stringify({ object: 'list', data: [{ id: 'fake-local', object: 'model' }] })); return; }
  if (!req.url.endsWith('/responses')) { res.writeHead(404); res.end(); return; }
  const received = Date.now();
  const input = Math.max(1, Math.round(body.length / 4));
  const cached = Math.floor(input * CACHED);
  const uncached = input - cached;
  const id = `resp_${++seq}`;
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
  let n = 0;
  const send = (type, data) => res.write(`event: ${type}\ndata: ${JSON.stringify({ type, sequence_number: n++, ...data })}\n\n`);
  send('response.created', { response: { id, object: 'response', status: 'in_progress', model: 'fake-local', output: [] } });
  await sleep(uncached * 1000 / PREFILL);
  const firstToken = Date.now();
  const reasoningId = `rs_${seq}`, messageId = `msg_${seq}`;
  send('response.output_item.added', { output_index: 0, item: { id: reasoningId, type: 'reasoning', summary: [] } });
  send('response.reasoning_summary_part.added', { item_id: reasoningId, output_index: 0, summary_index: 0, part: { type: 'summary_text', text: '' } });
  let summary = '';
  for (let i = 0; i < REASONING; i++) {
    const delta = i ? ` t${i}` : '**Thinking** t0';
    summary += delta;
    send('response.reasoning_summary_text.delta', { item_id: reasoningId, output_index: 0, summary_index: 0, delta });
    await sleep(1000 / DECODE);
  }
  send('response.reasoning_summary_text.done', { item_id: reasoningId, output_index: 0, summary_index: 0, text: summary });
  send('response.reasoning_summary_part.done', { item_id: reasoningId, output_index: 0, summary_index: 0, part: { type: 'summary_text', text: summary } });
  send('response.output_item.done', { output_index: 0, item: { id: reasoningId, type: 'reasoning', summary: [{ type: 'summary_text', text: summary }] } });
  send('response.output_item.added', { output_index: 1, item: { id: messageId, type: 'message', role: 'assistant', status: 'in_progress', content: [] } });
  send('response.content_part.added', { item_id: messageId, output_index: 1, content_index: 0, part: { type: 'output_text', text: '', annotations: [] } });
  let text = '';
  for (let i = 0; i < ANSWER; i++) {
    const delta = i ? ` w${i}` : 'Hello w0';
    text += delta;
    send('response.output_text.delta', { item_id: messageId, output_index: 1, content_index: 0, delta });
    await sleep(1000 / DECODE);
  }
  const lastToken = Date.now();
  send('response.output_text.done', { item_id: messageId, output_index: 1, content_index: 0, text });
  send('response.content_part.done', { item_id: messageId, output_index: 1, content_index: 0, part: { type: 'output_text', text, annotations: [] } });
  const message = { id: messageId, type: 'message', role: 'assistant', status: 'completed', content: [{ type: 'output_text', text, annotations: [] }] };
  send('response.output_item.done', { output_index: 1, item: message });
  const usage = { input_tokens: input, input_tokens_details: { cached_tokens: cached }, output_tokens: REASONING + ANSWER, output_tokens_details: { reasoning_tokens: REASONING }, total_tokens: input + REASONING + ANSWER };
  send('response.completed', { response: { id, object: 'response', status: 'completed', model: 'fake-local', output: [{ id: reasoningId, type: 'reasoning', summary: [{ type: 'summary_text', text: summary }] }, message], usage } });
  res.end();
  appendFileSync(LOG, JSON.stringify({ id, received, firstToken, lastToken, input, cached, uncached, prefillMs: firstToken - received, decodeMs: lastToken - firstToken, output: REASONING + ANSWER, prefillRate: uncached * 1000 / (firstToken - received), decodeRate: (REASONING + ANSWER - 1) * 1000 / (lastToken - firstToken) }) + '\n');
}).listen(PORT, '127.0.0.1', () => console.log(`fake local model on ${PORT}`));
