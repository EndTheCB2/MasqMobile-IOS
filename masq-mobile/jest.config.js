module.exports = {
  preset: '@react-native/jest-preset',
  watchman: false,
  transformIgnorePatterns: [
    'node_modules/(?!((@)?react-native|react-native-webview)/)',
  ],
};
