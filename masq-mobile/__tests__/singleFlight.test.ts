import {SingleFlight} from '../src/core/singleFlight';

describe('SingleFlight', () => {
  it('coalesces overlapping operations onto the active promise', async () => {
    let resolve!: (value: number) => void;
    const pending = new Promise<number>(done => {
      resolve = done;
    });
    const operation = jest.fn(() => pending);
    const flight = new SingleFlight<number>();

    const first = flight.run(operation);
    const second = flight.run(operation);

    expect(first).toBe(second);
    expect(flight.isRunning).toBe(true);
    resolve(42);
    await expect(first).resolves.toBe(42);
    await Promise.resolve();
    expect(operation).toHaveBeenCalledTimes(1);
    expect(flight.isRunning).toBe(false);
  });

  it('allows a new operation after the previous rejection', async () => {
    const flight = new SingleFlight<number>();
    const failure = new Error('failed');

    await expect(flight.run(async () => Promise.reject(failure))).rejects.toBe(
      failure,
    );
    await Promise.resolve();
    await expect(flight.run(async () => 7)).resolves.toBe(7);
  });

  it('waits until the active operation settles successfully', async () => {
    let resolve!: (value: number) => void;
    const pending = new Promise<number>(done => {
      resolve = done;
    });
    const flight = new SingleFlight<number>();
    flight.run(() => pending);
    const idle = flight.whenIdle();
    const observer = jest.fn();
    idle.then(observer);

    await Promise.resolve();
    expect(observer).not.toHaveBeenCalled();
    resolve(42);
    await expect(idle).resolves.toBeUndefined();
    expect(flight.isRunning).toBe(false);
  });

  it('becomes idle after the active operation rejects', async () => {
    let rejectFirst!: (reason: Error) => void;
    const first = new Promise<number>((_resolve, reject) => {
      rejectFirst = reject;
    });
    const flight = new SingleFlight<number>();
    const failed = flight.run(() => first);
    const idle = flight.whenIdle();

    rejectFirst(new Error('cancelled'));
    await expect(failed).rejects.toThrow('cancelled');
    await expect(idle).resolves.toBeUndefined();
    expect(flight.isRunning).toBe(false);
  });
});
