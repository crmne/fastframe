//! A [`Transport`] over a reqwest blocking client.

use std::time::Duration;

use anyhow::Result;
use reqwest::header::{ACCEPT, LOCATION, USER_AGENT};

use crate::transport::{Request, Response, Transport};

/// The app's reqwest client, set up the way the updater needs it.
#[derive(Clone, Debug)]
pub struct ReqwestTransport(reqwest::blocking::Client);

impl ReqwestTransport {
    /// Builds the client from the app's builder, which carries its proxy
    /// and TLS choices. Redirects are turned off (the updater follows them
    /// itself, only to release hosts), with a 15 second connect timeout and
    /// 15 minutes for a whole download.
    pub fn new(builder: reqwest::blocking::ClientBuilder) -> reqwest::Result<Self> {
        builder
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(15 * 60))
            .build()
            .map(Self)
    }
}

impl Transport for ReqwestTransport {
    fn get(&self, request: &Request<'_>) -> Result<Response> {
        let response = self
            .0
            .get(request.url)
            .header(ACCEPT, request.accept)
            .header(USER_AGENT, request.user_agent)
            .send()?;
        Ok(Response {
            status: response.status().as_u16(),
            location: response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            body: Box::new(response),
        })
    }
}
