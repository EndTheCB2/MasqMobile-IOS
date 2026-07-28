import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  ActivityIndicator,
  Modal,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
  type TextInputProps,
} from 'react-native';

import {
  isDescriptorForChain,
  parseNeighborList,
  validateConfig,
} from '../core/config';
import { discoverEntryNodes } from '../core/discovery';
import { masqCore } from '../core/masqCore';
import {
  HOP_OPTIONS,
  exitCountryOptionsForAvailability,
  exitCountryName,
} from '../core/routingPreferences';
import {
  DEFAULT_RPC_URLS,
  type Chain,
  type SetupDraft,
  type WalletImportMode,
} from '../core/types';
import { Button, ErrorBanner, ScreenHeader } from '../ui/components';
import { colors, radii } from '../ui/theme';

interface Props {
  initial: SetupDraft;
  busy: boolean;
  error: string | null;
  hasWallet: boolean;
  availableExitCountries: string[];
  exitCountryInventoryReady: boolean;
  onBack: () => void;
  onSave: (draft: SetupDraft) => Promise<void>;
}

export function SetupScreen({
  initial,
  busy,
  error,
  hasWallet,
  availableExitCountries,
  exitCountryInventoryReady,
  onBack,
  onSave,
}: Props) {
  const [draft, setDraft] = useState(initial);
  const [neighborsText, setNeighborsText] = useState(
    initial.neighbors.join('\n'),
  );
  const [submitted, setSubmitted] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [showCountryPicker, setShowCountryPicker] = useState(false);
  const [recoveryPhraseVisible, setRecoveryPhraseVisible] = useState(false);
  const [discoveryRequest, setDiscoveryRequest] = useState(0);
  const [discoveryState, setDiscoveryState] = useState<
    'loading' | 'ready' | 'error'
  >(initial.neighbors.length >= 2 ? 'ready' : 'loading');
  const [discoveryError, setDiscoveryError] = useState<string | null>(null);
  const normalizedDraft = useMemo(
    () => ({ ...draft, neighbors: parseNeighborList(neighborsText) }),
    [draft, neighborsText],
  );
  const errors = submitted
    ? validateConfig(normalizedDraft, { walletRequired: !hasWallet })
    : {};
  const neighborCount = normalizedDraft.neighbors.length;
  const wordCount = draft.walletSecret.trim()
    ? draft.walletSecret.trim().split(/\s+/).length
    : 0;
  const exitCountryOptions = useMemo(
    () =>
      exitCountryOptionsForAvailability(
        availableExitCountries,
        draft.exitCountry,
        exitCountryInventoryReady,
      ),
    [availableExitCountries, draft.exitCountry, exitCountryInventoryReady],
  );

  const findNodes = useCallback(async (chain: Chain, signal: AbortSignal) => {
    setDiscoveryState('loading');
    setDiscoveryError(null);
    try {
      const baseUrl = await masqCore.getNodeFinderUrl();
      const nodes = await discoverEntryNodes(chain, { baseUrl, signal });
      if (!signal.aborted) {
        setNeighborsText(nodes.join('\n'));
        setDiscoveryState('ready');
      }
    } catch (caught) {
      if (!signal.aborted) {
        setDiscoveryState('error');
        setDiscoveryError(
          caught instanceof Error
            ? caught.message
            : 'MASQ entry node discovery failed.',
        );
      }
    }
  }, []);

  useEffect(() => {
    const existing = parseNeighborList(neighborsText).filter(node =>
      isDescriptorForChain(node, draft.chain),
    );
    if (existing.length >= 2 && discoveryRequest === 0) {
      setDiscoveryState('ready');
      return;
    }
    const controller = new AbortController();
    findNodes(draft.chain, controller.signal);
    return () => controller.abort();
    // Entry node text is deliberately excluded: manual edits must not restart discovery.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [discoveryRequest, draft.chain, findNodes]);

  const save = async () => {
    setSubmitted(true);
    if (
      Object.keys(
        validateConfig(normalizedDraft, { walletRequired: !hasWallet }),
      ).length > 0
    ) {
      return;
    }
    try {
      await onSave(normalizedDraft);
      setDraft(current => ({ ...current, walletSecret: '' }));
      onBack();
    } catch {
      // The controller exposes the native validation error in the banner.
    }
  };

  const chooseWalletMode = (walletImportMode: WalletImportMode) => {
    setSubmitted(false);
    setRecoveryPhraseVisible(false);
    setDraft(current => ({ ...current, walletImportMode, walletSecret: '' }));
  };

  const chooseChain = (chain: Chain) => {
    if (chain === draft.chain) {
      return;
    }
    setSubmitted(false);
    setNeighborsText('');
    setDraft(current => ({
      ...current,
      chain,
      rpcUrl: DEFAULT_RPC_URLS[chain],
    }));
  };

  return (
    <View style={styles.screen}>
      <ScreenHeader title="Set up MASQ" onBack={onBack} />
      <ScrollView
        contentContainerStyle={styles.content}
        keyboardShouldPersistTaps="handled"
        showsVerticalScrollIndicator={false}
      >
        <Text style={styles.eyebrow}>CONSUME MODE</Text>
        <Text style={styles.title}>Private access, powered by MASQ</Text>
        <Text style={styles.intro}>
          This mobile Node only consumes MASQ routes. It never serves traffic or
          acts as an exit for other users.
        </Text>
        <ErrorBanner message={error} />

        <SectionTitle number="1" title="Network" />
        <View style={styles.segment}>
          <Choice
            label="Base"
            value="base-mainnet"
            selected={draft.chain}
            onSelect={chooseChain}
          />
          <Choice
            label="Base Sepolia"
            value="base-sepolia"
            selected={draft.chain}
            onSelect={chooseChain}
          />
        </View>

        <View style={styles.networkCard}>
          <View style={styles.networkRow}>
            <View style={styles.networkIcon}>
              <Text style={styles.networkIconText}>RPC</Text>
            </View>
            <View style={styles.networkBody}>
              <Text style={styles.networkTitle}>
                {draft.chain === 'base-mainnet'
                  ? 'Nodies RPC ready'
                  : 'RPC ready'}
              </Text>
              <Text numberOfLines={1} style={styles.networkDetail}>
                {draft.rpcUrl}
              </Text>
            </View>
            <View style={styles.readyDot} />
          </View>

          <View style={styles.networkDivider} />

          <View style={styles.networkRow}>
            <View style={styles.networkIcon}>
              {discoveryState === 'loading' ? (
                <ActivityIndicator color={colors.violet} size="small" />
              ) : (
                <Text style={styles.networkIconText}>2×</Text>
              )}
            </View>
            <View style={styles.networkBody}>
              <Text style={styles.networkTitle}>
                {discoveryState === 'loading'
                  ? 'Finding entry nodes…'
                  : discoveryState === 'ready'
                  ? `${neighborCount} entry node${
                      neighborCount === 1 ? '' : 's'
                    } ready`
                  : 'Entry nodes unavailable'}
              </Text>
              <Text style={styles.networkDetail}>
                {discoveryState === 'error'
                  ? discoveryError
                  : 'Selected automatically by the MASQ network.'}
              </Text>
            </View>
            {discoveryState === 'ready' ? (
              <View style={styles.readyDot} />
            ) : null}
          </View>

          {discoveryState === 'error' ? (
            <Pressable
              accessibilityRole="button"
              onPress={() => setDiscoveryRequest(value => value + 1)}
              style={styles.retryButton}
            >
              <Text style={styles.retryText}>Try again</Text>
            </Pressable>
          ) : null}
        </View>

        <Pressable
          accessibilityRole="button"
          accessibilityState={{ expanded: showAdvanced }}
          onPress={() => setShowAdvanced(value => !value)}
          style={styles.advancedToggle}
        >
          <Text style={styles.advancedText}>Advanced network settings</Text>
          <Text style={styles.advancedChevron}>{showAdvanced ? '−' : '+'}</Text>
        </Pressable>

        {showAdvanced ? (
          <View style={styles.advancedFields}>
            <Field
              label="Blockchain RPC"
              value={draft.rpcUrl}
              placeholder="https://base-mainnet…"
              autoCapitalize="none"
              autoCorrect={false}
              keyboardType="url"
              error={errors.rpcUrl}
              onChangeText={rpcUrl =>
                setDraft(current => ({ ...current, rpcUrl }))
              }
            />

            <Field
              label="Entry nodes"
              value={neighborsText}
              placeholder={`masq://${draft.chain}:public-key@host:port`}
              autoCapitalize="none"
              autoCorrect={false}
              multiline
              numberOfLines={4}
              error={errors.neighbors}
              helper="Normally filled automatically. You can enter one descriptor per line as a fallback."
              onChangeText={value => {
                setNeighborsText(value);
                setDiscoveryState('ready');
                setDiscoveryError(null);
              }}
            />
          </View>
        ) : null}

        {!showAdvanced && (errors.rpcUrl || errors.neighbors) ? (
          <Text style={styles.collapsedError}>
            {errors.rpcUrl || errors.neighbors} Open Advanced network settings
            to fix it.
          </Text>
        ) : null}

        <SectionTitle number="2" title="Privacy route" />
        <Text style={styles.sectionIntro}>
          This build protects its private browser. Choose how MASQ should
          construct that browser route.
        </Text>

        <Label text="Route length" />
        <View style={styles.hopGrid}>
          {HOP_OPTIONS.map(hops => {
            const active = draft.minHops === hops;
            return (
              <Pressable
                accessibilityRole="radio"
                accessibilityState={{ checked: active }}
                key={hops}
                onPress={() =>
                  setDraft(current => ({ ...current, minHops: hops }))
                }
                style={[styles.hopChoice, active && styles.hopChoiceActive]}
              >
                <Text
                  style={[styles.hopValue, active && styles.hopValueActive]}
                >
                  {hops}
                </Text>
                <Text
                  style={[styles.hopLabel, active && styles.hopLabelActive]}
                >
                  {hops === 1 ? 'hop' : 'hops'}
                </Text>
              </Pressable>
            );
          })}
        </View>
        {errors.minHops ? (
          <Text style={styles.fieldError}>{errors.minHops}</Text>
        ) : null}
        <Text style={styles.routeHelper}>
          Three or more hops provide stronger route separation, but need a
          larger live MASQ neighborhood. One hop is faster and currently more
          reliable.
        </Text>

        <Label text="Exit country" />
        <Pressable
          accessibilityRole="button"
          onPress={() => setShowCountryPicker(true)}
          style={styles.pickerButton}
        >
          <View>
            <Text style={styles.pickerValue}>
              {exitCountryName(draft.exitCountry)}
            </Text>
            <Text style={styles.pickerMeta}>
              {draft.exitCountry
                ? availableExitCountries.includes(draft.exitCountry)
                  ? `${draft.exitCountry} · available in the live neighborhood`
                  : exitCountryInventoryReady
                  ? `${draft.exitCountry} · currently unavailable`
                  : `${draft.exitCountry} · availability checked after connecting`
                : 'MASQ chooses the best available exit'}
            </Text>
          </View>
          <Text style={styles.pickerChevron}>⌄</Text>
        </Pressable>
        {errors.exitCountry ? (
          <Text style={styles.fieldError}>{errors.exitCountry}</Text>
        ) : null}

        {draft.exitCountry ? (
          <>
            <Text style={styles.preferenceLabel}>
              WHEN THE COUNTRY IS UNAVAILABLE
            </Text>
            <View style={styles.segment}>
              <Choice
                label="Allow fallback"
                value="fallback"
                selected={draft.exitCountryFallback ? 'fallback' : 'strict'}
                onSelect={() =>
                  setDraft(current => ({
                    ...current,
                    exitCountryFallback: true,
                  }))
                }
              />
              <Choice
                label="Block the route"
                value="strict"
                selected={draft.exitCountryFallback ? 'fallback' : 'strict'}
                onSelect={() =>
                  setDraft(current => ({
                    ...current,
                    exitCountryFallback: false,
                  }))
                }
              />
            </View>
          </>
        ) : null}

        <SectionTitle number="3" title="Consumer wallet" />
        <View style={styles.segment}>
          <Choice
            label="12 words"
            value="seedPhrase"
            selected={draft.walletImportMode}
            onSelect={chooseWalletMode}
          />
          <Choice
            label="Private key"
            value="privateKey"
            selected={draft.walletImportMode}
            onSelect={chooseWalletMode}
          />
        </View>

        {draft.walletImportMode === 'seedPhrase' ? (
          <Field
            accessibilityLabel="Recovery phrase input"
            label={hasWallet ? 'Recovery phrase (optional)' : 'Recovery phrase'}
            value={draft.walletSecret}
            placeholder={
              hasWallet
                ? 'Leave empty to keep the saved wallet'
                : 'word 1  word 2  word 3…'
            }
            autoCapitalize="none"
            autoComplete="off"
            autoCorrect={false}
            importantForAutofill="noExcludeDescendants"
            secureTextEntry={!recoveryPhraseVisible}
            spellCheck={false}
            textContentType="none"
            multiline
            numberOfLines={4}
            error={errors.walletSecret}
            helper={
              hasWallet
                ? "Leave this empty to keep the securely saved wallet, or enter 12 words to replace it. MASQ uses m/44'/60'/0'/0/0."
                : "Enter the 12 English words in order. MASQ uses m/44'/60'/0'/0/0 for the consumer wallet."
            }
            meta={
              hasWallet && !wordCount
                ? 'Saved securely on this device'
                : `${wordCount}/12 words`
            }
            actionLabel={recoveryPhraseVisible ? 'Hide' : 'Show'}
            actionAccessibilityLabel={
              recoveryPhraseVisible
                ? 'Hide recovery phrase'
                : 'Show recovery phrase'
            }
            onAction={() => setRecoveryPhraseVisible(current => !current)}
            onChangeText={walletSecret =>
              setDraft(current => ({ ...current, walletSecret }))
            }
          />
        ) : (
          <Field
            label={
              hasWallet
                ? 'Consumer wallet private key (optional)'
                : 'Consumer wallet private key'
            }
            value={draft.walletSecret}
            placeholder={
              hasWallet
                ? 'Leave empty to keep the saved wallet'
                : '64 hexadecimal characters'
            }
            autoCapitalize="none"
            autoCorrect={false}
            spellCheck={false}
            secureTextEntry
            error={errors.walletSecret}
            helper={
              hasWallet
                ? 'Leave this empty to keep the securely saved wallet, or enter a key to replace it.'
                : 'The key is passed directly to native process memory and cleared from this form after import.'
            }
            onChangeText={walletSecret =>
              setDraft(current => ({ ...current, walletSecret }))
            }
          />
        )}

        <View style={styles.notice}>
          <View style={styles.noticeDot} />
          <View style={styles.noticeBody}>
            <Text style={styles.noticeTitle}>Before you connect</Text>
            <Text style={styles.noticeText}>
              Your chosen RPC receives your IP address, wallet address and
              blockchain requests. MASQ peers process the routing metadata
              needed to carry traffic. This app has no ads, analytics or
              cross-app tracking, and the wallet secret stays in secure device
              storage.
            </Text>
          </View>
        </View>

        <View style={styles.notice}>
          <View style={styles.noticeDot} />
          <View style={styles.noticeBody}>
            <Text style={styles.noticeTitle}>Fail-closed by design</Text>
            <Text style={styles.noticeText}>
              If the local MASQ proxy or route fails, MASQ Private browsing is
              blocked. MASQ Private never falls back to a direct connection.
            </Text>
          </View>
        </View>

        <Button
          label="Save configuration"
          onPress={save}
          busy={busy}
          disabled={discoveryState === 'loading'}
        />
      </ScrollView>

      <Modal
        animationType="fade"
        onRequestClose={() => setShowCountryPicker(false)}
        transparent
        visible={showCountryPicker}
      >
        <View style={styles.modalBackdrop}>
          <View style={styles.modalCard}>
            <View style={styles.modalHeader}>
              <View>
                <Text style={styles.modalTitle}>Choose exit country</Text>
                <Text style={styles.modalSubtitle}>
                  {exitCountryInventoryReady
                    ? `${availableExitCountries.length} countr${
                        availableExitCountries.length === 1 ? 'y' : 'ies'
                      } currently available in the live MASQ neighborhood.`
                    : 'Connect once to load the live MASQ exit inventory.'}
                </Text>
              </View>
              <Pressable
                accessibilityLabel="Close country picker"
                accessibilityRole="button"
                onPress={() => setShowCountryPicker(false)}
                style={styles.modalClose}
              >
                <Text style={styles.modalCloseText}>×</Text>
              </Pressable>
            </View>
            <ScrollView showsVerticalScrollIndicator={false}>
              {exitCountryOptions.map(option => {
                const selected = draft.exitCountry === option.code;
                const live =
                  option.code !== null &&
                  availableExitCountries.includes(option.code);
                return (
                  <Pressable
                    accessibilityRole="radio"
                    accessibilityState={{ checked: selected }}
                    key={option.code || 'automatic'}
                    onPress={() => {
                      setDraft(current => ({
                        ...current,
                        exitCountry: option.code,
                      }));
                      setShowCountryPicker(false);
                    }}
                    style={[
                      styles.countryRow,
                      selected && styles.countryRowSelected,
                    ]}
                  >
                    <Text
                      style={[
                        styles.countryName,
                        selected && styles.countryNameSelected,
                      ]}
                    >
                      {option.name}
                    </Text>
                    <Text style={styles.countryCode}>
                      {live ? 'LIVE' : option.code || 'AUTO'}
                    </Text>
                  </Pressable>
                );
              })}
            </ScrollView>
          </View>
        </View>
      </Modal>
    </View>
  );
}

function SectionTitle({ number, title }: { number: string; title: string }) {
  return (
    <View style={styles.sectionTitle}>
      <View style={styles.sectionNumber}>
        <Text style={styles.sectionNumberText}>{number}</Text>
      </View>
      <Text style={styles.sectionTitleText}>{title}</Text>
    </View>
  );
}

function Choice<T extends string>({
  label,
  value,
  selected,
  onSelect,
}: {
  label: string;
  value: T;
  selected: T;
  onSelect: (value: T) => void;
}) {
  const active = value === selected;
  return (
    <Pressable
      accessibilityRole="radio"
      accessibilityState={{ checked: active }}
      onPress={() => onSelect(value)}
      style={[styles.segmentItem, active && styles.segmentActive]}
    >
      <Text style={[styles.segmentText, active && styles.segmentTextActive]}>
        {label}
      </Text>
    </Pressable>
  );
}

function Label({ text }: { text: string }) {
  return <Text style={styles.label}>{text}</Text>;
}

interface FieldProps extends TextInputProps {
  label: string;
  error?: string;
  helper?: string;
  meta?: string;
  actionLabel?: string;
  actionAccessibilityLabel?: string;
  onAction?: () => void;
}

function Field({
  label,
  error,
  helper,
  meta,
  actionLabel,
  actionAccessibilityLabel,
  onAction,
  ...inputProps
}: FieldProps) {
  return (
    <View style={styles.field}>
      <View style={styles.labelRow}>
        <Label text={label} />
        <View style={styles.fieldMetaActions}>
          {meta ? <Text style={styles.meta}>{meta}</Text> : null}
          {actionLabel && onAction ? (
            <Pressable
              accessibilityLabel={actionAccessibilityLabel || actionLabel}
              accessibilityRole="button"
              hitSlop={8}
              onPress={onAction}
            >
              <Text style={styles.fieldAction}>{actionLabel}</Text>
            </Pressable>
          ) : null}
        </View>
      </View>
      <TextInput
        placeholderTextColor="#587086"
        selectionColor={colors.violet}
        style={[
          styles.input,
          inputProps.multiline && styles.multiline,
          error && styles.inputError,
        ]}
        {...inputProps}
      />
      {error ? <Text style={styles.fieldError}>{error}</Text> : null}
      {!error && helper ? <Text style={styles.helper}>{helper}</Text> : null}
    </View>
  );
}

const styles = StyleSheet.create({
  screen: { backgroundColor: colors.ink, flex: 1 },
  content: { paddingBottom: 44, paddingHorizontal: 20, paddingTop: 16 },
  eyebrow: {
    color: colors.violet,
    fontSize: 11,
    fontWeight: '900',
    letterSpacing: 1.8,
    marginBottom: 10,
  },
  title: {
    color: colors.white,
    fontSize: 30,
    fontWeight: '800',
    letterSpacing: -0.8,
    lineHeight: 36,
  },
  intro: {
    color: colors.muted,
    fontSize: 14,
    lineHeight: 21,
    marginBottom: 28,
    marginTop: 10,
  },
  sectionTitle: {
    alignItems: 'center',
    flexDirection: 'row',
    marginBottom: 14,
    marginTop: 2,
  },
  sectionNumber: {
    alignItems: 'center',
    backgroundColor: '#10375A',
    borderRadius: 14,
    height: 28,
    justifyContent: 'center',
    marginRight: 10,
    width: 28,
  },
  sectionNumberText: { color: colors.violet, fontSize: 12, fontWeight: '900' },
  sectionTitleText: { color: colors.white, fontSize: 18, fontWeight: '800' },
  sectionIntro: {
    color: colors.muted,
    fontSize: 13,
    lineHeight: 19,
    marginBottom: 14,
    marginTop: -5,
  },
  segment: {
    backgroundColor: '#081522',
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    flexDirection: 'row',
    marginBottom: 22,
    padding: 4,
  },
  segmentItem: {
    alignItems: 'center',
    borderRadius: 12,
    flex: 1,
    paddingVertical: 12,
  },
  segmentActive: { backgroundColor: colors.violet },
  segmentText: { color: colors.muted, fontSize: 14, fontWeight: '700' },
  segmentTextActive: { color: colors.white },
  networkCard: {
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    marginBottom: 12,
    padding: 15,
  },
  networkRow: { alignItems: 'center', flexDirection: 'row' },
  networkIcon: {
    alignItems: 'center',
    backgroundColor: '#10375A',
    borderRadius: 12,
    height: 42,
    justifyContent: 'center',
    marginRight: 12,
    width: 42,
  },
  networkIconText: { color: colors.violet, fontSize: 11, fontWeight: '900' },
  networkBody: { flex: 1 },
  networkTitle: { color: colors.white, fontSize: 14, fontWeight: '800' },
  networkDetail: {
    color: '#71879B',
    fontSize: 11,
    lineHeight: 16,
    marginTop: 3,
  },
  readyDot: {
    backgroundColor: colors.mint,
    borderRadius: 5,
    height: 10,
    marginLeft: 10,
    width: 10,
  },
  networkDivider: {
    backgroundColor: colors.line,
    height: 1,
    marginVertical: 14,
  },
  retryButton: {
    alignItems: 'center',
    borderColor: colors.violet,
    borderRadius: 10,
    borderWidth: 1,
    marginTop: 13,
    paddingVertical: 10,
  },
  retryText: { color: colors.violet, fontSize: 13, fontWeight: '800' },
  advancedToggle: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'space-between',
    marginBottom: 24,
    paddingHorizontal: 3,
    paddingVertical: 6,
  },
  advancedText: { color: '#93AABD', fontSize: 13, fontWeight: '700' },
  advancedChevron: { color: colors.violet, fontSize: 22, fontWeight: '400' },
  advancedFields: { marginTop: 2 },
  collapsedError: {
    color: colors.red,
    fontSize: 12,
    lineHeight: 18,
    marginBottom: 20,
  },
  scopeList: { gap: 9, marginBottom: 22 },
  scopeRow: {
    alignItems: 'center',
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    flexDirection: 'row',
    padding: 14,
  },
  scopeRowActive: { borderColor: colors.violet },
  scopeRowDisabled: { opacity: 0.62 },
  scopeIndicator: {
    alignItems: 'center',
    backgroundColor: '#122535',
    borderRadius: 13,
    height: 26,
    justifyContent: 'center',
    marginRight: 11,
    width: 26,
  },
  scopeIndicatorActive: { backgroundColor: colors.violet },
  scopeIndicatorText: { color: colors.white, fontSize: 12, fontWeight: '900' },
  scopeBody: { flex: 1 },
  scopeTitle: { color: colors.white, fontSize: 14, fontWeight: '800' },
  scopeTextDisabled: { color: '#8193A4' },
  scopeDetail: { color: '#71879B', fontSize: 11, lineHeight: 16, marginTop: 3 },
  scopeLock: {
    color: '#587086',
    fontSize: 9,
    fontWeight: '900',
    letterSpacing: 0.8,
  },
  hopGrid: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: 8,
    marginBottom: 2,
  },
  hopChoice: {
    alignItems: 'center',
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: 13,
    borderWidth: 1,
    paddingVertical: 11,
    width: '31%',
  },
  hopChoiceActive: { backgroundColor: '#173B67', borderColor: colors.violet },
  hopValue: { color: colors.muted, fontSize: 18, fontWeight: '900' },
  hopValueActive: { color: colors.white },
  hopLabel: { color: '#71879B', fontSize: 10, fontWeight: '700', marginTop: 1 },
  hopLabelActive: { color: '#A8D9FA' },
  routeHelper: {
    color: '#71879B',
    fontSize: 12,
    lineHeight: 17,
    marginBottom: 20,
    marginTop: 8,
  },
  pickerButton: {
    alignItems: 'center',
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    flexDirection: 'row',
    justifyContent: 'space-between',
    marginBottom: 8,
    minHeight: 58,
    paddingHorizontal: 15,
    paddingVertical: 11,
  },
  pickerValue: { color: colors.white, fontSize: 14, fontWeight: '800' },
  pickerMeta: { color: '#71879B', fontSize: 10, marginTop: 3 },
  pickerChevron: { color: colors.violet, fontSize: 22 },
  preferenceLabel: {
    color: '#71879B',
    fontSize: 10,
    fontWeight: '900',
    letterSpacing: 1,
    marginBottom: 8,
    marginTop: 13,
  },
  labelRow: {
    alignItems: 'center',
    flexDirection: 'row',
    justifyContent: 'space-between',
  },
  fieldMetaActions: {
    alignItems: 'center',
    flexDirection: 'row',
    gap: 12,
  },
  fieldAction: {
    color: colors.violet,
    fontSize: 12,
    fontWeight: '800',
    marginBottom: 9,
  },
  label: {
    color: colors.white,
    fontSize: 13,
    fontWeight: '700',
    marginBottom: 9,
  },
  meta: {
    color: colors.violet,
    fontSize: 12,
    fontWeight: '700',
    marginBottom: 9,
  },
  field: { marginBottom: 22 },
  input: {
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.medium,
    borderWidth: 1,
    color: colors.white,
    fontSize: 15,
    minHeight: 54,
    paddingHorizontal: 15,
    paddingVertical: 14,
  },
  multiline: { minHeight: 110, textAlignVertical: 'top' },
  inputError: { borderColor: colors.red },
  fieldError: { color: colors.red, fontSize: 12, marginTop: 7 },
  helper: { color: '#71879B', fontSize: 12, lineHeight: 17, marginTop: 7 },
  notice: {
    backgroundColor: '#0B2030',
    borderColor: '#174A66',
    borderRadius: radii.medium,
    borderWidth: 1,
    flexDirection: 'row',
    marginBottom: 24,
    padding: 16,
  },
  noticeDot: {
    backgroundColor: colors.mint,
    borderRadius: 5,
    height: 10,
    marginRight: 12,
    marginTop: 4,
    width: 10,
  },
  noticeBody: { flex: 1 },
  noticeTitle: {
    color: colors.white,
    fontSize: 13,
    fontWeight: '800',
    marginBottom: 5,
  },
  noticeText: { color: '#93AABD', fontSize: 12, lineHeight: 18 },
  modalBackdrop: {
    backgroundColor: 'rgba(2, 9, 16, 0.88)',
    flex: 1,
    justifyContent: 'flex-end',
    padding: 16,
  },
  modalCard: {
    backgroundColor: '#081827',
    borderColor: colors.line,
    borderRadius: 24,
    borderWidth: 1,
    maxHeight: '78%',
    padding: 18,
  },
  modalHeader: {
    alignItems: 'flex-start',
    flexDirection: 'row',
    justifyContent: 'space-between',
    marginBottom: 15,
  },
  modalTitle: { color: colors.white, fontSize: 20, fontWeight: '900' },
  modalSubtitle: { color: colors.muted, fontSize: 11, marginTop: 4 },
  modalClose: {
    alignItems: 'center',
    backgroundColor: '#122535',
    borderRadius: 16,
    height: 32,
    justifyContent: 'center',
    width: 32,
  },
  modalCloseText: { color: colors.white, fontSize: 22, lineHeight: 24 },
  countryRow: {
    alignItems: 'center',
    borderBottomColor: colors.line,
    borderBottomWidth: StyleSheet.hairlineWidth,
    flexDirection: 'row',
    justifyContent: 'space-between',
    paddingHorizontal: 12,
    paddingVertical: 14,
  },
  countryRowSelected: { backgroundColor: '#10375A', borderRadius: 12 },
  countryName: { color: '#B8C8D6', fontSize: 14, fontWeight: '700' },
  countryNameSelected: { color: colors.white },
  countryCode: { color: colors.violet, fontSize: 11, fontWeight: '900' },
});
