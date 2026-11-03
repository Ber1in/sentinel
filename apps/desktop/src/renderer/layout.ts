export type SplitDirection = 'horizontal' | 'vertical'

/** Binary split tree: every leaf is a pane, every internal node splits its children. */
export type LayoutNode =
  | { kind: 'pane'; id: string }
  | {
      kind: 'split'
      id: string
      direction: SplitDirection
      /** Share of the space given to `first`. */
      ratio: number
      first: LayoutNode
      second: LayoutNode
    }

export interface PersistedPane {
  id: string
  tab_id: string
  parent_id: string | null
  direction: SplitDirection | null
  ratio: number
  host_id: string | null
  session_id: string | null
  sort_order: number
}

export const leaf = (id: string): LayoutNode => ({ kind: 'pane', id })

export const newNodeId = (): string => `split-${crypto.randomUUID()}`

/** Replace the leaf `target` with a split holding `target` and `added`. */
export function splitLeaf(
  node: LayoutNode,
  target: string,
  added: string,
  direction: SplitDirection
): LayoutNode {
  if (node.kind === 'pane') {
    if (node.id !== target) return node
    return {
      kind: 'split',
      id: newNodeId(),
      direction,
      ratio: 0.5,
      first: { kind: 'pane', id: target },
      second: { kind: 'pane', id: added }
    }
  }
  return {
    ...node,
    first: splitLeaf(node.first, target, added, direction),
    second: splitLeaf(node.second, target, added, direction)
  }
}

/** Remove a leaf; its sibling takes the parent's place. Returns null when empty. */
export function removeLeaf(node: LayoutNode, target: string): LayoutNode | null {
  if (node.kind === 'pane') return node.id === target ? null : node
  const first = removeLeaf(node.first, target)
  const second = removeLeaf(node.second, target)
  if (first && second) return { ...node, first, second }
  return first ?? second
}

export function setRatio(node: LayoutNode, splitId: string, ratio: number): LayoutNode {
  if (node.kind === 'pane') return node
  if (node.id === splitId) return { ...node, ratio }
  return {
    ...node,
    first: setRatio(node.first, splitId, ratio),
    second: setRatio(node.second, splitId, ratio)
  }
}

export function paneIds(node: LayoutNode): string[] {
  return node.kind === 'pane' ? [node.id] : [...paneIds(node.first), ...paneIds(node.second)]
}

export function firstPane(node: LayoutNode): string {
  return node.kind === 'pane' ? node.id : firstPane(node.first)
}

/** Flatten to the wire format: split nodes have no session, leaves carry one. */
export function flatten(
  node: LayoutNode,
  tabId: string,
  sessionOf: (paneId: string) => { hostId: string; sessionId: string } | undefined,
  parentId: string | null,
  order: number,
  out: PersistedPane[]
): number {
  if (node.kind === 'pane') {
    const session = sessionOf(node.id)
    out.push({
      id: node.id,
      tab_id: tabId,
      parent_id: parentId,
      direction: null,
      ratio: 1,
      host_id: session?.hostId ?? null,
      session_id: session?.sessionId || null,
      sort_order: order
    })
    return order + 1
  }
  out.push({
    id: node.id,
    tab_id: tabId,
    parent_id: parentId,
    direction: node.direction,
    ratio: node.ratio,
    host_id: null,
    session_id: null,
    sort_order: order
  })
  let next = order + 1
  next = flatten(node.first, tabId, sessionOf, node.id, next, out)
  next = flatten(node.second, tabId, sessionOf, node.id, next, out)
  return next
}

/** Rebuild a tree from rows that came back through the protocol. */
export function rebuild(
  rows: Array<PersistedPane>,
  tabId: string
): LayoutNode | null {
  const tabRows = rows.filter((row) => row.tab_id === tabId)
  const byParent = (parentId: string | null): PersistedPane[] =>
    tabRows
      .filter((row) => (row.parent_id ?? null) === parentId)
      .sort((a, b) => a.sort_order - b.sort_order)

  const build = (row: PersistedPane): LayoutNode => {
    if (row.direction && !row.session_id) {
      const children = byParent(row.id)
      if (children.length === 2) {
        return {
          kind: 'split',
          id: row.id,
          direction: row.direction,
          ratio: row.ratio,
          first: build(children[0]),
          second: build(children[1])
        }
      }
    }
    return { kind: 'pane', id: row.id }
  }

  const roots = byParent(null)
  return roots.length > 0 ? build(roots[0]) : null
}
