# harharhar

```
 _               _               _
| |__   __ _ _ _| |__   __ _ _ _| |__   __ _ _ _
| '_ \ / _` | '_| '_ \ / _` | '_| '_ \ / _` | '_|
|_| |_|\__,_|_| |_| |_|\__,_|_| |_| |_|\__,_|_|
                                    laughs in CLI
```

---

> **WARNING: This tool captures and stores real authentication tokens, cookies, and session data from your live browser sessions.**

**This is not a toy.** harharhar is open-claw on steroids. It intercepts and persists live credentials from your actual logged-in browser sessions. If someone gets access to `~/.harharhar/`, they have your sessions. Full stop.

- `~/.harharhar/` contains **live credentials** that grant full access to your accounts — Gmail, Slack, Jira, whatever you've browsed through it
- Anyone with read access to that folder can **impersonate you** on any captured app, no passwords needed
- **Do NOT use on shared machines** without understanding exactly what's being stored
- **Do NOT commit `~/.harharhar/` to version control.** Ever. Add it to your global `.gitignore` now.
- This is like giving your AI agent your house keys — make sure you trust where those keys are stored
- Treat `~/.harharhar/` like your browser's cookie jar — because that's literally what it is

**If you wouldn't leave your browser logged in and unlocked, don't leave `~/.harharhar/` unprotected.**

---

Your AI agent, operating Gmail, Jira, and Slack via raw API calls — no API tokens needed. harharhar is a browser that captures all API traffic as you use web apps, then hands the sessions, cookies, and auth tokens to your AI agent so it can operate those apps via `curl`.

**The pitch:** "I can't get API token approval at work, but I'm already logged into these apps in my browser."

## How it works

1. **Browse.** Open harharhar and use web apps like normal. It's a real browser (Tauri v2 + WKWebView) with a spoofed Chrome user-agent.
2. **Capture.** Every API call, auth header, cookie, and CSRF token is automatically saved to `~/.harharhar/apps/{app-name}/`.
3. **Operate.** Your AI agent (Claude Code, etc.) reads the captured sessions and operates the app via `curl_chrome` — indistinguishable from your real browser.

```
You browsing Slack          harharhar captures traffic         AI agent sends curl
in harharhar                to ~/.harharhar/apps/slack/        using your session
    |                              |                                |
    v                              v                                v
 [Browser] ──capture──> [sessions/latest.json] ──read──> [curl_chrome ...]
                        [captures/*.jsonl]
                        [endpoints.json]
```

## Quick start

```bash
# Clone
git clone https://github.com/rick-shar/harharhar.git
cd harharhar

# Install curl-impersonate (mimics Chrome's TLS fingerprint)
brew tap shakacode/brew && brew install curl-impersonate

# Build and run
npm install
cargo tauri build --debug

# The binary lands in src-tauri/target/debug/harharhar
./src-tauri/target/debug/harharhar
```

First launch will create `~/.harharhar/` and ask you to paste your Chrome user-agent.

## What gets saved

```
~/.harharhar/
├── AGENT.md                  # Instructions for AI agents
├── config.json               # Your Chrome UA string
└── apps/
    └── gmail/                # One folder per app (you name them)
        ├── sessions/latest.json   # Live cookies + auth tokens
        ├── captures/*.jsonl       # Raw API traffic
        ├── endpoints.json         # Auto-detected endpoints
        └── auth.json              # Auto-detected auth patterns
```

Your AI agent reads `AGENT.md` to understand how to use the data, then reads `sessions/latest.json` to make authenticated requests.

## Built with

- [Tauri v2](https://v2.tauri.app/) + WKWebView
- [curl-impersonate](https://github.com/lexiforest/curl-impersonate) (`curl_chrome`) for TLS fingerprint matching
- Rust backend, vanilla JS frontend

## License

MIT
