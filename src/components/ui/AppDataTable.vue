<script setup lang="ts">
import { NDataTable } from 'naive-ui'

defineOptions({ inheritAttrs: false })
</script>

<template>
  <NDataTable class="app-data-table" v-bind="$attrs" />
</template>
