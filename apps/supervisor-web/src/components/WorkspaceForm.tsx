import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { FormEvent, useEffect, useRef, useState } from 'react';
import { FolderOpen, FolderPlus, GitBranch } from 'lucide-react';

export type WorkspaceFormMode = 'folder' | 'path' | 'git';

export type WorkspaceFormInput =
  | { mode: 'folder' | 'path'; absPath: string; label?: string }
  | { mode: 'git'; gitUrl: string; label?: string };

const folderNamePattern = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

function isGitInput(value: string) {
  const trimmed = value.trim();
  return (
    /^https?:\/\/.+/i.test(trimmed) ||
    /^ssh:\/\/.+/i.test(trimmed) ||
    /^git@[^:]+:.+/.test(trimmed)
  );
}

function isAbsolutePath(value: string) {
  const trimmed = value.trim();
  return (
    trimmed.startsWith('/') ||
    /^[A-Za-z]:[\\/]/.test(trimmed) ||
    /^\\\\[^\\]+\\[^\\]+/.test(trimmed)
  );
}

function inferInitialMode(value: string): WorkspaceFormMode {
  if (isGitInput(value)) {
    return 'git';
  }
  return isAbsolutePath(value) ? 'path' : 'folder';
}

function inferWorkspaceLabel(value: string) {
  const trimmed = value.trim();
  const normalized = trimmed.replace(/[\\/]+$/, '');
  if (!normalized) {
    return '';
  }

  const withoutQuery = normalized.split(/[?#]/)[0] ?? normalized;
  const rawName = withoutQuery.split(/[\\/:]/).filter(Boolean).at(-1) ?? '';
  return rawName.endsWith('.git') ? rawName.slice(0, -4) : rawName;
}

const modeOptions = [
  { value: 'folder' as const, get label() { return translate("files.newFolder"); }, Icon: FolderPlus },
  { value: 'path' as const, get label() { return translate("files.existingPath"); }, Icon: FolderOpen },
  { value: 'git' as const, get label() { return translate("files.gitRepository"); }, Icon: GitBranch },
];

const modeFields: Record<
  WorkspaceFormMode,
  { label: string; placeholder: string; hint: string; submitLabel: string; busyLabel: string }
> = {
  folder: {
    get label() { return translate("files.folderName"); },
    placeholder: 'my-project',
    get hint() { return translate("files.createsANewDirectoryUnderTheConfigured"); },
    get submitLabel() { return translate("files.createFolder"); },
    get busyLabel() { return translate("files.creating"); },
  },
  path: {
    get label() { return translate("files.absolutePath"); },
    placeholder: '/Users/name/project',
    get hint() { return translate("files.registersADirectoryThatIsAlreadyAvailable"); },
    get submitLabel() { return translate("files.addWorkspace"); },
    get busyLabel() { return translate("files.adding"); },
  },
  git: {
    get label() { return translate("files.repositoryURL"); },
    placeholder: 'https://github.com/owner/repo.git',
    get hint() { return translate("files.clonesTheRepositoryIntoTheConfiguredDevelopment"); },
    get submitLabel() { return translate("files.cloneRepository"); },
    get busyLabel() { return translate("files.cloning"); },
  },
};

interface WorkspaceFormProps {
  initialPath?: string;
  initialLabel?: string;
  initialMode?: WorkspaceFormMode;
  newFolderRoot?: string | null;
  submitLabel?: string;
  error?: string | null;
  busy?: boolean;
  surface?: boolean;
  onCancel?: () => void;
  onInputChange?: () => void;
  onSubmit: (input: WorkspaceFormInput) => Promise<void> | void;
}

export function WorkspaceForm({
  initialPath = '',
  initialLabel = '',
  initialMode,
  newFolderRoot,
  submitLabel,
  error,
  busy = false,
  surface = true,
  onCancel,
  onInputChange,
  onSubmit,
}: WorkspaceFormProps) {
  useI18n();
  const resolvedInitialMode = initialMode ?? inferInitialMode(initialPath);
  const initialAutoLabel = inferWorkspaceLabel(initialPath);
  const [mode, setMode] = useState<WorkspaceFormMode>(resolvedInitialMode);
  const [targets, setTargets] = useState<Record<WorkspaceFormMode, string>>({
    folder: resolvedInitialMode === 'folder' ? initialPath : '',
    path: resolvedInitialMode === 'path' ? initialPath : '',
    git: resolvedInitialMode === 'git' ? initialPath : '',
  });
  const [label, setLabel] = useState(initialLabel || initialAutoLabel);
  const [localError, setLocalError] = useState<string | null>(null);
  const previousAutoLabelRef = useRef(initialAutoLabel);
  const targetInputRef = useRef<HTMLInputElement | null>(null);
  const activeTarget = targets[mode];
  const field = modeFields[mode];
  const fieldError = localError ?? error ?? null;

  useEffect(() => {
    const nextAutoLabel = inferWorkspaceLabel(activeTarget);
    setLabel((current) => {
      if (!current.trim() || current === previousAutoLabelRef.current) {
        return nextAutoLabel;
      }

      return current;
    });
    previousAutoLabelRef.current = nextAutoLabel;
  }, [activeTarget]);

  useEffect(() => {
    if (!error) {
      return;
    }

    const focusFrame = window.requestAnimationFrame(() => targetInputRef.current?.focus());
    return () => window.cancelAnimationFrame(focusFrame);
  }, [error]);

  function clearErrors() {
    setLocalError(null);
    onInputChange?.();
  }

  function changeMode(nextMode: WorkspaceFormMode) {
    setMode(nextMode);
    clearErrors();
  }

  function validateTarget(rawTarget: string) {
    if (!rawTarget) {
      return translate("files.isRequired", { value1: field.label });
    }
    if (mode === 'folder' && !folderNamePattern.test(rawTarget)) {
      return translate("files.use1128LettersNumbersPeriodsUnderscores");
    }
    if (mode === 'path' && !isAbsolutePath(rawTarget)) {
      return translate("files.enterAnAbsolutePathSuchAsUsers");
    }
    if (mode === 'git' && !isGitInput(rawTarget)) {
      return translate("files.enterAnHTTPSOrSSHGitRepository");
    }
    return null;
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();

    const rawTarget = activeTarget.trim();
    const validationError = validateTarget(rawTarget);
    if (validationError) {
      setLocalError(validationError);
      targetInputRef.current?.focus();
      return;
    }

    setLocalError(null);
    const normalizedLabel = label.trim();
    const optionalLabel = normalizedLabel ? { label: normalizedLabel } : {};
    const payload: WorkspaceFormInput =
      mode === 'git'
        ? { mode, gitUrl: rawTarget, ...optionalLabel }
        : { mode, absPath: rawTarget, ...optionalLabel };
    await onSubmit(payload);
  }

  const formClassName = surface
    ? 'product-panel space-y-6 p-5 sm:p-6'
    : 'space-y-6';
  const hintText =
    mode === 'folder' && newFolderRoot
      ? translate('workbench.createsFolderUnder', { value1: newFolderRoot })
      : field.hint;

  return (
    <form onSubmit={handleSubmit} className={formClassName} noValidate>
      <fieldset disabled={busy}>
        <legend className="host-form-label text-sm font-medium">{translate("files.workspaceSource")}</legend>
        <div className="product-segmented mt-2 !grid w-full grid-cols-3 !overflow-visible" aria-label={translate("files.workspaceSource")}>
          {modeOptions.map(({ value, label: optionLabel, Icon }) => (
            <button
              key={value}
              type="button"
              aria-pressed={mode === value}
              onClick={() => changeMode(value)}
              className="product-segment flex !min-h-14 min-w-0 items-center justify-center gap-1.5 !whitespace-normal px-1.5 text-center leading-4 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-accent-ring)] sm:!min-h-11 sm:px-3"
            >
              <Icon aria-hidden="true" className="hidden h-4 w-4 shrink-0 sm:block" />
              <span>{optionLabel}</span>
            </button>
          ))}
        </div>
      </fieldset>

      <div>
        <label htmlFor="workspace-target" className="host-form-label text-sm font-medium">
          {field.label}
        </label>
        <input
          ref={targetInputRef}
          id="workspace-target"
          name={mode === 'git' ? 'gitUrl' : mode === 'folder' ? 'folderName' : 'absPath'}
          value={activeTarget}
          onChange={(event) => {
            setTargets((current) => ({ ...current, [mode]: event.target.value }));
            clearErrors();
          }}
          placeholder={field.placeholder}
          autoFocus
          autoCapitalize={"none"}
          autoCorrect="off"
          spellCheck={false}
          disabled={busy}
          aria-invalid={fieldError ? true : undefined}
          aria-describedby={`workspace-target-hint${fieldError ? ' workspace-target-error' : ''}`}
          aria-errormessage={fieldError ? 'workspace-target-error' : undefined}
          className="host-form-control mt-2 min-h-11 w-full rounded-lg border px-4 py-2.5 outline-none transition disabled:cursor-not-allowed disabled:opacity-60"
        />
        <p id="workspace-target-hint" className="host-muted mt-2 break-words text-xs leading-5">
          {hintText}
        </p>
        {fieldError ? (
          <p
            id="workspace-target-error"
            role="alert"
            className="mt-2 text-sm leading-5 text-[var(--status-danger-fg)]"
          >
            {fieldError}
          </p>
        ) : null}
      </div>

      <div>
        <label htmlFor="workspace-label" className="host-form-label text-sm font-medium">
          {translate("files.displayLabel")} <span className="host-muted font-normal">{translate("files.optional")}</span>
        </label>
        <input
          id="workspace-label"
          name="label"
          value={label}
          onChange={(event) => {
            setLabel(event.target.value);
            clearErrors();
          }}
          placeholder={translate("files.usesTheFolderOrRepositoryName")}
          disabled={busy}
          className="host-form-control mt-2 min-h-11 w-full rounded-lg border px-4 py-2.5 outline-none transition disabled:cursor-not-allowed disabled:opacity-60"
        />
      </div>

      <div className="flex flex-wrap items-center justify-end gap-2 border-t border-[var(--theme-border)] pt-5">
        {onCancel ? (
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            className="host-secondary-button inline-flex h-11 items-center justify-center rounded-md border px-4 text-sm font-medium transition disabled:cursor-not-allowed disabled:opacity-60"
          >
            {translate("files.cancel")}</button>
        ) : null}
        <button
          type="submit"
          disabled={busy || !activeTarget.trim()}
          className="ui-action-primary inline-flex h-11 items-center justify-center rounded-md px-4 text-sm font-medium transition disabled:cursor-not-allowed"
        >
          {busy ? field.busyLabel : submitLabel ?? field.submitLabel}
        </button>
      </div>
    </form>
  );
}
