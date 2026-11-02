import { app, BrowserWindow, ipcMain } from 'electron'
import { join } from 'node:path'

import { DaemonClient, ProtocolMismatchError } from './daemon-client'

let client: DaemonClient | null = null

async function getClient(): Promise<DaemonClient> {
  if (client) return client
  client = await DaemonClient.connect()
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
        console.warn(
          `protocol mismatch: daemon v${error.daemonVersion}, client v${error.clientVersion}`
        )
        return {
          connected: false,
          info: null,
          port: null,
          error: error.message,
          mismatch: { daemonVersion: error.daemonVersion, clientVersion: error.clientVersion }
        }
      }
      return {
        connected: false,
        info: null,
        port: null,
        error: (error as Error).message,
        mismatch: null
      }
    }
  })

  ipcMain.handle('daemon:sessions', async () => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'list_sessions' })
    if (result.result === 'sessions') return result.sessions
    throw new Error(`unexpected daemon result: ${JSON.stringify(result)}`)
  })

  ipcMain.handle('daemon:ping', async () => {
    const daemon = await getClient()
    return { pong: true, daemon: daemon.info, port: daemon.port }
  })
}

function createWindow(): BrowserWindow {
  const window = new BrowserWindow({
    width: 1200,
    height: 780,
    show: false,
    title: 'Sentinel',
    backgroundColor: '#111318',
    webPreferences: {
      preload: join(__dirname, '../preload/index.mjs'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: false
    }
  })

  window.once('ready-to-show', () => window.show())

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
