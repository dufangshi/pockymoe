import { createRoot } from 'react-dom/client';
import { DeviceMonitor } from '../../src/components/DeviceMonitor';
import '../../src/lib/themePacks/anchor';
import '../../src/index.css';
import { applyTheme } from '../../src/lib/themePacks';
import '@pockymoe/thread-ui/styles.css';

const query = new URLSearchParams(location.search);
const mode = query.get('mode') ?? 'light';
document.documentElement.dataset.themeEffective = mode;
document.documentElement.style.colorScheme = mode;
void applyTheme(query.get('theme') ?? 'classic').then(() => createRoot(document.getElementById('root')!).render(
  <div className="thread-ui-shell" data-theme-effective={mode} data-theme-mode={mode}><DeviceMonitor /></div>,
));
