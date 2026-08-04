import type { CoreStatus, NetworkStatus } from './types';
import {
  SAFE_UNKNOWN_ISSUE_SUMMARY,
  safeDiagnosticIssueSummary,
  type MasqIssue,
} from './issues';
import { extractMasqErrorCode } from './errorCodes';

export function buildRedactedDiagnostics(
  status: CoreStatus,
  network: NetworkStatus,
  error: string | null,
  issue?: MasqIssue | null,
) {
  const safeSummary = issue
    ? safeDiagnosticIssueSummary(issue)
    : error
    ? SAFE_UNKNOWN_ISSUE_SUMMARY
    : null;
  const safeCode = issue?.code
    ? extractMasqErrorCode({ code: issue.code })
    : null;
  return {
    reportVersion: 2,
    app: 'MASQ Mobile',
    phase: status.phase,
    engineAvailable: status.engineAvailable,
    proxyEnabled: status.proxyEnabled,
    chain: status.chain,
    connectedNeighbors: status.connectedNeighbors,
    routeStage: status.routeStage,
    routeHops: status.routeHops,
    minHops: status.minHops,
    exitCountry: status.exitCountry,
    network,
    issueCategory: issue?.category ?? null,
    issueCode: safeCode,
    issueSummary: safeSummary,
    lastError: safeSummary,
    generatedAt: new Date().toISOString(),
    redacted: true,
  };
}

export function redactDiagnosticText(value: string): string {
  return value
    .replace(/masq:\/\/\S+/gi, '[entry-node]')
    .replace(/https?:\/\/\S+/gi, '[url]')
    .replace(/\b0x[a-f\d]{40,64}\b/gi, '[wallet]')
    .replace(/\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b/g, '[ip]')
    .replace(/\b[a-f\d:]{3,}:[a-f\d:]+(?::\d+)?\b/gi, '[ip]');
}
