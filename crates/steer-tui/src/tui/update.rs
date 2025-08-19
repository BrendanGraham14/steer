use semver::Version;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub enum UpdateStatus {
    Unknown,
    Checking,
    UpToDate,
    Available(UpdateInfo),
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

fn normalize_tag(tag: &str) -> Option<String> {
    let t = tag.trim();
    if let Some(rest) = t.strip_prefix("steer-v") {
        return Some(rest.to_string());
    }
    if let Some(rest) = t.strip_prefix('v') {
        return Some(rest.to_string());
    }
    if Version::parse(t).is_ok() {
        return Some(t.to_string());
    }
    None
}

fn parse_version(s: &str) -> Option<Version> {
    normalize_tag(s).and_then(|n| Version::parse(&n).ok())
}

pub async fn check_latest(repo_owner: &str, repo_name: &str, current: &str) -> UpdateStatus {
    let current_ver = match Version::parse(current) {
        Ok(v) => v,
        Err(_) => return UpdateStatus::Unknown,
    };

    let url = format!("https://api.github.com/repos/{repo_owner}/{repo_name}/releases/latest");

    let client = reqwest::Client::builder()
        .user_agent(format!("steer-tui/{current}"))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(_) => return UpdateStatus::Unknown,
    };

    let resp = match client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return UpdateStatus::Unknown,
    };

    if !resp.status().is_success() {
        return UpdateStatus::Unknown;
    }

    let release: GithubRelease = match resp.json().await {
        Ok(j) => j,
        Err(_) => return UpdateStatus::Unknown,
    };

    let latest_ver = match parse_version(&release.tag_name) {
        Some(v) => v,
        None => return UpdateStatus::Unknown,
    };

    if latest_ver > current_ver {
        UpdateStatus::Available(UpdateInfo {
            latest: latest_ver.to_string(),
            url: release.html_url,
        })
    } else {
        UpdateStatus::UpToDate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_tag() {
        assert_eq!(normalize_tag("steer-v1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(normalize_tag("v1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(normalize_tag("1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(normalize_tag(" v1.2.3 "), Some("1.2.3".to_string()));
        assert_eq!(normalize_tag("weird-v1.2.3"), None);
        assert_eq!(normalize_tag("steer-1.2.3"), None);
        assert_eq!(normalize_tag("V1.2.3"), None);
    }

    #[test]
    fn test_parse_version() {
        assert!(parse_version("steer-v1.2.3").is_some());
        assert!(parse_version("v1.2.3").is_some());
        assert!(parse_version("1.2.3").is_some());
        assert!(parse_version("not-a-version").is_none());
        assert!(parse_version("").is_none());
        assert!(parse_version("weird-v1.2.3").is_none());
    }

    #[test]
    fn test_version_comparison() {
        let v1 = parse_version("v0.3.0").unwrap();
        let v2 = parse_version("v0.4.0").unwrap();
        assert!(v2 > v1);

        let v3 = parse_version("0.4.0").unwrap();
        let v4 = parse_version("v0.4.0").unwrap();
        assert_eq!(v3, v4);
    }
}
