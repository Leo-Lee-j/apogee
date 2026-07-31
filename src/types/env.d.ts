/// <reference types="vite/client" />

/** vite.config.ts 的 define 注入。取值 'a' | 'b'，见 Spike S5。 */
declare const __S5_VARIANT__: string

declare module '*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}

/** 通过 resolve.alias 指向 AppShellNaive.vue 或 AppShellLean.vue */
declare module '@app-shell' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent<{}, {}, any>
  export default component
}
