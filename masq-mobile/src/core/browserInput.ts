import { resolveBrowserUrlTarget, type BrowserTarget } from './browserTarget';

export const TIMPI_HOME_URL = 'https://timpi.com/';
export const TIMPI_SEARCH_URL = 'https://timpi.com/search';
export const DUCKDUCKGO_HOME_URL = 'https://duckduckgo.com/';
export const DUCKDUCKGO_SEARCH_URL = 'https://duckduckgo.com/';
export const DEFAULT_BROWSER_SEARCH_PROVIDER = 'timpi' as const;
export type BrowserSearchProvider = 'timpi' | 'duckduckgo';

const NON_SEARCH_SCHEME_PATTERN =
  /^(?:about|blob|content|data|file|ftp|http|https|intent|javascript|mailto|tel|ws|wss):/i;
const CUSTOM_SCHEME_PATTERN = /^[a-zA-Z][a-zA-Z\d+.-]*:\/\//;
const DOMAIN_DOT_VARIANTS_PATTERN = /[\u3002\uFF0E\uFF61]/g;

export function browserSearchProviderName(
  provider: BrowserSearchProvider,
): string {
  return provider === 'duckduckgo' ? 'DuckDuckGo' : 'Timpi';
}

export function resolveBrowserInput(
  input: string,
  searchProvider: BrowserSearchProvider = DEFAULT_BROWSER_SEARCH_PROVIDER,
): string {
  return resolveBrowserInputTarget(input, searchProvider).transportUrl;
}

export function resolveBrowserInputTarget(
  input: string,
  searchProvider: BrowserSearchProvider = DEFAULT_BROWSER_SEARCH_PROVIDER,
): BrowserTarget {
  const candidate = normalizeBrowserInputCandidate(input);
  if (!candidate) {
    throw new Error('Enter a web address or search term.');
  }

  if (looksLikeWebAddress(candidate)) {
    return resolveBrowserUrlTarget(candidate);
  }

  const encodedQuery = encodeURIComponent(candidate);
  const searchUrl =
    searchProvider === 'duckduckgo'
      ? `${DUCKDUCKGO_SEARCH_URL}?q=${encodedQuery}`
      : `${TIMPI_SEARCH_URL}?q=${encodedQuery}`;
  return {
    displayUrl: searchUrl,
    isEns: false,
    transportUrl: searchUrl,
  };
}

function normalizeBrowserInputCandidate(input: string): string {
  return input.trim().replace(DOMAIN_DOT_VARIANTS_PATTERN, '.');
}

function looksLikeWebAddress(candidate: string): boolean {
  if (
    NON_SEARCH_SCHEME_PATTERN.test(candidate) ||
    CUSTOM_SCHEME_PATTERN.test(candidate)
  ) {
    return true;
  }

  const separatorIndex = candidate.search(/[/?#]/);
  const authority =
    separatorIndex === -1 ? candidate : candidate.slice(0, separatorIndex);
  if (!authority || /[\s@]/.test(authority)) {
    return false;
  }

  if (authority.startsWith('[')) {
    return true;
  }

  const host = authority.replace(/:\d+$/, '').toLowerCase();
  return (
    host === 'localhost' ||
    host.endsWith('.local') ||
    host.includes('.') ||
    /^(?:\d{1,3}\.){3}\d{1,3}$/.test(host)
  );
}
