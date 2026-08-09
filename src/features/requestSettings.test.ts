import { describe, expect, it } from 'vitest'
import { booleanSetting, httpVersionSetting, numberSetting, settingBooleanValue } from './requestSettings'

describe('request settings conversion', () => {
  it('preserves inherited boolean values', () => {
    expect(settingBooleanValue(undefined)).toBe('inherit')
    expect(settingBooleanValue(true)).toBe('true')
    expect(settingBooleanValue(false)).toBe('false')
    expect(booleanSetting('inherit')).toBeUndefined()
    expect(booleanSetting('true')).toBe(true)
    expect(booleanSetting('false')).toBe(false)
  })

  it('normalizes numeric controls and clears invalid values', () => {
    expect(numberSetting(1500.8)).toBe(1501)
    expect(numberSetting(0)).toBeUndefined()
    expect(numberSetting(-1)).toBeUndefined()
    expect(numberSetting(null)).toBeUndefined()
    expect(numberSetting(Number.NaN)).toBeUndefined()
  })

  it('keeps only supported HTTP versions', () => {
    expect(httpVersionSetting('inherit')).toBeUndefined()
    expect(httpVersionSetting('http1')).toBe('http1')
    expect(httpVersionSetting('http2')).toBe('http2')
  })
})
