import { normalizeBrowserUrl } from './config';

export const TIMPI_HOME_URL = 'https://timpi.com/';
export const TIMPI_SEARCH_URL = 'https://timpi.com/search';

const NON_SEARCH_SCHEME_PATTERN =
  /^(?:about|blob|content|data|file|ftp|http|https|intent|javascript|mailto|tel|ws|wss):/i;
const CUSTOM_SCHEME_PATTERN = /^[a-zA-Z][a-zA-Z\d+.-]*:\/\//;

export function resolveBrowserInput(input: string): string {
  const candidate = input.trim();
  if (!candidate) {
    throw new Error('Enter a web address or search term.');
  }

  if (looksLikeWebAddress(candidate)) {
    return normalizeBrowserUrl(candidate);
  }

  return `${TIMPI_SEARCH_URL}?q=${encodeURIComponent(candidate)}`;
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
