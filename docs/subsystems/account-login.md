---
status: current
audience: developers
owner: engineering
last_verified: 2026-09-26
---

# Captain Who desktop account login

The desktop client uses `@cloudbase/js-sdk@3.9.2` in Electron Main. Public environment configuration lives in `src/main/auth/accountConfig.ts`. The publishable key is a distributable anonymous client identifier, not a user session or server API key. Website OAuth/deep-link authorization is not implemented.

## Startup and account lifetime

- React and the account form render before full Host initialization. CloudBase restoration and backend startup run concurrently.
- While the initial session check is pending, the ambient startup screen stays visible rather than briefly presenting a sign-in form. Form mode and password visibility are local UI state; revealing a password does not submit it or switch login mode.
- The startup overlay exits when Electron Host/Core Server and local data hydration are ready and `/account-api/v1/me?includeEntitlements=false` returns HTTP 200 with a non-null, active profile. License verification is independent and never blocks entry. See [local usage and licensing](local-token-usage-and-license.md) for new-turn admission and the 24-hour license cache.
- Password and email OTP login share this gate. OTP uses the SDK's verification callback with `shouldCreateUser: false`; registration stays on the website.
- Logout invalidates pending authentication requests, clears the saved session, and attempts to revoke the current cloud session. It does not stop Core Server, terminals, agents, automation schedules, or tools.
- After first entry, logout does not cover the workspace. It blocks new user-initiated turns (including rewrites and queued user messages starting another turn) and new automation turns, but allows existing runs, steering, approvals and tools to continue. Clicking Sign in reuses the overlay without remounting the workspace. Background schedule denial records that occurrence as not executed without opening a login page; the schedule remains enabled for its next normal time.
- While a blocking login overlay is up (the startup gate or the re-login overlay), the whole workspace is force-hidden — keep-alive right-sidebar/bottom-panel pages and webview surfaces included — so nothing shows through it; descendant `visibility` resets are overridden, and the workspace returns to visible when the overlay closes.
- All accounts share the existing local database and model credentials. Login does not upload or migrate local conversations or settings.

## Session and profile data

`account-session.enc` under the existing Electron userData directory stores access/refresh tokens and their environment/region/account-API scope, encrypted through Electron safeStorage. Before restoring a session, the store rejects and removes unscoped or mismatched sessions, including its temporary companion. No token from another environment is submitted to the current authentication service. This only invalidates account credentials, not local chats or model settings. The SDK itself uses memory-only persistence. No password is persisted. Encryption failure never falls back to plaintext; a successful login can be memory-only, with a notice on the profile page. Logout storage failures are reported rather than hidden. Packaging privacy checks reject this file and its temporary companion.

Profile mapping: display name and avatar come from `data.profile.displayName` and `data.profile.avatarDataUrl`; email comes from the authenticated CloudBase user object. Accept inline PNG/JPEG/WebP avatars of at most 20,000 characters, otherwise use the built-in boat. The profile is read-only in the app; edits open the website.

The bottom-left account button displays only the avatar and a single-line username. The expanded menu header displays the avatar, username and email; a separate read-only “软件许可” row above Settings displays license validity. This row is not clickable or keyboard-focusable and never opens a website. A concrete expiry uses its Shanghai calendar date, and a verified allowed license with no expiry displays “长期有效”. Unknown/unavailable or signed-out states never imply an unlimited license. Email also remains available on the profile page.

An initial network outage retains saved credentials but does not bypass login. Transient runtime outages retain the last validated session; definite expiry or account deactivation blocks new turns without stopping existing work. The SDK adapter recognizes terminal symbolic errors (`unauthenticated`, `invalid_grant`, `user_blocked`) independently of numeric metadata, including rejected SDK promises, so definitive invalidation is not mistaken for a network outage. Account revalidation runs every five minutes and on foreground/profile refresh (foreground requests are throttled). License permission is process-local for at most 24 hours and must be freshly checked online after a cold restart or account change; saved login credentials still support automatic session restoration. This is not an always-online licensing/anti-tamper system.

Password-provider configuration failures are not reported as wrong passwords. The adapter distinguishes explicit invalid-credential messages from a disabled/unconfigured CloudBase password-login provider, which remains a safe `unknown` error. Email-code cooldown starts only after CloudBase accepts the send request: a failed request is immediately retryable, while an accepted request starts the 60-second local window. CloudBase may independently rate-limit requests.

Auth diagnostics log only bounded SDK code/category/status/request ID fields, never the SDK message, email, password, OTP or tokens. See [`CloudBaseAuthDriver.ts`](../../src/main/auth/CloudBaseAuthDriver.ts), [`AuthService.ts`](../../src/main/auth/AuthService.ts) and [`AccountLoginForm.tsx`](../../src/renderer/src/features/auth/AccountLoginForm.tsx).

## Verification

```sh
pnpm typecheck
pnpm exec vitest run --project unit src/main/core/accountAuth.test.ts src/main/core/accountAuthDriver.test.ts src/main/core/accountAuthIpc.test.ts
pnpm exec vitest run --project browser src/renderer/src/app/__tests__/AccountLogin.browser.test.tsx
node --test scripts/verify-packaged-privacy.test.mjs
pnpm build
node scripts/smoke-account-login.mjs
```

The Electron smoke script uses a new temporary data directory, captures both login modes, and closes the test app. It does not send email or use a real account. The website administrator has confirmed email OTP login is enabled. Before release, personally verify password login, OTP receipt/login, remembered login after restart, profile data, logout while a real task is running, and switching accounts without changing local data. A signed packaged-app check is still required for the actual macOS Keychain identity. No new DMG is produced by these implementation tests.

If CloudBase requires an additional interactive graphical captcha or MFA, this first version reports an additional-verification error instead of bypassing it or waiting forever for a browser callback. That flow requires a separate approved integration if enabled by the account service.

## Production connection

As of 2026-09-13, the desktop configuration matches the public login bundle served by `https://captainwhoagent.com/login`:

- Environment: `captainwho-prod-d7f1jv0p4d37c981`, region `ap-shanghai`.
- Account API: `https://captainwho-prod-d7f1jv0p4d37c981-1479807831.ap-shanghai.app.tcloudbase.com/account-api`.
- The production anonymous Publishable Key is included in the desktop public configuration; no website server credentials are copied.
- Registration, password reset and profile editing continue to use `https://captainwhoagent.com`.

Read-only live checks returned 200 for `/health` (production) and `/health/ready` (database reachable), and 401 `AUTHENTICATION_REQUIRED` for `/v1/me` without a token. These checks do not prove real-account password/OTP login or authenticated profile/avatar retrieval; those require manual verification in the production environment. Test accounts are not migrated by the desktop app. Restart the updated development build to connect to production and sign in again; already installed packages are unchanged until rebuilt and installed.
