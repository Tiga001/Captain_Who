import './styles/global.css'

import { hostClient } from './host/hostClient'
import {
  completeBootstrapStartup,
  failBootstrapStartup
} from './features/startup/bootstrapStartupEntry'

void hostClient.app
  .whenReady()
  .then(async () => {
    const [{ createElement, StrictMode }, { createRoot }, { default: App }] = await Promise.all([
      import('react'),
      import('react-dom/client'),
      import('./App')
    ])
    completeBootstrapStartup()
    createRoot(document.getElementById('root')!).render(
      createElement(StrictMode, null, createElement(App))
    )
  })
  .catch(() => failBootstrapStartup())
