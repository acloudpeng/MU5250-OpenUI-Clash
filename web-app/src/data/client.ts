// HTTP client for the agent: token handling, envelope unwrapping, timeouts.
// Error messages stay English here and are translated where shown (tm(), i18n/zh/agent.ts).

export const API_BASE = import.meta.env.MODE === 'demo' ? 'http://127.0.0.1:19090' : `http://${window.location.hostname}:9090`
export const AUTH_EXPIRED_EVENT = 'zte-auth-expired'

let _token: string | null = sessionStorage.getItem('zte_token')

export function setToken(t: string) {
  _token = t
  sessionStorage.setItem('zte_token', t)
}

export function clearToken() {
  _token = null
  sessionStorage.removeItem('zte_token')
}

export function hasToken() {
  return !!_token
}

export class ApiError extends Error {
  status?: number

  constructor(message: string, status?: number) {
    super(message)
    this.name = 'ApiError' // i18n-ignore
    this.status = status
  }
}

function emitAuthExpired() {
  clearToken()
  window.dispatchEvent(new Event(AUTH_EXPIRED_EVENT))
}

export async function req(
  method: string,
  path: string,
  body?: unknown,
  extraHeaders?: Record<string, string>,
  timeoutMs = 15_000,
  base = API_BASE,
  sendToken = true,
): Promise<Record<string, unknown>> {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), timeoutMs)
  const headers: Record<string, string> = { ...(extraHeaders ?? {}) }
  if (_token && sendToken) headers['Authorization'] = `Bearer ${_token}` // i18n-ignore
  if (body !== undefined) headers['Content-Type'] = 'application/json'
  try {
    let res: Response
    try {
      res = await fetch(`${base}${path}`, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      })
    } catch (error) {
      if (error instanceof Error && error.name === 'AbortError') {
        throw new ApiError('Timed out reaching the agent') // i18n-ignore
      }
      throw new ApiError(`Failed to reach the agent at ${API_BASE}`) // i18n-ignore
    }

    let json: { ok?: boolean; data?: unknown; error?: string }
    try {
      json = await res.json()
    } catch {
      throw new ApiError(`Invalid response from agent (${res.status})`, res.status) // i18n-ignore
    }

    if (res.status === 401 && sendToken && base === API_BASE && path !== '/api/auth/login') {
      emitAuthExpired()
    }
    if (!res.ok || !json.ok) {
      throw new ApiError(json.error ?? `request failed (${res.status})`, res.status) // i18n-ignore
    }
    return (json.data ?? {}) as Record<string, unknown>
  } finally {
    clearTimeout(timeout)
  }
}

/** PUTs a file as the raw body (past the agent's 1 MiB JSON limit), reporting how much was sent. */
export function upload(
  path: string,
  file: Blob,
  onProgress?: (sent: number, total: number) => void,
  timeoutMs = 600_000,
): Promise<Record<string, unknown>> {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest()
    xhr.open('PUT', `${API_BASE}${path}`)
    xhr.timeout = timeoutMs
    if (_token) xhr.setRequestHeader('Authorization', `Bearer ${_token}`) // i18n-ignore
    xhr.setRequestHeader('Content-Type', 'application/octet-stream') // i18n-ignore
    if (onProgress) xhr.upload.onprogress = (e) => onProgress(e.loaded, e.lengthComputable ? e.total : file.size)
    xhr.ontimeout = () => reject(new ApiError('Timed out reaching the agent')) // i18n-ignore
    xhr.onerror = () => reject(new ApiError(`Failed to reach the agent at ${API_BASE}`)) // i18n-ignore
    xhr.onload = () => {
      let json: { ok?: boolean; data?: unknown; error?: string }
      try {
        json = JSON.parse(xhr.responseText)
      } catch {
        reject(new ApiError(`Invalid response from agent (${xhr.status})`, xhr.status)) // i18n-ignore
        return
      }
      if (xhr.status === 401) emitAuthExpired()
      if (xhr.status < 200 || xhr.status > 299 || !json.ok) {
        reject(new ApiError(json.error ?? `request failed (${xhr.status})`, xhr.status)) // i18n-ignore
        return
      }
      resolve((json.data ?? {}) as Record<string, unknown>)
    }
    xhr.send(file)
  })
}

export const get = (path: string) => req('GET', path)
export const post = (path: string, body?: unknown, extraHeaders?: Record<string, string>) =>
  req('POST', path, body, extraHeaders)
export const put = (path: string, body: unknown) => req('PUT', path, body)

export async function login(
  credentials: string | { password?: string; pin?: string },
): Promise<{ token: string }> {
  const body = typeof credentials === 'string' ? { password: credentials } : credentials
  const data = await req('POST', '/api/auth/login', body)
  return { token: data.token as string }
}

// Only the private IPv4 address explicitly submitted by the user may receive
// a single-use confirmation token during a LAN transition. Never transfer the
// general session token: the proposed address could already belong to another host.
export async function confirmLan(ip: string, confirmationToken: string) {
  const octets = ip.split('.').map(Number)
  if (!/^\d{1,3}(\.\d{1,3}){3}$/.test(ip) || octets.some(n => n > 255) ||
      !(octets[0] === 10 || (octets[0] === 172 && octets[1] >= 16 && octets[1] <= 31) ||
        (octets[0] === 192 && octets[1] === 168))) {
    throw new ApiError('Invalid LAN reconnect address') // i18n-ignore
  }
  return req('POST', '/api/router/lan/confirm', { token: confirmationToken }, undefined, 3000, `http://${ip}:9090`, false)
}

export async function readCsv(path: string): Promise<{ csv: string }> {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(), 30_000)
  try {
    const response = await fetch(`${API_BASE}${path}`, {
      headers: _token ? { Authorization: `Bearer ${_token}` } : {} /* i18n-ignore */, signal: controller.signal,
    })
    if (response.status === 401) emitAuthExpired()
    if (!response.ok) throw new ApiError(`CSV download failed (${response.status})`, response.status) // i18n-ignore
    // Older agents and the emulator return a JSON envelope.
    if (response.headers.get('content-type')?.includes('application/json')) {
      const body = await response.json()
      if (!body.ok || typeof body.data?.csv !== 'string') throw new ApiError('Invalid CSV response') // i18n-ignore
      return { csv: body.data.csv }
    }
    return { csv: await response.text() }
  } finally { clearTimeout(timeout) }
}
