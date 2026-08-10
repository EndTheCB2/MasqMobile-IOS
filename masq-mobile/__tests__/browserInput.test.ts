import {
  DUCKDUCKGO_HOME_URL,
  DUCKDUCKGO_SEARCH_URL,
  TIMPI_HOME_URL,
  TIMPI_SEARCH_URL,
  browserSearchProviderName,
  resolveBrowserInput,
} from '../src/core/browserInput';

describe('browser address and search-provider input', () => {
  it('uses the public Timpi web search for free text', () => {
    expect(resolveBrowserInput('private mobile browser')).toBe(
      'https://timpi.com/search?q=private%20mobile%20browser',
    );
    expect(resolveBrowserInput('weather')).toBe(
      'https://timpi.com/search?q=weather',
    );
    expect(resolveBrowserInput('site:masq.ai mobile')).toBe(
      'https://timpi.com/search?q=site%3Amasq.ai%20mobile',
    );
    expect(resolveBrowserInput('privacy & vrijheid')).toBe(
      'https://timpi.com/search?q=privacy%20%26%20vrijheid',
    );
    expect(resolveBrowserInput('décentralisé')).toBe(
      'https://timpi.com/search?q=d%C3%A9centralis%C3%A9',
    );
  });

  it('uses DuckDuckGo when it is the selected search engine', () => {
    expect(resolveBrowserInput('private mobile browser', 'duckduckgo')).toBe(
      'https://duckduckgo.com/?q=private%20mobile%20browser',
    );
    expect(resolveBrowserInput('privacy & vrijheid', 'duckduckgo')).toBe(
      'https://duckduckgo.com/?q=privacy%20%26%20vrijheid',
    );
    expect(DUCKDUCKGO_SEARCH_URL).toBe(DUCKDUCKGO_HOME_URL);
    expect(browserSearchProviderName('duckduckgo')).toBe('DuckDuckGo');
    expect(browserSearchProviderName('timpi')).toBe('Timpi');
  });

  it('keeps public web addresses as HTTPS navigation', () => {
    expect(resolveBrowserInput('example.com')).toBe('https://example.com/');
    expect(resolveBrowserInput('voorbeeld.com')).toBe('https://voorbeeld.com/');
    expect(resolveBrowserInput('nieuws.voorbeeld.com/pad?q=1#artikel')).toBe(
      'https://nieuws.voorbeeld.com/pad?q=1#artikel',
    );
    expect(resolveBrowserInput('www.deredactie.be/news')).toBe(
      'https://www.deredactie.be/news',
    );
    expect(resolveBrowserInput('https://timpi.com/')).toBe(TIMPI_HOME_URL);
    expect(resolveBrowserInput('voorbeeld.com', 'duckduckgo')).toBe(
      'https://voorbeeld.com/',
    );
  });

  it.each(['voorbeeld。com', 'voorbeeld．com', 'voorbeeld｡com'])(
    'normalizes a mobile keyboard dot in the bare domain %s',
    value => {
      expect(resolveBrowserInput(value)).toBe('https://voorbeeld.com/');
    },
  );

  it('loads explicit ENS websites through the HTTPS gateway', () => {
    expect(resolveBrowserInput('privacy.eth')).toBe(
      'https://privacy.eth.limo/',
    );
    expect(resolveBrowserInput('docs.project.eth/path?q=1#intro')).toBe(
      'https://docs.project.eth.limo/path?q=1#intro',
    );
    expect(resolveBrowserInput('privacy.eth', 'duckduckgo')).toBe(
      'https://privacy.eth.limo/',
    );
  });

  it.each([
    'http://example.com',
    'file:///etc/passwd',
    // eslint-disable-next-line no-script-url -- verifies that script URLs are blocked
    'javascript:alert(1)',
    'https://127.0.0.1',
    'localhost',
    'https://[fd00::1]',
    'masq-example://open',
  ])('does not turn an unsafe address into a search for %s', value => {
    expect(() => resolveBrowserInput(value)).toThrow();
  });

  it('requires input and keeps the provider URL centralized', () => {
    expect(() => resolveBrowserInput('   ')).toThrow(
      'Enter a web address or search term.',
    );
    expect(TIMPI_SEARCH_URL).toBe('https://timpi.com/search');
  });
});
