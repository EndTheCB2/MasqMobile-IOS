import type {PropsWithChildren} from 'react';
import {
  ActivityIndicator,
  Image,
  Pressable,
  StyleSheet,
  Text,
  View,
  type PressableProps,
  type ViewStyle,
} from 'react-native';

import {colors, radii} from './theme';

const masqLogo = require('../../assets/masq-logo.png');

export function BrandMark({
  small = false,
  large = false,
}: {
  small?: boolean;
  large?: boolean;
}) {
  return (
    <View style={[styles.brand, small && styles.brandSmall, large && styles.brandLarge]}>
      <Image accessibilityLabel="MASQ" source={masqLogo} style={styles.brandImage} />
    </View>
  );
}

interface ButtonProps extends PressableProps {
  label: string;
  busy?: boolean;
  tone?: 'primary' | 'secondary' | 'danger';
}

export function Button({
  label,
  busy = false,
  tone = 'primary',
  disabled,
  style,
  ...props
}: ButtonProps) {
  return (
    <Pressable
      accessibilityRole="button"
      disabled={disabled || busy}
      style={({pressed}) => [
        styles.button,
        tone === 'secondary' && styles.buttonSecondary,
        tone === 'danger' && styles.buttonDanger,
        (disabled || busy) && styles.buttonDisabled,
        pressed && styles.buttonPressed,
        style as ViewStyle,
      ]}
      {...props}>
      {busy ? (
        <ActivityIndicator color={colors.white} />
      ) : (
        <Text
          style={[
            styles.buttonLabel,
            tone !== 'primary' && styles.buttonLabelSecondary,
          ]}>
          {label}
        </Text>
      )}
    </Pressable>
  );
}

export function ScreenHeader({
  title,
  onBack,
}: {
  title: string;
  onBack?: () => void;
}) {
  return (
    <View style={styles.header}>
      {onBack ? (
        <Pressable
          accessibilityLabel="Back"
          accessibilityRole="button"
          hitSlop={12}
          onPress={onBack}
          style={styles.headerSide}>
          <Text accessibilityLabel="Back" style={styles.back}>‹</Text>
        </Pressable>
      ) : (
        <View style={styles.headerSide}>
          <BrandMark small />
        </View>
      )}
      <Text style={styles.headerTitle}>{title}</Text>
      <View style={styles.headerSide} />
    </View>
  );
}

export function ErrorBanner({message}: {message: string | null}) {
  if (!message) {
    return null;
  }
  return (
    <View accessibilityRole="alert" style={styles.errorBanner}>
      <Text style={styles.errorText}>{message}</Text>
    </View>
  );
}

export function Card({children}: PropsWithChildren) {
  return <View style={styles.card}>{children}</View>;
}

const styles = StyleSheet.create({
  brand: {
    alignItems: 'center',
    borderRadius: 28,
    height: 56,
    justifyContent: 'center',
    overflow: 'hidden',
    width: 56,
  },
  brandSmall: {
    borderRadius: 17,
    height: 34,
    width: 34,
  },
  brandLarge: {borderRadius: 46, height: 92, width: 92},
  brandImage: {height: '100%', resizeMode: 'contain', width: '100%'},
  button: {
    alignItems: 'center',
    backgroundColor: colors.violet,
    borderRadius: radii.medium,
    justifyContent: 'center',
    minHeight: 54,
    paddingHorizontal: 20,
  },
  buttonSecondary: {
    backgroundColor: colors.panelRaised,
    borderColor: colors.line,
    borderWidth: 1,
  },
  buttonDanger: {
    backgroundColor: 'transparent',
    borderColor: colors.red,
    borderWidth: 1,
  },
  buttonDisabled: {
    opacity: 0.45,
  },
  buttonPressed: {
    opacity: 0.78,
    transform: [{scale: 0.99}],
  },
  buttonLabel: {
    color: colors.white,
    fontSize: 16,
    fontWeight: '700',
  },
  buttonLabelSecondary: {
    color: colors.white,
  },
  header: {
    alignItems: 'center',
    flexDirection: 'row',
    minHeight: 58,
    paddingHorizontal: 20,
  },
  headerSide: {
    alignItems: 'flex-start',
    justifyContent: 'center',
    width: 48,
  },
  headerTitle: {
    color: colors.white,
    flex: 1,
    fontSize: 17,
    fontWeight: '700',
    textAlign: 'center',
  },
  back: {
    color: colors.white,
    fontSize: 38,
    fontWeight: '300',
    lineHeight: 40,
  },
  errorBanner: {
    backgroundColor: '#351A22',
    borderColor: '#6C2D3A',
    borderRadius: radii.small,
    borderWidth: 1,
    marginBottom: 16,
    padding: 13,
  },
  errorText: {
    color: '#FFB7C0',
    fontSize: 14,
    lineHeight: 20,
  },
  card: {
    backgroundColor: colors.panel,
    borderColor: colors.line,
    borderRadius: radii.large,
    borderWidth: 1,
    padding: 18,
  },
});
