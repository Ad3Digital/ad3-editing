/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

/* @refresh reload */
import * as Sentry from '@sentry/solid'
import { render } from 'solid-js/web'
import './index.css'
import App from './app'
import { initAnalytics } from './lib/analytics'
import { restoreLastRoute } from './lib/persist-route'

if (import.meta.env.PROD) {
  Sentry.init({
    dsn: 'https://5786931d60606166d379cd2405683cb1@o4511326229889024.ingest.us.sentry.io/4511326232903680',
    sendDefaultPii: true,
    release: APP_VERSION,
    environment: 'production',
  })
}

document.addEventListener('contextmenu', (e) => e.preventDefault())

if (window.desktop) {
  document.documentElement.dataset.platform = window.desktop.platform;

  const origRequest = FileSystemHandle.prototype.requestPermission;
  FileSystemHandle.prototype.queryPermission = async function (desc) {
    try {
      return await origRequest.call(this, desc);
    } catch {
      return 'prompt';
    }
  };

  restoreLastRoute();
}

initAnalytics()

const root = document.getElementById('root')

render(() => <App />, root!)
