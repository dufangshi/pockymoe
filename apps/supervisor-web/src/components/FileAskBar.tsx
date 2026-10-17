import { useLayoutEffect, useRef, useState } from 'react';
import { ArrowUp, Check, FileText, Paperclip, X } from 'lucide-react';
import { translate, useI18n } from '@pockymoe/thread-ui/i18n';

const MAX_HEIGHT = 112;

/** A phone's one-field prompt under the files: ask about what you are reading
    without going back to the conversation. Sends like the main composer's Send.
    Given `value` and `onChange` it edits the conversation's own draft, so what is
    typed here is in the composer too and the other way round; the host then
    clears the draft when a send is accepted. The file being read goes with the
    question as its workspace path, unless the reader removes its label. */
export function FileAskBar({ onSend, disabledReason, value, onChange, attachmentCount = 0, file = null }: {
  onSend: (prompt: string) => Promise<boolean>;
  disabledReason?: string | null;
  value?: string;
  onChange?: (text: string) => void;
  /** Files already attached in the shared draft; they go with the text. */
  attachmentCount?: number;
  /** The workspace-relative path of the file being read. */
  file?: string | null;
}) {
  useI18n();
  const [ownText, setOwnText] = useState('');
  const controlled = value !== undefined && onChange !== undefined;
  const text = controlled ? value : ownText;
  const setText = (next: string) => (controlled ? onChange(next) : setOwnText(next));
  const [state, setState] = useState<'idle' | 'sending' | 'sent' | 'failed'>('idle');
  const field = useRef<HTMLTextAreaElement>(null);
  const [droppedFile, setDroppedFile] = useState<string | null>(null);
  const target = file && droppedFile !== file ? file : null;

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
    const sent = await onSend(target ? `\`${target}\` ${prompt}` : prompt).catch(() => false);
    if (sent && !controlled) setText('');
    setState(sent ? 'sent' : 'failed');
  };

  return (
    <form className="file-ask-bar" data-state={state} onSubmit={(event) => { event.preventDefault(); void send(); }}>
      {/* The reply lands in the conversation, which is out of sight here. */}
      {state === 'sent' || state === 'failed'
        ? <p className="file-ask-bar-note" role="status">{translate(state === 'sent' ? 'files.askSent' : 'files.askFailed')}</p>
        : null}
      {target ? (
        <p className="file-ask-bar-target">
          <FileText aria-hidden="true" />
          <span title={target}>{target.split('/').at(-1)}</span>
          <button type="button" aria-label={translate('files.askWithoutFile', { name: target })} onClick={() => setDroppedFile(target)}>
            <X aria-hidden="true" />
          </button>
        </p>
      ) : null}
      <div className="file-ask-bar-shell">
        {attachmentCount > 0 && (
          <span className="file-ask-bar-attachments" title={translate('files.askAttachments', { count: attachmentCount })}
            aria-label={translate('files.askAttachments', { count: attachmentCount })}>
            <Paperclip aria-hidden="true" />{attachmentCount}
          </span>
        )}
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
