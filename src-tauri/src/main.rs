#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::path::PathBuf;

const AGENT_MD: &str = r##"# harharhar — Instructions for AI Agents

You have access to web app API data captured by harharhar at `~/.harharhar/apps/`.

## App Folders

Each folder in `apps/` is a web app (e.g., `gmail/`, `slack/`, `discord/`).

Inside each app folder:
- `config.json` — domains this app uses, timestamps
- `README.md` — Overview you wrote. Read this first.
- `auth.md` — Auth guide you wrote. How to authenticate.
- `endpoints.md` — Endpoint reference you wrote.
- `examples.md` — Working curl_chrome examples you verified.
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
   - If you have what you need → construct a `curl_chrome` command using cookies/tokens from `sessions/latest.json`
   - If you don't → read `captures/*.jsonl` to learn new endpoints, then update the .md files

4. **If a request returns 401/403:**
   → "Session expired. Run `harharhar` and log into {app} again."

5. **Always update the knowledge files** when you learn something new.

## Making authenticated requests:

Use `curl_chrome` (from curl-impersonate) instead of regular `curl`.
It mimics Chrome's exact TLS fingerprint and HTTP/2 settings so the
server can't distinguish it from a real browser at any level.

Read `sessions/latest.json` and construct curl like:

```bash
curl_chrome 'https://example.com/api/endpoint' \
  -H 'Cookie: session=...; token=...' \
  -H 'Authorization: Bearer ...' \
  -H 'User-Agent: <exact UA from session file>' \
  -H 'Content-Type: application/json'
```

Always replay ALL headers from the captured request, not just auth.
Use the exact user-agent from the session file.

## Explore → Learn → Update cycle:

This is the core loop. Every interaction with the app should make the model better.

When the user browses (or you drive the browser), new captures appear in `captures/`.
You MUST read them and update the knowledge files:

1. **Read new captures** — check `captures/` for entries you haven't processed
2. **Update endpoints.md** — new endpoint? Add it with method, path, params, response shape
3. **Update auth.md** — saw a token refresh? New cookie? Document the pattern
4. **Update examples.md** — write a working `curl_chrome` example for every new endpoint
5. **Update README.md** — if your understanding of the app changed, reflect it

The knowledge files are your memory. If you don't write it down, you'll forget
it next conversation. Be thorough — include headers, params, response shapes,
error codes, and gotchas.

## Driving the browser to explore:

If harharhar is running, use `harharhar cmd` to control it:

```bash
# Check status
harharhar cmd '{"action":"status"}'

# Navigate somewhere
harharhar cmd '{"action":"navigate","url":"https://..."}'

# Click an element
harharhar cmd '{"action":"click","selector":"#some-btn"}'

# Type into a field
harharhar cmd '{"action":"type","selector":"#search","value":"test"}'

# Scroll down
harharhar cmd '{"action":"scroll","direction":"down","amount":500}'

# Run arbitrary JS
harharhar cmd '{"action":"eval","js":"document.title"}'

# Get page HTML
harharhar cmd '{"action":"read_page"}'

# Trigger endpoint/auth analysis
harharhar cmd '{"action":"generate_endpoints"}'
```

Then ALWAYS read the new captures and update the knowledge files.
Never explore without writing back what you learned.
"##;

fn data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".harharhar")
}

fn run_init() {
    let root = data_dir();
    let _ = fs::create_dir_all(root.join("apps"));
    let _ = fs::create_dir_all(root.join("incoming"));

    // Write AGENT.md
    let agent_path = root.join("AGENT.md");
    fs::write(&agent_path, AGENT_MD).expect("failed to write AGENT.md");
    println!("Created {}", agent_path.display());

    // Write config.json if it doesn't exist
    let config_path = root.join("config.json");
    if !config_path.exists() {
        fs::write(
            &config_path,
            r#"{
  "user_agent": null,
  "capture_port": null
}"#,
        )
        .expect("failed to write config.json");
        println!("Created {}", config_path.display());
    }

    println!("\nharharhar initialized at {}", root.display());
    println!("\nNext steps:");
    println!("  1. Add this to your project's CLAUDE.md:");
    println!("     See ~/.harharhar/AGENT.md for web app API interaction instructions.");
    println!("  2. Run `harharhar` to launch the browser");
    println!("  3. Browse a web app and sign in");
    println!("  4. Install curl-impersonate: brew tap shakacode/brew && brew install curl-impersonate");
}

fn run_cmd(body: &str) {
    let root = data_dir();
    let cmd_path = root.join("cmd.json");
    let result_path = root.join("cmd-result.json");

    // Clean up stale result
    let _ = fs::remove_file(&result_path);

    // Write command
    fs::write(&cmd_path, body).expect("failed to write command");

    // Wait for result (up to 10 seconds)
    for _ in 0..100 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if result_path.exists() {
            match fs::read_to_string(&result_path) {
                Ok(result) => {
                    println!("{result}");
                    let _ = fs::remove_file(&result_path);
                    return;
                }
                Err(_) => continue,
            }
        }
    }
    eprintln!("Timeout waiting for response. Is harharhar running?");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        match args[1].as_str() {
            "init" => {
                run_init();
                return;
            }
            "cmd" => {
                let body = args.get(2).map(|s| s.as_str()).unwrap_or("{}");
                run_cmd(body);
                return;
            }
            "generate" => {
                let root = data_dir().join("apps");
                if let Ok(entries) = fs::read_dir(&root) {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() {
                            if let Some(name) = entry.file_name().to_str() {
                                println!("Generating endpoints for {}...", name);
                                harharhar_lib::endpoints::generate_for_app(name);
                                // Trim bodies in old captures (no active session, so trim all)
                                harharhar_lib::cleanup::trim_captures_for_app(name, "");
                            }
                        }
                    }
                }
                println!("Done.");
                return;
            }
            "--help" | "-h" | "help" => {
                println!("harharhar - API exploration browser\n");
                println!("Usage:");
                println!("  harharhar                Launch browser GUI");
                println!("  harharhar init           Create ~/.harharhar/ and AGENT.md");
                println!("  harharhar cmd '<json>'   Send command to running browser");
                println!("  harharhar generate       Generate endpoints.json + auth.json for all apps");
                println!("  harharhar help           Show this help");
                println!("\nExamples:");
                println!("  harharhar cmd '{{\"action\":\"status\"}}'");
                println!("  harharhar cmd '{{\"action\":\"navigate\",\"url\":\"https://gmail.com\"}}'");
                return;
            }
            other => {
                eprintln!("Unknown command: {other}");
                eprintln!("Run `harharhar help` for usage.");
                std::process::exit(1);
            }
        }
    }

    harharhar_lib::run();
}
