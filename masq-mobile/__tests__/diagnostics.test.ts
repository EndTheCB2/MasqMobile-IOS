import {
  buildRedactedDiagnostics,
  redactDiagnosticText,
} from '../src/core/diagnostics';
import { classifyMasqIssue } from '../src/core/issues';
import { EMPTY_STATUS, type NetworkStatus } from '../src/core/types';

it('removes entry nodes, URLs, wallets and IP addresses from diagnostics', () => {
  const source =
    'masq://base-mainnet:key@198.51.100.2:443 https://rpc.example 0x1234567890abcdef1234567890abcdef12345678 [2001:db8::1]:443';
  const redacted = redactDiagnosticText(source);

  expect(redacted).not.toContain('198.51.100.2');
  expect(redacted).not.toContain('rpc.example');
  expect(redacted).not.toContain('1234567890abcdef');
  expect(redacted).not.toContain('2001:db8');
  expect(redacted).toContain('[entry-node]');
});

it('adds a stable issue category without sensitive connection details', () => {
  const network: NetworkStatus = {
    available: true,
    constrained: false,
    expensive: false,
    generation: 1,
    interface: 'wifi',
  };
  const issue = classifyMasqIssue(
    Object.assign(
      new Error(
        'MASQ could not find reachable entry node masq://base-mainnet:key@198.51.100.2:443',
      ),
      { code: 'E_ENTRY_NODE_DISCOVERY' },
    ),
    network,
    EMPTY_STATUS,
  );
  const report = buildRedactedDiagnostics(
    EMPTY_STATUS,
    network,
    'Failed through https://rpc.example at 198.51.100.2',
    issue,
  );

  expect(report.issueCategory).toBe('entry-nodes');
  expect(report.issueCode).toBe('E_ENTRY_NODE_DISCOVERY');
  expect(report.issueSummary).toBe(issue?.message);
  expect(report.lastError).toBe(issue?.message);
  expect(JSON.stringify(report)).not.toContain('rpc.example');
  expect(JSON.stringify(report)).not.toContain('198.51.100.2');
});

it('never shares a raw unclassified native error', () => {
  const network: NetworkStatus = {
    available: true,
    constrained: false,
    expensive: false,
    generation: 1,
    interface: 'wifi',
  };
  const report = buildRedactedDiagnostics(
    EMPTY_STATUS,
    network,
    'secret native detail at 198.51.100.2',
    null,
  );

  expect(report.issueCode).toBeNull();
  expect(report.issueSummary).toBe(
    'MASQ could not complete the operation. Retry once or share the redacted diagnostics.',
  );
  expect(JSON.stringify(report)).not.toContain('secret native detail');
  expect(JSON.stringify(report)).not.toContain('198.51.100.2');
});

it('does not trust a caller-provided issue message or malformed code', () => {
  const network: NetworkStatus = {
    available: true,
    constrained: false,
    expensive: false,
    generation: 1,
    interface: 'wifi',
  };
  const report = buildRedactedDiagnostics(EMPTY_STATUS, network, null, {
    action: 'none',
    category: 'unknown',
    code: 'private-value@198.51.100.2',
    message: 'private issue detail at 198.51.100.2',
  });

  expect(report.issueCode).toBeNull();
  expect(report.issueSummary).toBe(
    'MASQ could not complete the operation. Retry once or share the redacted diagnostics.',
  );
  expect(JSON.stringify(report)).not.toContain('private issue detail');
  expect(JSON.stringify(report)).not.toContain('198.51.100.2');
});
