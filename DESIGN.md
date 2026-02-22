# harharhar — Design: CLI Bridge + API Knowledge System

## Overview

harharhar is a two-part system:
1. **Browser** (Tauri + WKWebView) — You browse web apps. It auto-captures all API traffic, auth tokens, cookies, and sessions. Spoofs a real Chrome user-agent so sites behave normally and replayed requests match.
2. **AI Agent** (Claude Code / Auggie) — Reads the captured data from `~/.harharhar/apps/`, builds an API guide, and operates the app via raw `curl`. Only asks you to re-login when sessions expire.

The browser is the data collector. The AI is the brain. Files on disk are the bridge. **No special CLI needed for the AI** — it just reads files and runs curl.

## Data Store: `~/.harharhar/`

```
~/.harharhar/
├── AGENT.md                              # Prompt for AI agents (see below)
├── config.json                           # Global settings (chrome UA string, capture port)
│
└── apps/
    └── gmail/                            # Friendly name (user picks via browser prompt)
        │
        ├── config.json                   # App config
        │   {
        │     "domains": ["mail.google.com", "accounts.google.com"],
        │     "created": "2026-02-21T14:30:00Z",
        │     "last_session": "2026-02-21T15:00:00Z"
        │   }
        │
        │── AI-written knowledge (the "mental model") ──
        │
        ├── README.md                     # Overview: what this app does, high-level API behavior
        ├── auth.md                       # How auth works: login flow, token types, refresh patterns
        ├── endpoints.md                  # Endpoint reference: paths, methods, params, response shapes
        ├── examples.md                   # Verified working curl examples for common operations
        │
        │── Machine-generated from captures ──
        │
        ├── endpoints.json                # Auto-detected endpoint catalog (structured)
        ├── auth.json                     # Auto-detected auth patterns (structured)
        │
        │── Raw data ──
        │
        ├── sessions/
        │   └── latest.json              # Live: cookies, auth tokens, csrf tokens, user-agent
        │                                 # Auto-saved by browser as you browse
        │
        └── captures/
            ├── 2026-02-21T14-30.jsonl   # Raw API traffic from a browsing session
            └── 2026-02-21T16-00.jsonl   # Each session gets its own file
```

### File Details

**sessions/latest.json** — Auto-saved by browser whenever auth headers/cookies are seen:
```json
{
  "domain": "mail.google.com",
  "captured_at": "2026-02-21T14:30:00Z",
  "cookies": {
    "SID": "FgiA7g...",
    "HSID": "AZ5Cp..."
  },
  "auth_headers": {
    "Authorization": "SAPISIDHASH 1234_abcdef...",
    "X-Goog-AuthUser": "0"
  },
  "csrf_tokens": {
    "X-CSRF-Token": "abc123"
  },
  "user_agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
}
```

**endpoints.json** — Auto-built from captured traffic:
```json
{
  "endpoints": [
    {
      "pattern": "GET /mail/u/0/s/search",
      "observed_urls": ["https://mail.google.com/mail/u/0/s/search?q=..."],
      "query_params": ["q", "start", "num"],
      "response_content_type": "application/json",
      "response_shape_sample": {"threads": [{"id": "str", "snippet": "str"}]},
      "auth_required": true,
      "times_seen": 8,
      "last_seen": "2026-02-21T14:35:00Z"
    }
  ]
}
```

**auth.json** — Auto-detected from traffic patterns:
```json
{
  "mechanisms": [
    {
      "type": "cookie",
      "names": ["SID", "HSID", "SSID", "APISID", "SAPISID"],
      "domain": ".google.com"
    },
    {
      "type": "header",
      "name": "Authorization",
      "pattern": "SAPISIDHASH {timestamp}_{hash}"
    }
  ],
  "login_url": "https://accounts.google.com/signin",
  "observed_refresh_endpoints": [],
  "session_duration_estimate": "unknown"
}
```

**captures/{timestamp}.jsonl** — One line per request:
```jsonl
{"type":"fetch","method":"GET","url":"https://mail.google.com/mail/u/0/...","requestHeaders":{...},"requestBody":null,"status":200,"responseHeaders":{...},"responseBody":"{...}","duration":142,"timestamp":"2026-02-21T14:30:12Z"}
```

## AGENT.md — The Prompt

Lives at `~/.harharhar/AGENT.md`. Referenced from project CLAUDE.md. Tells the AI agent how to use the data.

```markdown
# harharhar — Instructions for AI Agents

You have access to web app API data captured by harharhar at `~/.harharhar/apps/`.

## App Folders

Each folder in `apps/` is a web app (e.g., `gmail/`, `slack/`, `jira/`).

Inside each app folder:
- `config.json` — domains this app uses, timestamps
- `README.md` — Overview you wrote. Read this first.
- `auth.md` — Auth guide you wrote. How to authenticate.
- `endpoints.md` — Endpoint reference you wrote.
- `examples.md` — Working curl examples you verified.
- `endpoints.json` — Auto-detected endpoints (machine-generated from captures)
- `auth.json` — Auto-detected auth patterns (machine-generated from captures)
- `sessions/latest.json` — Current session: cookies, auth tokens, user-agent
- `captures/` — Raw JSONL of every API call observed

## When asked to do something with a web app:

1. **Find the app folder.** If it doesn't exist:
   → "I don't have data for this app. Run `harharhar` and browse it so I can learn the API."

2. **Check the session.** Read `sessions/latest.json`. If `captured_at` is old (> 1 hour):
   → "Session might be expired. Run `harharhar` and log into {app} again."

3. **Check if you know how.** Read `README.md`, `endpoints.md`, `auth.md`.
   - If you have what you need → construct a curl command using cookies/tokens from `sessions/latest.json`
   - If you don't → read `captures/*.jsonl` to learn new endpoints, then update the .md files

4. **If a request returns 401/403:**
   → "Session expired. Run `harharhar` and log into {app} again."

5. **Always update the knowledge files** when you learn something new.

## Making authenticated requests:

Use `curl_chrome` (from curl-impersonate) instead of regular `curl`.
It mimics Chrome's exact TLS fingerprint and HTTP/2 settings so the
server can't distinguish it from a real browser at any level.

Install: `brew tap shakacode/brew && brew install curl-impersonate`

```bash
curl_chrome 'https://mail.google.com/mail/u/0/endpoint' \
  -H 'Cookie: SID=...; HSID=...' \
  -H 'Authorization: SAPISIDHASH ...' \
  -H 'User-Agent: Mozilla/5.0 (Macintosh; ...) Chrome/144.0.0.0 ...' \
  -H 'Content-Type: application/json'
```

Always replay ALL headers from the captured request, not just auth.
Use the exact user-agent from the session file.

## Explore → Learn → Update cycle:

This is the core loop. Every interaction with the app should make the model better.

**When the user browses** (or you drive the browser), new captures appear in `captures/`.
You MUST read them and update the knowledge files:

1. **Read new captures** — check `captures/` for entries you haven't processed
2. **Update endpoints.md** — new endpoint? Add it with method, path, params, response shape
3. **Update auth.md** — saw a token refresh? New cookie? Document the pattern
4. **Update examples.md** — write a working `curl_chrome` example for every new endpoint
5. **Update README.md** — if your understanding of the app changed, reflect it

The knowledge files are your memory. If you don't write it down, you'll forget
it next conversation. Be thorough — include headers, params, response shapes,
error codes, and gotchas.

**Driving the browser to explore:**

If harharhar is running, you can control it to discover new endpoints:

```bash
# See what's on the page
curl_chrome -s -X POST http://127.0.0.1:34512/command -d '{"action":"read_page"}'

# Click something to see what API call it triggers
curl_chrome -s -X POST http://127.0.0.1:34512/command -d '{"action":"click","selector":"#some-btn"}'

# Navigate somewhere
curl_chrome -s -X POST http://127.0.0.1:34512/command -d '{"action":"navigate","url":"https://..."}'
```

Then ALWAYS read the new captures and update the knowledge files.
Never explore without writing back what you learned.
```

## Chrome User-Agent

The UA must match your real Chrome exactly. Chrome's UA format changes over time (e.g., they now zero out the patch version: `Chrome/144.0.0.0`), so rather than guessing the format, we copy it from the source of truth.

**Setup (during `harharhar init` or first launch):**
1. Prompt: "Paste your Chrome user-agent (open Chrome → DevTools console → `navigator.userAgent`):"
2. Save the exact string to `~/.harharhar/config.json` as `user_agent`
3. If skipped, fall back to a hardcoded recent Chrome UA

**Or**: Button in the harharhar browser UI — "Import UA from clipboard". Copy from Chrome, click, done.

**When Chrome updates**: Re-paste. The UA lives in config and is read on each launch.

```json
// ~/.harharhar/config.json
{
  "user_agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36"
}
```

**Why this matters:**
- Exact match to your real Chrome — no detection, no fingerprint mismatch
- Auth tokens issued for this UA — curl with the same UA works
- Saved in `sessions/latest.json` so the AI always sends the matching one
- Future-proof: we don't guess the format, we copy the real thing

## Browser Changes

### Chrome UA on browser window
Set `.user_agent(CHROME_UA)` when building the WebviewWindow in lib.rs.

### App naming prompt
First time a new domain is seen in captures:
- Explorer panel shows: "New app detected: mail.google.com — Name this app:" with a text field
- Suggested default: domain minus TLD (e.g., "mail.google")
- User types "gmail" → creates `~/.harharhar/apps/gmail/`

### Auto-save to disk (no manual export)
As you browse, everything is saved automatically:
- Every captured request → appended to `captures/{session-timestamp}.jsonl`
- Auth headers detected → `sessions/latest.json` updated
- New endpoint patterns detected → `endpoints.json` updated
- Auth patterns detected → `auth.json` updated

### Cookie capture
intercept.js captures request/response headers. Additionally:
- Capture `document.cookie` on each page load → send as special entry
- This catches httpOnly cookies visible to the document

## Browser Remote Control (CLI → Browser)

The AI can drive the browser by POSTing to the capture server (`localhost:34512/command`). This lets it explore the app, trigger actions, and observe what API calls fire — all without the user touching the browser.

**Commands:**
```bash
# Navigate to a URL
curl_chrome -X POST http://127.0.0.1:34512/command \
  -d '{"action": "navigate", "url": "https://app.com/settings"}'

# Click an element
curl_chrome -X POST http://127.0.0.1:34512/command \
  -d '{"action": "click", "selector": "#delete-btn"}'

# Type into a field
curl_chrome -X POST http://127.0.0.1:34512/command \
  -d '{"action": "type", "selector": "#search-input", "value": "test query"}'

# Run arbitrary JavaScript (returns result)
curl_chrome -X POST http://127.0.0.1:34512/command \
  -d '{"action": "eval", "js": "document.title"}'

# Read page content (HTML or text, for AI to understand the page)
curl_chrome -X POST http://127.0.0.1:34512/command \
  -d '{"action": "read_page"}'

# Scroll
curl_chrome -X POST http://127.0.0.1:34512/command \
  -d '{"action": "scroll", "direction": "down", "amount": 500}'
```

**How the AI uses this:**
1. AI needs to discover the "delete project" endpoint
2. POSTs `read_page` → sees the DOM, finds the delete button selector
3. POSTs `click` on the delete button
4. A confirmation dialog appears → POSTs `click` on confirm
5. The API call fires → auto-captured in the app's captures folder
6. AI reads the new capture → now knows `DELETE /api/projects/{id}`
7. Updates endpoints.md and examples.md

**Implementation:** The capture server's TCP handler routes `/command` POSTs to Rust, which calls `webview.eval(...)` on the browser window and returns the result as JSON. Any API calls triggered by the action are captured through the normal intercept.js flow.

## CLI — Minimal

The `harharhar` binary only does:
```
harharhar          # Launch browser GUI (the main use)
harharhar init     # Create ~/.harharhar/ + AGENT.md (first-time setup)
```

**Everything else is just the AI reading files and running `curl_chrome`.** No clap, no reqwest, no subcommands. The AI agent reads `sessions/latest.json`, constructs `curl_chrome` commands (from curl-impersonate — mimics Chrome's TLS/HTTP2 fingerprint), and runs them via bash. Indistinguishable from a real browser at every level: UA, headers, TLS, HTTP/2.

**Setup dependency:** `brew tap shakacode/brew && brew install curl-impersonate` (gives you `curl_chrome` binary).

## Implementation Steps

### Step 1: UA config + data dir setup
**Files:** `src-tauri/src/lib.rs`
- On startup: create `~/.harharhar/` if missing, read `config.json`
- If `user_agent` not set: first-launch prompt in explorer UI to paste from Chrome
- Pass UA to `.user_agent(...)` on WebviewWindowBuilder
- Also add "Import UA from clipboard" button in explorer UI
- Fallback to hardcoded recent Chrome UA if not configured

### Step 2: Data dir + auto-save captures to disk
**Files:** `src-tauri/src/capture.rs`, `src-tauri/src/lib.rs`
- On app startup, create `~/.harharhar/` structure
- In capture.rs: on each request, extract domain, resolve to app folder, append JSONL
- Extract auth headers/cookies from captures, write `sessions/latest.json`
- Add `chrono` + `dirs` deps to Cargo.toml

### Step 3: Cookie capture from document
**Files:** `inject/intercept.js`
- On page load, capture `document.cookie` and send as a special capture entry
- This supplements the request/response header cookies

### Step 4: App naming prompt
**Files:** `ui/index.html`, `ui/main.js`, `src-tauri/src/lib.rs`
- When capture.rs sees a domain not mapped to any app, emit event to explorer UI
- Explorer shows inline prompt: "Name this app:" with text field
- On submit, Tauri command creates app folder + config.json

### Step 5: Auto-generate endpoints.json + auth.json
**Files:** `src-tauri/src/endpoints.rs` (new), `src-tauri/src/session.rs` (new)
- Parse captured URLs into endpoint patterns
- Detect auth mechanisms from header patterns
- Write to app folder periodically and on shutdown

### Step 6: AGENT.md + init command
**Files:** `src-tauri/src/main.rs`
- Simple arg check: if `args[1] == "init"` → create ~/.harharhar/ + write AGENT.md
- No clap needed — just match on the string

### Step 7 (later): HAR export
**Files:** `src-tauri/src/har.rs` (new)
- Convert JSONL captures to HAR 1.2 format
- Triggered via browser UI button or `harharhar export --har`
