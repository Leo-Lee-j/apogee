<script setup lang="ts">
import { NCard } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NCard class="app-card" v-bind="$attrs">
    <slot />
    <template v-if="$slots.header" #header>
      <slot name="header" />
    </template>
    <template v-if="$slots['header-extra']" #header-extra>
      <slot name="header-extra" />
    </template>
    <template v-if="$slots.action" #action>
      <slot name="action" />
    </template>
    <template v-if="$slots.footer" #footer>
      <slot name="footer" />
    </template>
  </NCard>
</template>
