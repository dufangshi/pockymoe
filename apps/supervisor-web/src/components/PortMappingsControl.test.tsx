import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PortMappingsControl, parseLocalPreviewLink } from './PortMappingsControl';
import { request } from '../lib/api';

vi.mock('../lib/api', () => ({ request: vi.fn(), ApiError: class extends Error {} }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.mocked(request).mockReset(); });

describe('device port mappings', () => {
  it('keeps the local path, query and fragment and rejects other origins/schemes', () => {
    expect(parseLocalPreviewLink('http://localhost:4013/a?q=1#b')).toEqual({ port: 4013, path: '/a?q=1#b' });
    expect(parseLocalPreviewLink('127.0.0.1:8080')).toEqual({ port: 8080, path: '/' });
    expect(parseLocalPreviewLink('http://[::1]:8080/')).toEqual({ port: 8080, path: '/' });
    for (const href of ['https://localhost:4013', 'http://localhost.evil:4013', 'http://user:pass@localhost:4013', 'http://localhost:0', 'http://192.168.1.1:4013', 'javascript:alert(1)', '/workspace/file']) {
      expect(parseLocalPreviewLink(href)).toBeNull();
    }
  });

  it('asks before enabling a chat link and uses the owner launch endpoint', async () => {
    const mapping = { id: 'a'.repeat(32), port: 4013, label: '', createdAt: new Date().toISOString() };
    vi.mocked(request).mockImplementation(async (url, init) => {
      if (String(url).endsWith('/config')) return { available: true };
      if (String(url).endsWith('/lease')) return { leaseId: 'test-lease', expiresAt: Date.now() + 10000 };
      if (String(url).includes('/open?')) return { url: 'https://p-preview.example/a?q=1#b' };
      if (init?.method === 'POST') return mapping;
      return { mappings: [] };
    });
    const replace = vi.fn();
    vi.spyOn(window, 'open').mockReturnValue({ opener: window, location: { replace }, close: vi.fn() } as unknown as Window);
    render(<><PortMappingsControl deviceId="device-1" /><div className="thread-timeline-surface"><a href="http://localhost:4013/a?q=1#b">Test website</a></div></>);
    fireEvent.click(screen.getByText('Test website'));
    expect(screen.getByRole('dialog', { name: 'Open device web service?' })).toBeVisible();
    expect(vi.mocked(request).mock.calls.some(([, init]) => init?.method === 'POST')).toBe(false);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Enable and open' })).toBeEnabled());
    fireEvent.click(screen.getByRole('button', { name: 'Enable and open' }));
    await waitFor(() => expect(replace).toHaveBeenCalledWith('https://p-preview.example/a?q=1#b'));
    expect(request).toHaveBeenCalledWith(`/relay/devices/device-1/api/port-mappings/${mapping.id}/lease`, expect.objectContaining({ method: 'POST' }));
    expect(request).toHaveBeenCalledWith(`/relay/devices/device-1/port-mappings/${mapping.id}/open?lease=test-lease`, expect.objectContaining({ method: 'POST', body: JSON.stringify({ path: '/a?q=1#b' }) }));
  });

  it('shows configuration failure without creating a mapping, and allows stopping an existing one', async () => {
    const mapping = { id: 'a'.repeat(32), port: 4013, label: 'Test website', createdAt: new Date().toISOString() };
    vi.mocked(request).mockImplementation(async (url, init) => String(url).endsWith('/config') ? { available: false } : init?.method === 'DELETE' ? {} : { mappings: [mapping] });
    render(<PortMappingsControl deviceId="device-1" />);
    fireEvent.click(screen.getByRole('button', { name: 'Port mappings' }));
    await screen.findByText('Port previews are not configured on this Relay yet.');
    expect(screen.getByRole('button', { name: 'Enable' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(screen.queryByText('Test website · 127.0.0.1:4013')).not.toBeInTheDocument());
    expect(request).toHaveBeenCalledWith(`/relay/devices/device-1/api/port-mappings/${mapping.id}`, expect.objectContaining({ method: 'DELETE' }));
  });
});
