# Guest-First Bilibili Session Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow public Bilibili lists and videos to load with the WebView2 guest session, prompting for in-app login only when Bilibili actually redirects to an authentication or verification page.

**Architecture:** Keep the existing persistent `bili-webview` User Data Folder as the only Cookie store. Rust classifies completed page URLs as `ready` or `verification-required`; a small TypeScript policy helper converts that state into user-facing guidance shared by import handling and the application notice.

**Tech Stack:** Rust 2021, Tauri 2.11.5, React 18, TypeScript, Vitest.

**Spec:** `docs/superpowers/specs/2026-08-26-bili-list-player-design.md`

## Global Constraints

- Do not read, import, decrypt, log, or persist Chrome Cookie values.
- Do not bypass Bilibili login, risk controls, access controls, or copyright restrictions.
- Keep the existing WebView2 data directory at `app_data_dir/bili-webview/`.
- Preserve the in-app login page as the fallback when Bilibili requires authentication or verification.
- Do not overwrite unrelated playback-progress work already present in the working tree.

---

### Task 1: Page Access State Contract

**Files:**
- Modify: `src-tauri/src/webview.rs`
- Create: `src/services/bilibiliPageState.ts`
- Create: `src/services/bilibiliPageState.test.ts`

**Interfaces:**
- Produces: Rust page-state values `ready` and `verification-required`.
- Produces: `getPageAccessErrorMessage(state: string): string | null`.

- [x] **Step 1: Write failing Rust and TypeScript tests**

Rust tests assert that a public list URL is `ready` and a Passport URL is `verification-required`.

TypeScript tests assert that `ready` and the legacy `guest` value do not block public access, while `verification-required` returns an actionable in-app verification message.

- [x] **Step 2: Run focused tests and verify RED**

Run:

```powershell
corepack pnpm vitest run src/services/bilibiliPageState.test.ts
Set-Location src-tauri; cargo test page_access_state
```

Expected: tests fail because the helper functions do not exist.

- [x] **Step 3: Implement the minimal classifiers**

Add a pure Rust `page_access_state` helper and use it when emitting `bilibili://page-state`.

Add the pure TypeScript policy helper without importing Tauri APIs.

- [x] **Step 4: Run focused tests and verify GREEN**

Run the two focused commands from Step 2 and require zero failures.

### Task 2: Guest-First Import and UI

**Files:**
- Modify: `src/services/parseService.ts`
- Modify: `src/App.tsx`
- Modify: `src/services/webviewStore.ts`

**Interfaces:**
- Consumes: `getPageAccessErrorMessage`.
- Preserves: `openBilibiliLogin()` as the fallback action.

- [x] **Step 1: Connect import handling to the tested policy**

Reject capture immediately only for `verification-required`. Change timeout text so it does not claim login is required for public pages.

- [x] **Step 2: Update application guidance**

Change the player copy to “公开内容无需登录，需要账号或验证时再登录” and rename the fallback action to “应用内登录”.

- [x] **Step 3: Run frontend tests and build**

Run:

```powershell
corepack pnpm test
corepack pnpm run build
```

Expected: all tests pass and TypeScript/Vite build exits successfully.

### Task 3: Requirements and Architecture Documentation

**Files:**
- Modify: `docs/index.md`
- Modify: `docs/explorations/2026-08-27-chrome-cookie-handoff.md`
- Modify: `docs/superpowers/specs/2026-08-26-bili-list-player-design.md`

**Interfaces:**
- Documents the implemented `ready` / `verification-required` state contract.
- Records Chrome Cookie handoff as rejected for the current product.

- [x] **Step 1: Update product requirements**

State that public content uses the persistent guest WebView2 session by default and login is requested only when Bilibili requires it.

- [x] **Step 2: Update the index and exploration decision**

Replace the proposed Chrome integration entry with the accepted guest-first route while retaining the rejected design as historical reasoning.

- [x] **Step 3: Run repository verification**

Run:

```powershell
corepack pnpm test
corepack pnpm run build
Set-Location src-tauri; cargo test --lib
git diff --check
```

Expected: all commands exit zero.
