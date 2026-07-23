import { normalizeBrowserUrl } from './config';

export interface BrowserNavigationRequest {
  isTopFrame: boolean;
  navigationType?: string;
  url: string;
}

export type BrowserNavigationDecision =
  | { action: 'allow'; url: string }
  | { action: 'upgrade'; url: string }
  | { action: 'block'; message: string | null };

const INSECURE_FORM_NAVIGATIONS = new Set(['formsubmit', 'formresubmit']);

export function decideBrowserNavigation(
  request: BrowserNavigationRequest,
): BrowserNavigationDecision {
  try {
    return { action: 'allow', url: normalizeBrowserUrl(request.url) };
  } catch {
    // A top-level HTTP redirect can be retried as HTTPS without issuing the
    // plaintext request. Subframes and every other scheme remain blocked.
  }

  if (!request.isTopFrame) {
    return { action: 'block', message: null };
  }

  try {
    const candidate = new URL(request.url);
    if (candidate.protocol !== 'http:') {
      throw new Error('Unsupported browser scheme.');
    }
    if (INSECURE_FORM_NAVIGATIONS.has(request.navigationType ?? '')) {
      return {
        action: 'block',
        message: 'The browser blocked an insecure form submission.',
      };
    }
    return {
      action: 'upgrade',
      url: normalizeBrowserUrl(
        candidate.toString().replace(/^http:/i, 'https:'),
      ),
    };
  } catch {
    return {
      action: 'block',
      message: 'Only public HTTPS websites can be opened in this browser.',
    };
  }
}
