<script setup lang="ts">
import { NCheckbox } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NCheckbox class="app-checkbox" v-bind="$attrs">
    <slot />
  </NCheckbox>
</template>
