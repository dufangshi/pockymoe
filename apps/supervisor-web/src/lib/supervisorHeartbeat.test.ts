import { describe, expect, it } from 'vitest';
import { SupervisorHeartbeat } from './supervisorHeartbeat';

describe('foreground Supervisor heartbeat', () => {
  it('tolerates delayed replies but reconnects a genuinely unresponsive socket', () => {
    const heartbeat = new SupervisorHeartbeat();
    heartbeat.receive(0);
    for (let now = 2000; now <= 28_000; now += 2000) expect(heartbeat.timedOut(now)).toBe(false);
    expect(heartbeat.responsive(6000)).toBe(true);
    expect(heartbeat.timedOut(30_000)).toBe(true);
    expect(heartbeat.responsive(30_000)).toBe(false);
    heartbeat.receive(31_000);
    expect(heartbeat.responsive(31_000)).toBe(true);
  });
  it('probes after suspension or foregrounding before declaring the socket dead', () => {
    const heartbeat = new SupervisorHeartbeat();
    heartbeat.receive(0);
    expect(heartbeat.timedOut(90_000)).toBe(false);
    expect(heartbeat.responsive(90_000)).toBe(true);
    heartbeat.probe(100_000);
    for (let now = 102_000; now < 130_000; now += 2000) expect(heartbeat.timedOut(now)).toBe(false);
    expect(heartbeat.timedOut(130_000)).toBe(true);
  });
  it('also times out a socket that has never returned any pong', () => {
    const heartbeat = new SupervisorHeartbeat();
    heartbeat.probe(0);
    expect(heartbeat.responsive(0)).toBe(false);
    for (let now = 2000; now < 30_000; now += 2000) expect(heartbeat.timedOut(now)).toBe(false);
    expect(heartbeat.timedOut(30_000)).toBe(true);
    heartbeat.reset();
    expect(heartbeat.timedOut(40_000)).toBe(false);
    expect(heartbeat.responsive(40_000)).toBe(false);
  });
});
