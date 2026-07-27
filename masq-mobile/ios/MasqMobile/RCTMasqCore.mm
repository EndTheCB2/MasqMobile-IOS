#import "RCTMasqCore.h"

#import <CFNetwork/CFNetwork.h>
#import <Network/Network.h>
#import <Security/Security.h>
#import <WebKit/WebKit.h>
#import <errno.h>
#import <math.h>
#import <string.h>
#import <unistd.h>

#include "../../native/masq-mobile-core/include/masq_mobile_core.h"

namespace {
constexpr uint16_t MasqBlockedBrowserProxyPort = 1;

#ifndef MASQ_PRIVATE_YOUTUBE_AD_BLOCKER
#define MASQ_PRIVATE_YOUTUBE_AD_BLOCKER 0
#endif

NSString *const MasqBrowserBlockAdsKey = @"MASQBrowserBlockAdsAndTrackers";
NSString *const MasqBrowserBlockCrossSiteCookiesKey =
    @"MASQBrowserBlockCrossSiteCookies";
NSString *const MasqBrowserHideCookieBannersKey =
    @"MASQBrowserHideCookieBanners";
NSString *const MasqBrowserRejectOptionalCookiesKey =
    @"MASQBrowserRejectOptionalCookies";
NSString *const MasqBrowserYouTubeBestEffortKey =
    @"MASQBrowserYouTubeBestEffort";
NSString *const MasqBrowserRememberedMasqSitesKey =
    @"MASQBrowserRememberedMasqSitesV1";
NSString *const MasqBrowserRememberedDirectSitesKey =
    @"MASQBrowserRememberedDirectSitesV1";
NSString *const MasqBrowserProtectionDisabledSitesKey =
    @"MASQBrowserProtectionDisabledSitesV1";

NSString *const MasqBrowserCookieRulesIdentifier =
    @"ai.masq.mobile.browser.cookies.v2";
NSString *const MasqBrowserAdRulesIdentifier =
    @"ai.masq.mobile.browser.ads.v2";
NSString *const MasqBrowserBannerRulesIdentifier =
    @"ai.masq.mobile.browser.cookie-banners.v2";
#if MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1
NSString *const MasqBrowserYouTubeRulesIdentifier =
    @"ai.masq.mobile.browser.youtube-private.v1";
#endif

NSObject *browserProtectionLock() {
  static NSObject *lock;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    lock = [NSObject new];
  });
  return lock;
}

static NSArray<WKContentRuleList *> *gActiveBrowserContentRuleLists = nil;
static BOOL gYouTubeBestEffortEnabled = NO;
static NSUInteger gBrowserProtectionGeneration = 0;

NSArray<WKContentRuleList *> *activeBrowserContentRuleLists() {
  return gActiveBrowserContentRuleLists ?: @[];
}

void setActiveBrowserContentRuleLists(NSArray<WKContentRuleList *> *ruleLists) {
  gActiveBrowserContentRuleLists = [ruleLists copy];
}

NSArray<WKContentRuleList *> *currentBrowserContentRuleLists() {
  return activeBrowserContentRuleLists();
}

BOOL currentYouTubeBestEffortEnabled() {
  return gYouTubeBestEffortEnabled;
}

void setCurrentYouTubeBestEffortEnabled(BOOL enabled) {
  gYouTubeBestEffortEnabled = enabled;
}

NSUInteger beginBrowserProtectionOperation() {
  @synchronized(browserProtectionLock()) {
    gBrowserProtectionGeneration += 1;
    return gBrowserProtectionGeneration;
  }
}

BOOL isCurrentBrowserProtectionOperation(NSUInteger generation) {
  @synchronized(browserProtectionLock()) {
    return generation == gBrowserProtectionGeneration;
  }
}

NSError *staleBrowserProtectionError() {
  return [NSError
      errorWithDomain:@"MASQBrowserProtection"
                 code:2
             userInfo:@{
               NSLocalizedDescriptionKey :
                   @"A newer browser-protection operation replaced this one.",
             }];
}

void configurePrivateBrowserProxy(WKWebsiteDataStore *dataStore, uint16_t port) {
  if (@available(iOS 17.0, *)) {
    NSString *portString = [NSString stringWithFormat:@"%u", port];
    nw_endpoint_t endpoint =
        nw_endpoint_create_host("127.0.0.1", portString.UTF8String);
    nw_proxy_config_t proxy = nw_proxy_config_create_http_connect(endpoint, nullptr);
    nw_proxy_config_set_failover_allowed(proxy, false);
    dataStore.proxyConfigurations = @[ proxy ];
  }
}

WKWebsiteDataStore *masqBrowserDataStore() {
  static WKWebsiteDataStore *dataStore = nil;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    dataStore = [WKWebsiteDataStore nonPersistentDataStore];
    // The real localhost MASQ port replaces this sink only after the native core reports a
    // usable route.
    configurePrivateBrowserProxy(dataStore, MasqBlockedBrowserProxyPort);
  });
  return dataStore;
}

WKWebsiteDataStore *directBrowserDataStore() {
  static WKWebsiteDataStore *dataStore = nil;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    dataStore = [WKWebsiteDataStore nonPersistentDataStore];
    // Direct browsing is an explicit runtime choice. Until that choice is made, this separate
    // non-persistent store is just as fail-closed as the MASQ-routed store.
    configurePrivateBrowserProxy(dataStore, MasqBlockedBrowserProxyPort);
  });
  return dataStore;
}

WKWebsiteDataStore *masqPersistentBrowserDataStore() {
  static WKWebsiteDataStore *dataStore = nil;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    if (@available(iOS 17.0, *)) {
      NSUUID *identifier = [[NSUUID alloc]
          initWithUUIDString:@"F29BA5A5-B51C-4B1B-9D4F-1295CF5301A1"];
      dataStore = [WKWebsiteDataStore dataStoreForIdentifier:identifier];
    } else {
      dataStore = [WKWebsiteDataStore nonPersistentDataStore];
    }
    configurePrivateBrowserProxy(dataStore, MasqBlockedBrowserProxyPort);
  });
  return dataStore;
}

WKWebsiteDataStore *directPersistentBrowserDataStore() {
  static WKWebsiteDataStore *dataStore = nil;
  static dispatch_once_t onceToken;
  dispatch_once(&onceToken, ^{
    if (@available(iOS 17.0, *)) {
      NSUUID *identifier = [[NSUUID alloc]
          initWithUUIDString:@"6C647FF5-9744-4CC6-B0B9-E66D0D91D4BC"];
      dataStore = [WKWebsiteDataStore dataStoreForIdentifier:identifier];
    } else {
      dataStore = [WKWebsiteDataStore nonPersistentDataStore];
    }
    configurePrivateBrowserProxy(dataStore, MasqBlockedBrowserProxyPort);
  });
  return dataStore;
}

NSArray<WKWebsiteDataStore *> *allBrowserDataStores() {
  return @[
    masqBrowserDataStore(),
    directBrowserDataStore(),
    masqPersistentBrowserDataStore(),
    directPersistentBrowserDataStore(),
  ];
}

void clearBrowserDataStores(
    NSArray<WKWebsiteDataStore *> *dataStores,
    void (^completion)(void)) {
  dispatch_group_t group = dispatch_group_create();
  for (WKWebsiteDataStore *dataStore in dataStores) {
    dispatch_group_enter(group);
    [dataStore
        removeDataOfTypes:WKWebsiteDataStore.allWebsiteDataTypes
            modifiedSince:NSDate.distantPast
        completionHandler:^{
          dispatch_group_leave(group);
        }];
  }
  dispatch_group_notify(group, dispatch_get_main_queue(), completion);
}

void clearTemporaryBrowserDataStores(void (^completion)(void)) {
  clearBrowserDataStores(
      @[ masqBrowserDataStore(), directBrowserDataStore() ], completion);
}

void clearAllBrowserDataStores(void (^completion)(void)) {
  clearBrowserDataStores(allBrowserDataStores(), completion);
}

BOOL isSafeBrowserHostname(NSString *hostname) {
  if (![hostname isKindOfClass:[NSString class]] || hostname.length < 3 ||
      hostname.length > 253 ||
      ![hostname isEqualToString:hostname.lowercaseString] ||
      [hostname hasSuffix:@"."] || [hostname isEqualToString:@"localhost"] ||
      [hostname hasSuffix:@".local"] || [hostname rangeOfString:@"."].location ==
          NSNotFound) {
    return NO;
  }
  NSRegularExpression *expression = [NSRegularExpression
      regularExpressionWithPattern:
          @"^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\\.)+[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$"
                         options:0
                           error:nil];
  return [expression firstMatchInString:hostname
                                options:0
                                  range:NSMakeRange(0, hostname.length)] != nil;
}

NSString *_Nullable rememberedSitesKeyForMode(NSString *mode) {
  if ([mode isEqualToString:@"masq"]) {
    return MasqBrowserRememberedMasqSitesKey;
  }
  if ([mode isEqualToString:@"direct"]) {
    return MasqBrowserRememberedDirectSitesKey;
  }
  return nil;
}

WKWebsiteDataStore *_Nullable persistentBrowserDataStoreForMode(
    NSString *mode) {
  if ([mode isEqualToString:@"masq"]) {
    return masqPersistentBrowserDataStore();
  }
  if ([mode isEqualToString:@"direct"]) {
    return directPersistentBrowserDataStore();
  }
  return nil;
}

NSMutableSet<NSString *> *savedBrowserHostnameSet(NSString *key) {
  NSArray *saved = [NSUserDefaults.standardUserDefaults arrayForKey:key] ?: @[];
  NSMutableSet<NSString *> *result = [NSMutableSet set];
  for (id value in saved) {
    if ([value isKindOfClass:[NSString class]] &&
        isSafeBrowserHostname((NSString *)value)) {
      [result addObject:value];
    }
  }
  return result;
}

void saveBrowserHostnameSet(NSSet<NSString *> *hostnames, NSString *key) {
  NSArray<NSString *> *sorted =
      [hostnames.allObjects sortedArrayUsingSelector:@selector(compare:)];
  [NSUserDefaults.standardUserDefaults setObject:sorted forKey:key];
}

NSArray<NSString *> *browserProtectionDisabledDomains() {
  NSSet<NSString *> *hostnames =
      savedBrowserHostnameSet(MasqBrowserProtectionDisabledSitesKey);
  NSMutableArray<NSString *> *domains = [NSMutableArray array];
  for (NSString *hostname in hostnames) {
    [domains addObject:hostname];
    [domains addObject:[@"*" stringByAppendingString:hostname]];
  }
  return domains;
}

NSDictionary *browserSiteSettingsResponse(
    NSString *mode,
    NSString *hostname) {
  NSString *rememberedKey = rememberedSitesKeyForMode(mode);
  NSSet<NSString *> *remembered =
      rememberedKey ? savedBrowserHostnameSet(rememberedKey) : [NSSet set];
  NSSet<NSString *> *disabled =
      savedBrowserHostnameSet(MasqBrowserProtectionDisabledSitesKey);
  return @{
    @"hostname" : hostname,
    @"mode" : mode,
    @"persistentSessionsSupported" : @YES,
    @"protectionDisabled" : @([disabled containsObject:hostname]),
    @"rememberSignIn" : @([remembered containsObject:hostname]),
  };
}

NSString *_Nullable serializeBrowserSiteSettings(
    NSDictionary *settings) {
  NSData *data =
      [NSJSONSerialization dataWithJSONObject:settings options:0 error:nil];
  return data
      ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding]
      : nil;
}

void clearBrowserWebsiteDataForHostname(
    WKWebsiteDataStore *dataStore,
    NSString *hostname,
    void (^completion)(void)) {
  NSSet<NSString *> *dataTypes = WKWebsiteDataStore.allWebsiteDataTypes;
  [dataStore fetchDataRecordsOfTypes:dataTypes
                  completionHandler:^(NSArray<WKWebsiteDataRecord *> *records) {
    NSPredicate *matching = [NSPredicate
        predicateWithBlock:^BOOL(WKWebsiteDataRecord *record,
                                 NSDictionary *bindings) {
              NSString *name = record.displayName.lowercaseString;
              return name.length > 0 &&
                  ([name isEqualToString:hostname] ||
                  [name hasSuffix:[@"." stringByAppendingString:hostname]] ||
                  [hostname hasSuffix:[@"." stringByAppendingString:name]]);
        }];
    NSArray<WKWebsiteDataRecord *> *matches =
        [records filteredArrayUsingPredicate:matching];
    if (matches.count == 0) {
      dispatch_async(dispatch_get_main_queue(), completion);
      return;
    }
    [dataStore removeDataOfTypes:dataTypes
                 forDataRecords:matches
              completionHandler:^{
                dispatch_async(dispatch_get_main_queue(), completion);
              }];
  }];
}

BOOL isJsonBoolean(id value) {
  return [value isKindOfClass:[NSNumber class]] &&
      CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID();
}

NSDictionary *browserProtectionPreferencesFromDefaults() {
  @synchronized(browserProtectionLock()) {
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    const BOOL youtubeAvailable = MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1;
    return @{
      @"blockAdsAndTrackers" :
          [defaults objectForKey:MasqBrowserBlockAdsKey]
              ? @([defaults boolForKey:MasqBrowserBlockAdsKey])
              : @YES,
      @"blockCrossSiteCookies" :
          [defaults objectForKey:MasqBrowserBlockCrossSiteCookiesKey]
              ? @([defaults boolForKey:MasqBrowserBlockCrossSiteCookiesKey])
              : @YES,
      @"hideCookieBanners" :
          [defaults objectForKey:MasqBrowserHideCookieBannersKey]
              ? @([defaults boolForKey:MasqBrowserHideCookieBannersKey])
              : @NO,
      @"rejectOptionalCookies" :
          [defaults objectForKey:MasqBrowserRejectOptionalCookiesKey]
              ? @([defaults boolForKey:MasqBrowserRejectOptionalCookiesKey])
              : @NO,
      @"youtubeBestEffort" :
          youtubeAvailable
              ? ([defaults objectForKey:MasqBrowserYouTubeBestEffortKey]
                     ? @([defaults boolForKey:MasqBrowserYouTubeBestEffortKey])
                     : @NO)
              : @NO,
    };
  }
}

NSDictionary *_Nullable decodeBrowserProtectionPreferences(
    NSString *serialized,
    NSString *_Nullable *_Nullable errorMessage) {
  NSData *data = [serialized dataUsingEncoding:NSUTF8StringEncoding];
  NSError *jsonError = nil;
  id decoded = data
      ? [NSJSONSerialization JSONObjectWithData:data options:0 error:&jsonError]
      : nil;
  NSDictionary *dictionary =
      [decoded isKindOfClass:[NSDictionary class]] ? decoded : nil;
  NSSet<NSString *> *expectedKeys = [NSSet setWithArray:@[
    @"blockAdsAndTrackers",
    @"blockCrossSiteCookies",
    @"hideCookieBanners",
    @"rejectOptionalCookies",
    @"youtubeBestEffort",
  ]];
  if (jsonError || !dictionary || dictionary.count != expectedKeys.count ||
      ![[NSSet setWithArray:dictionary.allKeys] isEqualToSet:expectedKeys]) {
    if (errorMessage) {
      *errorMessage = @"The browser-protection settings are invalid.";
    }
    return nil;
  }
  for (NSString *key in expectedKeys) {
    if (!isJsonBoolean(dictionary[key])) {
      if (errorMessage) {
        *errorMessage = @"Every browser-protection setting must be a boolean.";
      }
      return nil;
    }
  }
  if ([dictionary[@"youtubeBestEffort"] boolValue] &&
      MASQ_PRIVATE_YOUTUBE_AD_BLOCKER != 1) {
    if (errorMessage) {
      *errorMessage =
          @"YouTube best-effort filtering is unavailable in this public build.";
    }
    return nil;
  }
  return dictionary;
}

void saveBrowserProtectionPreferences(NSDictionary *preferences) {
  NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
  [defaults setBool:[preferences[@"blockAdsAndTrackers"] boolValue]
             forKey:MasqBrowserBlockAdsKey];
  [defaults setBool:[preferences[@"blockCrossSiteCookies"] boolValue]
             forKey:MasqBrowserBlockCrossSiteCookiesKey];
  [defaults setBool:[preferences[@"hideCookieBanners"] boolValue]
             forKey:MasqBrowserHideCookieBannersKey];
  [defaults setBool:[preferences[@"rejectOptionalCookies"] boolValue]
             forKey:MasqBrowserRejectOptionalCookiesKey];
  [defaults setBool:[preferences[@"youtubeBestEffort"] boolValue]
             forKey:MasqBrowserYouTubeBestEffortKey];
}

void resetBrowserProtectionPreferences() {
  @synchronized(browserProtectionLock()) {
    gBrowserProtectionGeneration += 1;
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    [defaults removeObjectForKey:MasqBrowserBlockAdsKey];
    [defaults removeObjectForKey:MasqBrowserBlockCrossSiteCookiesKey];
    [defaults removeObjectForKey:MasqBrowserHideCookieBannersKey];
    [defaults removeObjectForKey:MasqBrowserRejectOptionalCookiesKey];
    [defaults removeObjectForKey:MasqBrowserYouTubeBestEffortKey];
    [defaults removeObjectForKey:MasqBrowserRememberedMasqSitesKey];
    [defaults removeObjectForKey:MasqBrowserRememberedDirectSitesKey];
    [defaults removeObjectForKey:MasqBrowserProtectionDisabledSitesKey];
    setActiveBrowserContentRuleLists(@[]);
    setCurrentYouTubeBestEffortEnabled(NO);
  }
  clearAllBrowserDataStores(^{});
}

NSDictionary *browserProtectionResponse(NSDictionary *preferences) {
  NSMutableDictionary *response = [preferences mutableCopy];
  response[@"nativeRequestBlocking"] = @YES;
  response[@"youtubeBestEffortAvailable"] =
      @(MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1);
  return response;
}

NSDictionary *contentRule(NSDictionary *trigger, NSDictionary *action) {
  NSMutableDictionary *protectedTrigger = [trigger mutableCopy];
  NSMutableOrderedSet<NSString *> *excludedDomains =
      [NSMutableOrderedSet orderedSetWithArray:
          [trigger[@"unless-domain"] isKindOfClass:[NSArray class]]
              ? trigger[@"unless-domain"]
              : @[]];
  [excludedDomains addObjectsFromArray:browserProtectionDisabledDomains()];
  if (excludedDomains.count > 0) {
    protectedTrigger[@"unless-domain"] = excludedDomains.array;
  }
  return @{ @"trigger" : protectedTrigger, @"action" : action };
}

NSArray<NSDictionary *> *crossSiteCookieRules() {
  return @[
    contentRule(
        @{ @"url-filter" : @".*", @"load-type" : @[ @"third-party" ] },
        @{ @"type" : @"block-cookies" }),
  ];
}

NSArray<NSDictionary *> *adAndTrackerRules() {
  NSArray<NSString *> *hostPatterns = @[
    @"^https?://([^/]+\\.)?doubleclick\\.net/",
    @"^https?://([^/]+\\.)?googlesyndication\\.com/",
    @"^https?://([^/]+\\.)?googleadservices\\.com/",
    @"^https?://adservice\\.google\\.[^/]+/",
    @"^https?://([^/]+\\.)?amazon-adsystem\\.com/",
    @"^https?://([^/]+\\.)?adnxs\\.com/",
    @"^https?://([^/]+\\.)?adsrvr\\.org/",
    @"^https?://([^/]+\\.)?criteo\\.com/",
    @"^https?://([^/]+\\.)?criteo\\.net/",
    @"^https?://([^/]+\\.)?taboola\\.com/",
    @"^https?://([^/]+\\.)?outbrain\\.com/",
    @"^https?://([^/]+\\.)?pubmatic\\.com/",
    @"^https?://([^/]+\\.)?rubiconproject\\.com/",
    @"^https?://([^/]+\\.)?openx\\.net/",
    @"^https?://([^/]+\\.)?smartadserver\\.com/",
    @"^https?://([^/]+\\.)?casalemedia\\.com/",
    @"^https?://([^/]+\\.)?moatads\\.com/",
    @"^https?://([^/]+\\.)?advertising\\.com/",
    @"^https?://([^/]+\\.)?serving-sys\\.com/",
    @"^https?://([^/]+\\.)?yieldmo\\.com/",
    @"^https?://([^/]+\\.)?teads\\.tv/",
    @"^https?://([^/]+\\.)?scorecardresearch\\.com/",
    @"^https?://([^/]+\\.)?quantserve\\.com/",
    @"^https?://([^/]+\\.)?demdex\\.net/",
    @"^https?://([^/]+\\.)?bluekai\\.com/",
    @"^https?://([^/]+\\.)?google-analytics\\.com/",
    @"^https?://bat\\.bing\\.com/",
    @"^https?://([^/]+\\.)?clarity\\.ms/",
    @"^https?://static\\.ads-twitter\\.com/",
    @"^https?://snap\\.licdn\\.com/",
    @"^https?://analytics\\.tiktok\\.com/",
  ];
  NSArray<NSString *> *resourceTypes = @[
    @"image",
    @"style-sheet",
    @"script",
    @"font",
    @"media",
    @"svg-document",
    @"raw",
    @"popup",
    @"document",
  ];
  NSArray<NSString *> *youtubeDomains =
      @[ @"*youtube.com", @"*youtube-nocookie.com" ];
  NSMutableArray<NSDictionary *> *rules = [NSMutableArray array];
  for (NSString *pattern in hostPatterns) {
    [rules addObject:contentRule(
                         @{
                           @"url-filter" : pattern,
                           @"load-type" : @[ @"third-party" ],
                           @"resource-type" : resourceTypes,
                           @"unless-domain" : youtubeDomains,
                         },
                         @{ @"type" : @"block" })];
  }
  [rules addObject:contentRule(
                       @{
                         @"url-filter" : @".*",
                         @"unless-domain" : youtubeDomains,
                       },
                       @{
                         @"type" : @"css-display-none",
                         @"selector" :
                             @"ins.adsbygoogle,iframe[id^='google_ads_'],iframe[src*='doubleclick.net'],[data-ad-client],[data-ad-slot],.advertisement,.sponsored-ad,#ad-container,.ad-container",
                       })];
  return rules;
}

#if MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1
NSArray<NSDictionary *> *privateYouTubeRules() {
  NSArray<NSString *> *youtubeDomains =
      @[ @"*youtube.com", @"*youtube-nocookie.com" ];
  NSArray<NSString *> *patterns = @[
    @"^https?://([^/]+\\.)?doubleclick\\.net/",
    @"^https?://([^/]+\\.)?googlesyndication\\.com/",
    @"^https?://([^/]+\\.)?googleadservices\\.com/",
    @"^https?://www\\.youtube\\.com/api/stats/ads",
    @"^https?://www\\.youtube\\.com/pagead/",
    @"^https?://www\\.youtube\\.com/ptracking",
    @"^https?://www\\.youtube\\.com/get_midroll_info",
  ];
  NSMutableArray<NSDictionary *> *rules = [NSMutableArray array];
  for (NSString *pattern in patterns) {
    [rules addObject:contentRule(
                         @{
                           @"url-filter" : pattern,
                           @"if-domain" : youtubeDomains,
                         },
                         @{ @"type" : @"block" })];
  }
  [rules addObject:contentRule(
                       @{
                         @"url-filter" : @".*",
                         @"if-domain" : youtubeDomains,
                       },
                       @{
                         @"type" : @"css-display-none",
                         @"selector" :
                             @"#player-ads,#masthead-ad,ytd-ad-slot-renderer,ytd-display-ad-renderer,ytd-in-feed-ad-layout-renderer,ytd-promoted-sparkles-web-renderer,.ytp-ad-overlay-container,.ytp-ad-player-overlay",
                       })];
  return rules;
}

NSString *privateYouTubeBestEffortScript() {
  return
      @"(() => {"
       "if (window.__masqPrivateYouTubeFilter) return;"
       "if (!/(^|\\.)youtube\\.com$/i.test(location.hostname)) return;"
       "window.__masqPrivateYouTubeFilter = true;"
       "const saved = new WeakMap();"
       "const restore = (video) => {"
         "const state = saved.get(video);"
         "if (!state) return;"
         "video.muted = state.muted;"
         "video.playbackRate = state.rate;"
         "saved.delete(video);"
       "};"
       "const filter = () => {"
         "const player = document.querySelector('.html5-video-player');"
         "const video = document.querySelector('video');"
         "if (!player || !player.classList.contains('ad-showing')) {"
           "if (video) restore(video);"
           "return;"
         "}"
         "const skip = document.querySelector('.ytp-ad-skip-button,.ytp-skip-ad-button,.ytp-ad-skip-button-modern,.videoAdUiSkipButton');"
         "if (skip instanceof HTMLElement) skip.click();"
         "if (!(video instanceof HTMLVideoElement)) return;"
         "if (!saved.has(video)) saved.set(video, { muted: video.muted, rate: video.playbackRate });"
         "video.muted = true;"
         "video.playbackRate = 16;"
         "if (Number.isFinite(video.duration) && video.duration > 0) {"
           "try { video.currentTime = Math.max(0, video.duration - 0.05); } catch (_) {}"
         "}"
       "};"
       "const start = () => {"
         "const root = document.documentElement;"
         "if (!root) return;"
         "new MutationObserver(filter).observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ['class'] });"
         "setInterval(filter, 500);"
         "filter();"
       "};"
       "if (document.documentElement) start();"
       "else document.addEventListener('DOMContentLoaded', start, { once: true });"
      "})();";
}
#endif

NSString *_Nullable encodedContentRules(
    NSArray<NSDictionary *> *rules,
    NSError **error) {
  NSData *data = [NSJSONSerialization dataWithJSONObject:rules options:0 error:error];
  return data ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding]
              : nil;
}

void compileBrowserProtection(
    NSDictionary *preferences,
    NSUInteger generation,
    BOOL persistPreferences,
    void (^completion)(NSDictionary *_Nullable response,
                       NSError *_Nullable error)) {
  NSMutableArray<NSDictionary *> *tasks = [NSMutableArray array];
  if ([preferences[@"blockCrossSiteCookies"] boolValue]) {
    [tasks addObject:@{
      @"identifier" : MasqBrowserCookieRulesIdentifier,
      @"rules" : crossSiteCookieRules(),
    }];
  }
  if ([preferences[@"blockAdsAndTrackers"] boolValue]) {
    [tasks addObject:@{
      @"identifier" : MasqBrowserAdRulesIdentifier,
      @"rules" : adAndTrackerRules(),
    }];
  }
  // Consent surfaces are handled by the versioned page adapters only after a
  // verified Reject action succeeds. A native CSS rule cannot observe that
  // state and would risk hiding an unresolved consent gate.
#if MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1
  if ([preferences[@"youtubeBestEffort"] boolValue]) {
    [tasks addObject:@{
      @"identifier" : MasqBrowserYouTubeRulesIdentifier,
      @"rules" : privateYouTubeRules(),
    }];
  }
#endif

  if (tasks.count == 0) {
    BOOL stale = NO;
    @synchronized(browserProtectionLock()) {
      stale = generation != gBrowserProtectionGeneration;
      if (!stale) {
        setActiveBrowserContentRuleLists(@[]);
        setCurrentYouTubeBestEffortEnabled(NO);
        if (persistPreferences) {
          saveBrowserProtectionPreferences(preferences);
        }
      }
    }
    if (stale) {
      completion(nil, staleBrowserProtectionError());
    } else {
      completion(browserProtectionResponse(preferences), nil);
    }
    return;
  }

  dispatch_group_t group = dispatch_group_create();
  NSMutableArray<WKContentRuleList *> *compiledLists = [NSMutableArray array];
  __block NSError *firstError = nil;
  WKContentRuleListStore *store = WKContentRuleListStore.defaultStore;

  for (NSDictionary *task in tasks) {
    NSError *encodingError = nil;
    NSString *encodedRules = encodedContentRules(task[@"rules"], &encodingError);
    if (!encodedRules) {
      firstError = encodingError;
      break;
    }
    dispatch_group_enter(group);
    [store compileContentRuleListForIdentifier:task[@"identifier"]
                        encodedContentRuleList:encodedRules
                            completionHandler:^(WKContentRuleList *ruleList,
                                                NSError *error) {
      @synchronized(browserProtectionLock()) {
        if (error && !firstError) {
          firstError = error;
        } else if (ruleList) {
          [compiledLists addObject:ruleList];
        }
      }
      dispatch_group_leave(group);
    }];
  }

  dispatch_group_notify(group, dispatch_get_main_queue(), ^{
    if (firstError || compiledLists.count != tasks.count) {
      NSError *error = firstError ?: [NSError
          errorWithDomain:@"MASQBrowserProtection"
                     code:1
                 userInfo:@{
                   NSLocalizedDescriptionKey :
                       @"The browser protection rules could not be prepared.",
                 }];
      completion(nil, error);
      return;
    }
    BOOL stale = NO;
    @synchronized(browserProtectionLock()) {
      stale = generation != gBrowserProtectionGeneration;
      if (!stale) {
        setActiveBrowserContentRuleLists(compiledLists);
        setCurrentYouTubeBestEffortEnabled(
            MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1 &&
            [preferences[@"youtubeBestEffort"] boolValue]);
        if (persistPreferences) {
          saveBrowserProtectionPreferences(preferences);
        }
      }
    }
    if (stale) {
      completion(nil, staleBrowserProtectionError());
    } else {
      completion(browserProtectionResponse(preferences), nil);
    }
  });
}
}  // namespace

// react-native-webview is patched to call this symbol while constructing its incognito WKWebView.
// Keeping the store in this app target guarantees that proxy configuration and page loads use the
// same non-persistent WKWebsiteDataStore instance.
extern "C" __attribute__((visibility("default"), used))
WKWebsiteDataStore *masq_private_browser_data_store(void) {
  return masqBrowserDataStore();
}

// A second strong-link symbol gives direct browsing its own ephemeral store. It starts with the
// same sink proxy and is opened only by the explicit "direct" routing-mode transition.
extern "C" __attribute__((visibility("default"), used))
WKWebsiteDataStore *masq_direct_browser_data_store(void) {
  return directBrowserDataStore();
}

extern "C" __attribute__((visibility("default"), used))
WKWebsiteDataStore *masq_persistent_browser_data_store(void) {
  return masqPersistentBrowserDataStore();
}

extern "C" __attribute__((visibility("default"), used))
WKWebsiteDataStore *masq_direct_persistent_browser_data_store(void) {
  return directPersistentBrowserDataStore();
}

// The react-native-webview patch invokes this for both protected browser variants. Rules are
// compiled before either WebView is mounted.
extern "C" __attribute__((visibility("default"), used))
void masq_configure_private_browser_content_controller(
    WKUserContentController *controller) {
  if (!controller) {
    return;
  }
  NSArray<WKContentRuleList *> *ruleLists;
  BOOL youtubeBestEffort;
  @synchronized(browserProtectionLock()) {
    ruleLists = [currentBrowserContentRuleLists() copy];
    youtubeBestEffort = currentYouTubeBestEffortEnabled();
  }
  for (WKContentRuleList *ruleList in ruleLists) {
    [controller addContentRuleList:ruleList];
  }
#if MASQ_PRIVATE_YOUTUBE_AD_BLOCKER == 1
  if (youtubeBestEffort) {
    WKUserScript *script = [[WKUserScript alloc]
        initWithSource:privateYouTubeBestEffortScript()
         injectionTime:WKUserScriptInjectionTimeAtDocumentStart
      forMainFrameOnly:NO];
    [controller addUserScript:script];
  }
#endif
}

extern "C" int32_t masq_apple_tcp_connect(const char *host,
                                           uint16_t port,
                                           int32_t timeoutMilliseconds,
                                           int32_t *errorCode) {
  @autoreleasepool {
    if (errorCode != nullptr) {
      *errorCode = EIO;
    }
    if (host == nullptr || port == 0) {
      if (errorCode != nullptr) {
        *errorCode = EINVAL;
      }
      return -1;
    }

    NSString *hostString = [NSString stringWithUTF8String:host];
    CFReadStreamRef readStream = nullptr;
    CFWriteStreamRef writeStream = nullptr;
    CFStreamCreatePairWithSocketToHost(kCFAllocatorDefault,
                                       (__bridge CFStringRef)hostString, port,
                                       &readStream, &writeStream);
    if (readStream == nullptr || writeStream == nullptr) {
      if (readStream != nullptr) {
        CFRelease(readStream);
      }
      if (writeStream != nullptr) {
        CFRelease(writeStream);
      }
      return -1;
    }

    CFRunLoopRef runLoop = CFRunLoopGetCurrent();
    CFReadStreamScheduleWithRunLoop(readStream, runLoop, kCFRunLoopDefaultMode);
    CFWriteStreamScheduleWithRunLoop(writeStream, runLoop, kCFRunLoopDefaultMode);
    const bool opened = CFReadStreamOpen(readStream) && CFWriteStreamOpen(writeStream);
    const CFAbsoluteTime deadline = CFAbsoluteTimeGetCurrent() +
        (MAX(timeoutMilliseconds, 1) / 1000.0);
    int result = -1;

    while (opened && CFAbsoluteTimeGetCurrent() < deadline) {
      const CFStreamStatus readStatus = CFReadStreamGetStatus(readStream);
      const CFStreamStatus writeStatus = CFWriteStreamGetStatus(writeStream);
      const bool ready =
          (readStatus == kCFStreamStatusOpen ||
           readStatus == kCFStreamStatusReading) &&
          (writeStatus == kCFStreamStatusOpen ||
           writeStatus == kCFStreamStatusWriting);
      if (ready) {
        CFDataRef handleData = static_cast<CFDataRef>(
            CFReadStreamCopyProperty(readStream,
                                     kCFStreamPropertySocketNativeHandle));
        if (handleData != nullptr &&
            CFDataGetLength(handleData) >= sizeof(CFSocketNativeHandle)) {
          CFSocketNativeHandle nativeHandle = -1;
          CFDataGetBytes(handleData,
                         CFRangeMake(0, sizeof(CFSocketNativeHandle)),
                         reinterpret_cast<UInt8 *>(&nativeHandle));
          // Transfer the connected socket itself to Rust. Duplicating the descriptor and then
          // closing the CFStreams is unsafe: CFStreamClose may shut down the shared TCP socket,
          // leaving the duplicate descriptor alive but disconnected (ENOTCONN on iOS).
          const Boolean readTransferred = CFReadStreamSetProperty(
              readStream, kCFStreamPropertyShouldCloseNativeSocket,
              kCFBooleanFalse);
          const Boolean writeTransferred = CFWriteStreamSetProperty(
              writeStream, kCFStreamPropertyShouldCloseNativeSocket,
              kCFBooleanFalse);
          if (readTransferred && writeTransferred) {
            result = nativeHandle;
          } else if (errorCode != nullptr) {
            *errorCode = EIO;
          }
        }
        if (handleData != nullptr) {
          CFRelease(handleData);
        }
        break;
      }

      if (readStatus == kCFStreamStatusError ||
          writeStatus == kCFStreamStatusError ||
          readStatus == kCFStreamStatusClosed ||
          writeStatus == kCFStreamStatusClosed) {
        CFStreamError streamError = readStatus == kCFStreamStatusError
            ? CFReadStreamGetError(readStream)
            : CFWriteStreamGetError(writeStream);
        if (errorCode != nullptr && streamError.error > 0) {
          *errorCode = static_cast<int32_t>(streamError.error);
        }
        break;
      }
      CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
    }

    if (result < 0 && errorCode != nullptr && *errorCode == EIO && opened) {
      *errorCode = ETIMEDOUT;
    }
    CFReadStreamUnscheduleFromRunLoop(readStream, runLoop, kCFRunLoopDefaultMode);
    CFWriteStreamUnscheduleFromRunLoop(writeStream, runLoop, kCFRunLoopDefaultMode);
    if (result < 0) {
      CFReadStreamClose(readStream);
      CFWriteStreamClose(writeStream);
    }
    CFRelease(readStream);
    CFRelease(writeStream);
    return result;
  }
}

namespace {
using NoArgumentFunction = char *(*)();
using StringArgumentFunction = char *(*)(const char *);
using BooleanArgumentFunction = char *(*)(bool);
using UInt8ArgumentFunction = char *(*)(uint8_t);
using FreeStringFunction = void (*)(char *);

NSString *const MasqConfigDefaultsKey = @"MASQSavedConsumerConfig";
NSString *const MasqEntryNodeCachePrefix = @"MASQReachableEntryNodes";
NSString *const MasqWalletAccount = @"consumer-wallet";
NSString *const MasqPublicSuburb = @"masqpublic1";
constexpr NSUInteger MasqNodeFinderAttempts = 10;
constexpr NSUInteger MasqRequiredEntryNodes = 2;
constexpr int32_t MasqEntryNodePreflightTimeoutMilliseconds = 4000;
constexpr NSInteger MasqCurrentConfigVersion = 2;

NSString *_Nullable configuredNodeFinderBaseURL() {
  id rawValue = [NSBundle.mainBundle objectForInfoDictionaryKey:@"MASQNodeFinderURL"];
  if (![rawValue isKindOfClass:[NSString class]]) {
    return nil;
  }
  NSString *trimmed = [(NSString *)rawValue
      stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
  while ([trimmed hasSuffix:@"/"]) {
    trimmed = [trimmed substringToIndex:trimmed.length - 1];
  }
  NSURLComponents *components = [NSURLComponents componentsWithString:trimmed];
  if (![components.scheme.lowercaseString isEqualToString:@"https"] ||
      components.host.length == 0 || components.user.length > 0 ||
      components.password.length > 0 || components.query.length > 0 ||
      components.fragment.length > 0) {
    return nil;
  }
  return trimmed;
}

NSString *networkStatusJson() {
  static dispatch_once_t onceToken;
  static NSMutableDictionary *state;
  static nw_path_monitor_t monitor;
  dispatch_once(&onceToken, ^{
    state = [@{
      @"available" : @NO,
      @"interface" : @"unknown",
      @"expensive" : @NO,
      @"constrained" : @NO,
      @"generation" : @0,
    } mutableCopy];
    monitor = nw_path_monitor_create();
    dispatch_queue_t queue = dispatch_queue_create("ai.masq.mobile.network", DISPATCH_QUEUE_SERIAL);
    nw_path_monitor_set_queue(monitor, queue);
    nw_path_monitor_set_update_handler(monitor, ^(nw_path_t path) {
      NSString *interface = @"other";
      if (nw_path_uses_interface_type(path, nw_interface_type_wifi)) {
        interface = @"wifi";
      } else if (nw_path_uses_interface_type(path, nw_interface_type_cellular)) {
        interface = @"cellular";
      } else if (nw_path_uses_interface_type(path, nw_interface_type_wired)) {
        interface = @"wired";
      }
      @synchronized(state) {
        state[@"available"] = @(nw_path_get_status(path) == nw_path_status_satisfied);
        state[@"interface"] = interface;
        state[@"expensive"] = @(nw_path_is_expensive(path));
        state[@"constrained"] = @(nw_path_is_constrained(path));
        state[@"generation"] = @([state[@"generation"] unsignedIntegerValue] + 1);
      }
    });
    nw_path_monitor_start(monitor);
  });
  NSDictionary *snapshot;
  @synchronized(state) {
    snapshot = [state copy];
  }
  NSData *data = [NSJSONSerialization dataWithJSONObject:snapshot options:0 error:nil];
  return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}

NSMutableDictionary *_Nullable migratedConfig(NSString *configJson) {
  NSMutableDictionary *config = [[NSJSONSerialization
      JSONObjectWithData:[configJson dataUsingEncoding:NSUTF8StringEncoding]
                 options:NSJSONReadingMutableContainers
                   error:nil] mutableCopy];
  if (![config isKindOfClass:[NSMutableDictionary class]]) {
    return nil;
  }
  config[@"configVersion"] = @(MasqCurrentConfigVersion);
  if (!config[@"minHops"]) config[@"minHops"] = @1;
  if (!config[@"exitCountry"]) config[@"exitCountry"] = [NSNull null];
  if (!config[@"exitCountryFallback"]) config[@"exitCountryFallback"] = @YES;
  return config;
}

NSString *walletService() {
  return [NSString stringWithFormat:@"%@.secure-wallet",
                                    NSBundle.mainBundle.bundleIdentifier];
}

NSDictionary *walletQuery() {
  return @{
    (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
    (__bridge id)kSecAttrService : walletService(),
    (__bridge id)kSecAttrAccount : MasqWalletAccount,
  };
}

void deleteWalletSecret() {
  SecItemDelete((__bridge CFDictionaryRef)walletQuery());
}

BOOL saveWalletSecret(NSString *secret) {
  deleteWalletSecret();
  NSMutableDictionary *query = [walletQuery() mutableCopy];
  query[(__bridge id)kSecValueData] =
      [secret dataUsingEncoding:NSUTF8StringEncoding];
  query[(__bridge id)kSecAttrAccessible] =
      (__bridge id)kSecAttrAccessibleWhenUnlockedThisDeviceOnly;
  return SecItemAdd((__bridge CFDictionaryRef)query, nullptr) == errSecSuccess;
}

NSString *_Nullable loadWalletSecret() {
  NSMutableDictionary *query = [walletQuery() mutableCopy];
  query[(__bridge id)kSecReturnData] = @YES;
  query[(__bridge id)kSecMatchLimit] = (__bridge id)kSecMatchLimitOne;
  CFTypeRef result = nullptr;
  if (SecItemCopyMatching((__bridge CFDictionaryRef)query, &result) !=
      errSecSuccess) {
    return nil;
  }
  NSData *data = CFBridgingRelease(result);
  return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}

NSString *_Nullable normalizedNodeFinderDescriptor(NSData *data) {
  if (!data) {
    return nil;
  }
  NSString *text = [[NSString alloc] initWithData:data
                                          encoding:NSUTF8StringEncoding];
  NSString *trimmed =
      [text stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
  if (!trimmed.length) {
    return nil;
  }
  id decoded = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
  if ([decoded isKindOfClass:[NSString class]]) {
    return [(NSString *)decoded
        stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
  }
  return trimmed;
}

BOOL entryNodeParts(NSString *descriptor,
                    NSString *chain,
                    NSString *_Nullable *_Nullable host,
                    NSNumber *_Nullable *_Nullable port) {
  NSURLComponents *components = [NSURLComponents componentsWithString:descriptor];
  BOOL valid = [components.scheme.lowercaseString isEqualToString:@"masq"] &&
      [components.user isEqualToString:chain] && components.password.length > 0 &&
      components.host.length > 0 && components.port.integerValue > 0 &&
      components.port.integerValue <= 65535;
  if (!valid) {
    return NO;
  }
  if (host) {
    *host = components.host;
  }
  if (port) {
    *port = components.port;
  }
  return YES;
}

BOOL entryNodeIsReachable(NSString *descriptor,
                          NSString *chain,
                          int32_t *_Nullable connectionError) {
  NSString *host = nil;
  NSNumber *port = nil;
  if (!entryNodeParts(descriptor, chain, &host, &port)) {
    if (connectionError) {
      *connectionError = EINVAL;
    }
    return NO;
  }
  int32_t errorCode = EIO;
  int32_t descriptorFd = masq_apple_tcp_connect(
      host.UTF8String, (uint16_t)port.unsignedShortValue,
      MasqEntryNodePreflightTimeoutMilliseconds, &errorCode);
  if (descriptorFd < 0) {
    if (connectionError) {
      *connectionError = errorCode;
    }
    return NO;
  }
  close(descriptorFd);
  if (connectionError) {
    *connectionError = 0;
  }
  return YES;
}

void testReachableEntryNodes(
    NSArray<NSString *> *candidates,
    NSString *chain,
    void (^completion)(NSArray<NSString *> *nodes,
                       NSUInteger timedOut,
                       NSUInteger refused,
                       NSUInteger otherFailures)) {
  dispatch_group_t preflightGroup = dispatch_group_create();
  NSMutableSet<NSString *> *reachableSet = [NSMutableSet set];
  __block NSUInteger timedOut = 0;
  __block NSUInteger refused = 0;
  __block NSUInteger otherFailures = 0;

  for (NSString *candidate in candidates) {
    dispatch_group_async(
        preflightGroup,
        dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
          int32_t connectionError = EIO;
          BOOL reachable =
              entryNodeIsReachable(candidate, chain, &connectionError);
          @synchronized(reachableSet) {
            if (reachable) {
              [reachableSet addObject:candidate];
            } else if (connectionError == ETIMEDOUT) {
              timedOut += 1;
            } else if (connectionError == ECONNREFUSED) {
              refused += 1;
            } else {
              otherFailures += 1;
            }
          }
        });
  }

  dispatch_group_notify(
      preflightGroup,
      dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSMutableArray<NSString *> *reachable = [NSMutableArray array];
        @synchronized(reachableSet) {
          for (NSString *candidate in candidates) {
            if ([reachableSet containsObject:candidate]) {
              [reachable addObject:candidate];
            }
          }
        }
        completion(reachable, timedOut, refused, otherFailures);
      });
}

void discoverReachableEntryNodes(
    NSString *chain,
    NSArray<NSString *> *preferredNodes,
    void (^completion)(NSArray<NSString *> *_Nullable nodes,
                       NSString *_Nullable errorMessage)) {
  NSString *nodeFinderBaseURL = configuredNodeFinderBaseURL();
  if (!nodeFinderBaseURL) {
    completion(nil,
               @"A verified HTTPS MASQ node-finder is not configured for this release build.");
    return;
  }
  dispatch_group_t group = dispatch_group_create();
  NSMutableOrderedSet<NSString *> *candidates = [NSMutableOrderedSet orderedSet];
  NSString *cacheKey = [NSString stringWithFormat:@"%@.%@", MasqEntryNodeCachePrefix, chain];
  NSArray *cachedNodes = [NSUserDefaults.standardUserDefaults arrayForKey:cacheKey] ?: @[];
  NSMutableSet<NSString *> *previousNodes = [NSMutableSet set];
  for (id node in [cachedNodes arrayByAddingObjectsFromArray:preferredNodes]) {
    if ([node isKindOfClass:[NSString class]] &&
        entryNodeParts((NSString *)node, chain, nil, nil)) {
      [previousNodes addObject:(NSString *)node];
    }
  }

  for (NSUInteger index = 0; index < MasqNodeFinderAttempts; index++) {
    NSString *urlString = [NSString
        stringWithFormat:@"%@/randomnode/%@/%@?refresh=%@-%lu",
                         nodeFinderBaseURL, chain, MasqPublicSuburb,
                         NSUUID.UUID.UUIDString,
                         (unsigned long)index];
    NSURL *url = [NSURL URLWithString:urlString];
    if (!url) {
      continue;
    }
    dispatch_group_enter(group);
    NSMutableURLRequest *request = [NSMutableURLRequest
        requestWithURL:url
           cachePolicy:NSURLRequestReloadIgnoringLocalCacheData
       timeoutInterval:6];
    request.HTTPMethod = @"GET";
    [request setValue:@"text/plain" forHTTPHeaderField:@"Accept"];
    [request setValue:@"no-cache" forHTTPHeaderField:@"Cache-Control"];
    NSURLSessionDataTask *task = [NSURLSession.sharedSession
        dataTaskWithRequest:request
          completionHandler:^(NSData *data, NSURLResponse *response, NSError *error) {
            NSHTTPURLResponse *httpResponse = (NSHTTPURLResponse *)response;
            if (!error && [httpResponse isKindOfClass:[NSHTTPURLResponse class]] &&
                httpResponse.statusCode >= 200 && httpResponse.statusCode < 300) {
              NSString *candidate = normalizedNodeFinderDescriptor(data);
              if (candidate && entryNodeParts(candidate, chain, nil, nil)) {
                @synchronized(candidates) {
                  [candidates addObject:candidate];
                }
              }
            }
            dispatch_group_leave(group);
          }];
    [task resume];
  }

  dispatch_group_notify(group,
                        dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    NSArray<NSString *> *snapshot;
    @synchronized(candidates) {
      // Network results intentionally stay first. Previously cached Nodes were selected before
      // every fresh node-finder result; a TCP-reachable but MASQ-unusable Node could therefore
      // remain pinned forever and make Retry appear ineffective.
      NSMutableOrderedSet<NSString *> *ordered = [NSMutableOrderedSet orderedSet];
      for (NSString *candidate in candidates) {
        if (![previousNodes containsObject:candidate]) {
          [ordered addObject:candidate];
        }
      }
      for (NSString *candidate in candidates) {
        if ([previousNodes containsObject:candidate]) {
          [ordered addObject:candidate];
        }
      }
      for (id cachedNode in cachedNodes) {
        if ([cachedNode isKindOfClass:[NSString class]] &&
            entryNodeParts((NSString *)cachedNode, chain, nil, nil)) {
          [ordered addObject:(NSString *)cachedNode];
        }
      }
      for (id preferredNode in preferredNodes) {
        if ([preferredNode isKindOfClass:[NSString class]] &&
            entryNodeParts((NSString *)preferredNode, chain, nil, nil)) {
          [ordered addObject:(NSString *)preferredNode];
        }
      }
      snapshot = ordered.array;
    }
    testReachableEntryNodes(
        snapshot, chain,
        ^(NSArray<NSString *> *reachable, NSUInteger timedOut,
          NSUInteger refused, NSUInteger otherFailures) {
          if (reachable.count >= MasqRequiredEntryNodes) {
            NSArray *selected = [reachable subarrayWithRange:
                NSMakeRange(0, MasqRequiredEntryNodes)];
            NSString *cacheKey = [NSString stringWithFormat:@"%@.%@", MasqEntryNodeCachePrefix, chain];
            [NSUserDefaults.standardUserDefaults setObject:selected forKey:cacheKey];
            completion(selected, nil);
            return;
          }

          NSMutableArray<NSString *> *failureParts = [NSMutableArray array];
          if (timedOut > 0) {
            [failureParts
                addObject:[NSString stringWithFormat:@"%lu timed out",
                                                     (unsigned long)timedOut]];
          }
          if (refused > 0) {
            [failureParts
                addObject:[NSString stringWithFormat:@"%lu refused",
                                                     (unsigned long)refused]];
          }
          if (otherFailures > 0) {
            [failureParts
                addObject:[NSString stringWithFormat:@"%lu other failures",
                                                     (unsigned long)otherFailures]];
          }
          NSString *failureSummary = failureParts.count > 0
              ? [NSString stringWithFormat:@" (%@)",
                                           [failureParts componentsJoinedByString:@", "]]
              : @"";
          completion(
              nil,
              [NSString
                  stringWithFormat:
                      @"MASQ could not find two reachable entry nodes. Last refresh: %lu unique candidates, %lu reachable%@. Try mobile data or another network.",
                      (unsigned long)snapshot.count,
                      (unsigned long)reachable.count, failureSummary]);
        });
  });
}

// The Rust core is linked into the application as a static library. Resolve its
// functions directly instead of using dlsym: Release builds strip the dynamic
// symbol table, so RTLD_DEFAULT cannot reliably see symbols in the main binary.
template <typename T> T symbol(const char *name);

template <> NoArgumentFunction symbol<NoArgumentFunction>(const char *name) {
  if (strcmp(name, "masq_mobile_get_status") == 0) {
    return &masq_mobile_get_status;
  }
  if (strcmp(name, "masq_mobile_start") == 0) {
    return &masq_mobile_start;
  }
  if (strcmp(name, "masq_mobile_stop") == 0) {
    return &masq_mobile_stop;
  }
  if (strcmp(name, "masq_mobile_shutdown") == 0) {
    return &masq_mobile_shutdown;
  }
  if (strcmp(name, "masq_mobile_reset") == 0) {
    return &masq_mobile_reset;
  }
  if (strcmp(name, "masq_mobile_reset_network_profile") == 0) {
    return &masq_mobile_reset_network_profile;
  }
  if (strcmp(name, "masq_mobile_remove_wallet") == 0) {
    return &masq_mobile_remove_wallet;
  }
  if (strcmp(name, "masq_mobile_preflight_proxy") == 0) {
    return &masq_mobile_preflight_proxy;
  }
  return nullptr;
}

template <>
StringArgumentFunction symbol<StringArgumentFunction>(const char *name) {
  if (strcmp(name, "masq_mobile_configure") == 0) {
    return &masq_mobile_configure;
  }
  if (strcmp(name, "masq_mobile_import_wallet") == 0) {
    return &masq_mobile_import_wallet;
  }
  return nullptr;
}

template <>
BooleanArgumentFunction symbol<BooleanArgumentFunction>(const char *name) {
  return strcmp(name, "masq_mobile_set_proxy_enabled") == 0
      ? &masq_mobile_set_proxy_enabled
      : nullptr;
}

template <> UInt8ArgumentFunction symbol<UInt8ArgumentFunction>(const char *name) {
  return strcmp(name, "masq_mobile_update_min_hops") == 0
      ? &masq_mobile_update_min_hops
      : nullptr;
}

template <> FreeStringFunction symbol<FreeStringFunction>(const char *name) {
  return strcmp(name, "masq_mobile_string_free") == 0
      ? &masq_mobile_string_free
      : nullptr;
}

NSString *unavailableStatus(NSString *reason) {
  NSDictionary *status = @{
    @"phase" : @"blocked",
    @"engineAvailable" : @NO,
    @"proxyEnabled" : @NO,
    @"proxyPort" : [NSNull null],
    @"chain" : [NSNull null],
    @"walletAddress" : [NSNull null],
    @"connectedNeighbors" : @0,
    @"routeStage" : @0,
    @"routeHops" : @0,
    @"minHops" : @1,
    @"exitCountry" : [NSNull null],
    @"exitCountryFallback" : @YES,
    @"availableExitCountries" : @[],
    @"bytesUp" : @0,
    @"bytesDown" : @0,
    @"lastError" : reason,
  };
  NSData *data = [NSJSONSerialization dataWithJSONObject:status options:0 error:nil];
  return [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
}

bool coreAvailable() {
  return symbol<NoArgumentFunction>("masq_mobile_get_status") != nullptr &&
         symbol<StringArgumentFunction>("masq_mobile_configure") != nullptr &&
         symbol<StringArgumentFunction>("masq_mobile_import_wallet") != nullptr &&
         symbol<UInt8ArgumentFunction>("masq_mobile_update_min_hops") != nullptr &&
         symbol<NoArgumentFunction>("masq_mobile_start") != nullptr &&
         symbol<NoArgumentFunction>("masq_mobile_stop") != nullptr &&
         symbol<NoArgumentFunction>("masq_mobile_shutdown") != nullptr &&
         symbol<NoArgumentFunction>("masq_mobile_reset") != nullptr &&
         symbol<NoArgumentFunction>("masq_mobile_reset_network_profile") != nullptr &&
         symbol<NoArgumentFunction>("masq_mobile_remove_wallet") != nullptr &&
         symbol<NoArgumentFunction>("masq_mobile_preflight_proxy") != nullptr &&
         symbol<BooleanArgumentFunction>("masq_mobile_set_proxy_enabled") != nullptr &&
         symbol<FreeStringFunction>("masq_mobile_string_free") != nullptr;
}

BOOL statusSucceeded(NSString *statusJson) {
  NSData *data = [statusJson dataUsingEncoding:NSUTF8StringEncoding];
  NSDictionary *status =
      [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
  return ![status[@"phase"] isEqualToString:@"error"];
}

NSString *_Nullable nativeConfig(NSString *configJson, NSError **error) {
  NSMutableDictionary *config = [[NSJSONSerialization
      JSONObjectWithData:[configJson dataUsingEncoding:NSUTF8StringEncoding]
                 options:NSJSONReadingMutableContainers
                   error:error] mutableCopy];
  if (*error || ![config isKindOfClass:[NSMutableDictionary class]]) {
    return nil;
  }

  NSURL *applicationSupport = [[[NSFileManager defaultManager]
      URLsForDirectory:NSApplicationSupportDirectory
             inDomains:NSUserDomainMask] firstObject];
  NSURL *dataDirectory = [applicationSupport URLByAppendingPathComponent:@"MASQNode"
                                                              isDirectory:YES];
  if (![[NSFileManager defaultManager] createDirectoryAtURL:dataDirectory
                                withIntermediateDirectories:YES
                                                 attributes:nil
                                                      error:error]) {
    return nil;
  }
  [dataDirectory setResourceValue:@YES forKey:NSURLIsExcludedFromBackupKey error:nil];
  config[@"dataDirectory"] = dataDirectory.path;
  [config removeObjectForKey:@"configVersion"];
  NSData *nativeConfigData =
      [NSJSONSerialization dataWithJSONObject:config options:0 error:error];
  return nativeConfigData
             ? [[NSString alloc] initWithData:nativeConfigData
                                      encoding:NSUTF8StringEncoding]
             : nil;
}

NSString *_Nullable copyCoreResult(char *result) {
  if (result == nullptr) {
    return nil;
  }
  NSString *copied = [NSString stringWithUTF8String:result];
  symbol<FreeStringFunction>("masq_mobile_string_free")(result);
  return copied;
}

NSString *_Nullable invoke(NoArgumentFunction function) {
  return copyCoreResult(function());
}

NSString *_Nullable invoke(StringArgumentFunction function, NSString *argument) {
  return copyCoreResult(function(argument.UTF8String));
}

NSString *_Nullable invoke(BooleanArgumentFunction function, bool argument) {
  return copyCoreResult(function(argument));
}

NSString *_Nullable invoke(UInt8ArgumentFunction function, uint8_t argument) {
  return copyCoreResult(function(argument));
}

} // namespace

@interface RCTMasqCore ()
@property(nonatomic, assign) BOOL restoreAttempted;
- (void)restoreCoreIfNeeded;
- (void)invokeNoArgumentSymbol:(const char *)name
                       resolve:(RCTPromiseResolveBlock)resolve
                        reject:(RCTPromiseRejectBlock)reject;
- (void)invokeStringSymbol:(const char *)name
                  argument:(NSString *)argument
                   resolve:(RCTPromiseResolveBlock)resolve
                    reject:(RCTPromiseRejectBlock)reject;
@end

@implementation RCTMasqCore

+ (NSString *)moduleName {
  return @"NativeMasqCore";
}

- (void)getStatus:(RCTPromiseResolveBlock)resolve reject:(RCTPromiseRejectBlock)reject {
  if (!coreAvailable()) {
    resolve(unavailableStatus(@"The native MASQ core is missing from this build."));
    return;
  }
  [self restoreCoreIfNeeded];
  NSString *result = invoke(symbol<NoArgumentFunction>("masq_mobile_get_status"));
  result ? resolve(result) : reject(@"E_CORE_STATUS", @"The MASQ core returned no status.", nil);
}

- (void)getNetworkStatus:(RCTPromiseResolveBlock)resolve
                   reject:(RCTPromiseRejectBlock)reject {
  resolve(networkStatusJson());
}

- (void)getNodeFinderUrl:(RCTPromiseResolveBlock)resolve
                   reject:(RCTPromiseRejectBlock)reject {
  NSString *nodeFinderBaseURL = configuredNodeFinderBaseURL();
  if (!nodeFinderBaseURL) {
    reject(@"E_RELEASE_CONFIG",
           @"A verified HTTPS MASQ node-finder is not configured for this release build.",
           nil);
    return;
  }
  resolve(nodeFinderBaseURL);
}

- (void)prepareBrowserProtection:(RCTPromiseResolveBlock)resolve
                           reject:(RCTPromiseRejectBlock)reject {
  NSUInteger generation = beginBrowserProtectionOperation();
  NSDictionary *preferences = browserProtectionPreferencesFromDefaults();
  compileBrowserProtection(
      preferences,
      generation,
      NO,
      ^(NSDictionary *response, NSError *error) {
        if (error || !response) {
          reject(@"E_BROWSER_PROTECTION",
                 error.localizedDescription ?:
                     @"The browser protection rules could not be prepared.",
                 error);
          return;
        }
        clearTemporaryBrowserDataStores(^{
              if (!isCurrentBrowserProtectionOperation(generation)) {
                reject(@"E_BROWSER_PROTECTION_STALE",
                       staleBrowserProtectionError().localizedDescription,
                       nil);
                return;
              }
              NSData *serializedData =
                  [NSJSONSerialization dataWithJSONObject:response
                                                  options:0
                                                    error:nil];
              NSString *serialized = serializedData
                  ? [[NSString alloc] initWithData:serializedData
                                          encoding:NSUTF8StringEncoding]
                  : nil;
              if (!serialized) {
                reject(@"E_BROWSER_PROTECTION",
                       @"The browser protection status could not be created.",
                       nil);
                return;
              }
              resolve(serialized);
            });
      });
}

- (void)setBrowserProtection:(NSString *)configJson
                     resolve:(RCTPromiseResolveBlock)resolve
                      reject:(RCTPromiseRejectBlock)reject {
  NSString *validationError = nil;
  NSDictionary *preferences =
      decodeBrowserProtectionPreferences(configJson, &validationError);
  if (!preferences) {
    reject(@"E_BROWSER_PROTECTION_CONFIG",
           validationError ?: @"The browser-protection settings are invalid.",
           nil);
    return;
  }
  NSUInteger generation = beginBrowserProtectionOperation();
  compileBrowserProtection(
      preferences,
      generation,
      YES,
      ^(NSDictionary *response, NSError *error) {
        if (error || !response) {
          reject(@"E_BROWSER_PROTECTION",
                 error.localizedDescription ?:
                     @"The browser protection rules could not be prepared.",
                 error);
          return;
        }
        clearTemporaryBrowserDataStores(^{
              if (!isCurrentBrowserProtectionOperation(generation)) {
                reject(@"E_BROWSER_PROTECTION_STALE",
                       staleBrowserProtectionError().localizedDescription,
                       nil);
                return;
              }
              NSData *serializedData =
                  [NSJSONSerialization dataWithJSONObject:response
                                                  options:0
                                                    error:nil];
              NSString *serialized = serializedData
                  ? [[NSString alloc] initWithData:serializedData
                                          encoding:NSUTF8StringEncoding]
                  : nil;
              if (!serialized) {
                reject(@"E_BROWSER_PROTECTION",
                       @"The browser protection status could not be created.",
                       nil);
                return;
              }
              resolve(serialized);
            });
      });
}

- (void)getBrowserSiteSettings:(NSString *)mode
                      hostname:(NSString *)hostname
                       resolve:(RCTPromiseResolveBlock)resolve
                        reject:(RCTPromiseRejectBlock)reject {
  NSString *rememberedKey = rememberedSitesKeyForMode(mode);
  NSString *normalizedHostname = hostname.lowercaseString;
  if (!rememberedKey || ![hostname isEqualToString:normalizedHostname] ||
      !isSafeBrowserHostname(normalizedHostname)) {
    reject(@"E_BROWSER_SITE_SETTINGS",
           @"Choose a valid MASQ or Direct HTTPS website.", nil);
    return;
  }
  NSString *serialized = serializeBrowserSiteSettings(
      browserSiteSettingsResponse(mode, normalizedHostname));
  serialized
      ? resolve(serialized)
      : reject(@"E_BROWSER_SITE_SETTINGS",
               @"Website privacy settings could not be created.", nil);
}

- (void)setBrowserSiteSettings:(NSString *)mode
                      hostname:(NSString *)hostname
                rememberSignIn:(BOOL)rememberSignIn
            protectionDisabled:(BOOL)protectionDisabled
                       resolve:(RCTPromiseResolveBlock)resolve
                        reject:(RCTPromiseRejectBlock)reject {
  NSString *rememberedKey = rememberedSitesKeyForMode(mode);
  NSString *normalizedHostname = hostname.lowercaseString;
  if (!rememberedKey || ![hostname isEqualToString:normalizedHostname] ||
      !isSafeBrowserHostname(normalizedHostname)) {
    reject(@"E_BROWSER_SITE_SETTINGS",
           @"Choose a valid MASQ or Direct HTTPS website.", nil);
    return;
  }

  NSMutableSet<NSString *> *remembered =
      savedBrowserHostnameSet(rememberedKey);
  NSMutableSet<NSString *> *disabled =
      savedBrowserHostnameSet(MasqBrowserProtectionDisabledSitesKey);
  if (rememberSignIn) {
    [remembered addObject:normalizedHostname];
  } else {
    [remembered removeObject:normalizedHostname];
  }
  if (protectionDisabled) {
    [disabled addObject:normalizedHostname];
  } else {
    [disabled removeObject:normalizedHostname];
  }
  saveBrowserHostnameSet(remembered, rememberedKey);
  saveBrowserHostnameSet(disabled, MasqBrowserProtectionDisabledSitesKey);

  void (^finish)(void) = ^{
    NSString *serialized = serializeBrowserSiteSettings(
        browserSiteSettingsResponse(mode, normalizedHostname));
    serialized
        ? resolve(serialized)
        : reject(@"E_BROWSER_SITE_SETTINGS",
                 @"Website privacy settings could not be created.", nil);
  };
  if (!rememberSignIn) {
    clearBrowserWebsiteDataForHostname(
        persistentBrowserDataStoreForMode(mode), normalizedHostname, finish);
  } else {
    finish();
  }
}

- (void)clearBrowserSiteData:(NSString *)mode
                    hostname:(NSString *)hostname
                     resolve:(RCTPromiseResolveBlock)resolve
                      reject:(RCTPromiseRejectBlock)reject {
  NSString *rememberedKey = rememberedSitesKeyForMode(mode);
  NSString *normalizedHostname = hostname.lowercaseString;
  if (!rememberedKey || ![hostname isEqualToString:normalizedHostname] ||
      !isSafeBrowserHostname(normalizedHostname)) {
    reject(@"E_BROWSER_SITE_SETTINGS",
           @"Choose a valid MASQ or Direct HTTPS website.", nil);
    return;
  }
  NSMutableSet<NSString *> *remembered =
      savedBrowserHostnameSet(rememberedKey);
  NSMutableSet<NSString *> *disabled =
      savedBrowserHostnameSet(MasqBrowserProtectionDisabledSitesKey);
  [remembered removeObject:normalizedHostname];
  [disabled removeObject:normalizedHostname];
  saveBrowserHostnameSet(remembered, rememberedKey);
  saveBrowserHostnameSet(disabled, MasqBrowserProtectionDisabledSitesKey);
  clearBrowserWebsiteDataForHostname(
      persistentBrowserDataStoreForMode(mode), normalizedHostname, ^{
        NSString *serialized = serializeBrowserSiteSettings(
            browserSiteSettingsResponse(mode, normalizedHostname));
        serialized
            ? resolve(serialized)
            : reject(@"E_BROWSER_SITE_SETTINGS",
                     @"Website privacy settings could not be created.", nil);
      });
}

- (void)clearRememberedBrowserData:(RCTPromiseResolveBlock)resolve
                            reject:(RCTPromiseRejectBlock)reject {
  [NSUserDefaults.standardUserDefaults
      removeObjectForKey:MasqBrowserRememberedMasqSitesKey];
  [NSUserDefaults.standardUserDefaults
      removeObjectForKey:MasqBrowserRememberedDirectSitesKey];
  clearBrowserDataStores(
      @[ masqPersistentBrowserDataStore(),
         directPersistentBrowserDataStore() ],
      ^{
        resolve(@"ok");
      });
}

- (void)getSavedConfiguration:(RCTPromiseResolveBlock)resolve
                        reject:(RCTPromiseRejectBlock)reject {
  NSString *savedConfig =
      [NSUserDefaults.standardUserDefaults stringForKey:MasqConfigDefaultsKey];
  if (!savedConfig) {
    resolve(@"null");
    return;
  }
  NSMutableDictionary *config = migratedConfig(savedConfig);
  if (!config) {
    reject(@"E_SAVED_CONFIG", @"The saved MASQ configuration is invalid.", nil);
    return;
  }
  NSData *normalized =
      [NSJSONSerialization dataWithJSONObject:config options:0 error:nil];
  NSString *normalizedString = [[NSString alloc] initWithData:normalized
                                                      encoding:NSUTF8StringEncoding];
  [NSUserDefaults.standardUserDefaults setObject:normalizedString
                                          forKey:MasqConfigDefaultsKey];
  resolve(normalizedString);
}

- (void)configure:(NSString *)configJson
          resolve:(RCTPromiseResolveBlock)resolve
           reject:(RCTPromiseRejectBlock)reject {
  NSError *configError = nil;
  NSString *preparedConfig = nativeConfig(configJson, &configError);
  if (!preparedConfig) {
    reject(@"E_CONFIG_JSON", @"The mobile configuration could not be prepared.", configError);
    return;
  }
  self.restoreAttempted = YES;
  NSString *result = invoke(symbol<StringArgumentFunction>("masq_mobile_configure"),
                            preparedConfig);
  if (!result) {
    reject(@"E_CORE_CONFIG", @"The MASQ core rejected the configuration.", nil);
    return;
  }
  if (statusSucceeded(result)) {
    NSMutableDictionary *saved = migratedConfig(configJson);
    NSData *savedData = [NSJSONSerialization dataWithJSONObject:saved options:0 error:nil];
    NSString *savedString = [[NSString alloc] initWithData:savedData encoding:NSUTF8StringEncoding];
    [NSUserDefaults.standardUserDefaults setObject:savedString
                                            forKey:MasqConfigDefaultsKey];
  }
  resolve(result);
}

- (void)importWallet:(NSString *)privateKey
             resolve:(RCTPromiseResolveBlock)resolve
              reject:(RCTPromiseRejectBlock)reject {
  NSString *result = invoke(
      symbol<StringArgumentFunction>("masq_mobile_import_wallet"), privateKey);
  if (!result) {
    reject(@"E_CORE_WALLET", @"The MASQ core rejected the wallet.", nil);
    return;
  }
  if (statusSucceeded(result) && !saveWalletSecret(privateKey)) {
    reject(@"E_KEYCHAIN", @"The consumer wallet could not be saved in the iOS Keychain.", nil);
    return;
  }
  resolve(result);
}

- (void)updateMinHops:(double)minHops
               resolve:(RCTPromiseResolveBlock)resolve
                reject:(RCTPromiseRejectBlock)reject {
  if (minHops < 1 || minHops > 6 || floor(minHops) != minHops) {
    reject(@"E_MIN_HOPS", @"Choose between one and six MASQ hops.", nil);
    return;
  }
  NSString *savedConfig =
      [NSUserDefaults.standardUserDefaults stringForKey:MasqConfigDefaultsKey];
  NSMutableDictionary *config = savedConfig
      ? [[NSJSONSerialization
            JSONObjectWithData:[savedConfig dataUsingEncoding:NSUTF8StringEncoding]
                       options:NSJSONReadingMutableContainers
                         error:nil] mutableCopy]
      : nil;
  if (![config isKindOfClass:[NSMutableDictionary class]]) {
    reject(@"E_SAVED_CONFIG", @"The saved MASQ network profile is invalid.", nil);
    return;
  }

  NSString *result = invoke(
      symbol<UInt8ArgumentFunction>("masq_mobile_update_min_hops"),
      static_cast<uint8_t>(minHops));
  if (!result || !statusSucceeded(result)) {
    reject(@"E_MIN_HOPS", @"The MASQ route length could not be changed.", nil);
    return;
  }

  config[@"minHops"] = @(static_cast<NSInteger>(minHops));
  NSData *updatedData =
      [NSJSONSerialization dataWithJSONObject:config options:0 error:nil];
  NSString *updatedConfig = updatedData
      ? [[NSString alloc] initWithData:updatedData encoding:NSUTF8StringEncoding]
      : nil;
  if (!updatedConfig) {
    reject(@"E_SAVED_CONFIG", @"The updated MASQ profile could not be saved.", nil);
    return;
  }
  [NSUserDefaults.standardUserDefaults setObject:updatedConfig
                                          forKey:MasqConfigDefaultsKey];
  resolve(result);
}

- (void)start:(RCTPromiseResolveBlock)resolve reject:(RCTPromiseRejectBlock)reject {
  [self restoreCoreIfNeeded];
  if (!coreAvailable()) {
    reject(@"E_CORE_UNAVAILABLE", @"The native MASQ core is missing from this build.", nil);
    return;
  }

  NSString *savedConfig =
      [NSUserDefaults.standardUserDefaults stringForKey:MasqConfigDefaultsKey];
  NSData *savedConfigData = [savedConfig dataUsingEncoding:NSUTF8StringEncoding];
  NSMutableDictionary *config = savedConfigData
      ? [[NSJSONSerialization JSONObjectWithData:savedConfigData
                                         options:NSJSONReadingMutableContainers
                                           error:nil] mutableCopy]
      : nil;
  NSString *chain = [config[@"chain"] isKindOfClass:[NSString class]]
      ? config[@"chain"]
      : nil;
  if (!config || !chain.length) {
    reject(@"E_SAVED_CONFIG", @"The saved MASQ network profile is invalid.", nil);
    return;
  }

  NSArray<NSString *> *savedNeighbors =
      [config[@"neighbors"] isKindOfClass:[NSArray class]]
      ? config[@"neighbors"]
      : @[];
  discoverReachableEntryNodes(chain, savedNeighbors,
                              ^(NSArray<NSString *> *nodes,
                                NSString *errorMessage) {
    if (errorMessage) {
      reject(@"E_ENTRY_NODE_DISCOVERY", errorMessage, nil);
      return;
    }
    config[@"neighbors"] = nodes;
    NSError *serializationError = nil;
    NSData *refreshedData =
        [NSJSONSerialization dataWithJSONObject:config options:0 error:&serializationError];
    NSString *refreshedConfig = refreshedData
        ? [[NSString alloc] initWithData:refreshedData encoding:NSUTF8StringEncoding]
        : nil;
    NSString *preparedConfig = refreshedConfig
        ? nativeConfig(refreshedConfig, &serializationError)
        : nil;
    if (!preparedConfig) {
      reject(@"E_ENTRY_NODE_CONFIG",
             @"The refreshed MASQ entry-node profile could not be prepared.",
             serializationError);
      return;
    }

    NSString *configResult = invoke(
        symbol<StringArgumentFunction>("masq_mobile_configure"), preparedConfig);
    if (!configResult || !statusSucceeded(configResult)) {
      reject(@"E_ENTRY_NODE_CONFIG",
             @"The MASQ core rejected the refreshed entry nodes.", nil);
      return;
    }
    [NSUserDefaults.standardUserDefaults setObject:refreshedConfig
                                            forKey:MasqConfigDefaultsKey];
    [self invokeNoArgumentSymbol:"masq_mobile_start" resolve:resolve reject:reject];
  });
}

- (void)reset:(RCTPromiseResolveBlock)resolve reject:(RCTPromiseRejectBlock)reject {
  [NSUserDefaults.standardUserDefaults removeObjectForKey:MasqConfigDefaultsKey];
  resetBrowserProtectionPreferences();
  deleteWalletSecret();
  self.restoreAttempted = YES;
  [self invokeNoArgumentSymbol:"masq_mobile_reset" resolve:resolve reject:reject];
}

- (void)resetNetworkProfile:(RCTPromiseResolveBlock)resolve
                      reject:(RCTPromiseRejectBlock)reject {
  [NSUserDefaults.standardUserDefaults removeObjectForKey:MasqConfigDefaultsKey];
  self.restoreAttempted = YES;
  [self invokeNoArgumentSymbol:"masq_mobile_reset_network_profile" resolve:resolve reject:reject];
}

- (void)removeWallet:(RCTPromiseResolveBlock)resolve
               reject:(RCTPromiseRejectBlock)reject {
  deleteWalletSecret();
  self.restoreAttempted = YES;
  [self invokeNoArgumentSymbol:"masq_mobile_remove_wallet" resolve:resolve reject:reject];
}

- (void)preflightBrowserProxy:(RCTPromiseResolveBlock)resolve
                        reject:(RCTPromiseRejectBlock)reject {
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    NSString *result = invoke(symbol<NoArgumentFunction>("masq_mobile_preflight_proxy"));
    if (!result) {
      reject(@"E_PROXY_PREFLIGHT", @"The MASQ browser route test returned no result.", nil);
      return;
    }
    if (!statusSucceeded(result)) {
      NSData *data = [result dataUsingEncoding:NSUTF8StringEncoding];
      NSDictionary *status = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
      reject(@"E_PROXY_PREFLIGHT", status[@"lastError"] ?: @"The MASQ exit route test failed.", nil);
      return;
    }
    resolve(result);
  });
}

- (void)getSystemTunnelStatus:(RCTPromiseResolveBlock)resolve
                         reject:(RCTPromiseRejectBlock)reject {
  NSDictionary *status = @{
    @"supported" : @NO,
    @"active" : @NO,
    @"mode" : @"off",
    @"phase" : @"off",
    @"selectedApps" : @[],
    @"lastError" : [NSNull null],
  };
  NSData *data = [NSJSONSerialization dataWithJSONObject:status options:0 error:nil];
  resolve([[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding]);
}

- (void)getRoutableApps:(RCTPromiseResolveBlock)resolve
                   reject:(RCTPromiseRejectBlock)reject {
  resolve(@"[]");
}

- (void)setSystemTunnel:(NSString *)mode
              appIdsJson:(NSString *)appIdsJson
                 resolve:(RCTPromiseResolveBlock)resolve
                  reject:(RCTPromiseRejectBlock)reject {
  reject(@"E_NETWORK_EXTENSION",
         @"Whole-device routing requires a separately entitled iOS Packet Tunnel extension.",
         nil);
}

- (void)restoreCoreIfNeeded {
  if (self.restoreAttempted || !coreAvailable()) {
    return;
  }
  self.restoreAttempted = YES;
  NSString *savedConfig =
      [NSUserDefaults.standardUserDefaults stringForKey:MasqConfigDefaultsKey];
  NSString *savedWallet = loadWalletSecret();
  if (!savedConfig || !savedWallet) {
    return;
  }
  NSError *configError = nil;
  NSString *preparedConfig = nativeConfig(savedConfig, &configError);
  if (!preparedConfig) {
    return;
  }
  NSString *configStatus = invoke(
      symbol<StringArgumentFunction>("masq_mobile_configure"), preparedConfig);
  if (!configStatus || !statusSucceeded(configStatus)) {
    return;
  }
  invoke(symbol<StringArgumentFunction>("masq_mobile_import_wallet"), savedWallet);
}

- (void)stop:(RCTPromiseResolveBlock)resolve reject:(RCTPromiseRejectBlock)reject {
  if (!coreAvailable()) {
    resolve(unavailableStatus(@"The native MASQ core is missing from this build."));
    return;
  }
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    NSString *result = invoke(symbol<NoArgumentFunction>("masq_mobile_stop"));
    result ? resolve(result)
           : reject(@"E_CORE_STOP", @"The MASQ core could not be stopped.", nil);
  });
}

- (void)shutdown:(RCTPromiseResolveBlock)resolve
          reject:(RCTPromiseRejectBlock)reject {
  if (!coreAvailable()) {
    resolve(unavailableStatus(@"The native MASQ core is missing from this build."));
    return;
  }
  // Explicit direct browsing joins the embedded Node thread. Keep that bounded wait away from
  // the TurboModule/UI queue and resolve only after Rust confirms that the peer mesh is gone.
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    NSString *result = invoke(symbol<NoArgumentFunction>("masq_mobile_shutdown"));
    result ? resolve(result)
           : reject(@"E_CORE_SHUTDOWN",
                    @"The MASQ peer connection could not be shut down.", nil);
  });
}

- (void)setBrowserRoutingMode:(NSString *)mode
                      resolve:(RCTPromiseResolveBlock)resolve
                       reject:(RCTPromiseRejectBlock)reject {
  if (![mode isEqualToString:@"blocked"] &&
      ![mode isEqualToString:@"masq"] &&
      ![mode isEqualToString:@"direct"]) {
    reject(@"E_BROWSER_ROUTING_MODE",
           @"Choose the blocked, masq, or direct browser routing mode.",
           nil);
    return;
  }

  if (@available(iOS 17.0, *)) {
    dispatch_async(dispatch_get_main_queue(), ^{
      if ([mode isEqualToString:@"blocked"]) {
        configurePrivateBrowserProxy(masqBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        configurePrivateBrowserProxy(masqPersistentBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        configurePrivateBrowserProxy(directBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        configurePrivateBrowserProxy(directPersistentBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        if (coreAvailable()) {
          NSString *syncResult = invoke(
              symbol<BooleanArgumentFunction>("masq_mobile_set_proxy_enabled"),
              false);
          if (!syncResult || !statusSucceeded(syncResult)) {
            reject(@"E_PROXY_STATE",
                   @"The MASQ core could not confirm that browser proxying is disabled.",
                   nil);
            return;
          }
        }
        resolve(@"blocked");
        return;
      }

      if ([mode isEqualToString:@"direct"]) {
        configurePrivateBrowserProxy(masqBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        configurePrivateBrowserProxy(masqPersistentBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        [directBrowserDataStore() setProxyConfigurations:@[]];
        [directPersistentBrowserDataStore() setProxyConfigurations:@[]];
        if (coreAvailable()) {
          NSString *syncResult = invoke(
              symbol<BooleanArgumentFunction>("masq_mobile_set_proxy_enabled"),
              false);
          if (!syncResult || !statusSucceeded(syncResult)) {
            configurePrivateBrowserProxy(directBrowserDataStore(),
                                         MasqBlockedBrowserProxyPort);
            configurePrivateBrowserProxy(directPersistentBrowserDataStore(),
                                         MasqBlockedBrowserProxyPort);
            reject(@"E_PROXY_STATE",
                   @"The MASQ core could not confirm that browser proxying is disabled.",
                   nil);
            return;
          }
        }
        resolve(@"direct");
        return;
      }

      // The only remaining validated mode is MASQ. Sink both stores while the current proxy
      // endpoint is checked so that a stale port can never be reused during the transition.
      configurePrivateBrowserProxy(directBrowserDataStore(),
                                   MasqBlockedBrowserProxyPort);
      configurePrivateBrowserProxy(directPersistentBrowserDataStore(),
                                   MasqBlockedBrowserProxyPort);
      configurePrivateBrowserProxy(masqBrowserDataStore(),
                                   MasqBlockedBrowserProxyPort);
      configurePrivateBrowserProxy(masqPersistentBrowserDataStore(),
                                   MasqBlockedBrowserProxyPort);
      if (!coreAvailable()) {
        reject(@"E_CORE_UNAVAILABLE", @"The native MASQ core is missing from this build.", nil);
        return;
      }

      NSString *statusJson = invoke(symbol<NoArgumentFunction>("masq_mobile_get_status"));
      if (!statusJson) {
        reject(@"E_CORE_STATUS", @"The MASQ core returned no status.", nil);
        return;
      }
      NSData *statusData = [statusJson dataUsingEncoding:NSUTF8StringEncoding];
      NSDictionary *status = [NSJSONSerialization JSONObjectWithData:statusData options:0 error:nil];
      NSNumber *port = [status isKindOfClass:[NSDictionary class]]
          ? status[@"proxyPort"]
          : nil;
      if (![status isKindOfClass:[NSDictionary class]] ||
          ![status[@"phase"] isEqualToString:@"connected"] ||
          ![port isKindOfClass:[NSNumber class]] || port.integerValue < 1 ||
          port.integerValue > 65535) {
        reject(@"E_NOT_CONNECTED", @"Build a valid MASQ route first.", nil);
        return;
      }

      configurePrivateBrowserProxy(masqBrowserDataStore(),
                                   static_cast<uint16_t>(port.unsignedShortValue));
      configurePrivateBrowserProxy(
          masqPersistentBrowserDataStore(),
          static_cast<uint16_t>(port.unsignedShortValue));

      NSString *syncResult = invoke(
          symbol<BooleanArgumentFunction>("masq_mobile_set_proxy_enabled"), true);
      if (!syncResult || !statusSucceeded(syncResult)) {
        configurePrivateBrowserProxy(masqBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        configurePrivateBrowserProxy(masqPersistentBrowserDataStore(),
                                     MasqBlockedBrowserProxyPort);
        reject(@"E_PROXY_STATE", @"The MASQ core could not confirm the proxy.", nil);
        return;
      }
      resolve(@"masq");
    });
  } else {
    reject(@"E_PROXY_UNSUPPORTED", @"MASQ Mobile requires iOS 17 or later.", nil);
  }
}

- (std::shared_ptr<facebook::react::TurboModule>)
    getTurboModule:(const facebook::react::ObjCTurboModule::InitParams &)params {
  return std::make_shared<facebook::react::NativeMasqCoreSpecJSI>(params);
}

- (void)invokeNoArgumentSymbol:(const char *)name
                       resolve:(RCTPromiseResolveBlock)resolve
                        reject:(RCTPromiseRejectBlock)reject {
  if (!coreAvailable()) {
    reject(@"E_CORE_UNAVAILABLE", @"The native MASQ core is missing from this build.", nil);
    return;
  }
  NSString *result = invoke(symbol<NoArgumentFunction>(name));
  result ? resolve(result) : reject(@"E_CORE", @"The MASQ core rejected the request.", nil);
}

- (void)invokeStringSymbol:(const char *)name
                  argument:(NSString *)argument
                   resolve:(RCTPromiseResolveBlock)resolve
                    reject:(RCTPromiseRejectBlock)reject {
  if (!coreAvailable()) {
    reject(@"E_CORE_UNAVAILABLE", @"The native MASQ core is missing from this build.", nil);
    return;
  }
  NSString *result = invoke(symbol<StringArgumentFunction>(name), argument);
  result ? resolve(result) : reject(@"E_CORE", @"The MASQ core rejected the request.", nil);
}

@end
