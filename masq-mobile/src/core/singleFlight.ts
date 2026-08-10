/** Coalesces overlapping calls so one native operation is active at a time. */
export class SingleFlight<T> {
  private active: Promise<T> | null = null;

  get isRunning() {
    return this.active !== null;
  }

  run(operation: () => Promise<T>): Promise<T> {
    if (this.active) {
      return this.active;
    }

    const started = Promise.resolve().then(operation);
    this.active = started;
    started.then(
      () => this.clear(started),
      () => this.clear(started),
    );
    return started;
  }

  whenIdle(): Promise<void> {
    const running = this.active;
    if (!running) {
      return Promise.resolve();
    }
    return running.then(
      () => undefined,
      () => undefined,
    );
  }

  private clear(completed: Promise<T>) {
    if (this.active === completed) {
      this.active = null;
    }
  }
}
