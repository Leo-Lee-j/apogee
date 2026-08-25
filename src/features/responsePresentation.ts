import type { PublicResponseSummary } from '@/ipc'

/** Metadata the response UI needs to choose a safe presentation and save-file suggestion. */
type ResponseMetadata = Pick<PublicResponseSummary, 'id' | 'contentType' | 'headers'>

export interface ResponsePresentation {
  /** True when rendering as text would likely corrupt or obscure the response. */
  isFile: boolean
  /** Safe, user-editable default for the native save dialog. */
  suggestedFileName: string
}

const MIME_EXTENSIONS: Record<string, string> = {
  'application/json': 'json',
  'application/pdf': 'pdf',
  'application/zip': 'zip',
  'application/gzip': 'gz',
  'application/x-gzip': 'gz',
  'application/x-7z-compressed': '7z',
  'application/x-rar-compressed': 'rar',
  'application/octet-stream': 'bin',
  'application/vnd.ms-excel': 'xls',
  'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet': 'xlsx',
  'application/msword': 'doc',
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document': 'docx',
  'application/vnd.ms-powerpoint': 'ppt',
  'application/vnd.openxmlformats-officedocument.presentationml.presentation': 'pptx',
  'text/plain': 'txt',
  'text/csv': 'csv',
  'text/html': 'html',
  'application/xml': 'xml',
  'text/xml': 'xml',
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/svg+xml': 'svg',
}

const EXPLICIT_FILE_MIME_TYPES = new Set([
  'application/pdf',
  'application/zip',
  'application/gzip',
  'application/x-gzip',
  'application/x-7z-compressed',
  'application/x-rar-compressed',
  'application/octet-stream',
  'application/vnd.ms-excel',
  'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  'application/msword',
  'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  'application/vnd.ms-powerpoint',
  'application/vnd.openxmlformats-officedocument.presentationml.presentation',
])

function normalizedContentType(contentType?: string): string {
  return contentType?.split(';', 1)[0]?.trim().toLowerCase() ?? ''
}

function responseHeader(headers: ResponseMetadata['headers'], name: string): string | undefined {
  return headers?.find(([headerName]) => headerName.toLowerCase() === name)?.[1]
}

/** Parse disposition parameters without treating a semicolon inside a quoted-string as a separator. */
function contentDispositionParameters(value: string): Map<string, string> {
  const parameters = new Map<string, string>()
  let index = value.indexOf(';')
  if (index < 0) return parameters
  index += 1

  while (index < value.length) {
    while (index < value.length && (value[index] === ';' || /\s/.test(value[index] ?? ''))) index += 1
    const nameStart = index
    while (index < value.length && value[index] !== '=' && value[index] !== ';') index += 1
    const name = value.slice(nameStart, index).trim().toLowerCase()
    if (value[index] !== '=') {
      while (index < value.length && value[index] !== ';') index += 1
      continue
    }

    index += 1
    while (index < value.length && /\s/.test(value[index] ?? '')) index += 1
    let parameterValue = ''
    if (value[index] === '"') {
      index += 1
      while (index < value.length) {
        const character = value[index] ?? ''
        index += 1
        if (character === '\\' && index < value.length) {
          parameterValue += value[index] ?? ''
          index += 1
        } else if (character === '"') {
          break
        } else {
          parameterValue += character
        }
      }
    } else {
      const valueStart = index
      while (index < value.length && value[index] !== ';') index += 1
      parameterValue = value.slice(valueStart, index).trim()
    }
    if (name) parameters.set(name, parameterValue)
  }

  return parameters
}

/** RFC 5987 `filename*` takes precedence over legacy quoted `filename`. */
function contentDispositionFileName(value?: string): string | undefined {
  if (!value) return undefined
  const parameters = contentDispositionParameters(value)
  const encoded = parameters.get('filename*')
  if (encoded) {
    const payload = encoded.match(/^[^']*'[^']*'(.*)$/)?.[1] ?? encoded
    try {
      return decodeURIComponent(payload)
    } catch {
      return payload
    }
  }
  return parameters.get('filename')
}

function hasFileExtension(name: string): boolean {
  const dot = name.lastIndexOf('.')
  return dot > 0 && dot < name.length - 1
}

function isWindowsReservedName(name: string): boolean {
  const stem = name.split('.', 1)[0]?.toLowerCase()
  return stem === 'con'
    || stem === 'prn'
    || stem === 'aux'
    || stem === 'nul'
    || /^com[1-9]$/.test(stem ?? '')
    || /^lpt[1-9]$/.test(stem ?? '')
}

/** Keep a server-provided name displayable and safe as a native-dialog default on every platform. */
function sanitizeFileName(value: string | undefined, fallback: string): string {
  const finalPathPart = value?.split(/[\\/]/).at(-1) ?? ''
  const cleaned = [...finalPathPart]
    // Remove Cc/Cf characters, including bidi overrides and isolates that can visually spoof extensions.
    .filter((character) => !/[\p{Cc}\p{Cf}]/u.test(character))
    .map((character) => /[<>:"|?*]/.test(character) ? '_' : character)
    .join('')
    .trim()
    .replace(/[. ]+$/g, '')
  const limited = [...cleaned].slice(0, 160).join('')
  if (!limited || limited === '.' || limited === '..' || isWindowsReservedName(limited)) {
    return fallback
  }
  return limited
}

function isTextualApplicationMime(contentType: string): boolean {
  return contentType.includes('json')
    || contentType.includes('xml')
    || contentType === 'application/javascript'
    || contentType === 'application/x-javascript'
    || contentType === 'application/x-www-form-urlencoded'
    || contentType === 'application/graphql'
    || contentType === 'application/sql'
}

function isFileMime(contentType: string): boolean {
  if (EXPLICIT_FILE_MIME_TYPES.has(contentType)) return true
  if (contentType.startsWith('audio/') || contentType.startsWith('video/')) return true
  // A vendor-specific application MIME that is not a documented text format is safest as a file.
  return contentType.startsWith('application/') && !isTextualApplicationMime(contentType)
}

export function getResponsePresentation(response: ResponseMetadata): ResponsePresentation {
  const contentType = normalizedContentType(response.contentType)
  const disposition = responseHeader(response.headers, 'content-disposition')
  const dispositionName = contentDispositionFileName(disposition)
  const attachment = /(?:^|;)\s*attachment(?:\s*;|$)/i.test(disposition ?? '')
  const extension = MIME_EXTENSIONS[contentType] ?? (contentType.startsWith('text/') ? 'txt' : 'bin')
  const responseKey = response.id.replace(/[^a-z0-9]/gi, '').slice(0, 8) || 'body'
  const fallback = `response-${responseKey}.${extension}`
  let suggestedFileName = sanitizeFileName(dispositionName, fallback)
  if (!hasFileExtension(suggestedFileName)) suggestedFileName = `${suggestedFileName}.${extension}`

  return {
    // An attachment still gets the native save action, but textual and inline-preview MIME types
    // remain viewable. Only binary/unknown attachment responses replace the body with a file card.
    isFile: isFileMime(contentType) || (!contentType && attachment),
    suggestedFileName,
  }
}
