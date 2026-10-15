import { useLayoutEffect, useRef, useState } from 'react';
import { ArrowUp, Check } from 'lucide-react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';

const MAX_HEIGHT = 112;

/** A phone's one-field prompt under the files: ask about what you are reading
    without going back to the conversation. Sends like the main composer's Send. */
export function FileAskBar({ onSend, disabledReason }: {
  onSend: (prompt: string) => Promise<boolean>;
  disabledReason?: string | null;
}) {
  useI18n();
  const [text, setText] = useState('');
  const [state, setState] = useState<'idle' | 'sending' | 'sent' | 'failed'>('idle');
  const field = useRef<HTMLTextAreaElement>(null);

  useLayoutEffect(() => {
    const element = field.current;
    if (!element) return;
    element.style.height = 'auto';
    element.style.height = `${Math.min(element.scrollHeight, MAX_HEIGHT)}px`;
  }, [text]);

  useLayoutEffect(() => {
    if (state !== 'sent') return;
    const timer = window.setTimeout(() => setState('idle'), 1800);
    return () => window.clearTimeout(timer);
  }, [state]);

  const prompt = text.trim();
  const blocked = Boolean(disabledReason) || state === 'sending';
  const send = async () => {
    if (!prompt || blocked) return;
    setState('sending');
    const sent = await onSend(prompt).catch(() => false);
    if (sent) setText('');
    setState(sent ? 'sent' : 'failed');
  };

  return (
    <form className="file-ask-bar" data-state={state} onSubmit={(event) => { event.preventDefault(); void send(); }}>
      {/* The reply lands in the conversation, which is out of sight here. */}
      {state === 'sent' || state === 'failed'
        ? <p className="file-ask-bar-note" role="status">{translate(state === 'sent' ? 'files.askSent' : 'files.askFailed')}</p>
        : null}
      <div className="file-ask-bar-shell">
        <textarea
          ref={field}
          rows={1}
          value={text}
          enterKeyHint="send"
          aria-label={translate('files.askAgent')}
          placeholder={translate('files.askAgentPlaceholder')}
          title={disabledReason ?? undefined}
          disabled={Boolean(disabledReason)}
          onChange={(event) => { setText(event.target.value); if (state === 'failed' || state === 'sent') setState('idle'); }}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              void send();
            }
          }}
        />
        <button type="submit" className="file-ask-bar-send" disabled={!prompt || blocked}
          aria-label={state === 'sending' ? translate('chat.sending') : translate('chat.send')}>
          {state === 'sent' ? <Check aria-hidden="true" /> : <ArrowUp aria-hidden="true" />}
        </button>
      </div>
    </form>
  );
}
