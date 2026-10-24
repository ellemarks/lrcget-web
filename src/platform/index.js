// Platform layer: the same frontend runs inside the Tauri desktop app and in a regular browser
// talking to `lrcget-server`. Components import backend access from here instead of from
// `@tauri-apps/*` directly.

import { invoke as tauriInvoke } from '@tauri-apps/api/core'
import { listen as tauriListen } from '@tauri-apps/api/event'
import { getVersion as tauriGetVersion } from '@tauri-apps/api/app'
import { audioDir } from '@tauri-apps/api/path'
import { open as tauriOpen } from '@tauri-apps/plugin-dialog'
import { readText as tauriReadText } from '@tauri-apps/plugin-clipboard-manager'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { pickServerPath } from './path-picker.js'

export const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

const apiUrl = path => new URL(`api/${path}`, document.baseURI).toString()

/**
 * Calls a backend command. Rejects with the command's error value, like Tauri's `invoke`.
 */
export async function invoke(command, args = {}) {
  if (isTauri) {
    return tauriInvoke(command, args)
  }

  const response = await fetch(apiUrl(`invoke/${command}`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(args ?? {}),
  })
  const text = await response.text()
  let body = null
  if (text) {
    try {
      body = JSON.parse(text)
    } catch {
      body = text
    }
  }
  if (!response.ok) {
    throw body ?? `${response.status} ${response.statusText}`
  }
  return body
}

// Browser mode: one WebSocket shared by all listeners, reconnecting when the server restarts.
const eventHandlers = new Map()
let eventSocket = null
let reconnectDelay = 1000

function connectEvents() {
  const url = new URL(apiUrl('events'))
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  eventSocket = new WebSocket(url)
  eventSocket.onopen = () => {
    reconnectDelay = 1000
  }
  eventSocket.onmessage = message => {
    const { event, payload } = JSON.parse(message.data)
    eventHandlers.get(event)?.forEach(handler => handler({ event, payload }))
  }
  eventSocket.onclose = () => {
    eventSocket = null
    setTimeout(connectEvents, reconnectDelay)
    reconnectDelay = Math.min(reconnectDelay * 2, 30000)
  }
}

/**
 * Subscribes to a backend event. Resolves to an unlisten function, like Tauri's `listen`.
 */
export async function listen(event, handler) {
  if (isTauri) {
    return tauriListen(event, handler)
  }

  if (!eventSocket) {
    connectEvents()
  }
  if (!eventHandlers.has(event)) {
    eventHandlers.set(event, new Set())
  }
  eventHandlers.get(event).add(handler)
  return () => eventHandlers.get(event)?.delete(handler)
}

let serverInfo = null

/** `{ version, root, musicDir }` from the web server. */
export async function getServerInfo() {
  if (!serverInfo) {
    serverInfo = fetch(apiUrl('info')).then(response => response.json())
  }
  return serverInfo
}

export async function getVersion() {
  return isTauri ? tauriGetVersion() : (await getServerInfo()).version
}

/** Folder suggested when setting up a library, or null. */
export async function getDefaultMusicDir() {
  return isTauri ? audioDir() : (await getServerInfo()).musicDir
}

/**
 * Asks for a file or folder, like `@tauri-apps/plugin-dialog`'s `open` (single selection).
 * In the browser this browses the server's filesystem, since that is where the library lives.
 */
export async function open(options = {}) {
  return isTauri ? tauriOpen(options) : pickServerPath(options)
}

/** Lets the user pick a file on their own device and resolves to its text, or null. */
export function pickLocalTextFile(accept) {
  return new Promise((resolve, reject) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.accept = accept
    input.addEventListener('change', () => {
      const file = input.files?.[0]
      if (file) {
        file.text().then(resolve, reject)
      } else {
        resolve(null)
      }
    })
    input.addEventListener('cancel', () => resolve(null))
    input.click()
  })
}

export async function readClipboardText() {
  if (isTauri) {
    return tauriReadText()
  }
  if (!navigator.clipboard?.readText) {
    throw 'The browser only allows reading the clipboard over HTTPS or on localhost'
  }
  return navigator.clipboard.readText()
}

/** URL the browser player streams a track from (library track or file path on the server). */
export function audioUrl(track, { transcode = false } = {}) {
  const params = new URLSearchParams()
  if (track.id !== undefined && track.id !== null) {
    params.set('trackId', track.id)
  } else {
    params.set('path', track.file_path)
  }
  if (transcode) {
    params.set('transcode', 'true')
  }
  return apiUrl(`audio?${params}`)
}

/** Resolves to 'dark' or 'light'. */
export async function getSystemTheme() {
  if (isTauri) {
    return getCurrentWebviewWindow().theme()
  }
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

export function onSystemThemeChanged(handler) {
  if (isTauri) {
    getCurrentWebviewWindow().onThemeChanged(({ payload }) => handler(payload))
    return
  }
  window
    .matchMedia('(prefers-color-scheme: dark)')
    .addEventListener('change', event => handler(event.matches ? 'dark' : 'light'))
}
