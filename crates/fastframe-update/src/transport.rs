//! The app's HTTP client, and where updates may come from.

use std::io::Read;

use anyhow::{Context, Result, bail, ensure};
use url::Url;

use crate::UpdateConfig;

/// A GET request the updater needs answered.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    /// The absolute URL.
    pub url: &'a str,
    /// The `Accept` header to send.
    pub accept: &'a str,
    /// The `User-Agent` header to send: `<app_name>/<current_version>`.
    pub user_agent: &'a str,
}

/// The answer to a [`Request`]: its status, redirect target and body.
pub struct Response {
    /// The HTTP status code.
    pub status: u16,
    /// The `Location` header of a redirect, as sent.
    pub location: Option<String>,
    /// The body, read as it arrives.
    pub body: Box<dyn Read + Send>,
}

impl Response {
    /// A `200 OK` with this body.
    pub fn ok(body: impl Read + Send + 'static) -> Self {
        Self {
            status: 200,
            location: None,
            body: Box::new(body),
        }
    }

    /// A `302 Found` to `location`.
    pub fn redirect(location: impl Into<String>) -> Self {
        Self {
            status: 302,
            location: Some(location.into()),
            body: Box::new(std::io::empty()),
        }
    }
}

impl std::fmt::Debug for Response {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Response")
            .field("status", &self.status)
            .field("location", &self.location)
            .finish_non_exhaustive()
    }
}

/// The app's HTTP client, with the app's proxy and TLS settings.
///
/// An implementation sends one GET and returns what the server answered.
/// It must **not follow redirects**: the updater follows them itself, at
/// most five, and only to the release hosts of its [`Source`]. Give the
/// client a connect timeout (15 s) and an overall timeout long enough for a
/// large download (15 minutes); both apps use those.
pub trait Transport: Send + Sync {
    /// Sends `request` and returns the response, whatever its status.
    fn get(&self, request: &Request<'_>) -> Result<Response>;
}

/// Where releases come from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source(Origin);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Origin {
    #[default]
    GitHub,
    Local(Url),
}

const GITHUB_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "release-assets.githubusercontent.com",
    "objects.githubusercontent.com",
];

impl Source {
    /// GitHub releases of [`UpdateConfig::repository`]. The default.
    pub fn github() -> Self {
        Self(Origin::GitHub)
    }

    /// A feed on this computer, for demos and end-to-end tests:
    /// `<base>/latest.json` answers both the release check and the release
    /// metadata. On the pre-release channel
    /// ([`Prereleases::WhenRunningPrerelease`](crate::Prereleases)) the
    /// check reads `<base>/releases.json` instead, one page shaped like
    /// GitHub's release list, and `latest.json` answers the metadata of the
    /// release it picks. Only plain HTTP on `127.0.0.1` or `[::1]` is accepted, and
    /// downloads may not leave that origin. Signatures are still required
    /// when the configuration has a publisher key.
    pub fn local(base: &str) -> Result<Self> {
        let url = Url::parse(base.trim_end_matches('/'))?;
        ensure!(
            url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "[::1]"))
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "The demo update feed must use a loopback HTTP address"
        );
        Ok(Self(Origin::Local(url)))
    }

    /// Whether this is GitHub rather than a local feed.
    pub fn is_github(&self) -> bool {
        matches!(self.0, Origin::GitHub)
    }

    pub(crate) fn latest(&self, config: &UpdateConfig) -> String {
        match &self.0 {
            Origin::GitHub => format!(
                "https://api.github.com/repos/{}/releases/latest",
                config.repository
            ),
            Origin::Local(base) => feed(base),
        }
    }

    /// Page `page` (from 1) of the release list, or `None` past the last
    /// page a source has. A local feed has one.
    pub(crate) fn releases(
        &self,
        config: &UpdateConfig,
        page: u32,
        per_page: u32,
    ) -> Option<String> {
        match &self.0 {
            Origin::GitHub => Some(format!(
                "https://api.github.com/repos/{}/releases?per_page={per_page}&page={page}",
                config.repository
            )),
            Origin::Local(base) => (page == 1)
                .then(|| format!("{}/releases.json", base.as_str().trim_end_matches('/'))),
        }
    }

    pub(crate) fn release(&self, config: &UpdateConfig, version: &str) -> String {
        match &self.0 {
            Origin::GitHub => format!(
                "https://api.github.com/repos/{}/releases/tags/v{version}",
                config.repository
            ),
            Origin::Local(base) => feed(base),
        }
    }

    /// Whether a request or redirect may go to `url`.
    pub(crate) fn allowed(&self, url: &Url) -> bool {
        match &self.0 {
            Origin::GitHub => {
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.port().is_none()
                    && url
                        .host_str()
                        .is_some_and(|host| GITHUB_HOSTS.contains(&host))
            }
            Origin::Local(base) => url.origin() == base.origin(),
        }
    }

    /// Whether a release asset's published URL belongs to this release.
    pub(crate) fn owns_asset(
        &self,
        config: &UpdateConfig,
        url: &Url,
        version: &str,
        name: &str,
    ) -> bool {
        self.allowed(url)
            && match &self.0 {
                Origin::GitHub => {
                    url.host_str() == Some("github.com")
                        && url.path()
                            == format!("/{}/releases/download/v{version}/{name}", config.repository)
                }
                Origin::Local(_) => true,
            }
    }
}

fn feed(base: &Url) -> String {
    format!("{}/latest.json", base.as_str().trim_end_matches('/'))
}

/// GETs `url`, following redirects that stay on the source's hosts, and
/// returns the body of the final successful answer.
pub(crate) fn fetch(
    transport: &dyn Transport,
    source: &Source,
    config: &UpdateConfig,
    url: &str,
    accept: &str,
) -> Result<Box<dyn Read + Send>> {
    let user_agent = config.user_agent();
    let mut current = Url::parse(url).context("Invalid update address")?;
    for _ in 0..=5 {
        ensure!(source.allowed(&current), "Update redirect is not allowed");
        let response = transport.get(&Request {
            url: current.as_str(),
            accept,
            user_agent: &user_agent,
        })?;
        match response.status {
            200..=299 => return Ok(response.body),
            300..=399 => {
                let location = response
                    .location
                    .context("The update server sent a redirect without a location")?;
                current = current
                    .join(&location)
                    .context("The update server sent an invalid redirect")?;
            }
            status => bail!("The update server answered {status}"),
        }
    }
    bail!("The update server redirected too many times")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeTransport;
    use crate::tests::ZAPFAST;

    #[test]
    fn redirects_cannot_leave_release_hosts() {
        for address in [
            "http://github.com/file",
            "https://github.com.attacker.invalid/file",
            "https://example.com/file",
            "https://user:secret@github.com/file",
            "https://github.com:8443/file",
        ] {
            assert!(
                !Source::github().allowed(&Url::parse(address).unwrap()),
                "{address}"
            );
        }
        for address in [
            "https://release-assets.githubusercontent.com/file",
            "https://api.github.com/repos/crmne/zapfast/releases/latest",
        ] {
            assert!(Source::github().allowed(&Url::parse(address).unwrap()));
        }
    }

    #[test]
    fn the_github_source_names_the_configured_repository() {
        assert_eq!(
            Source::github().latest(&ZAPFAST),
            "https://api.github.com/repos/crmne/zapfast/releases/latest"
        );
        assert_eq!(
            Source::github().releases(&ZAPFAST, 2, 100).as_deref(),
            Some("https://api.github.com/repos/crmne/zapfast/releases?per_page=100&page=2")
        );
        assert_eq!(
            Source::github().release(&ZAPFAST, "0.17.0"),
            "https://api.github.com/repos/crmne/zapfast/releases/tags/v0.17.0"
        );
        let asset =
            Url::parse("https://github.com/crmne/zapfast/releases/download/v0.17.0/checksums.txt")
                .unwrap();
        assert!(Source::github().owns_asset(&ZAPFAST, &asset, "0.17.0", "checksums.txt"));
        assert!(!Source::github().owns_asset(&ZAPFAST, &asset, "0.16.0", "checksums.txt"));
        let other = Url::parse(
            "https://github.com/crmne/spotifast/releases/download/v0.17.0/checksums.txt",
        )
        .unwrap();
        assert!(!Source::github().owns_asset(&ZAPFAST, &other, "0.17.0", "checksums.txt"));
    }

    #[test]
    fn local_feeds_must_stay_on_loopback() {
        let local = Source::local("http://127.0.0.1:8123/").unwrap();
        assert!(!local.is_github());
        assert_eq!(local.latest(&ZAPFAST), "http://127.0.0.1:8123/latest.json");
        assert_eq!(
            local.releases(&ZAPFAST, 1, 100).as_deref(),
            Some("http://127.0.0.1:8123/releases.json")
        );
        assert_eq!(local.releases(&ZAPFAST, 2, 100), None);
        assert!(local.allowed(&Url::parse("http://127.0.0.1:8123/package").unwrap()));
        assert!(!local.allowed(&Url::parse("http://127.0.0.1:9000/package").unwrap()));
        for address in [
            "https://127.0.0.1:8123",
            "http://example.com",
            "http://user@127.0.0.1:8123",
            "http://127.0.0.1:8123/?next=x",
        ] {
            assert!(Source::local(address).is_err(), "{address}");
        }
    }

    #[test]
    fn fetch_follows_allowed_redirects_and_sends_the_apps_user_agent() {
        let transport = FakeTransport::default()
            .redirect(
                "https://github.com/crmne/zapfast/releases/download/v1.0.0/checksums.txt",
                "https://release-assets.githubusercontent.com/blob?sig=1",
            )
            .serve(
                "https://release-assets.githubusercontent.com/blob?sig=1",
                b"body",
            );
        let mut body = String::new();
        fetch(
            &transport,
            &Source::github(),
            &ZAPFAST,
            "https://github.com/crmne/zapfast/releases/download/v1.0.0/checksums.txt",
            "application/octet-stream",
        )
        .unwrap()
        .read_to_string(&mut body)
        .unwrap();
        assert_eq!(body, "body");
        assert!(
            transport
                .user_agents()
                .iter()
                .all(|agent| agent == "ZapFast/0.16.3")
        );
    }

    #[test]
    fn fetch_refuses_redirects_off_the_release_hosts_before_requesting_them() {
        let transport = FakeTransport::default()
            .redirect(
                "https://api.github.com/repos/crmne/zapfast/releases/latest",
                "https://example.com/latest",
            )
            .serve("https://example.com/latest", b"{}");
        let error = fetch(
            &transport,
            &Source::github(),
            &ZAPFAST,
            "https://api.github.com/repos/crmne/zapfast/releases/latest",
            "application/json",
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("redirect"), "{error:#}");
        assert_eq!(transport.requested().len(), 1);
    }

    #[test]
    fn fetch_stops_redirect_loops_and_reports_errors() {
        let url = "https://github.com/loop";
        let transport = FakeTransport::default().redirect(url, url);
        assert!(fetch(&transport, &Source::github(), &ZAPFAST, url, "*/*").is_err());
        assert_eq!(transport.requested().len(), 6);
        let missing = FakeTransport::default();
        let error = fetch(&missing, &Source::github(), &ZAPFAST, url, "*/*")
            .err()
            .unwrap();
        assert!(error.to_string().contains("404"), "{error:#}");
    }
}
