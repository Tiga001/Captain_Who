import { spawn } from 'node:child_process'
import { app } from 'electron'
import {
  buildDevelopmentStorageResetCommand,
  readDevelopmentElectronAppName,
  resolveElectronAppDataRoot
} from './reset-dev-storage-command.mjs'

const appDataRoot = resolveElectronAppDataRoot(app, readDevelopmentElectronAppName())
const command = buildDevelopmentStorageResetCommand(appDataRoot, process.argv.slice(2))
const child = spawn(command.executable, command.arguments, {
  cwd: command.cwd,
  env: process.env,
  stdio: 'inherit'
})

child.once('error', (error) => {
  console.error(`Failed to launch the development storage reset: ${error.message}`)
  app.exit(1)
})

child.once('exit', (code, signal) => {
  if (signal) {
    console.error(`Development storage reset stopped by signal ${signal}`)
    app.exit(1)
    return
  }
  app.exit(code ?? 1)
})
