use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// Browser UA: Safari — WKWebView IS Safari's engine, so this is truthful.
// Google/etc. won't block sign-in since the fingerprint matches the actual engine.
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3 Safari/605.1.15";

// Curl UA: Chrome — used in sessions/latest.json for curl replay.
// Most sites expect Chrome and may serve different responses to Safari.
const FALLBACK_CURL_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36";

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct GlobalConfig {
    /// Chrome UA for curl replay (paste from your real Chrome)
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub capture_port: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub domains: Vec<String>,
    pub created: String,
    pub last_session: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct SessionData {
    pub domain: String,
    pub captured_at: String,
    #[serde(default)]
    pub cookies: HashMap<String, String>,
    #[serde(default)]
    pub auth_headers: HashMap<String, String>,
    #[serde(default)]
    pub csrf_tokens: HashMap<String, String>,
    #[serde(default)]
    pub user_agent: String,
}

/// Root data directory: ~/.harharhar/
pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".harharhar")
}

/// Ensure the base directory structure exists
pub fn ensure_dirs() {
    let root = data_dir();
    let _ = fs::create_dir_all(root.join("apps"));

    // Write AGENT.md if it doesn't exist
    let agent_md = root.join("AGENT.md");
    if !agent_md.exists() {
        let _ = fs::write(&agent_md, AGENT_MD_TEMPLATE);
    }
}

const AGENT_MD_TEMPLATE: &str = include_str!("../../agent-md-template.txt");

/// Ensure an app's full directory structure exists
pub fn ensure_app_dirs(app_name: &str) {
    let app = data_dir().join("apps").join(app_name);
    let _ = fs::create_dir_all(app.join("captures"));
    let _ = fs::create_dir_all(app.join("sessions"));
}

/// Read the global config, or return defaults
pub fn read_config() -> GlobalConfig {
    let path = data_dir().join("config.json");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write the global config
pub fn write_config(config: &GlobalConfig) {
    let path = data_dir().join("config.json");
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = fs::write(path, json);
    }
}

/// Safari UA for the WKWebView browser (always Safari — it IS Safari)
pub fn get_browser_ua() -> String {
    BROWSER_UA.to_string()
}

/// Chrome UA for curl replay. Priority: config > fallback
pub fn get_curl_ua() -> String {
    read_config()
        .user_agent
        .unwrap_or_else(|| FALLBACK_CURL_UA.to_string())
}

/// Find which app name a domain belongs to, if any
pub fn find_app_for_domain(domain: &str) -> Option<String> {
    let apps_dir = data_dir().join("apps");
    let entries = fs::read_dir(&apps_dir).ok()?;
    for entry in entries.flatten() {
        let config_path = entry.path().join("config.json");
        if let Ok(contents) = fs::read_to_string(&config_path) {
            if let Ok(app_config) = serde_json::from_str::<AppConfig>(&contents) {
                if app_config.domains.iter().any(|d| d == domain) {
                    return entry.file_name().to_str().map(|s| s.to_string());
                }
            }
        }
    }
    None
}

/// List all known apps with their domains
pub fn list_app_details() -> Vec<(String, Vec<String>)> {
    let apps_dir = data_dir().join("apps");
    fs::read_dir(&apps_dir)
        .ok()
        .map(|entries| {
            let mut result: Vec<(String, Vec<String>)> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let name = e.file_name().to_str()?.to_string();
                    let config_path = e.path().join("config.json");
                    let contents = fs::read_to_string(&config_path).ok()?;
                    let app_cfg: AppConfig = serde_json::from_str(&contents).ok()?;
                    Some((name, app_cfg.domains))
                })
                .collect();
            result.sort_by(|a, b| a.0.cmp(&b.0));
            result
        })
        .unwrap_or_default()
}

/// List all known app names
pub fn list_apps() -> Vec<String> {
    let apps_dir = data_dir().join("apps");
    fs::read_dir(&apps_dir)
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Create a new app with a friendly name and initial domain
pub fn create_app(name: &str, domain: &str) -> PathBuf {
    let app_dir = data_dir().join("apps").join(name);
    ensure_app_dirs(name);

    let config = AppConfig {
        domains: vec![domain.to_string()],
        created: chrono::Utc::now().to_rfc3339(),
        last_session: None,
    };

    let config_path = app_dir.join("config.json");
    if let Ok(json) = serde_json::to_string_pretty(&config) {
        let _ = fs::write(config_path, json);
    }

    app_dir
}

/// Add a domain to an existing app
pub fn add_domain_to_app(app_name: &str, domain: &str) {
    let config_path = data_dir().join("apps").join(app_name).join("config.json");
    if let Ok(contents) = fs::read_to_string(&config_path) {
        if let Ok(mut config) = serde_json::from_str::<AppConfig>(&contents) {
            if !config.domains.contains(&domain.to_string()) {
                config.domains.push(domain.to_string());
                if let Ok(json) = serde_json::to_string_pretty(&config) {
                    let _ = fs::write(config_path, json);
                }
            }
        }
    }
}
