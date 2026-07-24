module.exports = {
  preset: '@react-native/jest-preset',
  // React Test Renderer uses these queues while modern fake timers are active.
  fakeTimers: {
    doNotFake: ['nextTick', 'setImmediate', 'queueMicrotask'],
  },
  watchman: false,
  transformIgnorePatterns: [
    'node_modules/(?!((@)?react-native|react-native-webview)/)',
  ],
};
