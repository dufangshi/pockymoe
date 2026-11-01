import React from 'react';
import ReactDOM from 'react-dom/client';
import 'streamdown/styles.css';

import { App } from './app';
import './index.css';
import { initializeFontSize } from './lib/fontSize';

initializeFontSize();

async function mount() {
  if (import.meta.env.DEV && new URLSearchParams(location.search).get('desktopMockHost') === '1') {
    const { installMockHost } = await import('./desktop/devMockHost');
    installMockHost();
  }
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
}
void mount();
