import { afterEach, describe, expect, it, vi } from 'vitest';
import { notifyOwnerUnlocked, readAfterOwnerUnlock } from './ownerUnlockRecovery';
afterEach(() => vi.restoreAllMocks());
describe('device-scoped owner unlock read recovery', () => {
  it.each(['owner_session_required', 'owner_session_revoked', 'device_strict_mode'])('waits after %s and resumes only for the matching device', async (code) => {
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
    fail({ code: 'owner_session_required' });
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
    const read = vi.fn().mockRejectedValue({ code: 'owner_session_required' });
    const pending = readAfterOwnerUnlock('a', read, controller.signal);
    await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' });
    notifyOwnerUnlocked(['a']);
    expect(read).toHaveBeenCalledTimes(1);
    expect(remove).toHaveBeenCalledWith('remote-codex-owner-unlocked', expect.any(Function));
  });
});
