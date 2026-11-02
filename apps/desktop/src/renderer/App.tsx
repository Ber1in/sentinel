import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { ChevronsRight, Command, Eye, EyeOff, Moon, Pencil, Plus, Server, Settings, Sun, Terminal as TerminalIcon, Trash2, X } from 'lucide-react'
import type { HostSummary } from '../shared/HostSummary'
import type { SessionState } from '../shared/SessionState'
import type { ProviderKind } from '../shared/ProviderKind'
import type { Terminal } from '@xterm/xterm'
import { TerminalPane } from './TerminalPane'
import { detectLanguage, translator, type Language } from './i18n'

type Theme = 'dark' | 'light'

interface Tab {
  id: string
  sessionId: string
  numeric: number
  hostId: string
  title: string
  state: SessionState
  provider: ProviderKind
}

interface Toast {
  id: number
  tone: 'error' | 'info'
  message: string
}

const AUTH_OPTIONS = ['password', 'key', 'agent'] as const
type AuthOption = (typeof AUTH_OPTIONS)[number]

export function App() {
  const [language, setLanguage] = useState<Language>(() => detectLanguage())
  const [theme, setTheme] = useState<Theme>(
    () => (localStorage.getItem('sentinel.theme') as Theme | null) ?? 'dark'
  )
  const [hosts, setHosts] = useState<HostSummary[]>([])
  const [tabs, setTabs] = useState<Tab[]>([])
  const [activeTab, setActiveTab] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [editing, setEditing] = useState<{ host: HostSummary; isNew: boolean } | null>(null)
  const [toasts, setToasts] = useState<Toast[]>([])
  const terminals = useRef(new Map<number, Terminal>())
  const t = useMemo(() => translator(language), [language])

  const pushToast = useCallback((message: string, tone: Toast['tone'] = 'error') => {
    const id = Date.now() + Math.random()
    setToasts((current) => [...current, { id, tone, message }])
    setTimeout(() => setToasts((current) => current.filter((toast) => toast.id !== id)), 6000)
  }, [])

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    localStorage.setItem('sentinel.theme', theme)
  }, [theme])

  useEffect(() => {
    localStorage.setItem('sentinel.language', language)
  }, [language])

  const refreshHosts = useCallback(async () => {
    try {
      setHosts(await window.desktop.listHosts())
    } catch {
      // Daemon may not be running yet; the status card covers that.
    }
  }, [])

  useEffect(() => {
    void refreshHosts()
  }, [refreshHosts])

  // Reattach sessions the daemon still owns (GUI restart, crash, upgrade).
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const sessions = await window.desktop.listSessions()
        if (cancelled || sessions.length === 0) return
        setTabs((current) => {
          const known = new Set(current.map((tab) => tab.sessionId))
          const restored = sessions
            .filter((session) => !known.has(session.session_id))
            .map((session) => ({
              id: session.session_id,
              sessionId: session.session_id,
              numeric: session.numeric,
              hostId: session.host_id,
              title: session.title,
              state: session.state,
              provider: session.provider
            }))
          return [...current, ...restored]
        })
      } catch {
        // daemon not up yet; the status card explains it
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  const register = useCallback((numeric: number, terminal: Terminal | null) => {
    if (terminal) terminals.current.set(numeric, terminal)
    else terminals.current.delete(numeric)
  }, [])

  useEffect(() => {
    return window.desktop.onTerminalData((data) => {
      terminals.current.get(data.numeric)?.write(data.bytes)
    })
  }, [])

  useEffect(() => {
    return window.desktop.onSessionState((data) => {
      setTabs((current) =>
        current.map((tab) =>
          tab.sessionId === data.sessionId ? { ...tab, state: data.state as SessionState } : tab
        )
      )
    })
  }, [])

  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault()
        setPaletteOpen((open) => !open)
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 't') {
        event.preventDefault()
        setDialogOpen(true)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const openSession = useCallback(
    async (host: HostSummary) => {
      setPaletteOpen(false)
      const hostId = host.id
      try {
        const reply = await window.desktop.createSession(
          {
            host_id: hostId,
            hostname: host.hostname,
            port: host.port,
            username: host.username,
            auth: host.auth
          },
          'main',
          null
        )
        const tab: Tab = {
          id: reply.session_id,
          sessionId: reply.session_id,
          numeric: reply.numeric,
          hostId,
          title: `${host.username}@${host.hostname}`,
          state: 'connecting',
          provider: reply.provider
        }
        setTabs((current) => [...current, tab])
        setActiveTab(tab.id)
      } catch (error) {
        pushToast((error as Error).message)
      }
    },
    [pushToast]
  )

  const closeTab = useCallback(async (tab: Tab) => {
    await window.desktop.detach(tab.sessionId)
    setTabs((current) => current.filter((item) => item.id !== tab.id))
    setActiveTab((current) => (current === tab.id ? null : current))
  }, [])

  return (
    <div className="flex h-full flex-col overflow-hidden bg-canvas text-ink">
      <header className="drag-region flex h-10 shrink-0 items-center justify-between border-b border-line px-3">
        <div className="flex items-center gap-2 text-[13px] font-medium tracking-tight">
          <span className="grid h-5 w-5 place-items-center rounded-md bg-accent text-[11px] font-bold text-canvas">
            S
          </span>
          Sentinel
          <span className="text-muted">·</span>
          <span className="text-muted">{t('app.tagline')}</span>
        </div>
        <div className="no-drag flex items-center gap-1">
          <IconButton label={t('settings.title')} onClick={() => setSettingsOpen(true)}>
            <Settings size={15} />
          </IconButton>
          <IconButton label={theme === 'dark' ? t('settings.theme.light') : t('settings.theme.dark')} onClick={() => setTheme(theme === 'dark' ? 'light' : 'dark')}>
            {theme === 'dark' ? <Sun size={15} /> : <Moon size={15} />}
          </IconButton>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="flex w-[268px] shrink-0 flex-col border-r border-line bg-surface">
          <div className="flex items-center justify-between px-3 pb-2 pt-3">
            <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">{t('hosts.title')}</span>
            <button
              className="grid h-6 w-6 place-items-center rounded-md text-muted transition hover:bg-elevated hover:text-ink"
              onClick={() => setDialogOpen(true)}
              title={t('action.new')}
            >
              <Plus size={14} />
            </button>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
            {hosts.length === 0 ? (
              <p className="px-2 py-3 text-[12px] leading-relaxed text-muted">{t('hosts.empty')}</p>
            ) : (
              hosts.map((host) => (
                <div
                  key={host.id}
                  className="group mb-1 flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left transition hover:bg-elevated"
                >
                  <button onClick={() => void openSession(host)} className="flex min-w-0 flex-1 items-center gap-2.5 text-left">
                    <span className="grid h-7 w-7 shrink-0 place-items-center rounded-md bg-elevated text-muted group-hover:text-accent">
                      <Server size={14} />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13px]">{host.name}</span>
                      <span className="block truncate text-[11px] text-muted">
                        {host.username}@{host.hostname}:{host.port}
                      </span>
                    </span>
                  </button>
                  <button
                    className="grid h-6 w-6 place-items-center rounded text-muted opacity-0 transition hover:bg-canvas hover:text-ink group-hover:opacity-100"
                    title={t('action.edit')}
                    onClick={() => setEditing({ host, isNew: false })}
                  >
                    <Pencil size={12} />
                  </button>
                  <ChevronsRight size={13} className="text-muted opacity-0 transition group-hover:opacity-100" />
                </div>
              ))
            )}
          </div>
          <div className="border-t border-line px-3 py-2 text-[11px] text-muted">
            <div className="flex items-center gap-1.5">
              <Command size={12} />
              Ctrl+K
            </div>
          </div>
        </aside>

        <main className="flex min-w-0 flex-1 flex-col bg-canvas">
          <div className="flex h-9 shrink-0 items-stretch border-b border-line">
            {tabs.map((tab) => (
              <div
                key={tab.id}
                onClick={() => setActiveTab(tab.id)}
                className={`group flex cursor-pointer items-center gap-2 border-r border-line px-3 text-[12px] transition ${
                  activeTab === tab.id ? 'bg-canvas text-ink' : 'bg-surface text-muted hover:text-ink'
                }`}
                style={activeTab === tab.id ? { boxShadow: 'inset 0 -2px 0 var(--accent)' } : undefined}
              >
                <span className="status-dot" style={{ background: stateColor(tab.state) }} />
                <span className="max-w-[220px] truncate">{tab.title}</span>
                <button
                  className="grid h-4 w-4 place-items-center rounded text-muted opacity-0 transition hover:bg-elevated hover:text-ink group-hover:opacity-100"
                  onClick={(event) => {
                    event.stopPropagation()
                    void closeTab(tab)
                  }}
                >
                  <X size={11} />
                </button>
              </div>
            ))}
            <button
              className="grid w-9 place-items-center text-muted transition hover:text-ink"
              onClick={() => setDialogOpen(true)}
              title={t('action.new')}
            >
              <Plus size={14} />
            </button>
          </div>

          <div className="relative min-h-0 flex-1">
            {tabs.length === 0 ? (
              <EmptyState title={t('empty.title')} body={t('empty.body')} onCreate={() => setDialogOpen(true)} />
            ) : (
              tabs.map((tab) => (
                <div key={tab.id} className="absolute inset-0">
                  <TerminalPane
                    sessionId={tab.sessionId}
                    numeric={tab.numeric}
                    active={activeTab === tab.id}
                    onRegister={register}
                    onReady={() => void window.desktop.resync(tab.sessionId)}
                  />
                </div>
              ))
            )}
          </div>

          <StatusBar tabs={tabs} activeTab={activeTab} label={t} />
        </main>
      </div>

      <QuickConnect
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        t={t}
        hosts={hosts}
        onDone={(tab) => {
          setTabs((current) => [...current, tab])
          setActiveTab(tab.id)
          void refreshHosts()
        }}
        onError={(message) => pushToast(message)}
      />

      <CommandPalette
        open={paletteOpen}
        onOpenChange={setPaletteOpen}
        hosts={hosts}
        t={t}
        onPick={(host) => void openSession(host)}
        onNew={() => {
          setPaletteOpen(false)
          setDialogOpen(true)
        }}
      />

      <HostDialog
        state={editing}
        onClose={() => setEditing(null)}
        t={t}
        onSaved={() => {
          setEditing(null)
          void refreshHosts()
        }}
        onError={(message) => pushToast(message)}
      />

      <SettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        language={language}
        setLanguage={setLanguage}
        theme={theme}
        setTheme={setTheme}
        t={t}
      />

      <div className="pointer-events-none fixed bottom-4 right-4 flex w-[320px] flex-col gap-2">
        {toasts.map((toast) => (
          <div
            key={toast.id}
            className="pointer-events-auto rounded-lg border border-line bg-elevated px-3 py-2 text-[12px] shadow-lg"
            style={{ borderLeft: `3px solid ${toast.tone === 'error' ? 'var(--danger)' : 'var(--accent)'}` }}
          >
            {toast.message}
          </div>
        ))}
      </div>
    </div>
  )
}

function stateColor(state: SessionState): string {
  switch (state) {
    case 'connected':
      return 'var(--success)'
    case 'reconnecting':
    case 'connecting':
      return 'var(--warning)'
    case 'failed':
      return 'var(--danger)'
    default:
      return 'var(--muted)'
  }
}

function IconButton({ label, onClick, children }: { label: string; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      title={label}
      onClick={onClick}
      className="grid h-7 w-7 place-items-center rounded-md text-muted transition hover:bg-elevated hover:text-ink"
    >
      {children}
    </button>
  )
}

function EmptyState({ title, body, onCreate }: { title: string; body: string; onCreate: () => void }) {
  return (
    <div className="grid h-full place-items-center">
      <div className="max-w-sm text-center">
        <div className="mx-auto mb-4 grid h-12 w-12 place-items-center rounded-xl bg-elevated text-muted">
          <TerminalIcon size={20} />
        </div>
        <h2 className="mb-1 text-[15px] font-medium">{title}</h2>
        <p className="mb-4 text-[12px] leading-relaxed text-muted">{body}</p>
        <button
          onClick={onCreate}
          className="rounded-lg bg-accent px-3 py-1.5 text-[12px] font-medium text-canvas transition hover:opacity-90"
        >
          New connection
        </button>
      </div>
    </div>
  )
}

function StatusBar({ tabs, activeTab, label }: { tabs: Tab[]; activeTab: string | null; label: (key: string) => string }) {
  const active = tabs.find((tab) => tab.id === activeTab)
  return (
    <footer className="flex h-6 shrink-0 items-center justify-between border-t border-line px-3 text-[11px] text-muted">
      <span>{active ? `${label(`state.${active.state}`)} · ${active.provider === 'tmux' ? 'tmux' : 'no persistence'}` : '—'}</span>
      <span>{active ? active.title : ''}</span>
    </footer>
  )
}

function QuickConnect(props: {
  open: boolean
  onOpenChange: (open: boolean) => void
  t: (key: string) => string
  hosts: HostSummary[]
  onDone: (tab: Tab) => void
  onError: (message: string) => void
}) {
  const { open, onOpenChange, t, hosts, onDone, onError } = props
  const [host, setHost] = useState('')
  const [port, setPort] = useState('22')
  const [user, setUser] = useState('root')
  const [auth, setAuth] = useState<AuthOption>('password')
  const [password, setPassword] = useState('')
  const [keyPath, setKeyPath] = useState('')
  const [busy, setBusy] = useState(false)

  const submit = async (event: React.FormEvent): Promise<void> => {
    event.preventDefault()
    setBusy(true)
    try {
      const existing = hosts.find((item) => item.hostname === host && item.username === user)
      const hostId = existing?.id ?? crypto.randomUUID()
      const credentialId = `${hostId}:password`
      if (auth === 'password') {
        if (password) {
          await window.desktop.setSecret(credentialId, password)
        }
      }
      const reply = await window.desktop.createSession(
        {
          host_id: hostId,
          hostname: host,
          port: Number(port) || 22,
          username: user,
          auth:
            auth === 'password'
              ? { method: 'password', credential_id: credentialId }
              : auth === 'key'
                ? { method: 'key', key_path: keyPath, credential_id: null }
                : { method: 'agent' }
        },
        'main',
        null
      )
      onDone({
        id: reply.session_id,
        sessionId: reply.session_id,
        numeric: reply.numeric,
        hostId,
        title: `${user}@${host}`,
        state: 'connecting',
        provider: reply.provider
      })
      onOpenChange(false)
      setPassword('')
    } catch (error) {
      onError((error as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content className="fixed left-1/2 top-1/2 w-[420px] -translate-x-1/2 -translate-y-1/2 rounded-xl border border-line bg-surface p-5 shadow-2xl">
          <Dialog.Title className="mb-4 text-[14px] font-medium">{t('connect.title')}</Dialog.Title>
          <form className="space-y-3" onSubmit={submit}>
            <Field label={t('connect.host')}>
              <input required autoFocus value={host} onChange={(event) => setHost(event.target.value)} className="input" placeholder="10.0.0.5" />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label={t('connect.port')}>
                <input value={port} onChange={(event) => setPort(event.target.value)} className="input" />
              </Field>
              <Field label={t('connect.user')}>
                <input required value={user} onChange={(event) => setUser(event.target.value)} className="input" />
              </Field>
            </div>
            <Field label={t('connect.auth')}>
              <select value={auth} onChange={(event) => setAuth(event.target.value as AuthOption)} className="input">
                <option value="password">{t('connect.auth.password')}</option>
                <option value="key">{t('connect.auth.key')}</option>
                <option value="agent">{t('connect.auth.agent')}</option>
              </select>
            </Field>
            {auth === 'password' ? (
              <Field label={t('connect.password')}>
                <input type="password" value={password} onChange={(event) => setPassword(event.target.value)} className="input" />
              </Field>
            ) : null}
            {auth === 'key' ? (
              <Field label={t('connect.key')}>
                <input value={keyPath} onChange={(event) => setKeyPath(event.target.value)} className="input" placeholder="/root/.ssh/id_ed25519" />
              </Field>
            ) : null}
            <div className="flex justify-end gap-2 pt-2">
              <Dialog.Close asChild>
                <button type="button" className="rounded-lg px-3 py-1.5 text-[12px] text-muted transition hover:text-ink">
                  {t('action.cancel')}
                </button>
              </Dialog.Close>
              <button
                type="submit"
                disabled={busy}
                className="rounded-lg bg-accent px-3 py-1.5 text-[12px] font-medium text-canvas transition hover:opacity-90 disabled:opacity-50"
              >
                {t('connect.submit')}
              </button>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 block text-[11px] uppercase tracking-wide text-muted">{label}</span>
      {children}
    </label>
  )
}

function CommandPalette(props: {
  open: boolean
  onOpenChange: (open: boolean) => void
  hosts: HostSummary[]
  t: (key: string) => string
  onPick: (host: HostSummary) => void
  onNew: () => void
}) {
  const { open, onOpenChange, hosts, t, onPick, onNew } = props
  const [query, setQuery] = useState('')
  const filtered = hosts.filter((host) => `${host.name} ${host.hostname}`.toLowerCase().includes(query.toLowerCase()))

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/40 backdrop-blur-sm" />
        <Dialog.Content className="fixed left-1/2 top-[18%] w-[520px] -translate-x-1/2 rounded-xl border border-line bg-surface shadow-2xl">
          <Dialog.Title className="sr-only">Command palette</Dialog.Title>
          <input
            autoFocus
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder={t('palette.placeholder')}
            className="w-full rounded-t-xl border-b border-line bg-transparent px-4 py-3 text-[13px] outline-none"
          />
          <div className="max-h-[320px] overflow-y-auto p-2">
            {filtered.map((host) => (
              <button
                key={host.id}
                onClick={() => onPick(host)}
                className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left text-[13px] transition hover:bg-elevated"
              >
                <Server size={14} className="text-muted" />
                <span className="flex-1 truncate">{host.name}</span>
                <span className="text-[11px] text-muted">{host.hostname}</span>
              </button>
            ))}
            <button
              onClick={onNew}
              className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left text-[13px] transition hover:bg-elevated"
            >
              <Plus size={14} className="text-muted" />
              {t('action.new')}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function SettingsDialog(props: {
  open: boolean
  onOpenChange: (open: boolean) => void
  language: Language
  setLanguage: (language: Language) => void
  theme: Theme
  setTheme: (theme: Theme) => void
  t: (key: string) => string
}) {
  const { open, onOpenChange, language, setLanguage, theme, setTheme, t } = props
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/40 backdrop-blur-sm" />
        <Dialog.Content className="fixed left-1/2 top-1/2 w-[360px] -translate-x-1/2 -translate-y-1/2 rounded-xl border border-line bg-surface p-5 shadow-2xl">
          <Dialog.Title className="mb-4 text-[14px] font-medium">{t('settings.title')}</Dialog.Title>
          <div className="space-y-4">
            <Field label={t('settings.language')}>
              <select value={language} onChange={(event) => setLanguage(event.target.value as Language)} className="input">
                <option value="en">English</option>
                <option value="zh-CN">简体中文</option>
              </select>
            </Field>
            <Field label={t('settings.theme')}>
              <select value={theme} onChange={(event) => setTheme(event.target.value as Theme)} className="input">
                <option value="dark">{t('settings.theme.dark')}</option>
                <option value="light">{t('settings.theme.light')}</option>
              </select>
            </Field>
          </div>
          <div className="mt-4 flex justify-end">
            <Dialog.Close asChild>
              <button className="rounded-lg bg-accent px-3 py-1.5 text-[12px] font-medium text-canvas">{t('action.close')}</button>
            </Dialog.Close>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

declare global {
  interface Window {
    desktop: import('../preload/index').DesktopApi
  }
}

type HostDialogState = { host: HostSummary; isNew: boolean }

interface HostDialogProps {
  state: HostDialogState | null
  onClose: () => void
  t: (key: string) => string
  onSaved: () => void
  onError: (message: string) => void
}

/**
 * Mount the inner dialog only when there is something to edit. Keeping the hooks out of
 * the wrapper avoids rendering a half-initialised Radix tree.
 */
function HostDialog(props: HostDialogProps) {
  if (!props.state) return null
  return <HostDialogInner {...props} state={props.state} />
}

function HostDialogInner(props: HostDialogProps & { state: HostDialogState }) {
  const { state, onClose, t, onSaved, onError } = props
  const [name, setName] = useState('')
  const [hostname, setHostname] = useState('')
  const [port, setPort] = useState('22')
  const [username, setUsername] = useState('')
  const [group, setGroup] = useState('')
  const [method, setMethod] = useState<'password' | 'key' | 'agent'>('password')
  const [keyPath, setKeyPath] = useState('')
  const [password, setPassword] = useState('')
  const [reveal, setReveal] = useState(false)
  const [busy, setBusy] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState(false)

  useEffect(() => {
    if (!state) return
    const host = state.host
    setName(host.name)
    setHostname(host.hostname)
    setPort(String(host.port))
    setUsername(host.username)
    setGroup(host.group_name ?? '')
    // A daemon from an older build may not send `auth`; fall back to agent.
    const auth = host.auth ?? { method: 'agent' as const }
    setMethod(auth.method === 'keyboard_interactive' ? 'password' : auth.method)
    setKeyPath(auth.method === 'key' ? auth.key_path : '')
    setPassword('')
    setReveal(false)
    setConfirmDelete(false)
  }, [state])

  const host = state.host

  const save = async (event: React.FormEvent): Promise<void> => {
    event.preventDefault()
    setBusy(true)
    try {
      const hostId = host.id
      const credentialId = `${hostId}:password`
      const previousAuth = host.auth ?? { method: 'agent' as const }
      const previousCredential =
        previousAuth.method === 'password'
          ? previousAuth.credential_id
          : previousAuth.method === 'key'
            ? previousAuth.credential_id
            : null

      if (method === 'password' && password) {
        await window.desktop.setSecret(credentialId, password)
      }
      if (previousCredential && method !== 'password') {
        await window.desktop.deleteSecret(previousCredential)
      }

      await window.desktop.updateHost(
        {
          host_id: hostId,
          hostname: hostname.trim(),
          port: Number(port) || 22,
          username: username.trim(),
          auth:
            method === 'password'
              ? { method: 'password', credential_id: credentialId }
              : method === 'key'
                ? { method: 'key', key_path: keyPath.trim(), credential_id: null }
                : { method: 'agent' }
        },
        name,
        group.trim() ? group.trim() : null
      )
      onSaved()
    } catch (error) {
      onError((error as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const remove = async (): Promise<void> => {
    if (!confirmDelete) {
      setConfirmDelete(true)
      return
    }
    setBusy(true)
    try {
      await window.desktop.deleteHost(host.id)
      onSaved()
    } catch (error) {
      onError((error as Error).message)
    } finally {
      setBusy(false)
    }
  }

  const duplicate = async (): Promise<void> => {
    setBusy(true)
    try {
      await window.desktop.duplicateHost(host.id, crypto.randomUUID())
      onSaved()
    } catch (error) {
      onError((error as Error).message)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog.Root open onOpenChange={(open) => (!open ? onClose() : undefined)}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/50 backdrop-blur-sm" />
        <Dialog.Content className="fixed left-1/2 top-1/2 w-[440px] -translate-x-1/2 -translate-y-1/2 rounded-xl border border-line bg-surface p-5 shadow-2xl">
          <Dialog.Title className="mb-4 text-[14px] font-medium">
            {state.isNew ? t('action.duplicate') : t('action.edit')} · {host.name}
          </Dialog.Title>
          <form className="space-y-3" onSubmit={save}>
            <Field label={t('host.name')}>
              <input value={name} onChange={(event) => setName(event.target.value)} className="input" />
            </Field>
            <div className="grid grid-cols-[1fr_92px] gap-3">
              <Field label={t('connect.host')}>
                <input required value={hostname} onChange={(event) => setHostname(event.target.value)} className="input" />
              </Field>
              <Field label={t('connect.port')}>
                <input value={port} onChange={(event) => setPort(event.target.value)} className="input" />
              </Field>
            </div>
            <div className="grid grid-cols-2 gap-3">
              <Field label={t('connect.user')}>
                <input required value={username} onChange={(event) => setUsername(event.target.value)} className="input" />
              </Field>
              <Field label={t('host.group')}>
                <input value={group} onChange={(event) => setGroup(event.target.value)} className="input" />
              </Field>
            </div>
            <Field label={t('connect.auth')}>
              <select value={method} onChange={(event) => setMethod(event.target.value as 'password' | 'key' | 'agent')} className="input">
                <option value="password">{t('connect.auth.password')}</option>
                <option value="key">{t('connect.auth.key')}</option>
                <option value="agent">{t('connect.auth.agent')}</option>
              </select>
            </Field>
            {method === 'password' ? (
              <Field label={t('connect.password')}>
                <div className="relative">
                  <input
                    type={reveal ? 'text' : 'password'}
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    placeholder={t('host.passwordKeep')}
                    className="input pr-9"
                  />
                  <button
                    type="button"
                    onClick={() => setReveal((current) => !current)}
                    title={reveal ? t('action.hide') : t('action.show')}
                    className="absolute right-1 top-1/2 grid h-7 w-7 -translate-y-1/2 place-items-center rounded text-muted transition hover:text-ink"
                  >
                    {reveal ? <EyeOff size={14} /> : <Eye size={14} />}
                  </button>
                </div>
              </Field>
            ) : null}
            {method === 'key' ? (
              <Field label={t('connect.key')}>
                <input value={keyPath} onChange={(event) => setKeyPath(event.target.value)} className="input" />
              </Field>
            ) : null}

            <div className="flex items-center justify-between gap-2 pt-2">
              <div className="flex gap-2">
                {!state.isNew ? (
                  <button
                    type="button"
                    onClick={() => void remove()}
                    disabled={busy}
                    className="rounded-lg px-2.5 py-1.5 text-[12px] text-danger transition hover:bg-elevated"
                  >
                    {confirmDelete ? t('host.deleteConfirm') : t('action.delete')}
                  </button>
                ) : null}
                {!state.isNew ? (
                  <button
                    type="button"
                    onClick={() => void duplicate()}
                    className="rounded-lg px-2.5 py-1.5 text-[12px] text-muted transition hover:bg-elevated hover:text-ink"
                  >
                    {t('action.duplicate')}
                  </button>
                ) : null}
              </div>
              <div className="flex gap-2">
                <button type="button" onClick={onClose} className="rounded-lg px-3 py-1.5 text-[12px] text-muted transition hover:text-ink">
                  {t('action.cancel')}
                </button>
                <button
                  type="submit"
                  disabled={busy}
                  className="rounded-lg bg-accent px-3 py-1.5 text-[12px] font-medium text-canvas transition hover:opacity-90 disabled:opacity-50"
                >
                  {t('action.save')}
                </button>
              </div>
            </div>
          </form>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}
