import {isDescriptorForChain} from './config';
import type {Chain} from './types';

export const MASQ_PUBLIC_SUBURB = 'masqpublic1';

interface DiscoveryOptions {
  baseUrl: string;
  count?: number;
  fetchImpl?: typeof fetch;
  maxAttempts?: number;
  signal?: AbortSignal;
  timeoutMs?: number;
}

export async function discoverEntryNodes(
  chain: Chain,
  options: DiscoveryOptions,
): Promise<string[]> {
  const count = options.count ?? 2;
  const fetchImpl = options.fetchImpl ?? fetch;
  const maxAttempts = Math.max(count, options.maxAttempts ?? 6);
  const timeoutMs = options.timeoutMs ?? 8000;
  const baseUrl = normalizeNodeFinderBaseUrl(options.baseUrl);
  const nodes = new Set<string>();

  for (let attempt = 0; attempt < maxAttempts && nodes.size < count; attempt += 2) {
    throwIfAborted(options.signal);
    const batchSize = Math.min(2, maxAttempts - attempt);
    const results = await Promise.allSettled(
      Array.from({length: batchSize}, () =>
        fetchEntryNode(chain, baseUrl, fetchImpl, timeoutMs, options.signal),
      ),
    );

    for (const result of results) {
      if (
        result.status === 'fulfilled' &&
        isDescriptorForChain(result.value, chain)
      ) {
        nodes.add(result.value);
      }
    }
  }

  throwIfAborted(options.signal);
  if (nodes.size < count) {
    throw new Error(
      `MASQ could not find ${count} unique entry nodes. Check your internet connection and try again.`,
    );
  }
  return [...nodes].slice(0, count);
}

async function fetchEntryNode(
  chain: Chain,
  baseUrl: string,
  fetchImpl: typeof fetch,
  timeoutMs: number,
  parentSignal?: AbortSignal,
): Promise<string> {
  const controller = new AbortController();
  const abortFromParent = () => controller.abort();
  parentSignal?.addEventListener('abort', abortFromParent, {once: true});
  const timeout = setTimeout(() => controller.abort(), timeoutMs);

  try {
    const response = await fetchImpl(
      `${baseUrl}/randomnode/${chain}/${MASQ_PUBLIC_SUBURB}`,
      {
        headers: {Accept: 'text/plain'},
        method: 'GET',
        signal: controller.signal,
      },
    );
    if (!response.ok) {
      throw new Error(`MASQ node-finder returned HTTP ${response.status}.`);
    }
    return normalizeDescriptor(await response.text());
  } finally {
    clearTimeout(timeout);
    parentSignal?.removeEventListener('abort', abortFromParent);
  }
}

export function normalizeNodeFinderBaseUrl(value: string): string {
  const normalized = value.trim().replace(/\/+$/, '');
  if (
    !/^https:\/\/[^\s/?#@]+(?::\d+)?(?:\/[^\s?#]*)?$/i.test(normalized)
  ) {
    throw new Error(
      'A verified HTTPS MASQ node-finder must be configured for this build.',
    );
  }
  return normalized;
}

function normalizeDescriptor(value: string): string {
  const trimmed = value.trim();
  try {
    const decoded: unknown = JSON.parse(trimmed);
    return typeof decoded === 'string' ? decoded.trim() : trimmed;
  } catch {
    return trimmed;
  }
}

function throwIfAborted(signal?: AbortSignal) {
  if (signal?.aborted) {
    const error = new Error('Entry node discovery was cancelled.');
    error.name = 'AbortError';
    throw error;
  }
}
