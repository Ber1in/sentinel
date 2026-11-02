import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'

import type { ClientMessage } from '../shared/ClientMessage'
import type { ClientOp } from '../shared/ClientOp'
import type { OpResult } from '../shared/OpResult'
import type { ServerInfo } from '../shared/ServerInfo'
import type { ServerMessage } from '../shared/ServerMessage'
import { PROTOCOL_VERSION } from '../shared/protocolVersion'

interface DaemonFile {
  pid: number
  port: number
  token: string
  protocol_version: number
  daemon_version: string
  started_at: number
}

export function daemonDataDir(): string {
  if (process.env.SENTINEL_DATA_DIR) return process.env.SENTINEL_DATA_DIR
  if (process.platform === 'win32') {
    const base = process.env.LOCALAPPDATA ?? join(homedir(), 'AppData', 'Local')
    return join(base, 'sentinel')
  }
  const base = process.env.XDG_DATA_HOME ?? join(homedir(), '.local', 'share')
  return join(base, 'sentinel')
}

export class ProtocolMismatchError extends Error {
  constructor(
    readonly daemonVersion: number,
    readonly clientVersion: number
  ) {
    super(
      `daemon speaks protocol v${daemonVersion} but this client speaks v${clientVersion}`
    )
    this.name = 'ProtocolMismatchError'
  }
}

export interface OutputEvent {
  numeric: number
  seq: number
  bytes: Uint8Array
}

export interface SessionStateEvent {
  sessionId: string
  state: string
  detail: string | null
}

/** Binary frame layout must match `crates/protocol/src/messages.rs`. */
export function encodeInputFrame(numeric: number, seq: number, payload: Uint8Array): ArrayBuffer {
  const buffer = new ArrayBuffer(13 + payload.byteLength)
  const view = new DataView(buffer)
  view.setUint8(0, 2)
  view.setUint32(1, numeric, true)
  view.setBigUint64(5, BigInt(seq), true)
  new Uint8Array(buffer, 13).set(payload)
  return buffer
}

export function decodeOutputFrame(buffer: ArrayBuffer): OutputEvent | null {
  if (buffer.byteLength < 13) return null
  const view = new DataView(buffer)
  if (view.getUint8(0) !== 1) return null
  return {
    numeric: view.getUint32(1, true),
    seq: Number(view.getBigUint64(5, true)),
    bytes: new Uint8Array(buffer, 13)
  }
}

interface Pending {
  resolve: (result: OpResult) => void
  reject: (error: Error) => void
  timer: NodeJS.Timeout
}

/**
 * The only place that talks to the daemon. Lives in the main process so the renderer
 * never holds a socket and cannot outlive its own permissions.
 */
export class DaemonClient {
  info: ServerInfo
  readonly port: number
  private readonly socket: WebSocket
  private readonly pending = new Map<number, Pending>()
  private nextId = 1
  private closed = false
  private readonly onOutput: ((event: OutputEvent) => void) | null
  private readonly onEvent: ((event: SessionStateEvent) => void) | null
  private readonly numericIds = new Map<string, number>()

  private constructor(
    socket: WebSocket,
    info: ServerInfo,
    port: number,
    onOutput: ((event: OutputEvent) => void) | null,
    onEvent: ((event: SessionStateEvent) => void) | null
  ) {
    this.socket = socket
    this.info = info
    this.port = port
    this.onOutput = onOutput
    this.onEvent = onEvent
    socket.addEventListener('message', (event) => this.onMessage(event.data))
    socket.addEventListener('close', () => this.failAll(new Error('daemon connection closed')))
    socket.addEventListener('error', () => this.failAll(new Error('daemon connection error')))
  }

  static async connect(
    onOutput: ((event: OutputEvent) => void) | null = null,
    onEvent: ((event: SessionStateEvent) => void) | null = null,
    timeoutMs = 5000
  ): Promise<DaemonClient> {
    const file = JSON.parse(
      await readFile(join(daemonDataDir(), 'daemon.json'), 'utf8')
    ) as DaemonFile

    if (file.protocol_version !== PROTOCOL_VERSION) {
      throw new ProtocolMismatchError(file.protocol_version, PROTOCOL_VERSION)
    }

    const socket = new WebSocket(`ws://127.0.0.1:${file.port}`)
    // Node's WebSocket defaults to Blob; the frame decoder only understands ArrayBuffer.
    socket.binaryType = 'arraybuffer'

    const info = await new Promise<ServerInfo>((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error('daemon handshake timed out')),
        timeoutMs
      )
      socket.addEventListener('open', () => {
        const hello: ClientMessage = {
          type: 'hello',
          protocol_version: PROTOCOL_VERSION,
          token: file.token,
          client_version: 'desktop'
        }
        socket.send(JSON.stringify(hello))
      })
      socket.addEventListener('message', (event) => {
        const message = parseServerMessage(event.data)
        if (message?.type === 'hello') {
          clearTimeout(timer)
          resolve(message.info)
        }
      })
      socket.addEventListener('error', () => {
        clearTimeout(timer)
        reject(new Error('cannot reach daemon'))
      })
      socket.addEventListener('close', () => {
        clearTimeout(timer)
        reject(new Error('daemon closed the connection during handshake'))
      })
    })

    return new DaemonClient(socket, info, file.port, onOutput, onEvent)
  }

  request(op: ClientOp, timeoutMs = 10000): Promise<OpResult> {
    if (this.closed) return Promise.reject(new Error('daemon connection closed'))
    const id = this.nextId++
    const message: ClientMessage = { type: 'request', id, op }
    return new Promise<OpResult>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new Error(`daemon request timed out: ${op.op}`))
      }, timeoutMs)
      this.pending.set(id, { resolve, reject, timer })
      this.socket.send(JSON.stringify(message))
    })
  }

  registerNumeric(sessionId: string, numeric: number): void {
    this.numericIds.set(sessionId, numeric)
  }

  sendInput(sessionId: string, payload: Uint8Array): boolean {
    const numeric = this.numericIds.get(sessionId)
    if (numeric === undefined || this.closed) return false
    this.socket.send(encodeInputFrame(numeric, 0, payload))
    return true
  }

  close(): void {
    this.closed = true
    this.socket.close()
    this.failAll(new Error('daemon client closed'))
  }

  private onMessage(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      const frame = decodeOutputFrame(data)
      if (frame && this.onOutput) this.onOutput(frame)
      return
    }
    const message = parseServerMessage(data)
    if (!message) return
    if (message.type === 'reply') {
      const pending = this.pending.get(message.id)
      if (!pending) return
      this.pending.delete(message.id)
      clearTimeout(pending.timer)
      pending.resolve(message.result)
      return
    }
    if (message.type === 'event' && this.onEvent) {
      const event = message.event
      if (event.event === 'session_state') {
        this.onEvent({ sessionId: event.session_id, state: event.state, detail: event.detail })
      }
    }
  }

  private failAll(error: Error): void {
    for (const [id, pending] of this.pending) {
      clearTimeout(pending.timer)
      pending.reject(error)
      this.pending.delete(id)
    }
  }
}

function parseServerMessage(data: unknown): ServerMessage | null {
  if (typeof data !== 'string') return null
  try {
    return JSON.parse(data) as ServerMessage
  } catch {
    return null
  }
}
