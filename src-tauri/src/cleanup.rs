use crate::config;
use crate::endpoints;
use std::collections::HashSet;
use std::fs;
use std::io::Write;

/// Auth header names used for domain cleanup (same set as capture filtering).
const AUTH_HEADER_NAMES: &[&str] = &["authorization", "x-csrf-token", "x-xsrf-token"];

/// Trim response/request bodies from captures where the endpoint
/// pattern has been seen more than 3 times in endpoints.json.
/// Replaces bodies with "[trimmed: {byte_count} bytes]" to preserve metadata.
/// Only trims in JSONL files that are NOT the current session.
pub fn trim_captures_for_app(app_name: &str, current_session_ts: &str) {
    let app_dir = config::data_dir().join("apps").join(app_name);
    let endpoints_path = app_dir.join("endpoints.json");

    // Load endpoints.json to find well-sampled patterns (>3 times_seen)
    let catalog: endpoints::EndpointCatalog = match fs::read_to_string(&endpoints_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(c) => c,
        None => return,
    };

    let well_sampled: HashSet<String> = catalog
        .endpoints
        .iter()
        .filter(|ep| ep.times_seen > 3)
        .map(|ep| ep.pattern.clone())
        .collect();

    if well_sampled.is_empty() {
        return;
    }

    let captures_dir = app_dir.join("captures");
    let entries = match fs::read_dir(&captures_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let current_file = format!("{current_session_ts}.jsonl");

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process .jsonl files
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }

        // Skip the current session file
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name == current_file {
                continue;
            }
        }

        trim_single_file(&path, &well_sampled);
    }
}

/// Trim bodies in a single JSONL file for well-sampled endpoint patterns.
fn trim_single_file(path: &std::path::Path, well_sampled: &HashSet<String>) {
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut modified = false;
    let mut output_lines: Vec<String> = Vec::new();

    for line in contents.lines() {
        let mut data: serde_json::Value = match serde_json::from_str(line) {
            Ok(d) => d,
            Err(_) => {
                output_lines.push(line.to_string());
                continue;
            }
        };

        let url_str = match data.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => {
                output_lines.push(line.to_string());
                continue;
            }
        };

        // Parse URL to get path, then normalize
        let parsed = match url::Url::parse(&url_str) {
            Ok(u) => u,
            Err(_) => {
                output_lines.push(line.to_string());
                continue;
            }
        };

        let method = data
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_string();
        let path_str = parsed.path().to_string();
        let pattern = format!("{} {}", method, endpoints::normalize_path(&path_str));

        // Only trim if the pattern is well-sampled
        if !well_sampled.contains(&pattern) {
            output_lines.push(line.to_string());
            continue;
        }

        let obj = match data.as_object_mut() {
            Some(o) => o,
            None => {
                output_lines.push(line.to_string());
                continue;
            }
        };

        // Trim responseBody if present and not already trimmed
        if let Some(body_val) = obj.get("responseBody") {
            if let Some(body_str) = body_val.as_str() {
                if !body_str.starts_with("[trimmed") {
                    let byte_count = body_str.len();
                    obj.insert(
                        "responseBody".to_string(),
                        serde_json::Value::String(format!("[trimmed: {byte_count} bytes]")),
                    );
                    modified = true;
                }
            }
        }

        // Trim requestBody if present and not already trimmed
        if let Some(body_val) = obj.get("requestBody") {
            if let Some(body_str) = body_val.as_str() {
                if !body_str.starts_with("[trimmed") {
                    let byte_count = body_str.len();
                    obj.insert(
                        "requestBody".to_string(),
                        serde_json::Value::String(format!("[trimmed: {byte_count} bytes]")),
                    );
                    modified = true;
                }
            }
        }

        match serde_json::to_string(&data) {
            Ok(l) => output_lines.push(l),
            Err(_) => output_lines.push(line.to_string()),
        }
    }

    if !modified {
        return;
    }

    // Write to a temp file, then rename to avoid corruption
    let tmp_path = path.with_extension("jsonl.tmp");
    let mut tmp_file = match fs::File::create(&tmp_path) {
        Ok(f) => f,
        Err(_) => return,
    };

    for line in &output_lines {
        if writeln!(tmp_file, "{line}").is_err() {
            let _ = fs::remove_file(&tmp_path);
            return;
        }
    }

    // Flush and rename
    drop(tmp_file);
    let _ = fs::rename(&tmp_path, path);
}

/// Remove domains from an app's config that have never been seen with auth headers.
/// Called during generate_all_endpoints to progressively clean up bloated domain lists.
/// Always keeps at least the first domain (the one the user originally named the app for).
pub fn clean_app_domains(app_name: &str) {
    let app_dir = config::data_dir().join("apps").join(app_name);
    let config_path = app_dir.join("config.json");
    let captures_dir = app_dir.join("captures");

    // Read current app config
    let app_cfg: config::AppConfig = match fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(c) => c,
        None => return,
    };

    // Need at least 2 domains to have anything to clean
    if app_cfg.domains.len() <= 1 {
        return;
    }

    // Scan all captures to find domains that had authenticated requests
    let mut authed_domains: HashSet<String> = HashSet::new();

    let entries = match fs::read_dir(&captures_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

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

            // Skip meta entries — they don't carry auth
            let entry_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if entry_type == "ui-action" || entry_type == "navigation" || entry_type == "cookies" {
                continue;
            }

            let url_str = match data.get("url").and_then(|v| v.as_str()) {
                Some(u) => u,
                None => continue,
            };

            let domain = match url::Url::parse(url_str) {
                Ok(u) => match u.host_str() {
                    Some(h) => h.to_string(),
                    None => continue,
                },
                Err(_) => continue,
            };

            // Check if this request had auth headers
            if let Some(headers) = data.get("requestHeaders").and_then(|v| v.as_object()) {
                let has_auth_header = headers.keys().any(|k| {
                    let lower = k.to_lowercase();
                    AUTH_HEADER_NAMES.contains(&lower.as_str()) || lower == "cookie"
                });
                if has_auth_header {
                    authed_domains.insert(domain);
                }
            }
        }
    }

    // Always keep the first domain (the one the user originally registered)
    let first_domain = app_cfg.domains[0].clone();
    authed_domains.insert(first_domain);

    // Filter to only domains that had auth
    let cleaned: Vec<String> = app_cfg
        .domains
        .into_iter()
        .filter(|d| authed_domains.contains(d))
        .collect();

    // Write back if we removed any domains
    let updated = config::AppConfig {
        domains: cleaned,
        created: app_cfg.created,
        last_session: app_cfg.last_session,
    };

    if let Ok(json) = serde_json::to_string_pretty(&updated) {
        let _ = fs::write(&config_path, json);
    }
}
