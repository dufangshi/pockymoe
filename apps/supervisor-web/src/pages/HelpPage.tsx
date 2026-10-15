import { LanguageSwitcher, translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { Children, isValidElement, useEffect, type ReactNode } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import { Link, NavLink, useLocation, useParams } from 'react-router-dom';
import remarkGfm from 'remark-gfm';
import { appUrl } from '../lib/webBase';

// Help articles are Markdown per locale. Headings end with `{#anchor}` so the
// app can link to one answer, for example /help/devices#other-account.
const sources = import.meta.glob('../help/*/*.md', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;

export const HELP_TOPICS = [
  { id: 'devices', title: 'common.helpDevices', summary: 'common.helpDevicesSummary' },
  { id: 'connection', title: 'common.helpConnection', summary: 'common.helpConnectionSummary' },
  { id: 'account', title: 'common.helpAccount', summary: 'common.helpAccountSummary' },
] as const;

export function helpSource(locale: string, topic: string) {
  return sources[`../help/${locale}/${topic}.md`] ?? sources[`../help/en/${topic}.md`];
}

function text(children: ReactNode): string {
  return Children.toArray(children).map(child =>
    typeof child === 'string' || typeof child === 'number' ? String(child)
      : isValidElement<{ children?: ReactNode }>(child) ? text(child.props.children) : '').join('');
}

/** Splits `Title {#anchor}` into the visible title and its id. */
export function headingAnchor(children: ReactNode) {
  const match = /^(.*?)\s*\{#([a-z0-9-]+)\}\s*$/.exec(text(children));
  return match ? { title: match[1]!, id: match[2]! } : { title: undefined, id: undefined };
}

function heading(Tag: 'h2' | 'h3') {
  return function Heading({ children }: { children?: ReactNode }) {
    const { title, id } = headingAnchor(children);
    return (
      <Tag id={id}>
        {title ?? children}
        {id ? <a aria-hidden="true" className="help-anchor" href={`#${id}`} tabIndex={-1}>#</a> : null}
      </Tag>
    );
  };
}

const components: Components = {
  h2: heading('h2'),
  h3: heading('h3'),
  a: ({ href = '', children }) => href.startsWith('/')
    ? <Link to={href}>{children}</Link>
    : <a href={href} rel="noopener noreferrer" target="_blank">{children}</a>,
};

export function HelpPage() {
  const { locale } = useI18n();
  const { topic } = useParams();
  const { hash } = useLocation();
  const source = topic ? helpSource(locale, topic) : undefined;

  useEffect(() => {
    if (!hash) { window.scrollTo(0, 0); return; }
    const frame = window.requestAnimationFrame(() =>
      document.getElementById(decodeURIComponent(hash.slice(1)))?.scrollIntoView({ block: 'start' }));
    return () => window.cancelAnimationFrame(frame);
  }, [topic, hash, locale]);

  return (
    <main className="help-shell min-h-[100dvh] bg-[var(--app-bg)] text-[var(--app-fg)]" data-page="help">
      <header className="auth-topbar mx-auto flex w-full max-w-5xl items-center justify-between gap-3 px-4 py-4 sm:px-6">
        <Link className="auth-brand flex min-h-11 items-center gap-2.5 text-[15px] font-semibold text-[var(--theme-fg)]" to="/help">
          <img alt="" className="h-8 w-8 rounded-lg" src={appUrl('icon-192.png?v=pockymoe')} />
          <span>{translate('common.help')}</span>
        </Link>
        <div className="flex items-center gap-1">
          <LanguageSwitcher className="auth-language" />
          <Link className="auth-topbar-link inline-flex min-h-11 items-center rounded-lg px-3 text-sm text-[var(--theme-fg-muted)] hover:text-[var(--theme-fg)]" to="/">
            {translate('common.openApp')}
          </Link>
        </div>
      </header>
      <div className="mx-auto grid w-full max-w-5xl gap-6 px-4 pb-20 sm:px-6 md:grid-cols-[13rem_minmax(0,1fr)] md:gap-10">
        <nav aria-label={translate('common.helpTopics')} className="help-nav flex flex-wrap gap-1 md:sticky md:top-4 md:flex-col md:self-start">
          {HELP_TOPICS.map(item => (
            <NavLink
              className={({ isActive }) => `help-nav-link shrink-0 rounded-lg px-3 py-2 text-sm ${isActive ? 'is-active' : ''}`}
              key={item.id}
              to={`/help/${item.id}`}
            >
              {translate(item.title)}
            </NavLink>
          ))}
        </nav>
        {!topic ? (
          <section className="min-w-0">
            <h1 className="text-2xl font-semibold text-[var(--theme-fg)]">{translate('common.help')}</h1>
            <p className="mt-2 text-sm text-[var(--theme-fg-muted)]">{translate('common.helpIntro')}</p>
            <ul className="mt-6 grid gap-3 sm:grid-cols-2">
              {HELP_TOPICS.map(item => (
                <li key={item.id}>
                  <Link className="help-topic-card block h-full rounded-xl border border-[var(--theme-border)] bg-[var(--theme-panel)] p-4 hover:border-[var(--theme-accent-strong)]" to={`/help/${item.id}`}>
                    <span className="block font-medium text-[var(--theme-fg)]">{translate(item.title)}</span>
                    <span className="mt-1 block text-sm text-[var(--theme-fg-muted)]">{translate(item.summary)}</span>
                  </Link>
                </li>
              ))}
            </ul>
          </section>
        ) : source ? (
          <article className="help-prose min-w-0">
            <ReactMarkdown components={components} remarkPlugins={[remarkGfm]}>{source}</ReactMarkdown>
          </article>
        ) : (
          <p className="text-sm text-[var(--theme-fg-muted)]" role="alert">{translate('common.helpNotFound')}</p>
        )}
      </div>
    </main>
  );
}
