import { decideBrowserNavigation } from '../src/core/browserNavigation';

describe('fail-closed private browser navigation', () => {
  it('allows a public HTTPS navigation', () => {
    expect(
      decideBrowserNavigation({
        isTopFrame: true,
        navigationType: 'other',
        url: 'https://www.vrt.be/vrtnws/nl/',
      }),
    ).toEqual({ action: 'allow', url: 'https://www.vrt.be/vrtnws/nl/' });
  });

  it('upgrades a top-level HTTP redirect without allowing its plaintext request', () => {
    expect(
      decideBrowserNavigation({
        isTopFrame: true,
        navigationType: 'other',
        url: 'http://deredactie.be/',
      }),
    ).toEqual({ action: 'upgrade', url: 'https://deredactie.be/' });
  });

  it.each([
    'mailto:news@example.com',
    'tel:+3200000000',
    'masq-example://open',
    'file:///etc/passwd',
    // eslint-disable-next-line no-script-url -- verifies that script URLs are blocked
    'javascript:alert(1)',
    'https://127.0.0.1/',
    'http://169.254.169.254/',
  ])('blocks an external or local top-level destination %s', url => {
    expect(
      decideBrowserNavigation({
        isTopFrame: true,
        navigationType: 'click',
        url,
      }).action,
    ).toBe('block');
  });

  it('blocks insecure subframes without replacing the top-level page', () => {
    expect(
      decideBrowserNavigation({
        isTopFrame: false,
        navigationType: 'other',
        url: 'http://ads.example/',
      }),
    ).toEqual({ action: 'block', message: null });
  });

  it('does not rewrite an insecure form submission as a GET request', () => {
    expect(
      decideBrowserNavigation({
        isTopFrame: true,
        navigationType: 'formsubmit',
        url: 'http://example.com/session',
      }),
    ).toEqual({
      action: 'block',
      message: 'The browser blocked an insecure form submission.',
    });
  });

  it('uses routing-neutral copy for blocked non-HTTPS navigation', () => {
    const result = decideBrowserNavigation({
      isTopFrame: true,
      navigationType: 'click',
      url: 'mailto:news@example.com',
    });

    if (result.action !== 'block') {
      throw new Error('Expected non-HTTPS navigation to be blocked.');
    }
    expect(result).toEqual({
      action: 'block',
      message: 'Only public HTTPS websites can be opened in this browser.',
    });
    expect(result.message).not.toContain('MASQ');
  });
});
