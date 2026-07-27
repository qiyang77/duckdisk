use reqwest::header::{ACCEPT, USER_AGENT};
use semver::Version;
use serde::{Deserialize, Serialize};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/qiyang77/duckdisk/releases/latest";

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    current_version: String,
    latest_version: String,
    update_available: bool,
    release_url: String,
}

pub async fn check(current_version: &str) -> Result<UpdateCheck, String> {
    let response = reqwest::Client::new()
        .get(LATEST_RELEASE_URL)
        .header(USER_AGENT, format!("DuckDisk/{current_version}"))
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| format!("Could not contact GitHub: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("GitHub returned {status} while checking releases"));
    }

    let release = response
        .json::<GitHubRelease>()
        .await
        .map_err(|err| format!("GitHub returned an invalid release response: {err}"))?;
    let current = parse_version(current_version)?;
    let latest = parse_version(&release.tag_name)?;
    let update_available = latest > current;

    Ok(UpdateCheck {
        current_version: current.to_string(),
        latest_version: latest.to_string(),
        update_available,
        release_url: if update_available {
            release.html_url
        } else {
            String::new()
        },
    })
}

fn parse_version(value: &str) -> Result<Version, String> {
    let normalized = value.trim().trim_start_matches(['v', 'V']).trim();
    Version::parse(normalized).map_err(|err| format!("Invalid version '{value}': {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_tags() {
        assert_eq!(parse_version("v0.5.2").unwrap(), Version::new(0, 5, 2));
        assert_eq!(parse_version(" V1.2.3 ").unwrap(), Version::new(1, 2, 3));
    }

    #[test]
    fn compares_versions_semantically() {
        assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.9").unwrap());
    }
}
