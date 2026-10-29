//! Asks GitHub whether a newer gitcraft release exists (at most every 12 hours, can be turned off).

use std::time::Duration;

/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Redirects to the latest release's page, `…/releases/tag/vX.Y.Z`. The web page rather than
/// the REST API: the API allows 60 anonymous requests an hour per IP, which shared addresses
/// (VPNs, offices) use up.
const LATEST: &str = "https://github.com/Vancete/gitcraft/releases/latest";
/// Seconds between two checks.
pub const INTERVAL: i64 = 12 * 3600;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// Without the leading `v`.
    pub version: String,
    pub url: String,
}

/// The latest published release.
pub fn latest() -> Result<Release, String> {
    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent("gitcraft")
        .build()
        .head(LATEST)
        .call()
        .map_err(|e| e.to_string())?;
    release_from_url(resp.get_url()).ok_or_else(|| "no release published yet".to_owned())
}

fn release_from_url(url: &str) -> Option<Release> {
    let tag = url
        .split_once("/releases/tag/")?
        .1
        .split(['?', '#'])
        .next()?;
    (!tag.is_empty()).then(|| Release {
        version: tag.trim_start_matches('v').to_owned(),
        url: url.to_owned(),
    })
}

/// Whether version `a` comes after `b` (`1.10.0` > `1.9.2`; pre-release suffixes are ignored).
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split(['-', '+'])
            .next()
            .unwrap_or_default()
            .split('.')
            .map(|n| n.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_page_urls() {
        assert_eq!(
            release_from_url("https://github.com/Vancete/gitcraft/releases/tag/v0.2.0"),
            Some(Release {
                version: "0.2.0".into(),
                url: "https://github.com/Vancete/gitcraft/releases/tag/v0.2.0".into()
            })
        );
        // No release yet: the redirect lands on the list.
        assert_eq!(
            release_from_url("https://github.com/Vancete/gitcraft/releases"),
            None
        );
    }

    #[test]
    fn version_order() {
        assert!(newer("0.2.0", "0.1.0"));
        assert!(newer("v1.10.0", "1.9.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.1.1"));
        assert!(!newer("1.0.0-rc1", "1.0.0"));
    }
}
