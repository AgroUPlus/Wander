use reqwest::{Client, RequestBuilder};

pub fn external_request(
    client: &Client,
    agro_config: Option<&crate::config::AgroConfig>,
    method: reqwest::Method,
    url: &str,
) -> RequestBuilder {
    if let Some(config) = agro_config {
        if config.enabled && config.proxy_enabled && !config.server.is_empty() && (!config.passphrase.is_empty() || !config.device_token.is_empty()) {
            let proxy_url = if config.server.ends_with('/') {
                format!("{}api/v1/proxy", config.server)
            } else {
                format!("{}/api/v1/proxy", config.server)
            };
            let token = if !config.device_token.is_empty() { &config.device_token } else { &config.passphrase };
            return client
                .request(method, proxy_url)
                .header("X-Agro-Proxy-Url", url)
                .bearer_auth(token);
        }
    }
    client.request(method, url)
}
