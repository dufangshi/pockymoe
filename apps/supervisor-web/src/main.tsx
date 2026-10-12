import WorkspaceEditorWorker from '@pockymoe/thread-ui/workspace-editor.worker?worker';
import React from 'react';
import { I18nProvider, initializeI18n } from '@pockymoe/thread-ui/i18n';
import ReactDOM from 'react-dom/client';
import 'streamdown/styles.css';

import { App } from './app';
import { SharingApprovalDialog } from './components/SharingApprovalDialog';
import './index.css';
import './native-subagents.css';
import { initializeFontSize } from './lib/fontSize';
import { initializeThemePreset } from './lib/themePreset';
import { requireTopLevelWeb } from './lib/staticWeb';

if (import.meta.env.VITE_POCKYMOE_STATIC_WEB === 'true') requireTopLevelWeb(window);

// The host bundler resolves the worker asset before dependency optimization.
Object.assign(window, { MonacoEnvironment: { getWorker: () => new WorkspaceEditorWorker() } });
initializeFontSize();
initializeThemePreset();
initializeI18n();

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <I18nProvider><App /><SharingApprovalDialog /></I18nProvider>
  </React.StrictMode>
);
