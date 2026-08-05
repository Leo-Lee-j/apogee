<script setup lang="ts">
import { NForm } from 'naive-ui'

type FormSize = 'small' | 'medium' | 'large'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<{ size?: FormSize }>(), { size: 'small' })
</script>

<template>
  <NForm class="app-form" v-bind="$attrs" :size="props.size">
    <slot />
  </NForm>
</template>
