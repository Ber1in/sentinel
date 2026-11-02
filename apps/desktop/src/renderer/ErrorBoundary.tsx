import React from 'react'

interface State {
  error: Error | null
}

/**
 * Without this a render error unmounts the whole tree and the window goes black, which
 * is impossible to diagnose from the outside.
 */
export class ErrorBoundary extends React.Component<{ children: React.ReactNode }, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('render error', error, info.componentStack)
  }

  render(): React.ReactNode {
    if (!this.state.error) return this.props.children
    return (
      <div style={{ padding: 24, fontFamily: 'system-ui, sans-serif', color: '#e6e8ee' }}>
        <h1 style={{ fontSize: 16, marginBottom: 8 }}>界面出错了</h1>
        <pre
          style={{
            whiteSpace: 'pre-wrap',
            fontSize: 12,
            color: '#f85149',
            background: '#171b24',
            padding: 12,
            borderRadius: 8
          }}
        >
          {this.state.error.message}
          {'\n'}
          {this.state.error.stack?.split('\n').slice(0, 6).join('\n')}
        </pre>
        <button
          onClick={() => window.location.reload()}
          style={{ marginTop: 12, padding: '6px 12px', borderRadius: 8, cursor: 'pointer' }}
        >
          重新加载
        </button>
      </div>
    )
  }
}
