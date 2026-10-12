import WorkspaceEditorWorker from '@pockymoe/thread-ui/workspace-editor.worker?worker';
import React from 'react';
import { I18nProvider, initializeI18n } from '@pockymoe/thread-ui/i18n';
import ReactDOM from 'react-dom/client';
import 'streamdown/styles.css';
// Before `./app`: fixes the theme pack's place in the cascade (see the module).
import './lib/themePacks/anchor';

import { App } from './app';
import './index.css';
import './native-subagents.css';
import { initializeFontSize } from './lib/fontSize';
import { initializeThemePacks } from './lib/themePacks';

// The host bundler resolves the worker asset before dependency optimization.
Object.assign(window, { MonacoEnvironment: { getWorker: () => new WorkspaceEditorWorker() } });
initializeFontSize();
initializeI18n();

// The stored theme pack is applied before the first render, so the page does not
// flash Classic; a slow pack applies when it arrives, a broken one falls back.
void initializeThemePacks({ timeoutMs: 1500 }).finally(() => {
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <I18nProvider><App /></I18nProvider>
    </React.StrictMode>
  );
});
