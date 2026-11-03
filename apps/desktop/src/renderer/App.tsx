import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import * as Dialog from '@radix-ui/react-dialog'
import { ChevronsRight, Command, Eye, EyeOff, Pencil, Plus, Server, Terminal as TerminalIcon, Trash2, X } from 'lucide-react'
import type { HostSummary } from '../shared/HostSummary'
import type { SessionState } from '../shared/SessionState'
import type { SessionSummary } from '../shared/SessionSummary'
import type { ProviderKind } from '../shared/ProviderKind'
import type { Terminal } from '@xterm/xterm'
import { TerminalPane } from './TerminalPane'
import { SplitView } from './SplitView'
import {
  firstPane,
  flatten,
  leaf,
  paneIds,
  rebuild,
  removeLeaf,
  setRatio,
  splitLeaf,
  type LayoutNode,
  type PersistedPane,
  type SplitDirection
} from './layout'
import { detectLanguage, translator, type Language } from './i18n'

type Theme = 'dark' | 'light' | 'system'

interface PaneState {
  id: string
  ratio: number
  hostId: string
  sessionId: string
  numeric: number
  title: string
  state: SessionState
  provider: ProviderKind
}

interface Tab {
  id: string
  /** Nested split tree; leaves are pane ids. */
  root: LayoutNode
  /** Pane that owns keyboard focus and is the target of "close pane". */
  activePaneId: string
  panes: PaneState[]
}

/** Fallback so a malformed or migrated-layout tab can never crash the UI. */
const EMPTY_PANE: PaneState = {
  id: '',
  ratio: 1,
  hostId: '',
  sessionId: '',
  numeric: 0,
  title: '',
  state: 'detached',
  provider: 'tmux'
}

const paneOf = (tab: Tab): PaneState => tab.panes[0] ?? EMPTY_PANE
const paneTitle = (tab: Tab): string => paneOf(tab).title
const paneState = (tab: Tab): SessionState => paneOf(tab).state

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
    () => (localStorage.getItem('sentinel.theme') as Theme | null) ?? 'system'
  )
  const [fontSize, setFontSize] = useState<number>(
    () => Number(localStorage.getItem('sentinel.fontSize')) || 13
  )
  const [menu, setMenu] = useState<'session' | 'settings' | null>(null)
  const [hosts, setHosts] = useState<HostSummary[]>([])
  const [daemonSessions, setDaemonSessions] = useState<SessionSummary[]>([])
  const [tabs, setTabs] = useState<Tab[]>([])
  const [activeTab, setActiveTab] = useState<string | null>(null)
  const [dialogOpen, setDialogOpen] = useState(false)
  const [paletteOpen, setPaletteOpen] = useState(false)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [editing, setEditing] = useState<{ host: HostSummary; isNew: boolean } | null>(null)
  const [tabMenu, setTabMenu] = useState<{ tabId: string; x: number; y: number } | null>(null)
  const [daemonOnline, setDaemonOnline] = useState(true)

  /** Exactly one tab per session; duplicates would multiply every keystroke. */
  const addTab = useCallback((tab: Tab) => {
    setTabs((current) => {
      const duplicate = current.some((item) =>
        item.panes.some((pane) => tab.panes.some((next) => next.sessionId === pane.sessionId))
      )
      return duplicate ? current : [...current, tab]
    })
    setActiveTab(tab.id)
  }, [])

  /** Update a specific pane (needed before its session id is known). */
  const patchPaneIn = useCallback((tabId: string, paneId: string, patch: Partial<PaneState>) => {
    setTabs((current) =>
      current.map((tab) =>
        tab.id === tabId
          ? {
              ...tab,
              panes: tab.panes.map((pane) => (pane.id === paneId ? { ...pane, ...patch } : pane))
            }
          : tab
      )
    )
  }, [])

  // Restore the persisted layout, without connecting anything yet.
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const layout = await window.desktop.loadLayout()
        if (cancelled || layout.tabs.length === 0) return
        const tabsLayout = layout.tabs as Array<{ id: string; title: string; sort_order: number }>
        const rows = layout.panes as unknown as PersistedPane[]
        const built: Tab[] = []
        for (const tab of tabsLayout) {
          const rowsForTab = rows.filter((row) => row.tab_id === tab.id)
          const root = rebuild(rowsForTab, tab.id)
          if (!root) continue
          // Leaves come from the tree, not from `direction === null`: older layouts wrote
          // a direction on every row, which made that filter drop every pane.
          const panes: PaneState[] = paneIds(root)
            .sort((a, b) => {
              const left = rowsForTab.find((row) => row.id === a)?.sort_order ?? 0
              const right = rowsForTab.find((row) => row.id === b)?.sort_order ?? 0
              return left - right
            })
            .map((paneId) => {
              const row = rowsForTab.find((item) => item.id === paneId)
              return {
                id: paneId,
                ratio: 1,
                hostId: row?.host_id ?? '',
                sessionId: row?.session_id ?? '',
                numeric: 0,
                title: tab.title,
                state: 'detached' as SessionState,
                provider: 'tmux' as ProviderKind
              }
            })
          if (panes.length === 0) continue
          built.push({
            id: tab.id,
            root,
            activePaneId: firstPane(root),
            panes
          })
        }
        setTabs((current) => (current.length > 0 ? current : built))
      } catch {
        // No saved layout yet.
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  // Connect restored panes lazily: only the tab the user opens dials out.
  useEffect(() => {
    const tab = tabs.find((item) => item.id === activeTab)
    if (!tab) return
    for (const pane of tab.panes) {
      if (pane.numeric !== 0 || !pane.hostId) continue
      const host = hosts.find((item) => item.id === pane.hostId)
      if (!host) continue
      void window.desktop
        .createSession(
          {
            host_id: host.id,
            hostname: host.hostname,
            port: host.port,
            username: host.username,
            auth: host.auth
          },
          pane.id,
          null
        )
        .then((reply) =>
          patchPaneIn(tab.id, pane.id, {
            sessionId: reply.session_id,
            numeric: reply.numeric,
            state: 'connecting'
          })
        )
        .catch(() => undefined)
    }
  }, [activeTab, tabs, hosts, patchPaneIn])

  // Persist the layout (debounced) whenever its shape or ratios change.
  useEffect(() => {
    if (tabs.length === 0) return
    const timer = setTimeout(() => {
      const panes: PersistedPane[] = []
      tabs.forEach((tab) => {
        flatten(
          tab.root,
          tab.id,
          (paneId) => {
            const pane = tab.panes.find((item) => item.id === paneId)
            return pane ? { hostId: pane.hostId, sessionId: pane.sessionId } : undefined
          },
          null,
          0,
          panes
        )
      })
      void window.desktop.saveLayout(
        tabs.map((tab, index) => ({ id: tab.id, title: paneTitle(tab), sort_order: index })),
        panes
      )
    }, 500)
    return () => clearTimeout(timer)
  }, [tabs])

  /** Update one pane wherever it lives. */
  const patchPane = useCallback((sessionId: string, patch: Partial<PaneState>) => {
    setTabs((current) =>
      current.map((tab) => ({
        ...tab,
        panes: tab.panes.map((pane) => (pane.sessionId === sessionId ? { ...pane, ...patch } : pane))
      }))
    )
  }, [])
  const [toasts, setToasts] = useState<Toast[]>([])
  const terminals = useRef(new Map<number, Terminal>())
  const t = useMemo(() => translator(language), [language])

  const pushToast = useCallback((message: string, tone: Toast['tone'] = 'error') => {
    const id = Date.now() + Math.random()
    setToasts((current) => [...current, { id, tone, message }])
    setTimeout(() => setToasts((current) => current.filter((toast) => toast.id !== id)), 6000)
  }, [])

  const resolvedTheme =
    theme === 'system'
      ? window.matchMedia('(prefers-color-scheme: light)').matches
        ? 'light'
        : 'dark'
      : theme

  useEffect(() => {
    document.documentElement.dataset.theme = resolvedTheme
    localStorage.setItem('sentinel.theme', theme)
  }, [resolvedTheme, theme])

  useEffect(() => {
    localStorage.setItem('sentinel.fontSize', String(fontSize))
  }, [fontSize])

  useEffect(() => {
    if (theme !== 'system') return
    const media = window.matchMedia('(prefers-color-scheme: light)')
    const listener = (): void => {
      document.documentElement.dataset.theme = media.matches ? 'light' : 'dark'
    }
    media.addEventListener('change', listener)
    return () => media.removeEventListener('change', listener)
  }, [theme])

  useEffect(() => {
    localStorage.setItem('sentinel.language', language)
  }, [language])

  const refreshHosts = useCallback(async () => {
    try {
      const [nextHosts, nextSessions] = await Promise.all([
        window.desktop.listHosts(),
        window.desktop.listSessions()
      ])
      setHosts(nextHosts)
      setDaemonSessions(nextSessions)
    } catch {
      // Daemon may not be running yet; the status card covers that.
    }
  }, [])

  useEffect(() => {
    void refreshHosts()
  }, [refreshHosts])

  const checkDaemon = useCallback(async () => {
    try {
      const status = await window.desktop.daemonStatus()
      setDaemonOnline(status.connected)
    } catch {
      setDaemonOnline(false)
    }
  }, [])

  useEffect(() => {
    void checkDaemon()
  }, [checkDaemon])

  // Reattach sessions the daemon still owns (GUI restart, crash, upgrade).
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const sessions = await window.desktop.listSessions()
        if (cancelled || sessions.length === 0) return
        setTabs((current) => {
          const known = new Set(current.flatMap((tab) => tab.panes.map((pane) => pane.sessionId)))
          const restored = sessions
            .filter((session) => !known.has(session.session_id))
            .map((session) => ({
              id: session.session_id,
              root: leaf('main'),
              activePaneId: 'main',
              panes: [
                {
                  id: 'main',
                  ratio: 1,
                  hostId: session.host_id,
                  sessionId: session.session_id,
                  numeric: session.numeric,
                  title: session.title,
                  state: session.state,
                  provider: session.provider
                }
              ]
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

  const background = useMemo(() => {
    const open = new Set(tabs.flatMap((tab) => tab.panes.map((pane) => pane.sessionId)))
    return daemonSessions.filter((session) => !open.has(session.session_id))
  }, [daemonSessions, tabs])

  const openBackground = useCallback(
    (session: SessionSummary) => {
      addTab({
        id: session.session_id,
        root: leaf('main'),
        activePaneId: 'main',
        panes: [
          {
            id: 'main',
            ratio: 1,
            hostId: session.host_id,
            sessionId: session.session_id,
            numeric: session.numeric,
            title: session.title,
            state: session.state,
            provider: session.provider
          }
        ]
      })
    },
    [addTab]
  )


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
        current.map((tab) => ({
          ...tab,
          panes: tab.panes.map((pane) =>
            pane.sessionId === data.sessionId
              ? { ...pane, state: data.state as SessionState }
              : pane
          )
        }))
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
        const pane: PaneState = {
          id: 'main',
          ratio: 1,
          hostId,
          sessionId: reply.session_id,
          numeric: reply.numeric,
          title: `${host.username}@${host.hostname}`,
          state: 'connecting',
          provider: reply.provider
        }
        addTab({
          id: reply.session_id,
          root: leaf(pane.id),
          activePaneId: pane.id,
          panes: [pane]
        })
      } catch (error) {
        pushToast((error as Error).message)
      }
    },
    [pushToast, addTab]
  )

  const closeTab = useCallback(async (tab: Tab) => {
    await Promise.all(tab.panes.map((pane) => window.desktop.detach(pane.sessionId)))
    setTabs((current) => current.filter((item) => item.id !== tab.id))
    setActiveTab((current) => (current === tab.id ? null : current))
  }, [])

  /** Close one split pane; closing the last one closes the tab. */
  const closePane = useCallback(
    async (tab: Tab, paneId: string) => {
      const pane = tab.panes.find((item) => item.id === paneId)
      if (!pane) return
      if (tab.panes.length === 1) {
        await closeTab(tab)
        return
      }
      await window.desktop.detach(pane.sessionId)
      setTabs((current) =>
        current.map((item) => {
          if (item.id !== tab.id) return item
          const root = removeLeaf(item.root, paneId)
          if (!root) return item
          const panes = item.panes.filter((next) => next.id !== paneId)
          return {
            ...item,
            root,
            panes,
            activePaneId: item.activePaneId === paneId ? firstPane(root) : item.activePaneId
          }
        })
      )
    },
    [closeTab]
  )

  /** Split the active tab using the first pane's host. */
  const splitTab = useCallback(
    async (tab: Tab, direction: SplitDirection) => {
      const source = tab.panes.find((pane) => pane.id === tab.activePaneId) ?? paneOf(tab)
      const host = hosts.find((item) => item.id === source.hostId)
      if (!host) {
        pushToast('host not found')
        return
      }
      try {
        const paneId = `pane-${crypto.randomUUID()}`
        const reply = await window.desktop.createSession(
          {
            host_id: host.id,
            hostname: host.hostname,
            port: host.port,
            username: host.username,
            auth: host.auth
          },
          paneId,
          null
        )
        setTabs((current) =>
          current.map((item) =>
            item.id === tab.id
              ? {
                  ...item,
                  root: splitLeaf(item.root, item.activePaneId, paneId, direction),
                  activePaneId: paneId,
                  panes: [
                    ...item.panes.map((pane) => ({ ...pane, ratio: item.panes.length })),
                    {
                      id: paneId,
                      ratio: 1,
                      hostId: host.id,
                      sessionId: reply.session_id,
                      numeric: reply.numeric,
                      title: `${host.username}@${host.hostname}`,
                      state: 'connecting' as SessionState,
                      provider: reply.provider
                    }
                  ]
                }
              : item
          )
        )
      } catch (error) {
        pushToast((error as Error).message)
      }
    },
    [hosts, pushToast]
  )

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
        <div className="no-drag relative flex items-center gap-1">
          <MenuButton
            label={t('menu.session')}
            open={menu === 'session'}
            onToggle={() => setMenu(menu === 'session' ? null : 'session')}
          />
          <MenuButton
            label={t('menu.settings')}
            open={menu === 'settings'}
            onToggle={() => setMenu(menu === 'settings' ? null : 'settings')}
          />
          {menu === 'session' ? (
            <MenuPanel onClose={() => setMenu(null)}>
              <MenuItem label={t('action.new')} onClick={() => { setMenu(null); setDialogOpen(true) }} />
              <MenuItem
                label={t('action.splitRight')}
                onClick={() => {
                  setMenu(null)
                  const tab = tabs.find((item) => item.id === activeTab)
                  if (tab) void splitTab(tab, 'horizontal')
                }}
              />
              <MenuItem
                label={t('action.splitDown')}
                onClick={() => {
                  setMenu(null)
                  const tab = tabs.find((item) => item.id === activeTab)
                  if (tab) void splitTab(tab, 'vertical')
                }}
              />
              <Divider />
              <MenuItem
                label={t('action.closePane')}
                onClick={() => {
                  setMenu(null)
                  const tab = tabs.find((item) => item.id === activeTab)
                  if (tab) void closePane(tab, tab.activePaneId)
                }}
              />
              <MenuItem
                label={t('action.close')}
                onClick={() => {
                  setMenu(null)
                  const tab = tabs.find((item) => item.id === activeTab)
                  if (tab) void closeTab(tab)
                }}
              />
            </MenuPanel>
          ) : null}
          {menu === 'settings' ? (
            <MenuPanel onClose={() => setMenu(null)}>
              <MenuLabel text={t('settings.language')} />
              <MenuItem label="English" trailing={language === 'en' ? '✓' : ''} onClick={() => { setLanguage('en'); setMenu(null) }} />
              <MenuItem label="简体中文" trailing={language === 'zh-CN' ? '✓' : ''} onClick={() => { setLanguage('zh-CN'); setMenu(null) }} />
              <Divider />
              <MenuLabel text={t('settings.theme')} />
              <MenuItem label={t('settings.theme.dark')} trailing={theme === 'dark' ? '✓' : ''} onClick={() => { setTheme('dark'); setMenu(null) }} />
              <MenuItem label={t('settings.theme.light')} trailing={theme === 'light' ? '✓' : ''} onClick={() => { setTheme('light'); setMenu(null) }} />
              <MenuItem label={t('settings.theme.system')} trailing={theme === 'system' ? '✓' : ''} onClick={() => { setTheme('system'); setMenu(null) }} />
              <Divider />
              <MenuLabel text={t('settings.fontSize')} />
              {[12, 13, 14, 16].map((size) => (
                <MenuItem
                  key={size}
                  label={`${size}px`}
                  trailing={fontSize === size ? '✓' : ''}
                  onClick={() => { setFontSize(size); setMenu(null) }}
                />
              ))}
            </MenuPanel>
          ) : null}
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
                    <span
                      className="grid h-7 w-7 shrink-0 place-items-center rounded-md bg-elevated text-muted group-hover:text-accent"
                      style={{ boxShadow: `inset 2px 0 0 ${hostColor(host.id)}` }}
                    >
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
          {background.length > 0 ? (
            <>
              <div className="flex items-center justify-between border-t border-line px-3 pb-1 pt-3">
                <span className="text-[11px] font-semibold uppercase tracking-wider text-muted">
                  {t('sessions.title')}
                </span>

              </div>
              <div className="max-h-[180px] overflow-y-auto px-2 pb-2">
                {background.map((session) => (
                  <button
                    key={session.session_id}
                    onClick={() => openBackground(session)}
                    className="mb-1 flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left transition hover:bg-elevated"
                  >
                    <span className="status-dot" style={{ background: stateColor(session.state) }} />
                    <span className="min-w-0 flex-1 truncate text-[12px]">{session.title}</span>
                    <span className="text-[10px] text-muted">{session.provider}</span>
                  </button>
                ))}
              </div>
            </>
          ) : null}
          <div className="border-t border-line px-3 py-2 text-[11px] text-muted">
            {daemonOnline ? (
              <div className="flex items-center gap-1.5">
                <Command size={12} />
                Ctrl+K
              </div>
            ) : (
              <div className="flex items-center justify-between gap-2">
                <span className="flex items-center gap-1.5 text-danger">
                  <span className="status-dot" style={{ background: 'var(--danger)' }} />
                  {t('daemon.offline')}
                </span>
                <button
                  className="rounded px-2 py-0.5 text-[11px] text-accent transition hover:bg-elevated"
                  onClick={() => {
                    void (async () => {
                      const result = await window.desktop.restartDaemon()
                      if (!result.ok && result.error) pushToast(result.error)
                      await checkDaemon()
                      await refreshHosts()
                    })()
                  }}
                >
                  {t('daemon.restart')}
                </button>
              </div>
            )}
          </div>
        </aside>

        <main className="flex min-w-0 flex-1 flex-col bg-canvas">
          <div className="flex h-9 shrink-0 items-stretch border-b border-line">
            {tabs.map((tab) => (
              <div
                key={tab.id}
                onClick={() => setActiveTab(tab.id)}
                onContextMenu={(event) => {
                  event.preventDefault()
                  setActiveTab(tab.id)
                  setTabMenu({ tabId: tab.id, x: event.clientX, y: event.clientY })
                }}
                className={`group flex cursor-pointer items-center gap-2 border-r border-line px-3 text-[12px] transition ${
                  activeTab === tab.id ? 'bg-canvas text-ink' : 'bg-surface text-muted hover:text-ink'
                }`}
                style={activeTab === tab.id ? { boxShadow: 'inset 0 -2px 0 var(--accent)' } : undefined}
              >
                <span className="status-dot" style={{ background: stateColor(paneState(tab)) }} />
                <span
                  className="h-2 w-2 rounded-sm"
                  style={{ background: hostColor(tab.panes[0]?.hostId ?? '') }}
                />
                <span className="max-w-[220px] truncate">{paneTitle(tab)}</span>
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
                <div
                  key={tab.id}
                  className="absolute inset-0"
                  style={{ display: activeTab === tab.id ? 'block' : 'none' }}
                >
                  <SplitView
                    node={tab.root}
                    onRatioChange={(splitId, ratio) =>
                      setTabs((current) =>
                        current.map((item) =>
                          item.id === tab.id ? { ...item, root: setRatio(item.root, splitId, ratio) } : item
                        )
                      )
                    }
                    renderPane={(paneId) => {
                      const payload = tab.panes.find((pane) => pane.id === paneId)
                      if (!payload) return null
                      return (
                      <div
                        className="group/pane relative h-full w-full"
                        onMouseDown={() =>
                          setTabs((current) =>
                            current.map((item) =>
                              item.id === tab.id ? { ...item, activePaneId: payload.id } : item
                            )
                          )
                        }
                      >
                        {tab.panes.length > 1 ? (
                          <button
                            className="absolute right-2 top-1.5 z-10 grid h-5 w-5 place-items-center rounded bg-elevated/80 text-muted opacity-0 transition hover:text-danger group-hover/pane:opacity-100"
                            title={t('action.closePane')}
                            onClick={(event) => {
                              event.stopPropagation()
                              void closePane(tab, payload.id)
                            }}
                          >
                            <X size={11} />
                          </button>
                        ) : null}
                        <TerminalPane
                          sessionId={payload.sessionId}
                          numeric={payload.numeric}
                          fontSize={fontSize}
                          active={activeTab === tab.id}
                          onRegister={register}
                          onReady={() => void window.desktop.resync(payload.sessionId)}
                        />
                      </div>
                      )
                    }}
                  />
                </div>
              ))
            )}
          </div>

          <StatusBar tabs={tabs} activeTab={activeTab} label={t} />
        </main>
      </div>

      {tabMenu ? (
        <>
          <div className="fixed inset-0 z-20" onClick={() => setTabMenu(null)} onContextMenu={() => setTabMenu(null)} />
          <div
            className="fixed z-30 w-[168px] rounded-lg border border-line bg-surface p-1 shadow-2xl"
            style={{ left: tabMenu.x, top: tabMenu.y }}
          >
            <MenuItem
              label={t('action.splitRight')}
              onClick={() => {
                const tab = tabs.find((item) => item.id === tabMenu.tabId)
                setTabMenu(null)
                if (tab) void splitTab(tab, 'horizontal')
              }}
            />
            <MenuItem
              label={t('action.splitDown')}
              onClick={() => {
                const tab = tabs.find((item) => item.id === tabMenu.tabId)
                setTabMenu(null)
                if (tab) void splitTab(tab, 'vertical')
              }}
            />
            <div className="my-1 h-px bg-line" />
            <MenuItem
              label={t('action.closePane')}
              onClick={() => {
                const tab = tabs.find((item) => item.id === tabMenu.tabId)
                setTabMenu(null)
                if (tab) void closePane(tab, tab.activePaneId)
              }}
            />
            <MenuItem
              label={t('action.close')}
              onClick={() => {
                const tab = tabs.find((item) => item.id === tabMenu.tabId)
                setTabMenu(null)
                if (tab) void closeTab(tab)
              }}
            />
          </div>
        </>
      ) : null}

      <QuickConnect
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        t={t}
        hosts={hosts}
        onDone={(tab) => {
          addTab(tab)
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
        actions={[
          {
            label: t('settings.title'),
            run: () => {
              setPaletteOpen(false)
              setSettingsOpen(true)
            }
          }
        ]}
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

/** Stable per-host accent so tabs and host cards share a colour. */
function hostColor(id: string): string {
  let hash = 0
  for (const character of id) hash = (hash * 31 + character.charCodeAt(0)) % 360
  return `hsl(${hash} 65% 58%)`
}

function MenuButton({ label, open, onToggle }: { label: string; open: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      className={`rounded-md px-2 py-1 text-[12px] transition ${
        open ? 'bg-elevated text-ink' : 'text-muted hover:bg-elevated hover:text-ink'
      }`}
    >
      {label}
    </button>
  )
}

function MenuPanel({ children, onClose }: { children: React.ReactNode; onClose: () => void }) {
  return (
    <>
      <div className="fixed inset-0 z-20" onClick={onClose} />
      <div className="absolute left-0 top-8 z-30 w-[200px] rounded-lg border border-line bg-surface p-1 shadow-2xl">
        {children}
      </div>
    </>
  )
}

function MenuLabel({ text }: { text: string }) {
  return <div className="px-2.5 pb-0.5 pt-1.5 text-[10px] uppercase tracking-wide text-muted">{text}</div>
}

function Divider() {
  return <div className="my-1 h-px bg-line" />
}

function MenuItem({ label, onClick, trailing }: { label: string; onClick: () => void; trailing?: string }) {
  return (
    <button
      onClick={onClick}
      className="flex w-full items-center justify-between rounded px-2.5 py-1.5 text-left text-[12px] text-ink transition hover:bg-elevated"
    >
      <span>{label}</span>
      {trailing ? <span className="text-accent">{trailing}</span> : null}
    </button>
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
  const title = active ? paneTitle(active) : ''
  return (
    <footer className="flex h-6 shrink-0 items-center justify-between border-t border-line px-3 text-[11px] text-muted">
      <span>{active ? `${label(`state.${paneState(active)}`)} · ${active.panes.length > 1 ? `${active.panes.length} panes` : 'tmux'}` : '—'}</span>
      <span>{title}</span>
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
  const [reveal, setReveal] = useState(false)
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
        root: leaf('main'),
        activePaneId: 'main',
        panes: [
          {
            id: 'main',
            ratio: 1,
            hostId,
            sessionId: reply.session_id,
            numeric: reply.numeric,
            title: `${user}@${host}`,
            state: 'connecting',
            provider: reply.provider
          }
        ]
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
                <div className="relative">
                  <input
                    type={reveal ? 'text' : 'password'}
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
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
  actions: Array<{ label: string; run: () => void }>
}) {
  const { open, onOpenChange, hosts, t, onPick, onNew, actions } = props
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
            {actions.map((action) => (
              <button
                key={action.label}
                onClick={action.run}
                className="flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left text-[13px] transition hover:bg-elevated"
              >
                <Command size={14} className="text-muted" />
                {action.label}
              </button>
            ))}
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

      if (method === 'password' && !password && previousAuth.method !== 'password') {
        throw new Error(t('host.passwordRequired'))
      }
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
