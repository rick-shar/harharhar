mod capture;
mod config;
pub mod endpoints;

use std::sync::Mutex;
use tauri::{Emitter, Manager};

pub struct AppState {
    pub domain_map: Mutex<std::collections::HashMap<String, String>>,
    /// Safari UA — used by WKWebView browser (matches the actual engine)
    pub browser_ua: String,
    /// Chrome UA — written to sessions/latest.json for curl replay
    pub curl_ua: String,
    pub session_file_lock: Mutex<()>,
    pub session_ts: String,
    pub pending_url: Mutex<Option<String>>,
    /// Which app the browser is currently browsing (for auto-adding new domains)
    pub current_app: Mutex<Option<String>>,
    /// Captures from unmapped domains, keyed by domain
    pub unmapped_captures: Mutex<std::collections::HashMap<String, Vec<serde_json::Value>>>,
    /// Pending eval callbacks: id -> sender
    pub eval_callbacks: Mutex<std::collections::HashMap<String, std::sync::mpsc::Sender<String>>>,
}

/// Called from injected JS on external pages via Tauri IPC.
/// This is the primary capture path — no network involved.
#[tauri::command]
fn save_capture_data(app: tauri::AppHandle, data: serde_json::Value) -> Result<(), String> {
    let state = app.state::<AppState>();
    let ts = state.session_ts.clone();
    let _ = app.emit("request-captured", &data);
    capture::process_single(&app, &data, &ts);
    Ok(())
}

#[tauri::command]
async fn navigate(app: tauri::AppHandle, url: String) -> Result<(), String> {
    let mut raw = url.clone();
    if !raw.starts_with("http") {
        raw = format!("https://{raw}");
    }
    let parsed: url::Url = raw.parse().map_err(|e: url::ParseError| e.to_string())?;

    let domain = parsed.host_str().unwrap_or("").to_string();
    let state = app.state::<AppState>();

    let app_name = {
        let map = state.domain_map.lock().unwrap();
        map.get(&domain).cloned()
    };

    // If domain unknown, block browser and ask for name first
    if app_name.is_none() {
        {
            let mut pending = state.pending_url.lock().unwrap();
            *pending = Some(raw);
        }
        let _ = app.emit("name-app-before-navigate", &domain);
        return Ok(());
    }

    // Track which app the browser is on (for auto-adding new domains)
    {
        let mut current = state.current_app.lock().unwrap();
        *current = app_name;
    }

    open_browser(&app, parsed)?;
    Ok(())
}

#[tauri::command]
async fn resume_navigate(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let url = {
        let mut pending = state.pending_url.lock().unwrap();
        pending.take()
    };

    if let Some(raw) = url {
        let parsed: url::Url = raw.parse().map_err(|e: url::ParseError| e.to_string())?;
        open_browser(&app, parsed)?;
    }

    Ok(())
}

pub fn open_browser(app: &tauri::AppHandle, url: url::Url) -> Result<(), String> {
    let state = app.state::<AppState>();
    let ua = state.browser_ua.clone();

    if let Some(wv) = app.get_webview_window("browser") {
        let js = format!(
            "window.location.href={}",
            serde_json::to_string(url.as_str()).unwrap()
        );
        wv.eval(&js).map_err(|e| e.to_string())?;
    } else {
        let inject = include_str!("../../inject/intercept.js");
        let mut builder = tauri::WebviewWindowBuilder::new(
            app,
            "browser",
            tauri::WebviewUrl::External(url),
        )
        .title("harharhar browser")
        .inner_size(1000.0, 800.0)
        .user_agent(&ua)
        .initialization_script(inject);

        // Position the browser window to the right of the explorer window
        if let Some(explorer) = app.get_webview_window("explorer") {
            if let (Ok(pos), Ok(size), Ok(scale)) = (
                explorer.outer_position(),
                explorer.outer_size(),
                explorer.scale_factor(),
            ) {
                let gap = 16.0; // logical pixels
                let x = (pos.x as f64 / scale) + (size.width as f64 / scale) + gap;
                let y = pos.y as f64 / scale;
                builder = builder.position(x, y);
            }
        }

        builder.build().map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
async fn set_user_agent(app: tauri::AppHandle, ua: String) -> Result<(), String> {
    let mut cleaned = ua.trim().to_string();
    while cleaned.starts_with('"') && cleaned.ends_with('"')
        || cleaned.starts_with('\'') && cleaned.ends_with('\'')
        || cleaned.starts_with('`') && cleaned.ends_with('`')
    {
        cleaned = cleaned[1..cleaned.len() - 1].trim().to_string();
    }

    if !cleaned.contains("Mozilla") {
        return Err("Doesn't look like a valid user-agent string. It should start with 'Mozilla/5.0'".to_string());
    }

    let mut cfg = config::read_config();
    cfg.user_agent = Some(cleaned);
    config::write_config(&cfg);
    let _ = app.emit("config-updated", "user_agent");
    Ok(())
}

#[tauri::command]
async fn get_config() -> Result<serde_json::Value, String> {
    let cfg = config::read_config();
    serde_json::to_value(&cfg).map_err(|e| e.to_string())
}

#[tauri::command]
async fn register_app(app: tauri::AppHandle, name: String, domain: String) -> Result<(), String> {
    config::create_app(&name, &domain);

    let ts = {
        let state = app.state::<AppState>();
        let mut map = state.domain_map.lock().unwrap();
        map.insert(domain.clone(), name.clone());
        state.session_ts.clone()
    };

    capture::flush_unmapped(&app, &domain, &name, &ts);
    Ok(())
}

#[tauri::command]
async fn get_apps() -> Result<Vec<String>, String> {
    Ok(config::list_apps())
}

#[tauri::command]
async fn get_app_details() -> Result<Vec<serde_json::Value>, String> {
    let details = config::list_app_details();
    Ok(details
        .into_iter()
        .map(|(name, domains)| {
            serde_json::json!({
                "name": name,
                "domains": domains
            })
        })
        .collect())
}

/// IPC callback from browser JS — resolves a pending eval_js_with_result call.
#[tauri::command]
fn eval_callback(app: tauri::AppHandle, id: String, result: String) -> Result<(), String> {
    let state = app.state::<AppState>();
    let tx = {
        let mut cbs = state.eval_callbacks.lock().unwrap();
        cbs.remove(&id)
    };
    if let Some(tx) = tx {
        let _ = tx.send(result);
    }
    Ok(())
}

/// Evaluate JS in the browser webview and return the result via IPC callback.
/// Works by wrapping the JS in code that calls back via Tauri IPC.
pub fn eval_js_with_result(app: &tauri::AppHandle, js: &str) -> Result<String, String> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let wv = app
        .get_webview_window("browser")
        .ok_or("browser window not open")?;

    let id = format!("e{}", COUNTER.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let state = app.state::<AppState>();
        state.eval_callbacks.lock().unwrap().insert(id.clone(), tx);
    }

    let id_json = serde_json::to_string(&id).unwrap();

    // Inline the raw JS directly (no eval()) to avoid CSP restrictions
    let wrapped = EVAL_TEMPLATE
        .replace("JS_PLACEHOLDER", js)
        .replace("ID_PLACEHOLDER", &id_json);

    wv.eval(&wrapped).map_err(|e| {
        // Clean up the callback on eval failure
        let state = app.state::<AppState>();
        state.eval_callbacks.lock().unwrap().remove(&id);
        e.to_string()
    })?;

    rx.recv_timeout(std::time::Duration::from_secs(10))
        .map_err(|_| {
            let state = app.state::<AppState>();
            state.eval_callbacks.lock().unwrap().remove(&id);
            "eval timeout".to_string()
        })
}

const EVAL_TEMPLATE: &str = r#"(function(){try{var __r=(JS_PLACEHOLDER);if(__r&&typeof __r.then==='function'){__r.then(function(v){var __s=typeof v==='string'?v:JSON.stringify(v);window.__TAURI_INTERNALS__.invoke('eval_callback',{id:ID_PLACEHOLDER,result:__s||'null'});}).catch(function(e){window.__TAURI_INTERNALS__.invoke('eval_callback',{id:ID_PLACEHOLDER,result:'error: '+e.message});});}else{var __s=typeof __r==='string'?__r:JSON.stringify(__r);window.__TAURI_INTERNALS__.invoke('eval_callback',{id:ID_PLACEHOLDER,result:__s||'null'});}}catch(e){window.__TAURI_INTERNALS__.invoke('eval_callback',{id:ID_PLACEHOLDER,result:'error: '+e.message});}})();"#;

/// Get cookies — returns document.cookie (non-httpOnly) from browser.
/// For full cookies including httpOnly, read sessions/latest.json directly.
#[tauri::command]
async fn get_cookies(app: tauri::AppHandle, _url: String) -> Result<String, String> {
    eval_js_with_result(&app, "document.cookie")
}

/// Evaluate JS in the browser and return the result.
#[tauri::command]
async fn eval_js(app: tauri::AppHandle, js: String) -> Result<String, String> {
    eval_js_with_result(&app, &js)
}

#[tauri::command]
async fn add_domain(app: tauri::AppHandle, name: String, domain: String) -> Result<(), String> {
    config::add_domain_to_app(&name, &domain);

    let ts = {
        let state = app.state::<AppState>();
        let mut map = state.domain_map.lock().unwrap();
        map.insert(domain.clone(), name.clone());
        state.session_ts.clone()
    };

    capture::flush_unmapped(&app, &domain, &name, &ts);
    Ok(())
}

pub fn run() {
    config::ensure_dirs();

    let browser_ua = config::get_browser_ua();
    let curl_ua = config::get_curl_ua();
    let session_ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M").to_string();

    let mut domain_map = std::collections::HashMap::new();
    for app_name in config::list_apps() {
        let config_path = config::data_dir()
            .join("apps")
            .join(&app_name)
            .join("config.json");
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if let Ok(app_cfg) = serde_json::from_str::<config::AppConfig>(&contents) {
                for d in app_cfg.domains {
                    domain_map.insert(d, app_name.clone());
                }
            }
        }
    }

    let state = AppState {
        domain_map: Mutex::new(domain_map),
        browser_ua,
        curl_ua,
        current_app: Mutex::new(None),
        session_file_lock: Mutex::new(()),
        session_ts,
        pending_url: Mutex::new(None),
        unmapped_captures: Mutex::new(std::collections::HashMap::new()),
        eval_callbacks: Mutex::new(std::collections::HashMap::new()),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            navigate,
            resume_navigate,
            set_user_agent,
            get_config,
            register_app,
            add_domain,
            get_apps,
            get_app_details,
            get_cookies,
            eval_js,
            eval_callback,
            save_capture_data,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(capture::start_command_watcher(handle));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error running tauri application");
}
