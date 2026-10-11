import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import { useAppShellNav } from './AppShellNavContext';

export function ComposerShortcutSettings() {
  useI18n();
  const nav = useAppShellNav();
  const disabled =
    !nav?.setSendShortcut || nav.sendShortcutLoading || nav.sendShortcutSaving;
  return (
    <fieldset className="settings-shortcuts py-5">
      <legend className="text-sm font-semibold">{translate("settings.messageShortcuts")}</legend>
      <p className="settings-hint mt-1 text-xs leading-5 text-[var(--theme-fg-muted)]">
        {translate("settings.savedToYourAccountAndAppliedAcross")}</p>
      <div className="settings-choice-group mt-3 space-y-2">
        {(
          [
            {
              value: 'ctrlEnter',
              label: translate("settings.ctrlEnterToSend"),
              description:
                translate("settings.enterInsertsANewLineCtrlShift"),
            },
            {
              value: 'enter',
              label: translate("settings.enterToSend"),
              description:
                translate("settings.shiftEnterInsertsANewLineCtrl"),
            },
          ] as const
        ).map((option) => (
          <label
            key={option.value}
            className="settings-choice flex min-h-11 items-start gap-3 rounded-md border border-[var(--theme-border)] p-3"
          >
            <input
              type="radio"
              name="settings-send-shortcut"
              value={option.value}
              checked={(nav?.sendShortcut ?? 'ctrlEnter') === option.value}
              disabled={disabled}
              onChange={() => {
                void nav?.setSendShortcut?.(option.value);
              }}
              className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--theme-accent-solid)]"
            />
            <span>
              <span className="block text-sm font-medium">{option.label}</span>
              <span className="settings-hint mt-1 block text-xs text-[var(--theme-fg-muted)]">
                {option.description}
              </span>
            </span>
          </label>
        ))}
      </div>
      <p className={`${nav?.setSendShortcut && !nav.sendShortcutSaving && !nav.sendShortcutLoading ? 'settings-hint ' : ''}mt-2 text-xs text-[var(--theme-fg-muted)]`}>
        {nav?.sendShortcutSaving
          ? translate("settings.saving")
          : nav?.sendShortcutLoading
            ? translate("settings.loading")
            : !nav?.setSendShortcut
              ? translate("settings.signInThroughRelayToChangeYour")
              : translate("settings.steerDeliversToTheRunningTurnImmediately")}
      </p>
      {nav?.sendShortcutError ? (
        <div className="mt-2 text-xs">
          <p className="host-error" role="alert">
            {nav.sendShortcutError}
          </p>
          <button
            type="button"
            className="mt-2 underline"
            onClick={() => {
              void nav.refreshSendShortcut?.();
            }}
          >
            {translate("settings.retry")}</button>
        </div>
      ) : null}
    </fieldset>
  );
}
