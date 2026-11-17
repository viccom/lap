<template>
  <!-- Web-client "leave this page" control.
       The desktop shell opens each secondary page as its own window, so closing
       the window returns you to the grid. A browser tab has no window chrome and
       cannot close itself, so the Web build renders this explicit back button.
       It is the ONE affordance shared by every secondary page's header so no
       page can be a dead end again (see transport/window.js `closeOrBack`). -->
  <button
    v-if="!isNative"
    type="button"
    class="flex items-center gap-1 h-8 px-2 shrink-0 rounded-box text-base-content/70 hover:text-base-content hover:bg-base-100/30 cursor-pointer"
    :title="$t('toolbar.tooltip.back')"
    :aria-label="$t('toolbar.tooltip.back')"
    data-testid="web-back"
    @click.stop="closeOrBack()"
  >
    <IconLeft class="w-5 h-5" />
    <span class="text-sm whitespace-nowrap hidden sm:inline">{{ $t('toolbar.tooltip.back') }}</span>
  </button>
</template>

<script setup>
import { IconLeft } from '@/common/icons';
import { closeOrBack } from '@/common/transport/window.js';
import { capabilities } from '@/common/transport';

// Desktop windows close normally, so the button is Web-only and self-gates here
// rather than at each call site.
const isNative = capabilities().native;
</script>
