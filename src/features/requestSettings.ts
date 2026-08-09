import type { RequestSettings } from '@/ipc'

export type InheritedBoolean = 'inherit' | 'true' | 'false'

export function settingBooleanValue(value: boolean | undefined): InheritedBoolean {
  return value === undefined ? 'inherit' : value ? 'true' : 'false'
}

export function booleanSetting(value: string | number | null): boolean | undefined {
  return value === 'inherit' ? undefined : value === 'true'
}

export function numberSetting(value: number | null): number | undefined {
  return value === null || !Number.isFinite(value) || value <= 0 ? undefined : Math.round(value)
}

export function httpVersionSetting(value: string | number | null): RequestSettings['httpVersion'] {
  if (value === 'http1' || value === 'http2') return value
  return undefined
}
