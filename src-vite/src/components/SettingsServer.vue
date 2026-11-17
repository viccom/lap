<template>
  <div class="flex flex-col space-y-2">
    <!-- server / device management: admin-only surface (the backend endpoints
         require an admin token or a loopback caller). -->
    <div class="rounded-box p-2 space-y-2 bg-base-300/30 border border-base-content/5 shadow-sm">
      <div class="flex items-center gap-2 text-base-content/30">
        <span class="font-bold uppercase text-[10px] tracking-widest">{{ $t('settings.server.section_devices') }}</span>
      </div>
      <div class="text-xs text-base-content/30 px-1">{{ $t('settings.server.devices_hint') }}</div>

      <div v-if="error" class="text-xs text-warning px-1" role="status">{{ error }}</div>

      <div v-if="devices.length === 0" class="text-sm text-base-content/40 px-1">
        {{ loading ? $t('tooltip.loading') : $t('settings.server.no_devices') }}
      </div>
      <div v-else class="space-y-1">
        <div
          v-for="device in devices"
          :key="device.id"
          class="flex items-center gap-2 px-2 py-1.5 rounded-box hover:bg-base-100/10"
        >
          <div class="min-w-0 flex-1 flex flex-col gap-0.5">
            <div class="text-sm truncate">{{ device.deviceName }}</div>
            <div class="text-xs text-base-content/30 truncate">
              {{ device.scope }} · {{ lastSeen(device.lastSeenAt) }}
            </div>
          </div>
          <button
            class="btn btn-xs btn-ghost rounded-box text-warning shrink-0"
            :disabled="revoking === device.id"
            @click="revoke(device)"
          >
            {{ $t('settings.server.revoke') }}
          </button>
        </div>
      </div>

      <div class="flex items-center justify-between gap-4 px-1 pt-1 border-t border-base-content/5">
        <div class="text-sm">{{ $t('settings.server.pair_new') }}</div>
        <button
          class="btn btn-sm btn-ghost rounded-box bg-base-100 border border-base-content/30 shrink-0"
          :disabled="minting"
          @click="mintCode"
        >
          {{ minting ? $t('tooltip.loading') : $t('settings.server.generate_code') }}
        </button>
      </div>
      <div v-if="pairingCode" class="rounded-box bg-base-100/40 border border-base-content/10 px-3 py-2 text-center">
        <div class="font-mono text-lg tracking-[0.3em]">{{ pairingCode }}</div>
        <div class="text-xs text-base-content/30">{{ $t('settings.server.code_hint', { minutes: codeMinutes }) }}</div>
      </div>
    </div>

    <!-- add album by server path: the Web client has no folder picker, so an
         admin types the SERVER's absolute path (restricted entry, §9-7). -->
    <div class="rounded-box p-2 space-y-2 bg-base-300/30 border border-base-content/5 shadow-sm">
      <div class="flex items-center gap-2 text-base-content/30">
        <span class="font-bold uppercase text-[10px] tracking-widest">{{ $t('settings.server.add_album_title') }}</span>
      </div>
      <div class="text-xs text-base-content/30 px-1">{{ $t('settings.server.add_album_hint') }}</div>
      <input
        v-model="albumPath"
        type="text"
        class="input input-sm w-full rounded-box"
        :placeholder="$t('settings.server.add_album_path_placeholder')"
        @keyup.enter="addByPath"
      />
      <input
        v-model="albumName"
        type="text"
        class="input input-sm w-full rounded-box"
        :placeholder="$t('settings.server.add_album_name_placeholder')"
        @keyup.enter="addByPath"
      />
      <div class="flex justify-end">
        <button
          class="btn btn-sm btn-primary rounded-box"
          :disabled="adding || !albumPath.trim()"
          @click="addByPath"
        >
          {{ adding ? $t('tooltip.loading') : $t('settings.server.add_album_button') }}
        </button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue';
import { useI18n } from 'vue-i18n';
import { useToast } from '@/common/toast';
import { devicesList, deviceRevoke, pairingCode as mintPairingCode } from '@/common/transport';
import { addAlbum } from '@/common/api';
import { formatTimestamp } from '@/common/utils';

const { t } = useI18n();
const toast = useToast();

const devices = ref<any[]>([]);
const loading = ref(true);
const error = ref('');
const revoking = ref<number | null>(null);
const minting = ref(false);
const pairingCode = ref('');
const codeMinutes = ref(5);
const albumPath = ref('');
const albumName = ref('');
const adding = ref(false);

const lastSeen = (ts: number) => {
  const ms = Number(ts || 0);
  return ms > 0 ? formatTimestamp(ms, 'date_time') : t('settings.server.never');
};

async function loadDevices() {
  loading.value = true;
  error.value = '';
  try {
    devices.value = await devicesList();
  } catch (e: any) {
    error.value = e?.body?.error || e?.message || String(e);
  } finally {
    loading.value = false;
  }
}

async function revoke(device: any) {
  revoking.value = device.id;
  try {
    await deviceRevoke(device.id);
    devices.value = devices.value.filter((d) => d.id !== device.id);
  } catch (e: any) {
    toast.error(e?.body?.error || e?.message || String(e));
  } finally {
    revoking.value = null;
  }
}

async function mintCode() {
  minting.value = true;
  try {
    const res = await mintPairingCode();
    pairingCode.value = res?.code || '';
    codeMinutes.value = Math.max(1, Math.round(Number(res?.expiresInSeconds || 300) / 60));
  } catch (e: any) {
    toast.error(e?.body?.error || e?.message || String(e));
  } finally {
    minting.value = false;
  }
}

async function addByPath() {
  const path = albumPath.value.trim();
  if (!path) return;
  adding.value = true;
  try {
    const name = albumName.value.trim() || path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
    const album = await addAlbum(path, name, '', { fileTypes: 0, smallImageFilter: 0, excludedFolders: [] });
    if (!album) throw new Error(t('settings.server.add_album_failed'));
    toast.success(t('settings.server.add_album_success', { name }));
    albumPath.value = '';
    albumName.value = '';
  } catch (e: any) {
    toast.error(e?.body?.error || e?.message || String(e));
  } finally {
    adding.value = false;
  }
}

onMounted(loadDevices);
</script>
