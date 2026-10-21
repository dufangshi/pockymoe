import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { Link, MemoryRouter } from 'react-router-dom';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { RuntimeManagement } from './RuntimeManagement';

const api = vi.hoisted(() => ({ request: vi.fn(), relay: true }));
vi.mock('../lib/api', async (original) => ({
  ...(await original<typeof import('../lib/api')>()),
  request: api.request,
  relayModeActive: () => api.relay,
}));
function mount(path: string) {
  return render(<MemoryRouter initialEntries={[path]}>
    <Link to="/devices/b/workspaces">Device B</Link>
    <Link to="/relay-devices">Devices</Link>
    <RuntimeManagement />
  </MemoryRouter>);
}
beforeEach(() => {
  api.relay = true;
  api.request.mockReset();
  localStorage.setItem('remote-codex-relay-device-id', 'remembered-device');
  api.request.mockImplementation(async (path: string) => path.endsWith('/harnesses') ? [] : {
    runningVersion: path.includes('/devices/b/') ? 'B-version' : 'A-version', canUpdate: true,
  });
});

it('distinguishes a prolonged restart disconnect and recovers without another click', async () => {
  let offline = false, completed = false;
  api.request.mockImplementation(async (path: string) => {
    if (offline) throw new Error('Device offline');
    if (path.endsWith('/harnesses')) return [];
    if (path.endsWith('/jobs')) return {};
    return { runningVersion: '0.12.38', canUpdate: true, job: { phase: completed ? 'completed' : 'restarting', action: 'update' } };
  });
  const view = mount('/devices/a/workspaces');
  await screen.findByText(/Restarting Supervisor/);
  vi.useFakeTimers();
  try {
    // The component's interval was created before fake timers; remount under them.
    view.unmount();
    const next = mount('/devices/a/workspaces');
    await act(async () => {});
    offline = true;
    await act(async () => { await vi.advanceTimersByTimeAsync(35_000); });
    expect(screen.getByText(/Waiting for the device to reconnect/)).toBeVisible();
    expect(screen.getByText(/has not returned yet/)).toBeVisible();
    completed = true; offline = false;
    await act(async () => { await vi.advanceTimersByTimeAsync(2500); });
    expect(screen.getByText(/Update completed/)).toBeVisible();
    expect(screen.queryByText(/Waiting for the device/)).not.toBeInTheDocument();
    next.unmount();
  } finally { vi.useRealTimers(); }
});

it('offers the native migration on the last npm release even when it matches latest', async () => {
  api.request.mockImplementation(async (path: string) => path.endsWith('/harnesses') ? [] : {
    runningVersion: '0.12.77', installedVersion: '0.12.77', latestVersion: '0.12.77', canUpdate: true,
    canRestart: false, nativeMigration: true,
    reason: 'This device still runs from the retired npm package. Update installs the native GitHub runtime and moves its service to it.',
  });
  mount('/devices/a/workspaces');
  expect(await screen.findByRole('button', { name: 'Update' })).toBeEnabled();
  expect(screen.getByText(/retired npm package/)).toBeVisible();
  expect(screen.queryByRole('button', { name: /Restart Supervisor/ })).not.toBeInTheDocument();
});

it('offers repair when npm is old but the running process already matches latest', async () => {
  api.request.mockImplementation(async (path: string) => path.endsWith('/harnesses') ? [] : {
    runningVersion: '0.12.32', installedVersion: '0.12.30', latestVersion: '0.12.32', canUpdate: true,
  });
  mount('/devices/a/workspaces');
  expect(await screen.findByText(/Installed 0.12.30; running 0.12.32/)).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Update' }));
  fireEvent.click(screen.getByRole('button', { name: 'Update' }));
  await waitFor(() => expect(api.request).toHaveBeenCalledWith('relay/devices/a/api/management/supervisor/update', expect.objectContaining({method:'POST'})));
});
describe('device-scoped runtime settings', () => {
  it('pins requests to the route and discards old device state and late responses', async () => {
    let resolveOld!: (value: unknown) => void;
    api.request.mockImplementation((path: string) => {
      if (path === 'relay/devices/a/api/management/supervisor') return new Promise(resolve => { resolveOld = resolve; });
      return Promise.resolve(path.endsWith('/harnesses') ? [] : {runningVersion:'B-version', canUpdate:true});
    });
    mount('/devices/a/threads/thread-a');
    await waitFor(() => expect(api.request).toHaveBeenCalledWith('relay/devices/a/api/management/harnesses', expect.anything()));
    fireEvent.click(screen.getByRole('link', {name:'Device B'}));
    expect(await screen.findByText('Running B-version')).toBeVisible();
    await act(async () => resolveOld({runningVersion:'OLD-A-version',canUpdate:true}));
    expect(screen.queryByText(/OLD-A-version/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', {name:'Check updates'}));
    await waitFor(() => expect(api.request).toHaveBeenCalledWith('relay/devices/b/api/management/supervisor/check', expect.objectContaining({method:'POST'})));
    fireEvent.click(screen.getByRole('link', {name:'Devices'}));
    expect(screen.getByText(/Open a device to view/)).toBeVisible();
    expect(screen.queryByText('Running B-version')).not.toBeInTheDocument();
    expect(api.request.mock.calls.every(([path]) => String(path).startsWith('relay/devices/a/') || String(path).startsWith('relay/devices/b/'))).toBe(true);
  });
});
it('shared device access exposes no restart or update controls', async () => {
  const { ApiError } = await import('../lib/api');
  api.request.mockRejectedValue(new ApiError(403,{code:'forbidden',message:'Owner only'}));
  mount('/devices/a/threads/shared');
  expect(await screen.findByText(/Only the device owner/)).toBeVisible();
  expect(screen.queryByRole('button',{name:/Restart|Check updates|Update/})).not.toBeInTheDocument();
  expect(api.request.mock.calls.every(([,options])=>options.method!=='POST')).toBe(true);
});

it('shows a missing adapter and its install action after a successful base update', async () => {
  api.request.mockImplementation((path: string) => Promise.resolve(path.endsWith('/harnesses') ? [{
    id:'codex', name:'OpenAI Codex', transport:'adapter',
    base:{path:'/usr/local/bin/codex',resolvedPath:'/usr/local/bin/codex',version:'0.154.0',manager:'npm',canUpdate:true},
    adapter:{installed:false,canInstall:true,canUpdate:false,reason:'Executable not found: codex-acp',updateCommand:'npm install --prefix /home/u/.local/share/remote-codex/adapters'},
    job:{state:'completed',action:'update'},
  }] : {}));
  mount('/devices/a/threads/thread-a');
  expect(await screen.findByText('ACP adapter required')).toBeVisible();
  expect(screen.getByText('0.154.0 · npm global package')).toBeVisible();
  expect(screen.getByText(/Not installed/)).toBeVisible();
  expect(screen.queryByText(/^Ready/)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button',{name:'Install OpenAI Codex adapter'}));
  fireEvent.click(screen.getByRole('button',{name:/^Install$/}));
  await waitFor(()=>expect(api.request).toHaveBeenCalledWith('relay/devices/a/api/management/harnesses/codex',expect.objectContaining({method:'POST',body:JSON.stringify({action:'update',component:'adapter'})})));
});

it('keeps the repair entry visible for an older device returning adapter null', async () => {
  api.request.mockImplementation((path: string) => Promise.resolve(path.endsWith('/harnesses') ? [{id:'codex',name:'OpenAI Codex',transport:'adapter',base:{path:'/usr/bin/codex',resolvedPath:'/usr/bin/codex',version:'0.150.0',manager:'npm',canUpdate:true},adapter:null}] : {}));
  mount('/devices/a/threads/thread-a');
  expect(await screen.findByText(/Adapter not detected/)).toBeVisible();
  const install=await screen.findByRole('button',{name:'Install OpenAI Codex adapter'});
  expect(install).toBeVisible();
  fireEvent.click(install);
  fireEvent.click(screen.getByRole('button',{name:/^Install$/}));
  await waitFor(()=>expect(api.request).toHaveBeenCalledWith('relay/devices/a/api/agent-runtimes/acp/install?agentId=codex',expect.objectContaining({method:'POST'})));
});

it('explains a command missing from PATH and blocks the adapter until it is installed', async () => {
  api.request.mockImplementation((path: string) => Promise.resolve(path.endsWith('/harnesses') ? [{
    id:'codex', name:'OpenAI Codex', transport:'adapter', baseCommand:'codex', adapterCommand:'codex-acp',
    base:{installed:false,canInstall:true,canUpdate:false,path:'',resolvedPath:'',manager:'pockymoe',reason:''},
    adapter:{installed:true,canInstall:true,canUpdate:true,version:'2.1.1',manager:'pockymoe',path:'/home/u/.local/share/remote-codex/adapters/bin/codex-acp',resolvedPath:'/x'},
    job:{state:'failed',action:'update',component:'adapter',error:'Install the base agent first: codex'},
  }] : {}));
  mount('/devices/a/threads/thread-a');
  const components = await screen.findByRole('list', {name:'OpenAI Codex components'});
  expect(components).toHaveTextContent('Command-line toolcodex');
  expect(components).toHaveTextContent('2.1.1 · Managed by Pockymoe');
  expect(screen.getByText(/cannot find codex on its PATH/)).toBeVisible();
  // The adapter's Update stays greyed; the mark after it says why.
  expect(screen.getByRole('button',{name:'Update OpenAI Codex adapter'})).toBeDisabled();
  fireEvent.click(within(components).getByRole('button',{name:'Why it is unavailable'}));
  expect(screen.getByRole('dialog',{name:'Why it is unavailable'})).toHaveTextContent('Install the command-line tool first.');
  expect(screen.getByRole('button',{name:'Install OpenAI Codex'})).toBeEnabled();
  expect(screen.queryByRole('button',{name:'Restart OpenAI Codex'})).not.toBeInTheDocument();
  expect(screen.getByText('Last operation failed: Install the base agent first: codex')).toHaveAttribute('role','alert');
  expect(screen.getByRole('button',{name:'OpenAI Codex'})).toHaveAttribute('title','Not installed');
});

it('installs DSH as one managed package with no separate adapter', async () => {
  api.request.mockImplementation((path: string) => Promise.resolve(path.endsWith('/harnesses') ? [{
    id:'deepseek', name:'DeepSeek Harness', transport:'native', baseCommand:'dsh',
    base:{installed:false,canInstall:true,canUpdate:false,path:'',resolvedPath:'',manager:'pockymoe',reason:''},
    adapter:null,
  }] : {}));
  mount('/devices/a/threads/thread-a');
  fireEvent.click(await screen.findByRole('button',{name:'DeepSeek Harness'}));
  const install = await screen.findByRole('button',{name:'Install DeepSeek Harness'});
  expect(install).toBeEnabled();
  expect(screen.queryByRole('button',{name:/DeepSeek Harness adapter/})).not.toBeInTheDocument();
  fireEvent.click(install);
  fireEvent.click(screen.getByRole('button',{name:/^Install$/}));
  await waitFor(()=>expect(api.request).toHaveBeenCalledWith('relay/devices/a/api/management/harnesses/deepseek',expect.objectContaining({method:'POST',body:JSON.stringify({action:'install',component:'base'})})));
});

it('lets a pockymoe-managed DSH update in place', async () => {
  api.request.mockImplementation((path: string) => Promise.resolve(path.endsWith('/harnesses') ? [{
    id:'deepseek', name:'DeepSeek Harness', transport:'native', baseCommand:'dsh',
    base:{path:'/home/u/.local/share/remote-codex/harnesses/bin/dsh',resolvedPath:'/home/u/.local/share/remote-codex/harnesses/lib/node_modules/@deepseek-ai/dsh/lib/bin.js',version:'0.2.1-rc.2',manager:'pockymoe',canUpdate:true,updateCommand:'npm install --global --prefix /home/u/.local/share/remote-codex/harnesses @deepseek-ai/dsh@latest --no-audit --no-fund'},
    adapter:null,
  }] : {}));
  mount('/devices/a/threads/thread-a');
  fireEvent.click(await screen.findByRole('button',{name:'DeepSeek Harness'}));
  const components = await screen.findByRole('list', {name:'DeepSeek Harness components'});
  expect(components).toHaveTextContent('0.2.1-rc.2 · Managed by Pockymoe');
  expect(screen.getByRole('button',{name:'Update DeepSeek Harness'})).toBeEnabled();
});
