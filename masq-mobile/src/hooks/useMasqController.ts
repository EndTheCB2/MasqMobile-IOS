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
import { stopMasqSafely } from '../core/connectionLifecycle';
import { startAndAwaitMasqConnection } from '../core/connectionReadiness';
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
  const [systemTunnel, setSystemTunnel] = useState<SystemTunnelStatus>(
    UNSUPPORTED_SYSTEM_TUNNEL,
  );
  const [routableApps, setRoutableApps] = useState<RoutableApp[]>([]);
  const [systemTunnelBusy, setSystemTunnelBusy] = useState(false);
  const operationEpoch = useRef(0);
  const systemTunnelOperationEpoch = useRef(0);
  const systemTunnelOperationInFlight = useRef(false);
  const connectAbort = useRef<AbortController | null>(null);
  const balanceAbort = useRef<AbortController | null>(null);
  const connectFlight = useRef(new SingleFlight<CoreStatus>());
  const desiredConnected = useRef(false);
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

  const connect = useCallback(() => {
    if (!profileReadyRef.current) {
      return Promise.reject(new Error(PROFILE_NOT_READY_MESSAGE));
    }
    desiredConnected.current = true;
    const flight = connectFlight.current;
    if (flight.isRunning) {
      return flight.run(() => masqCore.getStatus());
    }

    const controller = new AbortController();
    connectAbort.current = controller;
    return flight.run(() =>
      run(async () => {
        setStatus(current => ({
          ...current,
          phase: 'connecting',
          lastError: null,
        }));
        try {
          try {
            return await startWithEntryNodeRefresh(
              () =>
                startAndAwaitMasqConnection(
                  () => masqCore.start(),
                  () => masqCore.getStatus(),
                  {
                    onStatus: next => {
                      setStatus(next);
                      setEntryNodeRefresh(current => {
                        if (next.phase === 'connected') {
                          return null;
                        }
                        return current
                          ? { ...current, stage: 'handshake' }
                          : current;
                      });
                    },
                    signal: controller.signal,
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
          } catch (caught) {
            if (!isAbortError(caught) && connectAbort.current === controller) {
              desiredConnected.current = false;
              try {
                const stopped = await masqCore.shutdown();
                if (connectAbort.current === controller) {
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
          setEntryNodeRefresh(null);
        }
      }),
    );
  }, [run]);

  const disconnect = useCallback(async () => {
    desiredConnected.current = false;
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
        setNetwork(next);
        setIssue(current =>
          reconcileMasqIssue(current, statusRef.current, next),
        );
        const changed =
          next.generation !== previous.generation &&
          (next.interface !== previous.interface ||
            next.available !== previous.available);
        if (
          changed &&
          next.available &&
          profileReadyRef.current &&
          desiredConnected.current &&
          statusRef.current.phase !== 'connected' &&
          !reconnecting
        ) {
          reconnecting = true;
          connect()
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
  }, [connect]);

  useEffect(() => {
    const subscription = AppState.addEventListener('change', nextState => {
      const resumeEpoch = ++appStateResumeEpoch.current;
      if (nextState !== 'active') {
        cancelConnectionAttempt();
        masqCore.setBrowserRoutingMode('blocked').catch(() => 'blocked');
        return;
      }

      const resumeConnection = async () => {
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

        setStatus(nextStatus);
        setNetwork(nextNetwork);
        if (
          desiredConnected.current &&
          nextNetwork.available &&
          !(
            nextStatus.phase === 'connected' &&
            nextStatus.connectedNeighbors > 0
          )
        ) {
          await connect();
        }
      };

      resumeConnection().catch(() => undefined);
    });
    return () => {
      appStateResumeEpoch.current += 1;
      subscription.remove();
    };
  }, [cancelConnectionAttempt, connect]);

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
