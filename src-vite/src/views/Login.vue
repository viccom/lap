<template>
  <div class="min-h-screen w-screen flex items-center justify-center bg-base-300 text-base-content p-4">
    <div class="w-full max-w-sm rounded-box bg-base-200 shadow-xl p-6 space-y-5">
      <div class="text-center space-y-1">
        <div class="text-3xl font-bold tracking-wide">lappp</div>
        <div class="text-xs text-base-content/50">{{ $t('auth.subtitle') }}</div>
      </div>

      <div class="flex rounded-box bg-base-300 p-1 text-sm">
        <button
          v-if="!passwordSet"
          class="flex-1 py-1.5 rounded-box"
          :class="tab === 'setup' ? 'bg-primary text-primary-content' : ''"
          @click="tab = 'setup'"
        >{{ $t('auth.tab_setup') }}</button>
        <button
          v-if="passwordSet"
          class="flex-1 py-1.5 rounded-box"
          :class="tab === 'login' ? 'bg-primary text-primary-content' : ''"
          @click="tab = 'login'"
        >{{ $t('auth.tab_login') }}</button>
        <button
          class="flex-1 py-1.5 rounded-box"
          :class="tab === 'pair' ? 'bg-primary text-primary-content' : ''"
          @click="tab = 'pair'"
        >{{ $t('auth.tab_pair') }}</button>
      </div>

      <form class="space-y-4" @submit.prevent="submit">
        <label class="block space-y-1">
          <span class="text-xs text-base-content/60">
            {{ tab === 'pair' ? $t('auth.pair_code') : $t('auth.password') }}
          </span>
          <input
            v-model.trim="secret"
            :type="tab === 'pair' ? 'text' : 'password'"
            :placeholder="tab === 'pair' ? $t('auth.pair_placeholder') : $t('auth.password_placeholder')"
            class="input input-bordered w-full"
            autocomplete="off"
            autofocus
          />
        </label>
        <label v-if="tab !== 'pair'" class="block space-y-1">
          <span class="text-xs text-base-content/60">{{ $t('auth.device_name') }}</span>
          <input v-model.trim="deviceName" type="text" class="input input-bordered w-full" />
        </label>

        <div v-if="error" class="text-error text-sm text-center">{{ error }}</div>

        <button type="submit" class="btn btn-primary w-full" :disabled="busy || !secret">
          <span v-if="busy" class="loading loading-spinner loading-sm"></span>
          <span>{{ busy ? $t('auth.working') : submitLabel }}</span>
        </button>
      </form>

      <div class="text-center text-xs text-base-content/40">
        {{ $t('auth.hint_pairing_code') }}
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { useLibraryStore } from '@/stores/libraryStore';
import {
  webMode, passwordConfigured, checkSession, isAuthenticated,
  loginWithPassword, pairWithCode, setupPassword,
} from '@/common/auth';

const router = useRouter();
const { t: $t } = useI18n();
const libConfig = useLibraryStore();
const passwordSet = ref(false);
const tab = ref('login');
const secret = ref('');
const deviceName = ref('web');
const busy = ref(false);
const error = ref('');

onMounted(async () => {
  if (!webMode) { router.replace('/'); return; }
  passwordSet.value = passwordConfigured();
  tab.value = passwordSet.value ? 'login' : 'setup';
  // Re-check in case the session probe ran before this view mounted.
  await checkSession();
  if (isAuthenticated()) {
    router.replace('/');
    return;
  }
  passwordSet.value = passwordConfigured();
  tab.value = passwordSet.value ? 'login' : 'setup';
});

const submitLabel = computed(() => ({
  setup: $t('auth.submit_setup'),
  login: $t('auth.submit_login'),
  pair: $t('auth.submit_pair'),
}[tab.value]));

async function submit() {
  if (!secret.value || busy.value) return;
  busy.value = true;
  error.value = '';
  try {
    if (tab.value === 'setup') {
      await setupPassword(secret.value);
      passwordSet.value = true;
      tab.value = 'login';
      secret.value = '';
      error.value = $t('auth.setup_done');
    } else if (tab.value === 'login') {
      await loginWithPassword(secret.value, deviceName.value || 'web');
      await enterApp();
    } else {
      await pairWithCode(secret.value, 'web');
      await enterApp();
    }
  } catch (e) {
    error.value = e?.body?.error || e?.message || $t('auth.failed_default');
  } finally {
    busy.value = false;
  }
}

async function enterApp() {
  // Initialize library state now that we hold a token, then open the app.
  if (!libConfig._initialized) await libConfig.init();
  router.replace('/');
}
</script>
