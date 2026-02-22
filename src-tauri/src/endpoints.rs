use crate::config;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct EndpointCatalog {
    pub endpoints: Vec<Endpoint>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Endpoint {
    pub pattern: String,
    pub methods: Vec<String>,
    pub observed_urls: Vec<String>,
    #[serde(default)]
    pub query_params: Vec<String>,
    #[serde(default)]
    pub request_content_types: Vec<String>,
    #[serde(default)]
    pub response_content_types: Vec<String>,
    #[serde(default)]
    pub response_shape_sample: Option<serde_json::Value>,
    pub auth_required: bool,
    pub times_seen: u32,
    pub last_seen: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct AuthInfo {
    pub mechanisms: Vec<AuthMechanism>,
    #[serde(default)]
    pub login_url: Option<String>,
    #[serde(default)]
    pub observed_refresh_endpoints: Vec<String>,
    #[serde(default)]
    pub session_duration_estimate: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuthMechanism {
    #[serde(rename = "type")]
    pub mech_type: String,
    pub details: HashMap<String, serde_json::Value>,
}

const AUTH_HEADER_NAMES: &[&str] = &["authorization", "x-csrf-token", "x-xsrf-token"];
const AUTH_COOKIE_PATTERNS: &[&str] = &[
    "session", "sid", "token", "auth", "csrf", "xsrf", "jwt",
];

/// Process all captures for an app and generate endpoints.json + auth.json
pub fn generate_for_app(app_name: &str) {
    let app_dir = config::data_dir().join("apps").join(app_name);
    let captures_dir = app_dir.join("captures");

    let entries = match fs::read_dir(&captures_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut endpoints: HashMap<String, Endpoint> = HashMap::new();
    let mut seen_cookies: HashMap<String, String> = HashMap::new();
    let mut seen_auth_headers: HashMap<String, String> = HashMap::new();
    let mut login_urls: Vec<String> = Vec::new();
    let mut refresh_urls: Vec<String> = Vec::new();

    // Read all JSONL capture files
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let contents = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in contents.lines() {
            let data: serde_json::Value = match serde_json::from_str(line) {
                Ok(d) => d,
                Err(_) => continue,
            };

            // Skip cookie-only entries
            if data.get("type").and_then(|v| v.as_str()) == Some("cookies") {
                // But harvest cookies for auth detection
                if let Some(cookie_str) = data
                    .get("requestHeaders")
                    .and_then(|h| h.get("cookie"))
                    .and_then(|v| v.as_str())
                {
                    for part in cookie_str.split(';') {
                        let trimmed = part.trim();
                        if let Some(eq) = trimmed.find('=') {
                            let name = trimmed[..eq].trim().to_string();
                            let value = trimmed[eq + 1..].trim().to_string();
                            seen_cookies.insert(name, value);
                        }
                    }
                }
                continue;
            }

            let url_str = match data.get("url").and_then(|v| v.as_str()) {
                Some(u) => u,
                None => continue,
            };
            let method = data
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_string();
            let timestamp = data
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Parse URL to get path pattern
            let parsed = match url::Url::parse(url_str) {
                Ok(u) => u,
                Err(_) => continue,
            };

            let path = parsed.path().to_string();
            // Normalize: replace numeric path segments with {id}
            let pattern = normalize_path(&path);
            let key = format!("{} {}", method, pattern);

            // Collect query params
            let query_params: Vec<String> = parsed
                .query_pairs()
                .map(|(k, _)| k.to_string())
                .collect();

            // Check for auth headers
            let has_auth = if let Some(headers) = data.get("requestHeaders").and_then(|v| v.as_object()) {
                headers.keys().any(|k| {
                    let lower = k.to_lowercase();
                    AUTH_HEADER_NAMES.contains(&lower.as_str()) || lower == "cookie"
                })
            } else {
                false
            };

            // Track auth headers
            if let Some(headers) = data.get("requestHeaders").and_then(|v| v.as_object()) {
                for (k, v) in headers {
                    let lower = k.to_lowercase();
                    if AUTH_HEADER_NAMES.contains(&lower.as_str()) {
                        if let Some(val) = v.as_str() {
                            seen_auth_headers.insert(k.clone(), val.to_string());
                        }
                    }
                    if lower == "cookie" {
                        if let Some(cookie_str) = v.as_str() {
                            for part in cookie_str.split(';') {
                                let trimmed = part.trim();
                                if let Some(eq) = trimmed.find('=') {
                                    let name = trimmed[..eq].trim().to_string();
                                    let value = trimmed[eq + 1..].trim().to_string();
                                    seen_cookies.insert(name, value);
                                }
                            }
                        }
                    }
                }
            }

            // Detect login/refresh endpoints
            let path_lower = path.to_lowercase();
            if path_lower.contains("login")
                || path_lower.contains("signin")
                || path_lower.contains("auth")
                    && (method == "POST" || path_lower.contains("token"))
            {
                if path_lower.contains("refresh") || path_lower.contains("token") {
                    refresh_urls.push(url_str.to_string());
                } else {
                    login_urls.push(url_str.to_string());
                }
            }

            // Get response content type
            let resp_ct = data
                .get("responseHeaders")
                .and_then(|h| h.get("content-type"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Get a sample response shape (first 3 levels of keys for JSON)
            let response_shape = data
                .get("responseBody")
                .and_then(|v| v.as_str())
                .and_then(|body| serde_json::from_str::<serde_json::Value>(body).ok())
                .map(|v| extract_shape(&v, 0));

            // Upsert endpoint
            let ep = endpoints.entry(key).or_insert_with(|| Endpoint {
                pattern: format!("{} {}", method.clone(), pattern.clone()),
                methods: vec![],
                observed_urls: vec![],
                query_params: vec![],
                request_content_types: vec![],
                response_content_types: vec![],
                response_shape_sample: None,
                auth_required: false,
                times_seen: 0,
                last_seen: String::new(),
            });

            if !ep.methods.contains(&method) {
                ep.methods.push(method.clone());
            }
            if ep.observed_urls.len() < 3 && !ep.observed_urls.contains(&url_str.to_string()) {
                ep.observed_urls.push(url_str.to_string());
            }
            for qp in query_params {
                if !ep.query_params.contains(&qp) {
                    ep.query_params.push(qp);
                }
            }
            if !resp_ct.is_empty() && !ep.response_content_types.contains(&resp_ct) {
                ep.response_content_types.push(resp_ct);
            }
            if ep.response_shape_sample.is_none() {
                ep.response_shape_sample = response_shape;
            }
            ep.auth_required = ep.auth_required || has_auth;
            ep.times_seen += 1;
            ep.last_seen = timestamp;
        }
    }

    // Write endpoints.json
    let mut ep_list: Vec<Endpoint> = endpoints.into_values().collect();
    ep_list.sort_by(|a, b| b.times_seen.cmp(&a.times_seen));
    let catalog = EndpointCatalog { endpoints: ep_list };
    if let Ok(json) = serde_json::to_string_pretty(&catalog) {
        let _ = fs::write(app_dir.join("endpoints.json"), json);
    }

    // Build auth.json
    let mut mechanisms: Vec<AuthMechanism> = Vec::new();

    // Cookie-based auth
    let auth_cookies: Vec<String> = seen_cookies
        .keys()
        .filter(|name| {
            let lower = name.to_lowercase();
            AUTH_COOKIE_PATTERNS.iter().any(|p| lower.contains(p))
        })
        .cloned()
        .collect();
    if !auth_cookies.is_empty() {
        let mut details = HashMap::new();
        details.insert(
            "names".to_string(),
            serde_json::to_value(&auth_cookies).unwrap(),
        );
        mechanisms.push(AuthMechanism {
            mech_type: "cookie".to_string(),
            details,
        });
    }

    // Header-based auth
    for (header_name, sample_value) in &seen_auth_headers {
        let mut details = HashMap::new();
        details.insert(
            "header".to_string(),
            serde_json::Value::String(header_name.clone()),
        );
        // Detect pattern (e.g., "Bearer ...", "SAPISIDHASH ...")
        let pattern = if let Some(space) = sample_value.find(' ') {
            format!("{} ...", &sample_value[..space])
        } else {
            "opaque".to_string()
        };
        details.insert(
            "pattern".to_string(),
            serde_json::Value::String(pattern),
        );
        mechanisms.push(AuthMechanism {
            mech_type: "header".to_string(),
            details,
        });
    }

    let auth = AuthInfo {
        mechanisms,
        login_url: login_urls.first().cloned(),
        observed_refresh_endpoints: refresh_urls
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect(),
        session_duration_estimate: "unknown".to_string(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&auth) {
        let _ = fs::write(app_dir.join("auth.json"), json);
    }
}

/// Normalize a URL path: replace numeric segments and UUIDs with {id}
fn normalize_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            if seg.is_empty() {
                return seg.to_string();
            }
            // Pure numeric
            if seg.chars().all(|c| c.is_ascii_digit()) {
                return "{id}".to_string();
            }
            // UUID-like (32+ hex chars with dashes)
            if seg.len() >= 32
                && seg
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == '-')
            {
                return "{id}".to_string();
            }
            // Long hex strings (like MongoDB IDs)
            if seg.len() >= 20 && seg.chars().all(|c| c.is_ascii_hexdigit()) {
                return "{id}".to_string();
            }
            seg.to_string()
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Extract a JSON shape: replace values with type indicators, limit depth
fn extract_shape(value: &serde_json::Value, depth: u32) -> serde_json::Value {
    if depth > 2 {
        return serde_json::Value::String("...".to_string());
    }
    match value {
        serde_json::Value::Object(map) => {
            let mut shape = serde_json::Map::new();
            for (k, v) in map.iter().take(10) {
                shape.insert(k.clone(), extract_shape(v, depth + 1));
            }
            serde_json::Value::Object(shape)
        }
        serde_json::Value::Array(arr) => {
            if let Some(first) = arr.first() {
                serde_json::Value::Array(vec![extract_shape(first, depth + 1)])
            } else {
                serde_json::Value::Array(vec![])
            }
        }
        serde_json::Value::String(_) => serde_json::Value::String("str".to_string()),
        serde_json::Value::Number(_) => serde_json::Value::String("num".to_string()),
        serde_json::Value::Bool(_) => serde_json::Value::String("bool".to_string()),
        serde_json::Value::Null => serde_json::Value::Null,
    }
}
