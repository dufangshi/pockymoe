/** Judge a socket by a foreground probe, not the age of a suspended tab's pong. */
export const SUPERVISOR_HEARTBEAT_TIMEOUT_MS = 30_000;

export class SupervisorHeartbeat {
  private deadline: number | null = null;
  private lastCheck: number | null = null;
  private receivedResponse = false;

  reset() {
    this.deadline = this.lastCheck = null;
    this.receivedResponse = false;
  }

  probe(now: number) {
    this.deadline = now + SUPERVISOR_HEARTBEAT_TIMEOUT_MS;
    this.lastCheck = now;
  }

  receive(now: number) {
    this.receivedResponse = true;
    this.probe(now);
  }

  timedOut(now: number) {
    // Mobile browsers can freeze timers without delivering visibilitychange.
    if (this.deadline === null || (this.lastCheck !== null && now - this.lastCheck > SUPERVISOR_HEARTBEAT_TIMEOUT_MS)) {
      this.probe(now);
    }
    this.lastCheck = now;
    return now >= this.deadline!;
  }

  responsive(now: number) {
    return this.receivedResponse && this.deadline !== null && now < this.deadline;
  }
}
