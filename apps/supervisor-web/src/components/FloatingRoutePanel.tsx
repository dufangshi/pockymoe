import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type { ReactNode } from 'react';
import { ArrowLeft } from 'lucide-react';

interface FloatingRoutePanelProps {
  eyebrow?: string;
  title: string;
  description?: string;
  children: ReactNode;
  maxWidthClassName?: string;
  backLabel?: string;
  onBack?: () => void;
}

export function FloatingRoutePanel({
  eyebrow,
  title,
  description,
  children,
  maxWidthClassName = '!max-w-2xl',
  backLabel = translate("workbench.back"),
  onBack,
}: FloatingRoutePanelProps) {
  useI18n();
  return (
    <div className={`product-page ${maxWidthClassName} pt-[calc(env(safe-area-inset-top)+1rem)] sm:pt-8`}>
      <section>
        {onBack ? (
          <div className="product-topbar">
            <button
              className="inline-flex min-h-11 items-center gap-2 rounded-md px-2 text-sm font-medium text-[var(--theme-fg-soft)] hover:bg-[var(--theme-hover)] hover:text-[var(--theme-fg)]"
              onClick={onBack}
              type="button"
            >
              <ArrowLeft aria-hidden="true" className="h-4 w-4" />
              {backLabel}
            </button>
          </div>
        ) : null}
        <header className={`border-b border-[var(--theme-border)] pb-5 ${onBack ? 'pt-6' : ''}`}>
          {eyebrow ? <p className="product-eyebrow">{eyebrow}</p> : null}
          <h1 className={`product-title${eyebrow ? ' mt-1.5' : ''}`}>
            {title}
          </h1>
          {description ? (
            <p className="product-description mt-2">
              {description}
            </p>
          ) : null}
        </header>
        <div className="py-6">{children}</div>
      </section>
    </div>
  );
}
