<script setup lang="ts">
import { NFormItem } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NFormItem class="app-form-item" v-bind="$attrs">
    <slot />
  </NFormItem>
</template>
