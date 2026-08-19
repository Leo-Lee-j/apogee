import type { GlobalThemeOverrides } from 'naive-ui'

/**
 * Naive UI 主题覆写。
 *
 * # 为什么是模块级常量（02 文档 §7.5 第 2 条）
 *
 * naive-ui 是 CSS-in-JS 方案。`themeOverrides` 的**引用**变化就会触发整套样式
 * 重新计算并重新注入。如果把它写在 `computed` 里，每次依赖变化都会重建对象，
 * 引用变了 → 全量样式重算 → 首屏和主题切换都会明显卡顿。
 *
 * 所以：定义为模块级 `const`，永不重建。
 * 需要动态换主题时，切换的是「引用哪个常量」，而不是「改这个常量的内容」。
 *
 * # 与 tokens.css 的关系
 *
 * 数值与 src/styles/tokens.css 手工对齐 —— 不能在运行时读 CSS 变量，那会把
 * 首屏拖进一次同步布局。两边必须一致，否则 naive-ui 组件和自研组件会出现色差。
 *
 * 这条约束由 `node scripts/check-contrast.mjs` 守着：它会解析 tokens.css 和
 * 本文件，比对关键色值，不一致直接 fail。改色值时两处一起改。
 */

/** 与 tokens.css `:root` 的 --a-* 对齐。 */
const commonDark = {
  primaryColor: '#4f80ff',
  primaryColorHover: '#6f98ff',
  primaryColorPressed: '#4576f5',
  primaryColorSuppl: '#4f80ff',

  // 语义色用 Apogee 的基色（而非 pill 前景色）：naive-ui 会拿它们做实色填充，
  // pill 前景那一档太浅，填充后压不住白字。
  successColor: '#34d399',
  warningColor: '#fbbf24',
  errorColor: '#f87171',
  infoColor: '#4f80ff',

  textColorBase: '#f1f5f9',
  textColor1: '#f1f5f9',
  textColor2: '#cbd5e1',
  textColor3: '#94a3b8',

  bodyColor: '#0c0e12',
  cardColor: '#14171f',
  modalColor: '#14171f',
  popoverColor: '#1a1e29',
  inputColor: '#1a1e29',
  tableColor: '#14171f',

  borderColor: '#232733',
  dividerColor: '#232733',

  // --a-radius-md / --a-radius-sm。Apogee 的 9px/6px 各下移一档，
  // 28px 行高配 9px 圆角在密集表格里会显胖。
  borderRadius: '12px',
  borderRadiusSmall: '8px',

  // 字体与字号统一从 tokens.css 读取，设置变更后无需重建主题对象。
  fontFamily: 'var(--a-font-ui)',
  fontFamilyMono: 'var(--a-font-mono)',
  fontSize: 'var(--a-fs-base)',
  fontSizeSmall: 'var(--a-fs-sm)',
  lineHeight: 'var(--a-lh)',
} as const

/** 与 tokens.css `:root[data-theme='light']` 对齐。 */
const commonLight = {
  ...commonDark,
  primaryColor: '#5b5bd6',
  primaryColorHover: '#4f46e5',
  primaryColorPressed: '#4338ca',
  primaryColorSuppl: '#5b5bd6',

  successColor: '#059669',
  warningColor: '#d97706',
  errorColor: '#dc2626',
  infoColor: '#5b5bd6',

  textColorBase: '#111827',
  textColor1: '#111827',
  textColor2: '#374151',
  textColor3: '#5e6c80',

  // 亮主题里画布是云母白、面板是纯白（层级方向与暗主题相反，见 tokens.css）
  bodyColor: '#f8fafc',
  cardColor: '#ffffff',
  modalColor: '#ffffff',
  popoverColor: '#ffffff',
  inputColor: '#f1f5f9',
  tableColor: '#ffffff',

  borderColor: '#e2e8f0',
  dividerColor: '#e2e8f0',
} as const

/** 组件级只覆写必要的部分。改得越多，样式计算越慢。 */
const components = {
  Input: {
    heightSmall: 'var(--a-h-input-sm)',
    paddingSmall: '0 10px',
    fontSizeSmall: 'var(--a-fs-sm)',
    lineHeight: 'var(--a-lh)',
    borderRadius: '12px',
    heightMedium: 'var(--a-h-input-md)',
    paddingMedium: '0 10px',
    fontSizeMedium: 'var(--a-fs-base)',
    heightLarge: 'var(--a-h-input-lg)',
    paddingLarge: '0 12px',
    fontSizeLarge: 'var(--a-fs-md)',
  },
  Button: {
    heightSmall: 'var(--a-h-control-sm)',
    paddingSmall: '0 12px',
    fontSizeSmall: 'var(--a-fs-sm)',
    borderRadiusSmall: '12px',
    heightMedium: 'var(--a-h-control)',
    paddingMedium: '0 14px',
    fontSizeMedium: 'var(--a-fs-base)',
    borderRadiusMedium: '12px',
    textColorPrimary: '#0c0e12',
    textColorHoverPrimary: '#0c0e12',
    textColorPressedPrimary: '#0c0e12',
    textColorFocusPrimary: '#0c0e12',
    textColorDisabledPrimary: '#0c0e12',
  },
  // Checkbox 尺寸较小，不能继承 8px 小圆角，否则视觉上会成为圆形。
  Checkbox: {
    borderRadius: '2px',
  },
  Card: {
    borderRadius: '12px',
    paddingSmall: '12px',
    fontSizeSmall: 'var(--a-fs-sm)',
    titleFontSizeSmall: 'var(--a-fs-base)',
  },
  Form: {
    lineHeight: 'var(--a-lh)',
    labelFontSizeLeftSmall: 'var(--a-fs-sm)',
    labelFontSizeTopSmall: 'var(--a-fs-sm)',
  },
  Alert: {
    fontSize: 'var(--a-fs-sm)',
    lineHeight: 'var(--a-lh)',
    borderRadius: '6px',
    padding: '8px 10px',
  },
  Tabs: {
    tabFontSizeSmall: 'var(--a-fs-sm)',
    tabGapSmallLine: '2px',
    tabPaddingSmallLine: '0 8px',
  },
  Select: {
    peers: {
      InternalSelection: {
        fontSizeSmall: 'var(--a-fs-sm)',
        heightSmall: 'var(--a-h-input-sm)',
        borderRadius: '12px',
        paddingSingle: '0 8px',
        fontSizeMedium: 'var(--a-fs-base)',
        heightMedium: 'var(--a-h-input-md)',
      },
    },
  },
} as const

/** 暗色覆写。**不要在渲染函数里重建这个对象。** */
export const darkThemeOverrides: GlobalThemeOverrides = {
  common: commonDark,
  ...components,
}

/** 亮色覆写。 */
export const lightThemeOverrides: GlobalThemeOverrides = {
  common: commonLight,
  ...components,
  Button: {
    ...components.Button,
    textColorPrimary: '#ffffff',
    textColorHoverPrimary: '#ffffff',
    textColorPressedPrimary: '#ffffff',
    textColorFocusPrimary: '#ffffff',
    textColorDisabledPrimary: '#ffffff',
  },
}
