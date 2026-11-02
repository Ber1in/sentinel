import { app, BrowserWindow, ipcMain } from 'electron'
import { join } from 'node:path'

function createWindow(): BrowserWindow {
  const window = new BrowserWindow({
    width: 1200,
    height: 780,
    show: false,
    title: 'sshTerminal',
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

ipcMain.handle('app:ping', () => ({ pong: true, version: app.getVersion() }))

void app.whenReady().then(() => {
  createWindow()
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit()
})
