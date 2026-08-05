<script setup lang="ts">
import { NAlert } from 'naive-ui'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ showIcon?: boolean }>(), { showIcon: false })
</script>

<template>
  <NAlert class="app-alert" v-bind="$attrs" :show-icon="props.showIcon">
    <template v-if="$slots.icon" #icon>
      <slot name="icon" />
    </template>
    <slot />
  </NAlert>
</template>
