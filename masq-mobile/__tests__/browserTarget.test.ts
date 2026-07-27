import {
  browserTargetHostname,
  displayBrowserUrl,
  resolveBrowserUrlTarget,
} from '../src/core/browserTarget';

describe('ENS browser targets', () => {
  it('maps a normalized ENS name to the explicit HTTPS gateway', () => {
    const target = resolveBrowserUrlTarget(
      'https://privacy.eth/news?id=1#details',
    );

    expect(target).toEqual({
      displayUrl: 'https://privacy.eth/news?id=1#details',
      isEns: true,
      transportUrl: 'https://privacy.eth.limo/news?id=1#details',
    });
    expect(browserTargetHostname(target)).toBe('privacy.eth.limo');
  });

  it('supports normalized subnames and preserves an existing gateway URL', () => {
    expect(resolveBrowserUrlTarget('docs.project.eth/start')).toEqual({
      displayUrl: 'https://docs.project.eth/start',
      isEns: true,
      transportUrl: 'https://docs.project.eth.limo/start',
    });
    expect(
      resolveBrowserUrlTarget('https://docs.project.eth.limo/start'),
    ).toEqual({
      displayUrl: 'https://docs.project.eth/start',
      isEns: true,
      transportUrl: 'https://docs.project.eth.limo/start',
    });
  });

  it('keeps ordinary HTTPS websites unchanged', () => {
    expect(resolveBrowserUrlTarget('example.com')).toEqual({
      displayUrl: 'https://example.com/',
      isEns: false,
      transportUrl: 'https://example.com/',
    });
  });

  it.each([
    'https://-bad.eth/',
    'https://bad-.eth/',
    'https://bad..eth/',
    'https://user:secret@name.eth/',
    'https://name.eth:444/',
  ])('rejects unsafe or ambiguous ENS input %s', value => {
    expect(() => resolveBrowserUrlTarget(value)).toThrow(/ENS/);
  });

  it('shows the logical ENS name without changing unknown URLs', () => {
    expect(displayBrowserUrl('https://name.eth.limo/path')).toBe(
      'https://name.eth/path',
    );
    expect(displayBrowserUrl('not a url')).toBe('not a url');
  });
});
