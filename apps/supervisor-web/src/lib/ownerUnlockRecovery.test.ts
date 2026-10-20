import { afterEach, describe, expect, it, vi } from 'vitest';
import { currentBrowser } from './accountVaultState';
import { notifyOwnerUnlocked, OWNER_READ_BLOCKED_EVENT, readAfterOwnerUnlock } from './ownerUnlockRecovery';
vi.mock('./accountVaultState', () => ({ currentBrowser: vi.fn(async () => null) }));
afterEach(() => { vi.restoreAllMocks(); vi.mocked(currentBrowser).mockResolvedValue(null); });
describe('device-scoped owner unlock read recovery', () => {
  it.each(['browser_registration_required', 'browser_revoked', 'device_strict_mode'])('waits after %s and resumes only for the matching device', async (code) => {
    const read = vi.fn().mockResolvedValueOnce({ response: Response.json({ code }, { status: 401 }) })
      .mockResolvedValue({ response: Response.json({ ok: true }) });
    const pending = readAfterOwnerUnlock('a', read);
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    notifyOwnerUnlocked(['b']);
    expect(read).toHaveBeenCalledTimes(1);
    notifyOwnerUnlocked(['a']);
    expect((await pending).response.ok).toBe(true);
    expect(read).toHaveBeenCalledTimes(2);
    notifyOwnerUnlocked(['a']);
    expect(read).toHaveBeenCalledTimes(2);
  });
  it('recovers exceptions thrown before sending and does not lose an unlock while the failure is in flight', async () => {
    let fail!: (reason: unknown) => void;
    const read = vi.fn().mockImplementationOnce(() => new Promise((_resolve, reject) => { fail = reject; }))
      .mockResolvedValue({ response: Response.json({ ok: true }) });
    const pending = readAfterOwnerUnlock('a', read);
    notifyOwnerUnlocked(['a']);
    fail({ code: 'browser_registration_required' });
    expect((await pending).response.ok).toBe(true);
    expect(read).toHaveBeenCalledTimes(2);
  });
  it('does not hide other errors or automatically repeat completed reads', async () => {
    const read = vi.fn().mockResolvedValue({ response: Response.json({ code: 'not_found' }, { status: 404 }) });
    expect((await readAfterOwnerUnlock('a', read)).response.status).toBe(404);
    notifyOwnerUnlocked(['a']);
    expect(read).toHaveBeenCalledTimes(1);
    read.mockRejectedValue(new Error('offline'));
    await expect(readAfterOwnerUnlock('a', read)).rejects.toThrow('offline');
  });
  it('cancels a waiting read and removes its event subscription', async () => {
    const controller = new AbortController();
    const remove = vi.spyOn(window, 'removeEventListener');
    const read = vi.fn().mockRejectedValue({ code: 'browser_registration_required' });
    const pending = readAfterOwnerUnlock('a', read, controller.signal);
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' });
    notifyOwnerUnlocked(['a']);
    expect(read).toHaveBeenCalledTimes(1);
    expect(remove).toHaveBeenCalledWith('remote-codex-owner-unlocked', expect.any(Function));
  });
  describe('when this browser still has its key', () => {
    it.each(['browser_registration_required', 'device_strict_mode'])('retries %s once by itself and never asks for the password', async (code) => {
      vi.mocked(currentBrowser).mockResolvedValue({ browserId: 'b' } as never);
      const blocked = vi.fn();
      window.addEventListener(OWNER_READ_BLOCKED_EVENT, blocked);
      const read = vi.fn().mockResolvedValue({ response: Response.json({ code }, { status: 401 }) });
      expect((await readAfterOwnerUnlock('a', read)).response.status).toBe(401);
      expect(read).toHaveBeenCalledTimes(2);
      read.mockResolvedValueOnce({ response: Response.json({ code }, { status: 401 }) }).mockResolvedValueOnce({ response: Response.json({ ok: true }) });
      expect((await readAfterOwnerUnlock('a', read)).response.ok).toBe(true);
      read.mockRejectedValue({ code });
      await expect(readAfterOwnerUnlock('a', read)).rejects.toMatchObject({ code });
      expect(blocked).not.toHaveBeenCalled();
      window.removeEventListener(OWNER_READ_BLOCKED_EVENT, blocked);
    });
    it('still asks after a removed browser', async () => {
      vi.mocked(currentBrowser).mockResolvedValue({ browserId: 'b' } as never);
      const read = vi.fn().mockResolvedValueOnce({ response: Response.json({ code: 'browser_revoked' }, { status: 401 }) })
        .mockResolvedValue({ response: Response.json({ ok: true }) });
      const pending = readAfterOwnerUnlock('a', read);
      await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
      notifyOwnerUnlocked(['a']);
      expect((await pending).response.ok).toBe(true);
    });
  });
});
