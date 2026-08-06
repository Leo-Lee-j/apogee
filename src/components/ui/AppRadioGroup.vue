<script setup lang="ts">
import { NRadioGroup, NRadioButton } from 'naive-ui'

defineOptions({ inheritAttrs: false })
defineProps<{ options: Array<{ label: string; value: string }> }>()
</script>

<template>
  <NRadioGroup class="app-radio-group" v-bind="$attrs">
    <NRadioButton v-for="option in options" :key="option.value" :value="option.value">
      {{ option.label }}
    </NRadioButton>
  </NRadioGroup>
</template>
