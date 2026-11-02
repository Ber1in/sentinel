import React from 'react'
import { createRoot } from 'react-dom/client'
import '@fontsource/inter/400.css'
import '@fontsource/inter/500.css'
import '@fontsource/inter/600.css'
import '@fontsource/jetbrains-mono/400.css'
import '@xterm/xterm/css/xterm.css'
import './styles.css'
import { App } from './App'

const container = document.getElementById('root')
if (!container) throw new Error('root element missing')

createRoot(container).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)
