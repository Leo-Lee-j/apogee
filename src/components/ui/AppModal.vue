<script setup lang="ts">
import { NModal } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NModal class="app-modal" v-bind="$attrs">
    <slot />
    <template v-if="$slots.header" #header>
      <slot name="header" />
    </template>
    <template v-if="$slots.footer" #footer>
      <slot name="footer" />
    </template>
    <template v-if="$slots.action" #action>
      <slot name="action" />
    </template>
  </NModal>
</template>
