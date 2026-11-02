import { app, BrowserWindow, ipcMain, shell } from 'electron'
import { execFileSync, spawn } from 'node:child_process'
import { mkdirSync, openSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

import type { HostSpec } from '../shared/HostSpec'
import { DaemonClient, ProtocolMismatchError, type OutputEvent, type SessionStateEvent } from './daemon-client'

let client: DaemonClient | null = null
/**
 * In-flight connection attempt. Several IPC handlers and the session-restore effect call
 * `getClient()` at once; without this each would open its own WebSocket and the daemon
 * would broadcast every output chunk once per connection (tripled terminal output).
 */
let clientPromise: Promise<DaemonClient> | null = null
let window: BrowserWindow | null = null

function forwardOutput(event: OutputEvent): void {
  window?.webContents.send('terminal:data', event)
}

function forwardSessionState(event: SessionStateEvent): void {
  window?.webContents.send('session:state', event)
}

function daemonBinary(): string {
  if (app.isPackaged) return join(process.resourcesPath, 'daemon.exe')
  // Development: the workspace target directory next to apps/desktop.
  return join(app.getAppPath(), '..', '..', 'target', 'debug', 'daemon')
}

/** Daemon output goes to a file so connection problems are diagnosable. */
function daemonLogTarget(): number | 'ignore' {
  const base = process.env.LOCALAPPDATA
  if (!base) return 'ignore'
  try {
    const dir = join(base, 'sentinel')
    mkdirSync(dir, { recursive: true })
    return openSync(join(dir, 'daemon.log'), 'a')
  } catch {
    return 'ignore'
  }
}

function spawnDaemon(): void {
  const binary = daemonBinary()
  const target = daemonLogTarget()
  // --supervise keeps the daemon alive across crashes, even after the GUI exits.
  const child = spawn(binary, ['--supervise'], {
    detached: true,
    windowsHide: true,
    stdio: ['ignore', target, target]
  })
  child.on('error', (error) => console.warn(`cannot start daemon: ${error.message}`))
  child.unref()
}

const wait = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))

function dataDir(): string {
  const base = process.env.LOCALAPPDATA ?? process.env.XDG_DATA_HOME ?? ''
  return join(base, 'sentinel')
}

function readPid(file: string): number | null {
  try {
    const parsed = JSON.parse(readFileSync(join(dataDir(), file), 'utf8')) as { pid?: number }
    return typeof parsed.pid === 'number' ? parsed.pid : null
  } catch {
    return null
  }
}

/** Guard against killing an unrelated process after a pid was reused. */
function looksLikeOurs(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false
  try {
    if (process.platform === 'win32') {
      const output = execFileSync(
        'tasklist',
        ['/FI', `PID eq ${pid}`, '/FO', 'CSV', '/NH'],
        { encoding: 'utf8' }
      ).toLowerCase()
      return output.includes('daemon.exe')
    }
    return readFileSync(`/proc/${pid}/comm`, 'utf8').toLowerCase().includes('daemon')
  } catch {
    return false
  }
}

function killPid(pid: number): void {
  if (!looksLikeOurs(pid)) return
  try {
    process.kill(pid)
  } catch {
    // Already gone.
  }
}

/**
 * Stop the supervisor first (otherwise it immediately restarts the old daemon) and then
 * the daemon. Needed whenever the bundled daemon is newer than the running one.
 */
async function stopDaemon(): Promise<void> {
  killPid(readPid('supervisor.json') ?? -1)
  await wait(200)
  killPid(readPid('daemon.json') ?? -1)
  await wait(400)
}

function daemonLogTail(): string {
  try {
    const text = readFileSync(join(dataDir(), 'daemon.log'), 'utf8').split('\n').slice(-8).join('\n')
    return text.trim()
  } catch {
    return ''
  }
}

async function connectOrStart(): Promise<DaemonClient> {
  let lastError: Error | null = null
  try {
    return await DaemonClient.connect(forwardOutput, forwardSessionState)
  } catch (error) {
    lastError = error as Error
  }

  // A stale daemon (older protocol or a stuck supervisor) is the common cause, so stop
  // whatever is running before spawning the bundled binary.
  await stopDaemon()
  spawnDaemon()

  for (let attempt = 0; attempt < 30; attempt += 1) {
    await wait(300)
    try {
      return await DaemonClient.connect(forwardOutput, forwardSessionState)
    } catch (error) {
      lastError = error as Error
    }
  }

  const tail = daemonLogTail()
  throw new Error(
    `daemon did not start: ${lastError?.message ?? 'unknown error'}${tail ? `\n\n${tail}` : ''}`
  )
}

async function getClient(): Promise<DaemonClient> {
  if (client) return client
  if (!clientPromise) {
    clientPromise = connectOrStart()
      .then((daemon) => {
        if (client && client !== daemon) client.close()
        client = daemon
        console.log(
          `connected to daemon pid=${daemon.info.pid} protocol=${daemon.info.protocol_version}`
        )
        return daemon
      })
      .finally(() => {
        clientPromise = null
      })
  }
  return clientPromise
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

  ipcMain.handle('daemon:restart', async () => {
    client?.close()
    client = null
    clientPromise = null
    await stopDaemon()
    spawnDaemon()
    return getClient()
      .then(() => ({ ok: true, error: null }))
      .catch((error: Error) => ({ ok: false, error: error.message }))
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

  ipcMain.handle('secret:delete', async (_event, id: string) => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'delete_secret', id })
    if (result.result === 'error') throw new Error(result.error.message)
  })

  ipcMain.handle('host:update', async (_event, host: HostSpec, name: string, groupName: string | null) => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'update_host', host, name, group_name: groupName })
    if (result.result === 'error') throw new Error(result.error.message)
  })

  ipcMain.handle('host:duplicate', async (_event, hostId: string, newId: string) => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'duplicate_host', host_id: hostId, new_id: newId })
    if (result.result === 'error') throw new Error(result.error.message)
  })

  ipcMain.handle('host:delete', async (_event, hostId: string) => {
    const daemon = await getClient()
    const result = await daemon.request({ op: 'delete_host', host_id: hostId })
    if (result.result === 'error') throw new Error(result.error.message)
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

  ipcMain.handle('session:resync', async (_event, sessionId: string) => {
    const daemon = await getClient()
    await daemon.request({ op: 'resync', session_id: sessionId, from_seq: BigInt(0) })
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

  window.webContents.setWindowOpenHandler(({ url }) => {
    void shell.openExternal(url)
    return { action: 'deny' }
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
