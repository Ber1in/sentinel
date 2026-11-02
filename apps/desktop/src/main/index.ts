import { app, BrowserWindow, ipcMain } from 'electron'
import { spawn } from 'node:child_process'
import { join } from 'node:path'

import type { HostSpec } from '../shared/HostSpec'
import { DaemonClient, ProtocolMismatchError, type OutputEvent } from './daemon-client'

let client: DaemonClient | null = null
let window: BrowserWindow | null = null

function forwardOutput(event: OutputEvent): void {
  window?.webContents.send('terminal:data', event)
}

function daemonBinary(): string {
  if (app.isPackaged) return join(process.resourcesPath, 'daemon.exe')
  // Development: the workspace target directory next to apps/desktop.
  return join(app.getAppPath(), '..', '..', 'target', 'debug', 'daemon')
}

function spawnDaemon(): void {
  const binary = daemonBinary()
  const child = spawn(binary, [], { detached: true, stdio: 'ignore' })
  child.on('error', (error) => console.warn(`cannot start daemon: ${error.message}`))
  child.unref()
}

const wait = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))

async function connectOrStart(): Promise<DaemonClient> {
  try {
    return await DaemonClient.connect(forwardOutput)
  } catch {
    spawnDaemon()
    // The daemon writes daemon.json before it accepts clients; poll briefly.
    for (let attempt = 0; attempt < 20; attempt += 1) {
      await wait(250)
      try {
        return await DaemonClient.connect(forwardOutput)
      } catch {
        // keep waiting
      }
    }
    throw new Error('daemon did not start')
  }
}

async function getClient(): Promise<DaemonClient> {
  if (client) return client
  client = await connectOrStart()
  console.log(`connected to daemon pid=${client.info.pid} protocol=${client.info.protocol_version}`)
  return client
}

function registerIpc(): void {
  ipcMain.handle('daemon:status', async () => {
    try {
      const daemon = await getClient()
      return { connected: true, info: daemon.info, port: daemon.port, error: null, mismatch: null }
    } catch (error) {
      client = null
      if (error instanceof ProtocolMismatchError) {
        console.warn(`protocol mismatch: daemon v${error.daemonVersion}, client v${error.clientVersion}`)
        return {
          connected: false,
          info: null,
          port: null,
          error: error.message,
          mismatch: { daemonVersion: error.daemonVersion, clientVersion: error.clientVersion }
        }
      }
      return { connected: false, info: null, port: null, error: (error as Error).message, mismatch: null }
    }
  })

  ipcMain.handle('hosts:list', async () => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'list_hosts' })
    return result.result === 'hosts' ? result.hosts : []
  })

  ipcMain.handle('sessions:list', async () => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'list_sessions' })
    return result.result === 'sessions' ? result.sessions : []
  })

  ipcMain.handle('session:create', async (_event, host: HostSpec, paneId: string, cwd: string | null) => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'create_session', host, pane_id: paneId, cwd })
    if (result.result !== 'session') {
      throw new Error(result.result === 'error' ? result.error.message : 'unexpected daemon reply')
    }
    daemon.registerNumeric(result.session_id, result.numeric)
    return result
  })

  ipcMain.handle('secret:set', async (_event, id: string, secret: string) => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'set_secret', id, secret })
    if (result.result === 'error') throw new Error(result.error.message)
  })

  ipcMain.handle('session:input', async (_event, sessionId: string, payload: Uint8Array) => {
    const daemon = await getClient()
    return daemon.sendInput(sessionId, payload)
  })

  ipcMain.handle('session:resize', async (_event, sessionId: string, cols: number, rows: number) => {
    const daemon = await getClient()
    await daemon.request({ op: 'resize', session_id: sessionId, cols, rows })
  })

  ipcMain.handle('session:detach', async (_event, sessionId: string) => {
    const daemon = await getClient()
    await daemon.request({ op: 'detach', session_id: sessionId })
  })

  ipcMain.handle('session:destroy', async (_event, sessionId: string) => {
    const daemon = await getClient()
    await daemon.request({ op: 'destroy', session_id: sessionId })
  })
}

function createWindow(): BrowserWindow {
  window = new BrowserWindow({
    width: 1360,
    height: 860,
    minWidth: 960,
    minHeight: 600,
    show: false,
    title: 'Sentinel',
    backgroundColor: '#0b0d12',
    titleBarStyle: 'hidden',
    titleBarOverlay: { color: '#0b0d12', symbolColor: '#e6e8ee', height: 40 },
    webPreferences: {
      preload: join(__dirname, '../preload/index.mjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  })

  window.once('ready-to-show', () => window?.show())
  window.on('closed', () => {
    window = null
  })

  const devUrl = process.env['ELECTRON_RENDERER_URL']
  if (devUrl) {
    void window.loadURL(devUrl)
  } else {
    void window.loadFile(join(__dirname, '../renderer/index.html'))
  }

  return window
}

void app.whenReady().then(() => {
  registerIpc()
  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})

app.on('before-quit', () => {
  client?.close()
})
