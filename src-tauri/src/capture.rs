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

/// JS that builds a lean accessibility-tree-like UI model.
/// Stores element refs in window.__hh_refs for click_ref/type_ref.
const READ_UI_JS: &str = r#"(() => {
  const refs = [];
  window.__hh_refs = refs;
  const lines = [];

  function isVis(el) {
    if (el.checkVisibility) return el.checkVisibility();
    if (el.offsetParent === null && el.tagName !== 'BODY' && el.tagName !== 'HTML') return false;
    var s = getComputedStyle(el);
    return s.display !== 'none' && s.visibility !== 'hidden';
  }

  function role(el) {
    var ar = el.getAttribute('role');
    if (ar) return ar;
    var t = el.tagName.toLowerCase();
    var ty = (el.getAttribute('type') || '').toLowerCase();
    switch(t) {
      case 'a': return el.href ? 'link' : null;
      case 'button': return 'button';
      case 'input':
        if (ty === 'submit' || ty === 'button') return 'button';
        if (ty === 'checkbox') return 'checkbox';
        if (ty === 'radio') return 'radio';
        if (ty === 'hidden') return null;
        return 'input[' + (ty || 'text') + ']';
      case 'select': return 'select';
      case 'textarea': return 'textarea';
      case 'img': return 'img';
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6': return t;
      case 'nav': return 'nav';
      case 'main': return 'main';
      case 'aside': return 'aside';
      case 'header': return 'banner';
      case 'footer': return 'footer';
      case 'form': return 'form';
      case 'table': return 'table';
      case 'tr': return 'row';
      case 'td': case 'th': return 'cell';
      case 'ul': case 'ol': return 'list';
      case 'li': return 'listitem';
      case 'dialog': return 'dialog';
      case 'summary': return 'summary';
      default:
        if (el.onclick || el.getAttribute('tabindex') === '0') return 'clickable';
        return null;
    }
  }

  function label(el) {
    var al = el.getAttribute('aria-label');
    if (al) return al.trim().substring(0, 80);
    var t = el.tagName;
    if (t === 'INPUT' || t === 'TEXTAREA') {
      if (el.placeholder) return el.placeholder.substring(0, 80);
      if (el.title) return el.title.substring(0, 80);
      if (el.id) { var lb = document.querySelector('label[for="' + el.id + '"]'); if (lb) return lb.textContent.trim().substring(0, 80); }
    }
    if (t === 'IMG') return (el.alt || el.title || '').substring(0, 80);
    var txt = '';
    for (var i = 0; i < el.childNodes.length; i++) {
      if (el.childNodes[i].nodeType === 3) txt += el.childNodes[i].textContent;
    }
    txt = txt.trim();
    if (txt) return txt.substring(0, 80);
    if (el.children.length <= 2) {
      var inner = (el.innerText || '').trim();
      if (inner && inner.length < 120) return inner.substring(0, 80);
    }
    return '';
  }

  function state(el) {
    var s = [];
    if (el.disabled) s.push('disabled');
    if (el.checked) s.push('checked');
    if (el.getAttribute('aria-selected') === 'true') s.push('selected');
    if (el.getAttribute('aria-expanded') === 'true') s.push('expanded');
    if (el.getAttribute('aria-expanded') === 'false') s.push('collapsed');
    if (el.getAttribute('aria-current')) s.push('current');
    if ((el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') && el.value) s.push('val="' + el.value.substring(0, 50) + '"');
    if (el.tagName === 'SELECT' && el.selectedOptions.length) s.push('val="' + el.selectedOptions[0].text.substring(0, 50) + '"');
    return s.join(' ');
  }

  function walk(el, depth) {
    if (depth > 12 || !isVis(el)) return;
    var r = role(el);
    if (r) {
      var idx = refs.length;
      refs.push(el);
      var lb = label(el);
      var st = state(el);
      var indent = '';
      for (var i = 0; i < Math.min(depth, 8); i++) indent += '  ';
      var line = indent + '[' + idx + '] ' + r;
      if (lb) line += ' "' + lb.replace(/"/g, '\\"') + '"';
      if (st) line += ' ' + st;
      lines.push(line);
    }
    for (var c = 0; c < el.children.length; c++) {
      walk(el.children[c], depth + (r ? 1 : 0));
    }
  }

  walk(document.body, 0);
  if (lines.length > 500) {
    lines.length = 500;
    lines.push('... truncated (' + refs.length + ' total refs)');
  }
  return 'Page: ' + document.title + '\nURL: ' + location.href + '\n' + refs.length + ' elements\n---\n' + lines.join('\n');
})()"#;

// --- Noise filtering ---

/// Known analytics/tracking domains that have zero value for AI agents learning APIs.
const NOISE_DOMAINS: &[&str] = &[
    "google-analytics.com",
    "doubleclick.net",
    "googletagmanager.com",
    "facebook.com/tr",
    "play.google.com/log",
    "accounts.google.com/ListAccounts",
    "apis.google.com/js/",
    "www.gstatic.com",
    "fonts.googleapis.com",
    "fonts.gstatic.com",
    "ssl.gstatic.com",
];

/// Static asset extensions that carry no API information.
const NOISE_EXTENSIONS: &[&str] = &[
    ".js", ".css", ".png", ".jpg", ".gif", ".svg", ".woff", ".woff2", ".ico",
];

/// URL path fragments indicating tracking/telemetry requests.
const NOISE_PATH_PATTERNS: &[&str] = &[
    "/log?", "/beacon", "/pixel", "/analytics", "/telemetry", "/collect?",
];

/// Returns true if the URL matches known noise patterns and should be skipped.
pub fn should_skip_capture(url: &str) -> bool {
    let url_lower = url.to_lowercase();

    // Check noise domains / domain-path prefixes
    for pattern in NOISE_DOMAINS {
        if url_lower.contains(pattern) {
            return true;
        }
    }

    // Check static asset extensions — match against the path portion only
    // (strip query string first so ".js?v=123" still matches ".js")
    let path_part = url_lower.split('?').next().unwrap_or(&url_lower);
    for ext in NOISE_EXTENSIONS {
        if path_part.ends_with(ext) {
            return true;
        }
    }

    // Check tracking/telemetry path patterns
    for pattern in NOISE_PATH_PATTERNS {
        if url_lower.contains(pattern) {
            return true;
        }
    }

    false
}

// --- Process a single capture entry (called from Tauri IPC command) ---

pub fn process_single(app: &tauri::AppHandle, data: &serde_json::Value, session_ts: &str) {
    save_capture(app, data, session_ts);

    let count = CAPTURE_COUNT.fetch_add(1, Ordering::Relaxed);
    if count > 0 && count % 50 == 0 {
        generate_all_endpoints(session_ts);
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
            let mut raw = url.to_string();
            if !raw.starts_with("http") {
                raw = format!("https://{raw}");
            }
            match url::Url::parse(&raw) {
                Ok(parsed) => {
                    // Set current_app if domain is known
                    let domain = parsed.host_str().unwrap_or("").to_string();
                    {
                        let state = app.state::<crate::AppState>();
                        let map = state.domain_map.lock().unwrap();
                        if let Some(name) = map.get(&domain) {
                            let mut current = state.current_app.lock().unwrap();
                            *current = Some(name.clone());
                        }
                    }
                    match crate::open_browser(app, parsed) {
                        Ok(_) => r#"{"ok":true}"#.to_string(),
                        Err(e) => serde_json::json!({"error": e}).to_string(),
                    }
                }
                Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
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

        "read_ui" => {
            exec_js_with_result(app, READ_UI_JS)
        }

        "click_ref" => {
            let ref_id = cmd.get("ref").and_then(|v| v.as_u64()).unwrap_or(0);
            let result = exec_js_with_result(app, &format!(
                "(() => {{ const refs = window.__hh_refs || []; const el = refs[{}]; if(!el) return JSON.stringify({{ok:false,err:'ref not found'}}); var role = el.getAttribute('role') || el.tagName.toLowerCase(); var label = (el.getAttribute('aria-label') || el.innerText || '').substring(0,80).trim(); el.scrollIntoView({{block:'center'}}); el.click(); return JSON.stringify({{ok:true,role:role,label:label,url:location.href}}); }})()",
                ref_id
            ));
            log_ui_action(app, "click_ref", ref_id, None, &result);
            result
        }

        "type_ref" => {
            let ref_id = cmd.get("ref").and_then(|v| v.as_u64()).unwrap_or(0);
            let value = cmd.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let result = exec_js_with_result(app, &format!(
                "(() => {{ const refs = window.__hh_refs || []; const el = refs[{}]; if(!el) return JSON.stringify({{ok:false,err:'ref not found'}}); var role = el.getAttribute('role') || el.tagName.toLowerCase(); var label = (el.getAttribute('aria-label') || el.placeholder || '').substring(0,80).trim(); el.focus(); el.value = {}; el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); return JSON.stringify({{ok:true,role:role,label:label,url:location.href}}); }})()",
                ref_id,
                serde_json::to_string(value).unwrap()
            ));
            log_ui_action(app, "type_ref", ref_id, Some(value), &result);
            result
        }

        "select_ref" => {
            let ref_id = cmd.get("ref").and_then(|v| v.as_u64()).unwrap_or(0);
            let value = cmd.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let result = exec_js_with_result(app, &format!(
                "(() => {{ const refs = window.__hh_refs || []; const el = refs[{}]; if(!el) return JSON.stringify({{ok:false,err:'ref not found'}}); var role = el.getAttribute('role') || el.tagName.toLowerCase(); var label = (el.getAttribute('aria-label') || '').substring(0,80).trim(); el.value = {}; el.dispatchEvent(new Event('change', {{bubbles:true}})); return JSON.stringify({{ok:true,role:role,label:label,selected:el.value,url:location.href}}); }})()",
                ref_id,
                serde_json::to_string(value).unwrap()
            ));
            log_ui_action(app, "select_ref", ref_id, Some(value), &result);
            result
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
            let state = app.state::<AppState>();
            let ts = state.session_ts.clone();
            generate_all_endpoints(&ts);
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

fn generate_all_endpoints(session_ts: &str) {
    for app_name in config::list_apps() {
        endpoints::generate_for_app(&app_name);
        crate::cleanup::trim_captures_for_app(&app_name, session_ts);
        crate::cleanup::clean_app_domains(&app_name);
        crate::digest::generate_for_app(&app_name);
    }
}

/// Log a UI interaction to the active app's capture JSONL.
/// This correlates UI actions with the API calls they trigger.
fn log_ui_action(app: &tauri::AppHandle, action: &str, ref_id: u64, value: Option<&str>, raw_result: &str) {
    let state = app.state::<AppState>();
    let current_app = state.current_app.lock().unwrap().clone();
    let app_name = match current_app {
        Some(name) => name,
        None => return,
    };
    let session_ts = state.session_ts.clone();

    // Parse the JS result to extract role/label
    let js_info: serde_json::Value = serde_json::from_str(
        // The raw_result is {"ok":true,"result":"..."} — extract the inner result string
        &serde_json::from_str::<serde_json::Value>(raw_result)
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.as_str()).map(|s| s.to_string()))
            .unwrap_or_default(),
    )
    .unwrap_or_default();

    let mut entry = serde_json::json!({
        "type": "ui-action",
        "action": action,
        "ref": ref_id,
        "role": js_info.get("role").and_then(|v| v.as_str()).unwrap_or(""),
        "label": js_info.get("label").and_then(|v| v.as_str()).unwrap_or(""),
        "url": js_info.get("url").and_then(|v| v.as_str()).unwrap_or(""),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    if let Some(val) = value {
        entry.as_object_mut().unwrap().insert("value".to_string(), serde_json::Value::String(val.to_string()));
    }

    append_capture(&app_name, &entry, &session_ts);
}

// --- Auth-based capture filtering ---

/// Check if a request carries auth headers or session cookies.
/// Requests without auth can never be replayed by an AI agent, so they're useless to capture.
fn has_auth(data: &serde_json::Value, session_cookies: &std::collections::HashSet<String>) -> bool {
    let headers = match data.get("requestHeaders").and_then(|v| v.as_object()) {
        Some(h) => h,
        None => return false,
    };

    // Check for auth headers (NOT x-requested-with — present on almost all XHRs regardless of auth)
    for key in headers.keys() {
        let lower = key.to_lowercase();
        if lower == "authorization" || lower == "x-csrf-token" || lower == "x-xsrf-token" {
            return true;
        }
    }

    // Check for session cookies — if any cookie name in the request matches a known session cookie
    if let Some(cookie_str) = headers
        .get("cookie")
        .or_else(|| headers.get("Cookie"))
        .and_then(|v| v.as_str())
    {
        for part in cookie_str.split(';') {
            let name = part.trim().split('=').next().unwrap_or("").trim();
            if session_cookies.contains(name) {
                return true;
            }
        }
    }

    false
}

// --- Capture saving ---

fn save_capture(app: &tauri::AppHandle, data: &serde_json::Value, session_ts: &str) {
    // Skip xhr-start entries — always followed by the full xhr completion entry
    let entry_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if entry_type == "xhr-start" {
        return;
    }

    let url_str = match data.get("url").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => return,
    };

    // Meta entries (ui-action, navigation, cookies) always pass through — no auth check needed
    let is_meta = entry_type == "ui-action" || entry_type == "navigation" || entry_type == "cookies";

    if !is_meta {
        // API call — apply filters
        // Static blocklist catches known noise even if authed (e.g. google analytics sharing SID cookies)
        if should_skip_capture(url_str) {
            return;
        }

        // Auth-based filter: no auth headers/cookies = AI can't replay = useless
        let state = app.state::<AppState>();
        let session_cookies = state.session_cookie_names.lock().unwrap();
        if !has_auth(data, &session_cookies) {
            return;
        }
    }

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
            // If browser is open for a known app, auto-add this domain — but ONLY if authed.
            // This prevents third-party domains (amp4mail.googleusercontent.com, etc.) from being added.
            let current = app.state::<AppState>().current_app.lock().unwrap().clone();

            if let Some(ref name) = current {
                // Only auto-add domain if the request carries auth
                let state_ref = app.state::<AppState>();
                let session_cookies = state_ref.session_cookie_names.lock().unwrap();
                if !has_auth(data, &session_cookies) {
                    // No auth = don't add domain, don't save capture, silently drop
                    return;
                }
                drop(session_cookies); // release lock before further state access

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
    // Skip xhr-start entries — redundant with the full xhr completion
    let entry_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if entry_type == "xhr-start" {
        return;
    }

    // Skip noise URLs, but always let ui-action entries through
    if entry_type != "ui-action" {
        if let Some(url_str) = data.get("url").and_then(|v| v.as_str()) {
            if should_skip_capture(url_str) {
                return;
            }
        }
    }

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
                        session.cookies.insert(name.clone(), value);
                        // Track cookie name for auth-based capture filtering
                        state.session_cookie_names.lock().unwrap().insert(name);
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
