import {
  decodeDebtSettlementQuote,
  decodeDebtSettlementStatus,
  decodeDebtSummary,
} from '../src/core/masqCore';

describe('mobile MASQ debt settlement decoding', () => {
  it('accepts string-denominated values without exposing creditor addresses', () => {
    expect(
      decodeDebtSummary(
        JSON.stringify({
          totalMasqWei: '230081000000000',
          creditorCount: 11,
          settlementInProgress: false,
        }),
      ),
    ).toEqual({
      totalMasqWei: '230081000000000',
      creditorCount: 11,
      settlementInProgress: false,
    });

    const quote = decodeDebtSettlementQuote(
      JSON.stringify({
        quoteId: 'a'.repeat(32),
        createdAtUnixSeconds: 1_700_000_000,
        expiresAtUnixSeconds: 1_700_000_300,
        totalMasqWei: '230081000000000',
        estimatedL2FeeWei: '1234000000000',
        masqBalanceWei: '1000000000000000000',
        baseEthBalanceWei: '1000000000000000',
        creditorCount: 11,
        hasMoreCreditors: false,
        feeEstimateIncludesL1DataFee: false,
        requiresDeviceAuthentication: false,
        requiresExplicitConfirmation: true,
      }),
    );
    expect(quote).not.toHaveProperty('creditors');
    expect(quote.requiresDeviceAuthentication).toBe(false);
    expect(quote.requiresExplicitConfirmation).toBe(true);
  });

  it('rejects imprecise numbers, leaked recipients and unsafe confirmation flags', () => {
    expect(() =>
      decodeDebtSummary(
        JSON.stringify({
          totalMasqWei: 230081000000000,
          creditorCount: 11,
          settlementInProgress: false,
        }),
      ),
    ).toThrow('invalid MASQ debt summary');

    expect(() =>
      decodeDebtSettlementQuote(
        JSON.stringify({
          quoteId: 'a'.repeat(32),
          createdAtUnixSeconds: 1,
          expiresAtUnixSeconds: 2,
          totalMasqWei: '1',
          estimatedL2FeeWei: '1',
          masqBalanceWei: '1',
          baseEthBalanceWei: '1',
          creditorCount: 1,
          hasMoreCreditors: false,
          feeEstimateIncludesL1DataFee: true,
          requiresDeviceAuthentication: true,
          requiresExplicitConfirmation: false,
          creditorAddress: '0x0000000000000000000000000000000000000001',
        }),
      ),
    ).toThrow('invalid settlement quote');
  });

  it('validates transaction hashes and confirmation counts', () => {
    const hash = `0x${'b'.repeat(64)}`;
    expect(
      decodeDebtSettlementStatus(
        JSON.stringify({
          operationId: 'c'.repeat(32),
          phase: 'submitted',
          totalMasqWei: '42',
          estimatedL2FeeWei: '7',
          transactionCount: 1,
          confirmedTransactionCount: 0,
          transactionHashes: [hash],
          errorCode: null,
        }),
      ).transactionHashes,
    ).toEqual([hash]);

    expect(
      decodeDebtSettlementStatus(
        JSON.stringify({
          operationId: 'c'.repeat(32),
          phase: 'failed',
          totalMasqWei: '42',
          estimatedL2FeeWei: '7',
          transactionCount: 1,
          confirmedTransactionCount: 0,
          transactionHashes: [hash],
          errorCode: 'E_SETTLEMENT_REVERTED',
        }),
      ).phase,
    ).toBe('failed');

    expect(() =>
      decodeDebtSettlementStatus(
        JSON.stringify({
          operationId: 'c'.repeat(32),
          phase: 'submitted',
          totalMasqWei: '42',
          estimatedL2FeeWei: '7',
          transactionCount: 1,
          confirmedTransactionCount: 2,
          transactionHashes: [hash],
          errorCode: null,
        }),
      ),
    ).toThrow('invalid settlement status');
  });
});
