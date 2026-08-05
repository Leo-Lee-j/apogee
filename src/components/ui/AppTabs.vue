<script setup lang="ts">
import { NTabs } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NTabs class="app-tabs" v-bind="$attrs">
    <slot />
  </NTabs>
</template>
