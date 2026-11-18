import { createApp, watch } from 'vue'
import { i18n, initializeLanguage, resolveSystemLanguage } from '@/common/i18n'
import { createPinia } from 'pinia'
import piniaPersistedState from 'pinia-plugin-persistedstate'
import { emit, listen, transportMode, capabilities, serverInfo } from '@/common/transport'
import { invoke } from '@/common/transport'
import { setPathSeparator } from '@/common/utils'
// Desktop startup telemetry is not on lapd's HTTP whitelist (phase 3 keeps the
// bridge to app commands); the Web client fires these only in the desktop shell.
const recordStartupStage = (stage) => {
  if (!capabilities().native) return
  void invoke('record_startup_stage', { stage }).catch(console.error)
}
import { getCurrentWebviewWindow } from '@/common/transport/window.js'
// Web client: navigator.language is the browser equivalent of plugin-os's
// locale; the real plugin has no browser implementation (phase 4).
const getSystemLanguage = transportMode === 'http'
  ? async () => navigator.language
  : async () => (await import('@tauri-apps/plugin-os')).locale()
import { getSystemRegionLocale } from '@/common/api'
import 'cally'
import router from '@/common/router'
import App from '@/App.vue'
import { useConfigStore } from '@/stores/configStore'
import '@/assets/app.css'

performance.mark('startup-frontend-started')

// Create the app instance
const app = createApp(App)

// Create Pinia store and use the persisted state plugin
const pinia = createPinia()
pinia.use(piniaPersistedState)
app.use(pinia) // Use Pinia
const config = useConfigStore() // Use the config store
const currentWindowLabel = getCurrentWebviewWindow().label
const isMainWindow = currentWindowLabel === 'main'
const isSettingsWindow = currentWindowLabel === 'settings'
let settingsSyncRequest = 0

listen('montage-config-changed', (event) => {
  // Keep other windows current so their later writes preserve collage preferences.
  if (currentWindowLabel !== 'montage') {
    Object.assign(config.montage, event.payload)
  }
})

listen('image-editor-config-changed', (event) => {
  // Keep other windows current so their later writes preserve editor preferences.
  if (currentWindowLabel !== 'imageeditor') {
    Object.assign(config.imageEditor, event.payload)
  }
})

listen('dedup-config-changed', (event) => {
  // Keep other windows current so their later writes preserve dedup keep rules.
  if (currentWindowLabel !== 'main') {
    Object.assign(config.dedup, event.payload)
  }
})

listen('af-area-display-changed', (event) => {
  // All open windows keep the same preference so later store writes preserve it.
  if (typeof event.payload === 'boolean' && config.mediaViewer.showAfAreas !== event.payload) {
    config.mediaViewer.showAfAreas = event.payload
  }
})

// Resolve Regional Format before components first render dates.
await getSystemRegionLocale().then((loc) => config.setSystemLocale(loc)).catch(() => {})

if (isMainWindow) {
  config.$subscribe((_mutation, state) => {
    void emit('config-settings-synced', JSON.parse(JSON.stringify(state.settings)))
  })
  watch(() => config.dedup.keepRules, () => {
    void emit('dedup-config-changed', JSON.parse(JSON.stringify(config.dedup)))
  }, { deep: true })
} else {
  listen('config-settings-synced', async (event) => {
    const request = ++settingsSyncRequest
    if (isSettingsWindow) {
      // Keep main-window choices in sync while preserving edits in Settings.
      config.settings.deletePermanently = event.payload.deletePermanently === true
      config.setLanguage(event.payload.language)
    } else {
      const { language, ...settings } = event.payload
      await config.setLanguage(language)
      if (request !== settingsSyncRequest) return
      Object.assign(config.settings, settings)
    }
  })
}

// Load only the active language and English before components read messages.
if (!config.settings.languageInitialized) {
  const systemLanguage = await getSystemLanguage().catch(() => null)
  if (!config.settings.languageInitialized) config.settings.language = resolveSystemLanguage(systemLanguage)
}
const initialLanguage = config.settings.language
const loadedLanguage = await initializeLanguage(initialLanguage)
if (config.settings.language === initialLanguage) {
  config.settings.language = loadedLanguage
  config.settings.languageInitialized = true
}
else i18n.global.locale.value = config.settings.language
performance.mark('startup-language-ready')
if (isMainWindow) recordStartupStage('language-ready')

// Set up global properties
app.config.globalProperties.$invoke = invoke

// Web client: resolve the session BEFORE installing the router, so its very
// first navigation already knows whether to show /login (phase 4). Installing
// the router first would run the guard while the session is still unknown and
// bounce a signed-in user to /login on every refresh.
if (transportMode === 'http') {
  const { checkSession } = await import('@/common/auth')
  await checkSession()
  // Adopt the SERVER's path separator (the library's paths use it, not the
  // browser's). Non-fatal: a failure keeps the browser-derived default.
  await serverInfo()
    .then((info) => setPathSeparator(info?.pathSeparator))
    .catch(() => {})
  // Install the app-shell service worker so the PWA opens offline. Web only:
  // the desktop shell has no service worker and its assets come from the bundle.
  // `__TAURI_INTERNALS__` guards the mixed case of an http-flavored bundle run
  // inside the desktop webview: registering there poisons the tauri.localhost
  // origin for every later desktop build (network-first navigation falls back
  // to a stale/error shell and the app never boots).
  // Register eagerly when the document has already finished loading: the awaits
  // above (checkSession / serverInfo) can outlast the `load` event, so waiting
  // for it would attach the listener too late and never register.
  if ('serviceWorker' in navigator && !window.__TAURI_INTERNALS__) {
    const registerSw = () => navigator.serviceWorker.register('/sw.js').catch(() => {})
    if (document.readyState === 'complete') {
      registerSw()
    } else {
      window.addEventListener('load', registerSw)
    }
  }
} else if ('serviceWorker' in navigator && window.caches) {
  // Self-heal the poisoned-origin case above: unregister any service worker an
  // http-flavored bundle left on this desktop origin and drop its shell cache.
  navigator.serviceWorker.getRegistrations()
    .then((registrations) => Promise.all(registrations.map((r) => r.unregister())))
    .then((removed) => {
      if (!removed.some(Boolean)) return undefined
      return caches.keys().then((keys) => Promise.all(keys.map((k) => caches.delete(k))))
    })
    .catch(() => {})
}

// Use the router and i18n
app.use(router)
app.use(i18n)

// Mount the app
app.mount('#app')
performance.mark('startup-app-mounted')
if (isMainWindow) recordStartupStage('app-mounted')

// Listen for events
if (isMainWindow) {
  listen('settings-appearance-changed', (event) => {
    config.setAppearance(event.payload)
  })
  listen('settings-lightTheme-changed', (event) => {
    config.setLightTheme(event.payload)
  })
  listen('settings-darkTheme-changed', (event) => {
    config.setDarkTheme(event.payload)
  })
  listen('settings-scale-changed', (event) => {
    config.setScale(event.payload)
  })
  listen('settings-externalApps-changed', (event) => {
    config.setExternalApps(event.payload)
  })
  listen('settings-language-changed', (event) => {
    config.setLanguage(event.payload, { fromUser: true })
  })
  listen('settings-showToolTip-changed', (event) => {
    config.setShowToolTip(event.payload)
  })
  listen('settings-showStatusBar-changed', (event) => {
    config.setShowStatusBar(event.payload)
  })
  listen('settings-autoCheckUpdates-changed', (event) => {
    config.setAutoCheckUpdates(event.payload)
  })
  listen('settings-debugMode-changed', (event) => {
    config.setDebugMode(event.payload)
  })
  listen('settings-settingsTabIndex-changed', (event) => {
    config.setSettingsTabIndex(event.payload)
  })
  listen('settings-folderSort-changed', (event) => {
    config.setFolderSort(event.payload)
  })
  listen('settings-dateTimeFormat-changed', (event) => {
    config.setDateTimeFormat(event.payload)
  })
  listen('settings-showSubfolderFiles-changed', (event) => {
    config.setShowSubfolderFiles(event.payload)
  })
  listen('settings-thumbnailSize-changed', (event) => {
    config.setThumbnailSize(event.payload)
  })
  listen('settings-rawPairClipboardMode-changed', (event) => {
    config.setRawPairClipboardMode(event.payload)
  })
  listen('settings-rawPairDisplaySource-changed', (event) => {
    config.setRawPairDisplaySource(event.payload)
  })
  listen('settings-rawPreviewSource-changed', (event) => {
    config.setRawPreviewSource(event.payload)
  })
  listen('settings-rawRenderBrightness-changed', (event) => {
    config.setRawRenderBrightness(event.payload)
  })
  listen('settings-mapProvider-changed', (event) => {
    config.setMapProvider(event.payload)
  })
  listen('settings-tiandituToken-changed', (event) => {
    config.setTiandituToken(event.payload)
  })
  listen('settings-mapMarkerSize-changed', (event) => {
    config.setMapMarkerSize(event.payload)
  })
  listen('settings-gridStyle-changed', (event) => {
    config.setGridStyle(event.payload)
  })
  listen('settings-gridScaling-changed', (event) => {
    config.setGridScaling(event.payload)
  })
  listen('settings-gridThumbnailCorners-changed', (event) => {
    config.setGridThumbnailCorners(event.payload)
  })
  listen('settings-gridLabelPrimary-changed', (event) => {
    config.setGridLabelPrimary(event.payload)
  })
  listen('settings-gridLabelSecondary-changed', (event) => {
    config.setGridLabelSecondary(event.payload)
  })
  listen('settings-gridThumbnailBadge-changed', (event) => {
    config.setGridThumbnailBadge(event.payload)
  })
  listen('settings-filmStripViewPreviewPosition-changed', (event) => {
    config.setFilmStripViewPreviewPosition(event.payload)
  })
  listen('settings-mouseWheelMode-changed', (event) => {
    config.setMouseWheelMode(event.payload)
  })
  listen('settings-slideShowInterval-changed', (event) => {
    config.setSlideShowInterval(event.payload)
  })
  listen('settings-autoPlayVideo-changed', (event) => {
    config.setAutoPlayVideo(event.payload)
  })
  listen('settings-loopVideo-changed', (event) => {
    config.settings.loopVideo = event.payload
  })
  listen('settings-groupRawJpegPairs-changed', (event) => {
    config.settings.groupRawJpegPairs = event.payload
  })
  listen('settings-navigatorViewMode-changed', (event) => {
    config.setNavigatorViewMode(event.payload)
  })
  listen('settings-navigatorViewSize-changed', (event) => {
    config.setNavigatorViewSize(event.payload)
  })
  listen('settings-navigatorSharpnessGrid-changed', (event) => {
    config.settings.navigatorSharpnessGrid = event.payload
  })
  listen('settings-viewBackground-changed', (event) => {
    config.setViewBackground(event.payload)
  })
  listen('settings-slideShowTransition-changed', (event) => {
    config.setSlideShowTransition(event.payload)
  })
  listen('settings-imageSearchThresholdIndex-changed', (event) => {
    config.setImageSearchThresholdIndex(event.payload)
  })
  listen('settings-imageSearchModel-changed', (event) => {
    config.setImageSearchModel(event.payload)
  })
  listen('settings-similarPhotoGroupingThresholdIndex-changed', (event) => {
    config.setSimilarPhotoGroupingThresholdIndex(event.payload)
  })
  listen('settings-faceClusterThresholdIndex-changed', (event) => {
    config.setFaceClusterThresholdIndex(event.payload)
  })
  listen('settings-faceEnabled-changed', (event) => {
    config.setFaceEnabled(event.payload)
  })
  listen('libraries-changed', () => {
    config.notifyLibrariesChanged()
  })
}
