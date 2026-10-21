// Keyless DSH provider for E2E: `CALL <tool> <json>` in the latest user text
// makes one tool call, `TOOLS?` lists the tools DSH offered (the run mode's
// tool set), `HISTORY?` lists the earlier user texts the model saw (what a
// fork kept); otherwise it answers. Text and reasoning stream in small chunks
// so bridge streaming is observable.
export const name = 'pockymoe-e2e-scripted-llm';
export const inject = ['llm'];

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

function userTexts(messages) {
  return messages.filter(message => message.role === 'user').map(message =>
    typeof message.content === 'string'
      ? message.content
      : (message.content ?? []).filter(block => block.type === 'text').map(block => block.text).join(''))
    .filter(text => !text.startsWith('Current runtime context'));
}

function lastUserText(messages) {
  return userTexts(messages).at(-1) ?? '';
}

class ScriptedAdapter {
  providerInfo(provider) { return { id: provider, name: 'E2E Scripted' }; }
  providerRetryPolicy() { return undefined; }
  imageRequestPricing() { return undefined; }
  async listModels(provider) { return [{ provider, id: 'scripted', name: 'Scripted' }]; }
  async resolveModel(provider, model) {
    return { provider, id: model, name: model, context: { contextWindow: 128000 },
      reasoning: { efforts: [{ id: 'low', name: 'Low' }, { id: 'high', name: 'High' }], defaultEffort: 'high' } };
  }
  async prepareCall(provider, model, signal) {
    return { model: await this.resolveModel(provider, model, signal), stream: options => this.stream(options) };
  }
  async *stream({ messages = [], tools = [] }) {
    const text = lastUserText(messages);
    const afterTool = messages.at(-1)?.role !== 'user';
    let index = 0;
    async function* block(kind, body) {
      const at = index++;
      yield { type: 'block-start', index: at, blockType: kind };
      for (const part of body.match(/.{1,6}/gs) ?? []) {
        yield { type: kind === 'text' ? 'text-delta' : 'reasoning-delta', index: at, text: part };
        await sleep(20);
      }
      yield { type: 'block-end', index: at, block: { type: kind, text: body } };
    }
    yield* block('reasoning', `Thinking about: ${text.slice(0, 40)}`);
    const call = !afterTool && /CALL (\w+) (\{.*\})/s.exec(text);
    if (call) {
      yield* block('text', `I will call ${call[1]}.`);
      const at = index++;
      const id = `call_${Math.random().toString(36).slice(2, 8)}`;
      yield { type: 'block-start', index: at, blockType: 'tool-call' };
      yield { type: 'tool-call-delta', index: at, id, name: call[1], argumentsDelta: call[2] };
      yield { type: 'block-end', index: at, block: { type: 'tool-call', id, name: call[1], arguments: call[2] } };
      yield { type: 'usage', usage: { inputTokens: 100, outputTokens: 20, totalTokens: 120 } };
      yield { type: 'finish', reason: { kind: 'tool-calls' } };
      return;
    }
    const answer = afterTool ? 'Tool finished; final answer SCRIPTED_DONE.'
      : text.includes('TOOLS?') ? `TOOLS=${tools.map(tool => tool.name).sort().join(',')}`
      : text.includes('HISTORY?') ? `HISTORY=${userTexts(messages).slice(0, -1).join(' | ')}`
      : `Plain answer SCRIPTED_OK for: ${text.slice(0, 60)}`;
    yield* block('text', answer);
    yield { type: 'usage', usage: { inputTokens: 90, outputTokens: 10, totalTokens: 100 } };
    yield { type: 'finish', reason: { kind: 'stop' } };
  }
}

export function apply(ctx) {
  ctx.llm.registerAdapter(['e2e-scripted'], new ScriptedAdapter());
}
