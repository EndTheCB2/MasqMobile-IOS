import { AppState } from 'react-native';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  isValidSavedConfig,
  normalizeWalletSecret,
  toMasqConfig,
} from '../core/config';
import {
  type EntryNodeRefreshProgress,
  isAbortError,
  startWithEntryNodeRefresh,
} from '../core/entryNodeRefresh';
import { extractMasqErrorCode } from '../core/errorCodes';
import { stopMasqSafely } from '../core/connectionLifecycle';
import {
  isCoreRouteReady,
  startAndAwaitMasqConnection,
} from '../core/connectionReadiness';
import {
  isSavedProfileError,
  masqCore,
  SavedProfileError,
} from '../core/masqCore';
import {
  classifyMasqIssue,
  reconcileMasqIssue,
  type MasqIssue,
} from '../core/issues';
import { chooseReachableRpc } from '../core/rpcHealth';
import { SingleFlight } from '../core/singleFlight';
import {
  EMPTY_WALLET_BALANCE,
  fetchWalletBalance,
  type WalletBalanceState,
} from '../core/walletBalance';
import {
  UNSUPPORTED_SYSTEM_TUNNEL,
  type RoutableApp,
  type SystemTunnelMode,
  type SystemTunnelStatus,
} from '../core/systemTunnel';
import {
  DEFAULT_SETUP,
  EMPTY_STATUS,
  type CoreStatus,
  type DebtSettlementQuote,
  type DebtSettlementStatus,
  type DebtSummary,
  type MasqConfig,
  type NetworkStatus,
  type SetupDraft,
} from '../core/types';

const UNKNOWN_NETWORK: NetworkStatus = {
  available: false,
  interface: 'unknown',
  expensive: false,
  constrained: false,
  generation: 0,
};

const EMPTY_DEBT_SUMMARY: DebtSummary = {
  totalMasqWei: '0',
  creditorCount: 0,
  settlementInProgress: false,
};

const IDLE_DEBT_SETTLEMENT: DebtSettlementStatus = {
  operationId: null,
  phase: 'idle',
  totalMasqWei: '0',
  estimatedL2FeeWei: '0',
  transactionCount: 0,
  confirmedTransactionCount: 0,
  transactionHashes: [],
  errorCode: null,
};

export type ControllerInitializationState = 'loading' | 'ready' | 'error';

export const PROFILE_NOT_READY_MESSAGE =
  'MASQ has not finished loading the saved Node and wallet profile. Wait for profile loading to complete and try again.';
export const PROFILE_RECOVERY_NOT_AVAILABLE_MESSAGE =
  'Network-profile recovery is available only after saved profile loading fails.';

const SAVED_PROFILE_INVALID_MESSAGE =
  'The saved MASQ configuration is invalid.';
const SAVED_PROFILE_MISMATCH_MESSAGE =
  'The saved MASQ configuration does not match the active native network profile.';
const NETWORK_PROFILE_RESET_NOT_CONFIRMED_MESSAGE =
  'MASQ could not confirm that the invalid network profile was removed while preserving the consumer wallet.';

export const AUTOMATIC_RECONNECT_BACKOFF_MS = [2_000, 5_000, 15_000, 30_000];
export const NATIVE_RECOVERY_OBSERVATION_MS = 1_000;

export function automaticReconnectDelayMs(failedAttempts: number): number {
  return AUTOMATIC_RECONNECT_BACKOFF_MS[
    Math.max(
      0,
      Math.min(failedAttempts - 1, AUTOMATIC_RECONNECT_BACKOFF_MS.length - 1),
    )
  ];
}

export function shouldAutomaticallyRetryMasqIssue(
  currentIssue: MasqIssue | null,
): boolean {
  return currentIssue?.action === 'retry';
}

/**
 * Android can restore the consumer session before JavaScript observes a
 * validated network. Do not replace that native attempt merely because its
 * route has not reached the final proof stage yet.
 */
export function isNativeMasqRecoveryInProgress(status: CoreStatus): boolean {
  return (
    status.engineAvailable &&
    status.engineGeneration > 0 &&
    status.lastError === null &&
    (status.phase === 'connecting' ||
      (status.phase === 'connected' &&
        status.connectedNeighbors > 0 &&
        status.routeStage > 0))
  );
}

export function useMasqController() {
  const [status, setStatus] = useState<CoreStatus>(EMPTY_STATUS);
  const [network, setNetwork] = useState<NetworkStatus>(UNKNOWN_NETWORK);
  const [draft, setDraft] = useState<SetupDraft>(DEFAULT_SETUP);
  const [busy, setBusy] = useState(true);
  const [initializationState, setInitializationState] =
    useState<ControllerInitializationState>('loading');
  const [profileRecoveryAvailable, setProfileRecoveryAvailable] =
    useState(false);
  const [issue, setIssue] = useState<MasqIssue | null>(null);
  const [entryNodeRefresh, setEntryNodeRefresh] =
    useState<EntryNodeRefreshProgress | null>(null);
  const [walletBalance, setWalletBalance] =
    useState<WalletBalanceState>(EMPTY_WALLET_BALANCE);
  const [debtSummary, setDebtSummary] =
    useState<DebtSummary>(EMPTY_DEBT_SUMMARY);
  const [debtSettlementQuote, setDebtSettlementQuote] =
    useState<DebtSettlementQuote | null>(null);
  const [debtSettlementStatus, setDebtSettlementStatus] =
    useState<DebtSettlementStatus>(IDLE_DEBT_SETTLEMENT);
  const [debtSettlementBusy, setDebtSettlementBusy] = useState(false);
  const [debtSettlementError, setDebtSettlementError] = useState<string | null>(
    null,
  );
  const [systemTunnel, setSystemTunnel] = useState<SystemTunnelStatus>(
    UNSUPPORTED_SYSTEM_TUNNEL,
  );
  const [routableApps, setRoutableApps] = useState<RoutableApp[]>([]);
  const [systemTunnelBusy, setSystemTunnelBusy] = useState(false);
  const operationEpoch = useRef(0);
  const systemTunnelOperationEpoch = useRef(0);
  const systemTunnelOperationInFlight = useRef(false);
  const connectAbort = useRef<AbortController | null>(null);
  const activeConnectController = useRef<AbortController | null>(null);
  const balanceAbort = useRef<AbortController | null>(null);
  const connectFlight = useRef(new SingleFlight<CoreStatus>());
  const connectAttemptEpoch = useRef(0);
  const lastConnectAttemptNetworkGeneration = useRef<number | null>(null);
  const nativeRecoveryObservationGeneration = useRef<number | null>(null);
  const desiredConnected = useRef(false);
  const automaticReconnectFailures = useRef(0);
  const automaticReconnectNotBefore = useRef(0);
  const appStateResumeEpoch = useRef(0);
  const initializationEpoch = useRef(0);
  const initializationStateRef =
    useRef<ControllerInitializationState>('loading');
  const profileRecoveryAvailableRef = useRef(false);
  const profileReadyRef = useRef(false);
  const statusRef = useRef(status);
  const networkRef = useRef(network);

  useEffect(() => {
    statusRef.current = status;
  }, [status]);
  useEffect(() => {
    networkRef.current = network;
  }, [network]);

  const run = useCallback(async (operation: () => Promise<CoreStatus>) => {
    const epoch = ++operationEpoch.current;
    setBusy(true);
    setIssue(null);
    try {
      const next = await operation();
      if (epoch === operationEpoch.current) {
        statusRef.current = next;
        setStatus(next);
      }
      return next;
    } catch (caught) {
      if (epoch === operationEpoch.current && !isAbortError(caught)) {
        setIssue(
          classifyMasqIssue(caught, networkRef.current, statusRef.current),
        );
      }
      throw caught;
    } finally {
      if (epoch === operationEpoch.current) {
        setBusy(false);
      }
    }
  }, []);

  const requireProfileReady = useCallback(() => {
    if (!profileReadyRef.current) {
      throw new Error(PROFILE_NOT_READY_MESSAGE);
    }
  }, []);

  const initialize = useCallback(async () => {
    const initialization = ++initializationEpoch.current;
    operationEpoch.current += 1;
    profileReadyRef.current = false;
    initializationStateRef.current = 'loading';
    profileRecoveryAvailableRef.current = false;
    setInitializationState('loading');
    setProfileRecoveryAvailable(false);
    setBusy(true);
    setIssue(null);
    try {
      const saved = await masqCore.getSavedConfiguration();
      if (initialization !== initializationEpoch.current) {
        return;
      }
      if (saved && !isValidSavedConfig(saved)) {
        throw new SavedProfileError(SAVED_PROFILE_INVALID_MESSAGE);
      }
      const [initialStatus, initialNetwork] = await Promise.all([
        masqCore.getStatus(),
        masqCore.getNetworkStatus().catch(() => UNKNOWN_NETWORK),
      ]);
      if (initialization !== initializationEpoch.current) {
        return;
      }
      assertSavedProfileMatchesStatus(saved, initialStatus);
      const loadedDraft: SetupDraft = saved
        ? {
            ...DEFAULT_SETUP,
            ...saved,
            neighbors: [...saved.neighbors],
            walletSecret: '',
          }
        : { ...DEFAULT_SETUP, neighbors: [], walletSecret: '' };
      desiredConnected.current =
        initialStatus.phase === 'connecting' ||
        (initialStatus.phase === 'connected' &&
          initialStatus.connectedNeighbors > 0 &&
          initialStatus.routeStage > 0);
      statusRef.current = initialStatus;
      networkRef.current = initialNetwork;
      setStatus(initialStatus);
      setNetwork(initialNetwork);
      setDraft(loadedDraft);
      profileReadyRef.current = true;
      initializationStateRef.current = 'ready';
      setInitializationState('ready');
    } catch (caught) {
      if (initialization !== initializationEpoch.current) {
        return;
      }
      profileReadyRef.current = false;
      initializationStateRef.current = 'error';
      const recoveryAvailable = isSavedProfileError(caught);
      const initializationError =
        recoveryAvailable && !(caught instanceof SavedProfileError)
          ? new SavedProfileError()
          : caught;
      profileRecoveryAvailableRef.current = recoveryAvailable;
      setInitializationState('error');
      setProfileRecoveryAvailable(recoveryAvailable);
      setIssue(
        classifyMasqIssue(
          initializationError,
          networkRef.current,
          statusRef.current,
        ),
      );
    } finally {
      if (initialization === initializationEpoch.current) {
        setBusy(false);
      }
    }
  }, []);

  const cancelConnectionAttempt = useCallback(() => {
    connectAbort.current?.abort();
    connectAbort.current = null;
    setEntryNodeRefresh(null);
  }, []);

  const refresh = useCallback(() => run(() => masqCore.getStatus()), [run]);

  const refreshWalletBalance = useCallback(async () => {
    requireProfileReady();
    const walletAddress = status.walletAddress;
    const chain = status.chain;
    if (!walletAddress || !chain) {
      balanceAbort.current?.abort();
      balanceAbort.current = null;
      setWalletBalance(EMPTY_WALLET_BALANCE);
      return;
    }

    balanceAbort.current?.abort();
    const controller = new AbortController();
    balanceAbort.current = controller;
    setWalletBalance(current => ({
      state: 'loading',
      value: current.value,
      message: null,
    }));
    try {
      const value = await fetchWalletBalance(
        chain,
        draft.rpcUrl,
        walletAddress,
        { signal: controller.signal },
      );
      if (!controller.signal.aborted) {
        setWalletBalance({ state: 'ready', value, message: null });
      }
    } catch (caught) {
      if (!controller.signal.aborted) {
        setWalletBalance(current => ({
          state: 'error',
          value: current.value,
          message:
            caught instanceof TypeError
              ? 'Balance check unavailable. Verify the RPC and internet connection.'
              : caught instanceof Error
              ? caught.message
              : 'Balance check unavailable.',
        }));
      }
    } finally {
      if (balanceAbort.current === controller) {
        balanceAbort.current = null;
      }
    }
  }, [draft.rpcUrl, requireProfileReady, status.chain, status.walletAddress]);

  const refreshDebtSummary = useCallback(async () => {
    requireProfileReady();
    if (!status.walletAddress || !status.chain) {
      setDebtSummary(EMPTY_DEBT_SUMMARY);
      return EMPTY_DEBT_SUMMARY;
    }
    const summary = await masqCore.getDebtSummary();
    setDebtSummary(summary);
    return summary;
  }, [requireProfileReady, status.chain, status.walletAddress]);

  const reviewDebtSettlement = useCallback(async () => {
    requireProfileReady();
    setDebtSettlementBusy(true);
    setDebtSettlementError(null);
    try {
      const quote = await masqCore.prepareDebtSettlement();
      setDebtSettlementQuote(quote);
      return quote;
    } catch (caught) {
      const message =
        caught instanceof Error
          ? caught.message
          : 'The MASQ debt settlement could not be prepared.';
      setDebtSettlementError(message);
      throw caught;
    } finally {
      setDebtSettlementBusy(false);
    }
  }, [requireProfileReady]);

  const confirmDebtSettlement = useCallback(async () => {
    requireProfileReady();
    const quote = debtSettlementQuote;
    if (!quote) {
      throw new Error('Review the current MASQ debts before settling.');
    }
    setDebtSettlementBusy(true);
    setDebtSettlementError(null);
    try {
      const settlement = await masqCore.confirmDebtSettlement(
        quote.quoteId,
        quote.totalMasqWei,
        quote.estimatedL2FeeWei,
      );
      setDebtSettlementQuote(null);
      setDebtSettlementStatus(settlement);
      setDebtSummary(current => ({
        ...current,
        settlementInProgress:
          settlement.phase === 'reserved' ||
          settlement.phase === 'submitted' ||
          settlement.phase === 'attention',
      }));
      await refresh().catch(() => undefined);
      return settlement;
    } catch (caught) {
      const message =
        caught instanceof Error
          ? caught.message
          : 'The reviewed MASQ debt settlement could not be submitted.';
      setDebtSettlementError(message);
      throw caught;
    } finally {
      setDebtSettlementBusy(false);
    }
  }, [debtSettlementQuote, refresh, requireProfileReady]);

  const dismissDebtSettlement = useCallback(() => {
    if (!debtSettlementBusy) {
      setDebtSettlementQuote(null);
      setDebtSettlementError(null);
    }
  }, [debtSettlementBusy]);

  const retryDebtSettlement = useCallback(async () => {
    requireProfileReady();
    setDebtSettlementBusy(true);
    setDebtSettlementError(null);
    try {
      const settlement = await masqCore.retryDebtSettlement();
      setDebtSettlementStatus(settlement);
      setDebtSummary(current => ({
        ...current,
        settlementInProgress:
          settlement.phase === 'reserved' ||
          settlement.phase === 'submitted' ||
          settlement.phase === 'attention',
      }));
      await refresh().catch(() => undefined);
      return settlement;
    } catch (caught) {
      const message =
        caught instanceof Error
          ? caught.message
          : 'The exact saved MASQ settlement could not be retried.';
      setDebtSettlementError(message);
      throw caught;
    } finally {
      setDebtSettlementBusy(false);
    }
  }, [refresh, requireProfileReady]);

  const updateSystemTunnel = useCallback(
    async (mode: SystemTunnelMode, selectedApps: string[]) => {
      requireProfileReady();
      const operation = ++systemTunnelOperationEpoch.current;
      systemTunnelOperationInFlight.current = true;
      setSystemTunnelBusy(true);
      try {
        const next = await masqCore.setSystemTunnel(mode, selectedApps);
        if (operation === systemTunnelOperationEpoch.current) {
          setSystemTunnel(next);
        }
        return next;
      } catch (caught) {
        // Permission denial, activity recreation, or native start failure can
        // change the persisted desired/captured scopes before the bridge
        // rejects. Reconcile that authoritative state without replacing the
        // user's draft with a transient applied-scope poll.
        const reconciled = await masqCore
          .getSystemTunnelStatus()
          .catch(() => null);
        if (reconciled && operation === systemTunnelOperationEpoch.current) {
          setSystemTunnel(reconciled);
        }
        throw caught;
      } finally {
        if (operation === systemTunnelOperationEpoch.current) {
          // Invalidate any poll that began while the native mutation was in
          // flight before exposing the operation as complete.
          systemTunnelOperationEpoch.current += 1;
          systemTunnelOperationInFlight.current = false;
          setSystemTunnelBusy(false);
        }
      }
    },
    [requireProfileReady],
  );

  const disableSystemTunnel = useCallback(async () => {
    const operation = ++systemTunnelOperationEpoch.current;
    systemTunnelOperationInFlight.current = true;
    setSystemTunnelBusy(true);
    try {
      const current = await masqCore.getSystemTunnelStatus();
      if (operation === systemTunnelOperationEpoch.current) {
        setSystemTunnel(current);
      }
      if (!current.supported) {
        return current;
      }
      const next = await masqCore.setSystemTunnel('off', []);
      if (operation === systemTunnelOperationEpoch.current) {
        setSystemTunnel(next);
      }
      if (next.active || next.phase !== 'off' || next.mode !== 'off') {
        throw new Error('MASQ could not confirm that system routing stopped.');
      }
      return next;
    } finally {
      if (operation === systemTunnelOperationEpoch.current) {
        systemTunnelOperationEpoch.current += 1;
        systemTunnelOperationInFlight.current = false;
        setSystemTunnelBusy(false);
      }
    }
  }, []);

  const saveSetup = useCallback(
    async (nextDraft: SetupDraft) => {
      requireProfileReady();
      desiredConnected.current = false;
      cancelConnectionAttempt();
      let savedDraft = nextDraft;
      await run(async () => {
        const rpcUrl = await chooseReachableRpc(
          nextDraft.chain,
          nextDraft.rpcUrl,
        );
        const resolvedDraft = { ...nextDraft, rpcUrl };
        savedDraft = resolvedDraft;
        await masqCore.configure(toMasqConfig(resolvedDraft));
        const walletSecret = normalizeWalletSecret(resolvedDraft);
        return walletSecret
          ? masqCore.importWallet(walletSecret)
          : masqCore.getStatus();
      });
      setDraft({ ...savedDraft, configVersion: 2, walletSecret: '' });
    },
    [cancelConnectionAttempt, requireProfileReady, run],
  );

  const connectWithFailurePolicy = useCallback(
    () => {
      if (!profileReadyRef.current) {
        return Promise.reject(new Error(PROFILE_NOT_READY_MESSAGE));
      }
      desiredConnected.current = true;
      const flight = connectFlight.current;
      if (flight.isRunning) {
        // The one native flight owns success/failure accounting; every waiter shares its result.
        return flight.run(() => masqCore.getStatus());
      }

      connectAttemptEpoch.current += 1;
      lastConnectAttemptNetworkGeneration.current =
        networkRef.current.generation;
      nativeRecoveryObservationGeneration.current =
        networkRef.current.generation;
      const controller = new AbortController();
      connectAbort.current = controller;
      activeConnectController.current = controller;
      return flight.run(() =>
        run(async () => {
          setStatus(current => ({
            ...current,
            phase: 'connecting',
            lastError: null,
          }));
          try {
            try {
              const connected = await startWithEntryNodeRefresh(
                attemptBudget =>
                  startAndAwaitMasqConnection(
                    () => masqCore.start(),
                    () => masqCore.getStatus(),
                    {
                      deadlineAtMs: attemptBudget.deadlineAtMs,
                      timeoutMs: attemptBudget.readinessTimeoutMs,
                      onStatus: next => {
                        statusRef.current = next;
                        setStatus(next);
                        setEntryNodeRefresh(current => {
                          if (isCoreRouteReady(next)) {
                            return null;
                          }
                          return current
                            ? { ...current, stage: 'handshake' }
                            : current;
                        });
                      },
                      signal: controller.signal,
                      verifyRoute: () => masqCore.preflightBrowserProxy(),
                    },
                  ),
                {
                  onAttempt: progress => {
                    setStatus(current => ({
                      ...current,
                      phase: 'connecting',
                      lastError: null,
                    }));
                    setEntryNodeRefresh(progress);
                  },
                  signal: controller.signal,
                },
              );
              automaticReconnectFailures.current = 0;
              automaticReconnectNotBefore.current = 0;
              try {
                const refreshedProfile =
                  await masqCore.getSavedConfiguration();
                if (
                  refreshedProfile &&
                  isValidSavedConfig(refreshedProfile) &&
                  activeConnectController.current === controller
                ) {
                  setDraft(current => ({
                    ...current,
                    ...refreshedProfile,
                    neighbors: [...refreshedProfile.neighbors],
                    walletSecret: '',
                  }));
                }
              } catch {
                // A successful live route remains authoritative if a later profile read fails.
              }
              return connected;
            } catch (caught) {
              const currentIssue = classifyMasqIssue(
                caught,
                networkRef.current,
                statusRef.current,
              );
              const networkHandoverRetry =
                extractMasqErrorCode(caught) === 'E_NETWORK_HANDOVER_RETRY';
              const retryableWithoutShutdown =
                networkHandoverRetry ||
                currentIssue?.category === 'offline' ||
                shouldAutomaticallyRetryMasqIssue(currentIssue);
              if (
                retryableWithoutShutdown &&
                !isAbortError(caught) &&
                activeConnectController.current === controller
              ) {
                const failures = automaticReconnectFailures.current + 1;
                automaticReconnectFailures.current = failures;
                automaticReconnectNotBefore.current =
                  Date.now() + automaticReconnectDelayMs(failures);
              }
              if (
                !retryableWithoutShutdown &&
                !isAbortError(caught) &&
                activeConnectController.current === controller
              ) {
                desiredConnected.current = false;
                try {
                  const stopped = await masqCore.shutdown();
                  if (activeConnectController.current === controller) {
                    setStatus(stopped);
                  }
                } catch {
                  // The original connection diagnostic remains authoritative.
                }
              }
              throw caught;
            }
          } finally {
            if (connectAbort.current === controller) {
              connectAbort.current = null;
            }
            if (activeConnectController.current === controller) {
              activeConnectController.current = null;
            }
            setEntryNodeRefresh(null);
          }
        }),
      );
    },
    [run],
  );

  const connect = useCallback(async () => {
    return connectWithFailurePolicy();
  }, [connectWithFailurePolicy]);

  const reconnectAutomatically = useCallback(
    () => connectWithFailurePolicy(),
    [connectWithFailurePolicy],
  );

  const disconnect = useCallback(async () => {
    desiredConnected.current = false;
    automaticReconnectFailures.current = 0;
    automaticReconnectNotBefore.current = 0;
    cancelConnectionAttempt();
    const connectIdle = connectFlight.current.whenIdle();
    return run(async () => {
      try {
        setStatus(current => ({ ...current, phase: 'stopping' }));
        await disableSystemTunnel();
        return await stopMasqSafely(masqCore);
      } finally {
        await connectIdle;
      }
    });
  }, [cancelConnectionAttempt, disableSystemTunnel, run]);

  const updateMinHops = useCallback(
    async (minHops: number) => {
      requireProfileReady();
      const next = await run(() => masqCore.updateMinHops(minHops));
      setDraft(current => ({ ...current, minHops }));
      return next;
    },
    [requireProfileReady, run],
  );

  const reset = useCallback(async () => {
    requireProfileReady();
    desiredConnected.current = false;
    cancelConnectionAttempt();
    const next = await run(() => masqCore.reset());
    setDraft({ ...DEFAULT_SETUP, neighbors: [] });
    return next;
  }, [cancelConnectionAttempt, requireProfileReady, run]);

  const resetNetworkProfile = useCallback(async () => {
    requireProfileReady();
    desiredConnected.current = false;
    cancelConnectionAttempt();
    const next = await run(async () => {
      await disableSystemTunnel();
      return masqCore.resetNetworkProfile();
    });
    setDraft(current => ({
      ...DEFAULT_SETUP,
      walletImportMode: current.walletImportMode,
      walletSecret: '',
      neighbors: [],
    }));
    return next;
  }, [cancelConnectionAttempt, disableSystemTunnel, requireProfileReady, run]);

  const removeWallet = useCallback(async () => {
    requireProfileReady();
    desiredConnected.current = false;
    cancelConnectionAttempt();
    const next = await run(async () => {
      await disableSystemTunnel();
      return masqCore.removeWallet();
    });
    setDraft(current => ({ ...current, walletSecret: '' }));
    return next;
  }, [cancelConnectionAttempt, disableSystemTunnel, requireProfileReady, run]);

  const recoverNetworkProfile = useCallback(async () => {
    if (
      initializationStateRef.current !== 'error' ||
      !profileRecoveryAvailableRef.current
    ) {
      throw new Error(PROFILE_RECOVERY_NOT_AVAILABLE_MESSAGE);
    }
    const recovery = ++initializationEpoch.current;
    desiredConnected.current = false;
    cancelConnectionAttempt();
    profileReadyRef.current = false;
    initializationStateRef.current = 'loading';
    profileRecoveryAvailableRef.current = false;
    setInitializationState('loading');
    setProfileRecoveryAvailable(false);
    setBusy(true);
    setIssue(null);
    try {
      const tunnel = await disableSystemTunnel();
      if (tunnel.active || tunnel.phase !== 'off' || tunnel.mode !== 'off') {
        throw new Error(
          'MASQ could not confirm that system routing stopped before network-profile recovery.',
        );
      }
      const next = await masqCore.resetNetworkProfile();
      if (recovery !== initializationEpoch.current) {
        return;
      }
      if (
        next.phase !== 'unconfigured' ||
        next.chain !== null ||
        next.connectedNeighbors !== 0 ||
        next.proxyEnabled ||
        next.proxyPort !== null ||
        next.lastError !== null
      ) {
        throw new Error(NETWORK_PROFILE_RESET_NOT_CONFIRMED_MESSAGE);
      }
      setStatus(next);
      await initialize();
    } catch (caught) {
      if (recovery !== initializationEpoch.current) {
        return;
      }
      profileReadyRef.current = false;
      initializationStateRef.current = 'error';
      profileRecoveryAvailableRef.current = true;
      setInitializationState('error');
      setProfileRecoveryAvailable(true);
      setIssue(
        classifyMasqIssue(caught, networkRef.current, statusRef.current),
      );
      setBusy(false);
      throw caught;
    }
  }, [cancelConnectionAttempt, disableSystemTunnel, initialize]);

  useEffect(() => {
    initialize().catch(() => undefined);
    return () => {
      initializationEpoch.current += 1;
      profileReadyRef.current = false;
      initializationStateRef.current = 'loading';
      profileRecoveryAvailableRef.current = false;
    };
  }, [initialize]);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const initialTunnelEpoch = systemTunnelOperationEpoch.current;
    Promise.all([masqCore.getSystemTunnelStatus(), masqCore.getRoutableApps()])
      .then(([tunnel, apps]) => {
        if (!cancelled) {
          if (
            !systemTunnelOperationInFlight.current &&
            initialTunnelEpoch === systemTunnelOperationEpoch.current
          ) {
            setSystemTunnel(tunnel);
          }
          setRoutableApps(apps);
        }
      })
      .catch(() => undefined);
    const poll = async () => {
      const pollEpoch = systemTunnelOperationEpoch.current;
      try {
        const next = await masqCore.getSystemTunnelStatus();
        if (
          !cancelled &&
          !systemTunnelOperationInFlight.current &&
          pollEpoch === systemTunnelOperationEpoch.current
        ) {
          setSystemTunnel(next);
        }
      } catch {
        // The MASQ connection status remains usable if the optional tunnel API is unavailable.
      } finally {
        if (!cancelled) timer = setTimeout(poll, 2000);
      }
    };
    timer = setTimeout(poll, 2000);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    if (
      initializationState !== 'ready' ||
      !status.walletAddress ||
      !status.chain
    ) {
      balanceAbort.current?.abort();
      balanceAbort.current = null;
      setWalletBalance(EMPTY_WALLET_BALANCE);
      return;
    }
    refreshWalletBalance().catch(() => undefined);
    const timer = setInterval(() => {
      if (AppState.currentState === 'active') {
        refreshWalletBalance().catch(() => undefined);
      }
    }, 60_000);
    return () => {
      clearInterval(timer);
      balanceAbort.current?.abort();
      balanceAbort.current = null;
    };
  }, [
    initializationState,
    refreshWalletBalance,
    status.chain,
    status.walletAddress,
  ]);

  useEffect(() => {
    if (
      initializationState !== 'ready' ||
      !status.walletAddress ||
      !status.chain
    ) {
      setDebtSummary(EMPTY_DEBT_SUMMARY);
      setDebtSettlementQuote(null);
      setDebtSettlementStatus(IDLE_DEBT_SETTLEMENT);
      setDebtSettlementError(null);
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      try {
        const [summary, settlement] = await Promise.all([
          masqCore.getDebtSummary(),
          masqCore.getDebtSettlementStatus(),
        ]);
        if (!cancelled) {
          setDebtSummary(summary);
          setDebtSettlementStatus(settlement);
        }
      } catch {
        // Debt monitoring is optional and must not interfere with route status.
      } finally {
        if (!cancelled) {
          const active =
            debtSettlementStatus.phase === 'reserved' ||
            debtSettlementStatus.phase === 'submitted' ||
            debtSettlementStatus.phase === 'attention';
          timer = setTimeout(poll, active ? 15_000 : 60_000);
        }
      }
    };
    poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [
    debtSettlementStatus.phase,
    initializationState,
    status.chain,
    status.walletAddress,
  ]);

  // The next poll is scheduled only after the previous native call settles.
  // The epoch prevents an old poll from overwriting a newer user operation.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (!profileReadyRef.current) {
        if (!cancelled) timer = setTimeout(poll, 1000);
        return;
      }
      if (AppState.currentState !== 'active') {
        if (!cancelled) timer = setTimeout(poll, 5000);
        return;
      }
      const epoch = operationEpoch.current;
      const initialization = initializationEpoch.current;
      try {
        const next = await masqCore.getStatus();
        if (
          !cancelled &&
          profileReadyRef.current &&
          epoch === operationEpoch.current &&
          initialization === initializationEpoch.current
        ) {
          statusRef.current = next;
          setStatus(next);
          setIssue(current =>
            reconcileMasqIssue(current, next, networkRef.current),
          );
        }
      } catch (caught) {
        if (
          !cancelled &&
          profileReadyRef.current &&
          epoch === operationEpoch.current &&
          initialization === initializationEpoch.current
        ) {
          setIssue(
            classifyMasqIssue(caught, networkRef.current, statusRef.current),
          );
        }
      } finally {
        const activeRoute = ['connecting', 'connected', 'stopping'].includes(
          statusRef.current.phase,
        );
        if (!cancelled) timer = setTimeout(poll, activeRoute ? 1000 : 5000);
      }
    };
    timer = setTimeout(poll, 1000);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let reconnecting = false;
    const pollNetwork = async () => {
      if (!profileReadyRef.current) {
        if (!cancelled) timer = setTimeout(pollNetwork, 2000);
        return;
      }
      if (AppState.currentState !== 'active') {
        if (!cancelled) timer = setTimeout(pollNetwork, 5000);
        return;
      }
      const epoch = operationEpoch.current;
      const initialization = initializationEpoch.current;
      const observedConnectAttempt = connectAttemptEpoch.current;
      const connectWasRunning = connectFlight.current.isRunning;
      try {
        const next = await masqCore.getNetworkStatus();
        if (
          cancelled ||
          !profileReadyRef.current ||
          epoch !== operationEpoch.current ||
          initialization !== initializationEpoch.current
        ) {
          return;
        }
        const previous = networkRef.current;
        networkRef.current = next;
        setNetwork(next);
        setIssue(current =>
          reconcileMasqIssue(current, statusRef.current, next),
        );
        const changed = next.generation !== previous.generation;
        let recoveryStatus = statusRef.current;
        if (
          next.available &&
          profileReadyRef.current &&
          desiredConnected.current &&
          !isCoreRouteReady(recoveryStatus)
        ) {
          // Android's foreground service may already be restoring this exact
          // session after network validation. Re-read native truth before
          // deciding to create a new engine/discovery generation.
          recoveryStatus = await masqCore.getStatus();
          if (
            cancelled ||
            !profileReadyRef.current ||
            epoch !== operationEpoch.current ||
            initialization !== initializationEpoch.current
          ) {
            return;
          }
          statusRef.current = recoveryStatus;
          setStatus(recoveryStatus);
          setIssue(current =>
            reconcileMasqIssue(current, recoveryStatus, next),
          );
        }
        const connectionAttemptObserved =
          connectWasRunning ||
          connectFlight.current.isRunning ||
          observedConnectAttempt !== connectAttemptEpoch.current;
        if (next.available && connectionAttemptObserved) {
          // An in-flight attempt observed across this transition owns the new
          // network opportunity, even if it began while JavaScript still had
          // the previous network snapshot.
          lastConnectAttemptNetworkGeneration.current = next.generation;
          nativeRecoveryObservationGeneration.current = next.generation;
        }
        if (
          changed &&
          next.available &&
          desiredConnected.current &&
          !connectionAttemptObserved &&
          lastConnectAttemptNetworkGeneration.current !== next.generation
        ) {
          // Android's service receives the same validated-network callback and
          // owns the first recovery opportunity. Its state publication trails
          // the callback briefly, so observe one bounded window even while the
          // immediate status still says `ready`.
          if (
            nativeRecoveryObservationGeneration.current !== next.generation
          ) {
            nativeRecoveryObservationGeneration.current = next.generation;
            automaticReconnectFailures.current = 0;
            automaticReconnectNotBefore.current =
              Date.now() + NATIVE_RECOVERY_OBSERVATION_MS;
          }
        }
        if (
          next.available &&
          profileReadyRef.current &&
          desiredConnected.current &&
          !isCoreRouteReady(recoveryStatus) &&
          !isNativeMasqRecoveryInProgress(recoveryStatus) &&
          Date.now() >= automaticReconnectNotBefore.current &&
          !reconnecting &&
          !connectWasRunning &&
          !connectFlight.current.isRunning &&
          observedConnectAttempt === connectAttemptEpoch.current
        ) {
          reconnecting = true;
          reconnectAutomatically()
            .catch(() => undefined)
            .finally(() => {
              reconnecting = false;
            });
        }
      } catch {
        // Core status remains authoritative when the OS monitor is unavailable.
      } finally {
        if (!cancelled) timer = setTimeout(pollNetwork, 2000);
      }
    };
    pollNetwork();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [reconnectAutomatically]);

  useEffect(() => {
    const subscription = AppState.addEventListener('change', nextState => {
      const resumeEpoch = ++appStateResumeEpoch.current;
      if (nextState !== 'active') {
        masqCore.setBrowserRoutingMode('blocked').catch(() => 'blocked');
        return;
      }

      const resumeConnection = async () => {
        const observedConnectAttempt = connectAttemptEpoch.current;
        const connectWasRunning = connectFlight.current.isRunning;
        await connectFlight.current.whenIdle();
        if (
          resumeEpoch !== appStateResumeEpoch.current ||
          !profileReadyRef.current
        ) {
          return;
        }

        const epoch = operationEpoch.current;
        const initialization = initializationEpoch.current;
        const [nextStatus, nextNetwork] = await Promise.all([
          masqCore.getStatus(),
          masqCore.getNetworkStatus(),
        ]);
        if (
          resumeEpoch !== appStateResumeEpoch.current ||
          !profileReadyRef.current ||
          epoch !== operationEpoch.current ||
          initialization !== initializationEpoch.current
        ) {
          return;
        }

        statusRef.current = nextStatus;
        networkRef.current = nextNetwork;
        setStatus(nextStatus);
        setNetwork(nextNetwork);
        const connectionAttemptObserved =
          connectWasRunning ||
          connectFlight.current.isRunning ||
          observedConnectAttempt !== connectAttemptEpoch.current;
        if (nextNetwork.available && connectionAttemptObserved) {
          lastConnectAttemptNetworkGeneration.current =
            nextNetwork.generation;
          nativeRecoveryObservationGeneration.current =
            nextNetwork.generation;
        }
        const freshNetworkOpportunity =
          !connectionAttemptObserved &&
          lastConnectAttemptNetworkGeneration.current !==
          nextNetwork.generation;
        if (
          desiredConnected.current &&
          nextNetwork.available &&
          freshNetworkOpportunity &&
          nativeRecoveryObservationGeneration.current !==
            nextNetwork.generation
        ) {
          nativeRecoveryObservationGeneration.current =
            nextNetwork.generation;
          automaticReconnectFailures.current = 0;
          automaticReconnectNotBefore.current =
            Date.now() + NATIVE_RECOVERY_OBSERVATION_MS;
        }
        if (
          desiredConnected.current &&
          nextNetwork.available &&
          !isCoreRouteReady(nextStatus) &&
          !isNativeMasqRecoveryInProgress(nextStatus) &&
          !connectWasRunning &&
          !connectFlight.current.isRunning &&
          observedConnectAttempt === connectAttemptEpoch.current
        ) {
          // A flight already started on this network keeps its bounded
          // backoff. A newly observed generation first gives the native
          // service its short publication window above.
          if (Date.now() >= automaticReconnectNotBefore.current) {
            await reconnectAutomatically();
          }
        }
      };

      resumeConnection().catch(() => undefined);
    });
    return () => {
      appStateResumeEpoch.current += 1;
      subscription.remove();
    };
  }, [reconnectAutomatically]);

  useEffect(
    () => () => {
      connectAbort.current?.abort();
      connectAbort.current = null;
      balanceAbort.current?.abort();
      balanceAbort.current = null;
    },
    [],
  );

  const connectionProgress = useMemo(
    () => describeConnectionProgress(status, network, entryNodeRefresh),
    [entryNodeRefresh, network, status],
  );

  return {
    status,
    network,
    connectionProgress,
    draft,
    busy,
    profileReady: initializationState === 'ready',
    initializationState,
    profileRecoveryAvailable,
    issue,
    entryNodeRefresh,
    walletBalance,
    debtSummary,
    debtSettlementQuote,
    debtSettlementStatus,
    debtSettlementBusy,
    debtSettlementError,
    systemTunnel,
    routableApps,
    systemTunnelBusy,
    retryInitialization: initialize,
    recoverNetworkProfile,
    saveSetup,
    connect,
    disconnect,
    updateMinHops,
    reset,
    resetNetworkProfile,
    removeWallet,
    refresh,
    refreshWalletBalance,
    refreshDebtSummary,
    reviewDebtSettlement,
    confirmDebtSettlement,
    retryDebtSettlement,
    dismissDebtSettlement,
    updateSystemTunnel,
  };
}

function assertSavedProfileMatchesStatus(
  saved: MasqConfig | null,
  status: CoreStatus,
) {
  const savedProfilePresent = saved !== null;
  const nativeProfilePresent = status.chain !== null;
  if (
    savedProfilePresent !== nativeProfilePresent ||
    status.phase === 'error'
  ) {
    throw new SavedProfileError(SAVED_PROFILE_MISMATCH_MESSAGE);
  }
  if (!saved || status.chain === null) {
    return;
  }
  if (
    saved.chain !== status.chain ||
    saved.minHops !== status.minHops ||
    saved.exitCountry !== status.exitCountry ||
    saved.exitCountryFallback !== status.exitCountryFallback
  ) {
    throw new SavedProfileError(SAVED_PROFILE_MISMATCH_MESSAGE);
  }
}

function describeConnectionProgress(
  status: CoreStatus,
  network: NetworkStatus,
  refresh: EntryNodeRefreshProgress | null,
) {
  if (!network.available && network.interface !== 'unknown') {
    return { step: 1, total: 5, label: 'Waiting for an internet connection' };
  }
  if (refresh?.stage === 'discovery') {
    return {
      step: 2,
      total: 5,
      label: `Finding reachable entry nodes (${refresh.attempt}/${refresh.maxAttempts})`,
    };
  }
  if (refresh?.stage === 'handshake') {
    return {
      step: 3,
      total: 5,
      label: `Connecting to an entry peer (attempt ${refresh.attempt}/${refresh.maxAttempts})`,
    };
  }
  if (status.connectedNeighbors < 1) {
    return { step: 3, total: 5, label: 'Connecting to an entry peer' };
  }
  if (status.routeStage < 2) {
    return { step: 4, total: 5, label: 'Preparing a private exit route' };
  }
  return {
    step: 5,
    total: 5,
    label: status.proxyEnabled
      ? 'Private browser protected'
      : 'Private route verified',
  };
}
