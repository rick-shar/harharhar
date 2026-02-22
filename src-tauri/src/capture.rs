use crate::config;
use crate::endpoints;
use crate::AppState;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{Emitter, Manager};

const AUTH_HEADERS: &[&str] = &[
    "authorization",
    "x-csrf-token",
    "x-xsrf-token",
    "x-requested-with",
];

const COOKIE_HEADERS: &[&str] = &["cookie"];

/// Counter to trigger periodic endpoint generation
static CAPTURE_COUNT: AtomicU32 = AtomicU32::new(0);

// --- Process a single capture entry (called from Tauri IPC command) ---

pub fn process_single(app: &tauri::AppHandle, data: &serde_json::Value, session_ts: &str) {
    save_capture(app, data, session_ts);

    let count = CAPTURE_COUNT.fetch_add(1, Ordering::Relaxed);
    if count > 0 && count % 50 == 0 {
        generate_all_endpoints();
    }
}

// --- File-based command watcher ---

pub async fn start_command_watcher(app: tauri::AppHandle) {
    let cmd_path = config::data_dir().join("cmd.json");
    let result_path = config::data_dir().join("cmd-result.json");

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        if !cmd_path.exists() {
            continue;
        }

        let body = match fs::read_to_string(&cmd_path) {
            Ok(b) => b,
            Err(_) => continue,
        };

        // Delete command file immediately so we don't re-process
        let _ = fs::remove_file(&cmd_path);

        let result = handle_command(&app, &body);
        let _ = fs::write(&result_path, &result);
    }
}

/// Handle a command from the CLI (via file)
fn handle_command(app: &tauri::AppHandle, body: &str) -> String {
    let cmd: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return serde_json::json!({"error": e.to_string()}).to_string(),
    };

    let action = cmd.get("action").and_then(|v| v.as_str()).unwrap_or("");

    match action {
        "navigate" => {
            let url = cmd.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                return r#"{"error":"missing url"}"#.to_string();
            }
            let wv = app.get_webview_window("browser");
            match wv {
                Some(wv) => {
                    let js = format!(
                        "window.location.href={}",
                        serde_json::to_string(url).unwrap()
                    );
                    match wv.eval(&js) {
                        Ok(_) => r#"{"ok":true}"#.to_string(),
                        Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
                    }
                }
                None => r#"{"error":"browser window not open"}"#.to_string(),
            }
        }

        "click" => {
            let selector = cmd.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            exec_js_with_result(app, &format!(
                "(() => {{ const el = document.querySelector({}); if(el) {{ el.click(); return 'clicked'; }} else {{ return 'not found'; }} }})()",
                serde_json::to_string(selector).unwrap()
            ))
        }

        "type" => {
            let selector = cmd.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            let value = cmd.get("value").and_then(|v| v.as_str()).unwrap_or("");
            exec_js_with_result(app, &format!(
                "(() => {{ const el = document.querySelector({}); if(el) {{ el.focus(); el.value = {}; el.dispatchEvent(new Event('input', {{bubbles:true}})); return 'typed'; }} else {{ return 'not found'; }} }})()",
                serde_json::to_string(selector).unwrap(),
                serde_json::to_string(value).unwrap()
            ))
        }

        "scroll" => {
            let amount = cmd.get("amount").and_then(|v| v.as_i64()).unwrap_or(500);
            let direction = cmd.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
            let y = if direction == "up" { -amount } else { amount };
            exec_js_with_result(app, &format!("window.scrollBy(0, {y}); 'scrolled'"))
        }

        "eval" => {
            let js = cmd.get("js").and_then(|v| v.as_str()).unwrap_or("");
            exec_js_with_result(app, js)
        }

        "read_page" => {
            exec_js_with_result(app, "document.documentElement.outerHTML.substring(0, 500000)")
        }

        "get_cookies" => {
            let url = cmd.get("url").and_then(|v| v.as_str()).unwrap_or("https://mail.google.com");
            get_browser_cookies(app, url)
        }

        "status" => {
            let result = serde_json::json!({
                "browser_open": app.get_webview_window("browser").is_some(),
                "apps": config::list_apps(),
            });
            result.to_string()
        }

        "ws_send" => {
            let message = cmd.get("message").and_then(|v| v.as_str()).unwrap_or("");
            let index = cmd.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            exec_js_with_result(app, &format!(
                "(() => {{ const sockets = window.__hh_ws || []; const ws = sockets.filter(s => s.readyState === 1)[{}]; if(ws) {{ ws.send({}); return 'sent'; }} else {{ return 'no open websocket'; }} }})()",
                index,
                serde_json::to_string(message).unwrap()
            ))
        }

        "ws_list" => {
            exec_js_with_result(app, "(() => { const sockets = window.__hh_ws || []; return JSON.stringify(sockets.map((s,i) => ({index:i, url:s.url, state:['CONNECTING','OPEN','CLOSING','CLOSED'][s.readyState]}))); })()")
        }

        "generate_endpoints" => {
            generate_all_endpoints();
            r#"{"ok":true}"#.to_string()
        }

        _ => {
            serde_json::json!({"error": format!("unknown action: {}", action)}).to_string()
        }
    }
}

fn exec_js_with_result(app: &tauri::AppHandle, js: &str) -> String {
    match crate::eval_js_with_result(app, js) {
        Ok(result) => serde_json::json!({"ok": true, "result": result}).to_string(),
        Err(e) => serde_json::json!({"error": e}).to_string(),
    }
}

fn get_browser_cookies(app: &tauri::AppHandle, _url: &str) -> String {
    exec_js_with_result(app, "document.cookie")
}

fn generate_all_endpoints() {
    for app_name in config::list_apps() {
        endpoints::generate_for_app(&app_name);
    }
}

// --- Capture saving ---

fn save_capture(app: &tauri::AppHandle, data: &serde_json::Value, session_ts: &str) {
    let url_str = match data.get("url").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return,
    };

    let domain = match url::Url::parse(url_str) {
        Ok(u) => match u.host_str() {
            Some(h) => h.to_string(),
            None => return,
        },
        Err(_) => return,
    };

    let state = app.state::<AppState>();
    let app_name = {
        let map = state.domain_map.lock().unwrap();
        map.get(&domain).cloned()
    };

    match app_name {
        Some(name) => {
            config::ensure_app_dirs(&name);
            append_capture(&name, data, session_ts);
            update_session(app, &name, &domain, data);
        }
        None => {
            // If browser is open for a known app, auto-add this domain to it
            let current = app.state::<AppState>().current_app.lock().unwrap().clone();

            if let Some(ref name) = current {
                // Auto-add domain to the current app
                config::add_domain_to_app(name, &domain);
                {
                    let state = app.state::<AppState>();
                    state.domain_map.lock().unwrap().insert(domain.clone(), name.clone());
                }
                config::ensure_app_dirs(name);
                append_capture(name, data, session_ts);
                update_session(app, name, &domain, data);
            } else {
                // No active app — buffer and ask the user
                let state = app.state::<AppState>();
                let mut buf = state.unmapped_captures.lock().unwrap();
                let entries = buf.entry(domain.clone()).or_insert_with(Vec::new);
                entries.push(data.clone());
                let _ = app.emit("unknown-domain", &domain);
            }
        }
    }
}

/// Flush buffered captures for a domain that was just mapped to an app
pub fn flush_unmapped(app: &tauri::AppHandle, domain: &str, app_name: &str, session_ts: &str) {
    let state = app.state::<AppState>();
    let entries = {
        let mut buf = state.unmapped_captures.lock().unwrap();
        buf.remove(domain).unwrap_or_default()
    };

    if entries.is_empty() {
        return;
    }

    config::ensure_app_dirs(app_name);
    for data in &entries {
        append_capture(app_name, data, session_ts);
        update_session(app, app_name, domain, data);
    }
}

fn append_capture(app_name: &str, data: &serde_json::Value, session_ts: &str) {
    let captures_dir = config::data_dir()
        .join("apps")
        .join(app_name)
        .join("captures");
    let file_path = captures_dir.join(format!("{session_ts}.jsonl"));

    let line = match serde_json::to_string(data) {
        Ok(l) => l,
        Err(_) => return,
    };

    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
    {
        let _ = writeln!(file, "{line}");
    }
}

fn update_session(
    app: &tauri::AppHandle,
    app_name: &str,
    domain: &str,
    data: &serde_json::Value,
) {
    let req_headers = match data.get("requestHeaders").and_then(|v| v.as_object()) {
        Some(h) => h,
        None => return,
    };

    let has_auth = req_headers.keys().any(|k| {
        let lower = k.to_lowercase();
        AUTH_HEADERS.contains(&lower.as_str()) || COOKIE_HEADERS.contains(&lower.as_str())
    });

    if !has_auth {
        return;
    }

    let state = app.state::<AppState>();
    let _lock = state.session_file_lock.lock().unwrap();

    let session_path = config::data_dir()
        .join("apps")
        .join(app_name)
        .join("sessions")
        .join("latest.json");

    let mut session: config::SessionData = fs::read_to_string(&session_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    session.domain = domain.to_string();
    session.captured_at = chrono::Utc::now().to_rfc3339();
    session.user_agent = state.curl_ua.clone();

    for (k, v) in req_headers {
        let lower = k.to_lowercase();
        if COOKIE_HEADERS.contains(&lower.as_str()) {
            if let Some(cookie_str) = v.as_str() {
                for part in cookie_str.split(';') {
                    let trimmed = part.trim();
                    if let Some(eq) = trimmed.find('=') {
                        let name = trimmed[..eq].trim().to_string();
                        let value = trimmed[eq + 1..].trim().to_string();
                        session.cookies.insert(name, value);
                    }
                }
            }
        }
    }

    for (k, v) in req_headers {
        let lower = k.to_lowercase();
        if AUTH_HEADERS.contains(&lower.as_str()) {
            if let Some(val) = v.as_str() {
                session.auth_headers.insert(k.clone(), val.to_string());
            }
        }
    }

    if let Some(resp_headers) = data.get("responseHeaders").and_then(|v| v.as_object()) {
        for (k, v) in resp_headers {
            let lower = k.to_lowercase();
            if lower.contains("csrf") || lower.contains("xsrf") {
                if let Some(val) = v.as_str() {
                    session.csrf_tokens.insert(k.clone(), val.to_string());
                }
            }
        }
    }

    if let Ok(json) = serde_json::to_string_pretty(&session) {
        let _ = fs::write(&session_path, json);
    }
}
