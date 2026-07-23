import {decideBrowserRecovery} from '../src/core/browserRecovery';

describe('private browser recovery', () => {
  it.each([
    [-1005, 'NSURLErrorDomain'],
    [-1001, 'NSURLErrorDomain'],
    [-8, 'android.webkit.WebViewClient'],
    [-6, 'android.webkit.WebViewClient'],
  ])('retries transient error %s on %s', (code, domain) => {
    const first = decideBrowserRecovery(
      {code, description: 'connection interrupted', domain},
      0,
      2,
    );
    const second = decideBrowserRecovery(
      {code, description: 'connection interrupted', domain},
      1,
      2,
    );

    expect(first).toMatchObject({retry: true, nextAttempt: 1, delayMs: 600});
    expect(second).toMatchObject({retry: true, nextAttempt: 2, delayMs: 1200});
  });

  it('stops after the bounded retry window', () => {
    const result = decideBrowserRecovery(
      {code: -1005, description: 'lost', domain: 'NSURLErrorDomain'},
      2,
      2,
    );

    expect(result).toMatchObject({retry: false, delayMs: null, nextAttempt: 2});
    expect(result.message).toContain('automatic recovery');
  });

  it('uses normal-connection recovery copy in direct mode', () => {
    const transient = decideBrowserRecovery(
      {code: -1005, description: 'lost', domain: 'NSURLErrorDomain'},
      0,
      2,
      'direct',
    );
    const exhausted = decideBrowserRecovery(
      {code: -1005, description: 'lost', domain: 'NSURLErrorDomain'},
      2,
      2,
      'direct',
    );

    expect(transient.message).toContain('normal internet connection');
    expect(transient.message).toContain('Retrying directly');
    expect(transient.message).not.toContain('private route');
    expect(exhausted.message).toContain('direct connection');
    expect(exhausted.message).not.toContain('Reconnect MASQ');
  });

  it('waits for user action when the device is offline', () => {
    const result = decideBrowserRecovery(
      {code: -1009, description: 'offline', domain: 'NSURLErrorDomain'},
      0,
      2,
    );

    expect(result).toMatchObject({retry: false, delayMs: null});
    expect(result.message).toContain('offline');
  });

  it('never automatically retries certificate failures', () => {
    const result = decideBrowserRecovery(
      {code: -1202, description: 'untrusted certificate', domain: 'NSURLErrorDomain'},
      0,
      2,
    );

    expect(result).toMatchObject({retry: false, delayMs: null});
    expect(result.message).toContain('trusted HTTPS');
  });

  it('does not expose native descriptions or domains in user messages', () => {
    const result = decideBrowserRecovery(
      {code: -42, description: 'secret.internal.example', domain: 'PrivateDomain'},
      0,
      2,
    );

    expect(result.message).not.toContain('secret.internal.example');
    expect(result.message).not.toContain('PrivateDomain');
  });
});
