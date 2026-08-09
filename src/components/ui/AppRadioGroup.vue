<script setup lang="ts">
import { NRadio, NRadioGroup } from 'naive-ui'

defineOptions({ inheritAttrs: false })
defineProps<{ options: Array<{ label: string; value: string }> }>()
</script>

<template>
  <NRadioGroup class="app-radio-group" v-bind="$attrs">
    <NRadio v-for="option in options" :key="option.value" :value="option.value">
      {{ option.label }}
    </NRadio>
  </NRadioGroup>
</template>
