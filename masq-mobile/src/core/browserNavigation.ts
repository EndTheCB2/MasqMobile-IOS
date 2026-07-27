import { normalizeBrowserUrl } from './config';
import { resolveBrowserUrlTarget } from './browserTarget';

export interface BrowserNavigationRequest {
  isTopFrame: boolean;
  navigationType?: string;
  url: string;
}

export type BrowserNavigationDecision =
  | { action: 'allow'; url: string }
  | { action: 'upgrade'; url: string }
  | { action: 'redirect'; displayUrl: string; url: string }
  | { action: 'block'; message: string | null };

const INSECURE_FORM_NAVIGATIONS = new Set(['formsubmit', 'formresubmit']);
const FORM_NAVIGATIONS = INSECURE_FORM_NAVIGATIONS;

export function decideBrowserNavigation(
  request: BrowserNavigationRequest,
): BrowserNavigationDecision {
  try {
    const normalized = normalizeBrowserUrl(request.url);
    const target = resolveBrowserUrlTarget(normalized);
    if (target.transportUrl !== normalized) {
      if (!request.isTopFrame) {
        return { action: 'block', message: null };
      }
      if (FORM_NAVIGATIONS.has(request.navigationType ?? '')) {
        return {
          action: 'block',
          message:
            'The browser blocked an ENS form submission because rewriting it through the gateway cannot preserve its submitted data.',
        };
      }
      return {
        action: 'redirect',
        displayUrl: target.displayUrl,
        url: target.transportUrl,
      };
    }
    return { action: 'allow', url: target.transportUrl };
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
    const target = resolveBrowserUrlTarget(
      candidate.toString().replace(/^http:/i, 'https:'),
    );
    return target.isEns
      ? {
          action: 'redirect',
          displayUrl: target.displayUrl,
          url: target.transportUrl,
        }
      : { action: 'upgrade', url: target.transportUrl };
  } catch {
    return {
      action: 'block',
      message: 'Only public HTTPS websites can be opened in this browser.',
    };
  }
}
