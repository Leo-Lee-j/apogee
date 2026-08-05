<script setup lang="ts">
import { NTabPane } from 'naive-ui'

defineOptions({ inheritAttrs: false })

const props = defineProps<{
  name: string | number
}>()
</script>

<template>
  <NTabPane class="app-tab-pane" :name="props.name" v-bind="$attrs">
    <slot />
  </NTabPane>
</template>
