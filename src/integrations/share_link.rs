//! Sending share links out on a domain of the user's own.
//!
//! Navidrome's `createShare` returns a link on that server, which plays for anyone who can reach
//! it and for nobody else. Rewriting it onto a domain running the `/listen` forwarder — Agro's
//! route, or the static page — gives the recipient something that resolves wherever they are.
//!
//! The rewrite is silent in both directions. A link that can be carried is; anything else is
//! shared exactly as its backend minted it, with no warning and no refusal.
//!
//! That silence is what makes the allowlist safe to enforce strictly: a forwarder that will send a
//! visitor to any address handed to it is an open redirect wearing the user's domain. So only
//! links this app could have produced are ever wrapped.

/// Hosts that need no configuring — the ones a music player mints links for.
const DEFAULT_HOSTS: &[&str] = &[
    "music.youtube.com",
    "youtube.com",
    "www.youtube.com",
    "youtu.be",
];

/// Where a share link should be carried, and what may be carried there.
#[derive(Debug, Clone, Default)]
pub struct ShareDomain {
    /// Bare host. Empty disables rewriting entirely.
    pub domain: String,
    /// Hosts allowed on top of [`DEFAULT_HOSTS`] — the user's own music server, usually.
    pub hosts: Vec<String>,
}

impl ShareDomain {
    /// The link to hand the user, minting a short UID via Agro when paired.
    pub async fn rewrite_async(
        &self,
        url: &str,
        agro: Option<&crate::integrations::agro::AgroClient>,
        expires_ms: Option<i64>,
    ) -> String {
        if self.domain.trim().is_empty() {
            return url.to_string();
        }
        let Some(host) = https_host(url) else {
            return url.to_string();
        };
        if !self.allows(&host) {
            return url.to_string();
        }
        let domain = self.domain.trim().trim_matches('/');

        // When Agro is paired, mint a short UID with synchronized TTL so all links use clean ?id=<uid>
        if let Some(agro_client) = agro {
            if let Ok(uid) = agro_client.create_short_link(url, expires_ms).await {
                if !uid.trim().is_empty() {
                    return format!("https://{domain}/listen?id={uid}");
                }
            }
        }

        // Fallback for YouTube links when Agro is not paired
        if let Some(video_id) = youtube_video_id(url) {
            return format!("https://{domain}/listen?v={video_id}");
        }

        format!("https://{domain}/listen?u={}", urlencoding::encode(url))
    }

    /// The link to hand the user: wrapped when it can be, untouched when it cannot.
    pub fn rewrite(&self, url: &str) -> String {
        if self.domain.trim().is_empty() {
            return url.to_string();
        }
        let Some(host) = https_host(url) else {
            return url.to_string();
        };
        if !self.allows(&host) {
            return url.to_string();
        }
        let domain = self.domain.trim().trim_matches('/');

        // A YouTube id travels in the open: it is public already, and it is what lets the
        // forwarding page offer "open in YouTube Music" without unpacking anything.
        if let Some(video_id) = youtube_video_id(url) {
            return format!("https://{domain}/listen?v={video_id}");
        }
        format!("https://{domain}/listen?u={}", urlencoding::encode(url))
    }

    fn allows(&self, host: &str) -> bool {
        DEFAULT_HOSTS.contains(&host)
            || self
                .hosts
                .iter()
                .any(|allowed| allowed.trim().to_lowercase() == host)
    }
}

/// The host of an `https` URL, lowercased.
///
/// `http` is refused: a downgrade the recipient never agreed to. So is a URL carrying credentials
/// in its authority, which is how a link is made to read as one host while resolving to another.
fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?.to_lowercase();
    (!host.is_empty() && host.contains('.')).then_some(host)
}

/// The video id in a YouTube or YouTube Music link, if it names a single video.
///
/// A channel, playlist or search URL has no track behind it. Ids are a fixed eleven characters of
/// URL-safe base64, and checking that is what stops a decorated path segment from being pasted
/// into a URL.
fn youtube_video_id(url: &str) -> Option<String> {
    let host = https_host(url)?;
    let path = url
        .strip_prefix("https://")?
        .split_once('/')
        .map(|(_, rest)| rest)
        .unwrap_or("");
    let (path, query) = match path.split_once('?') {
        Some((path, query)) => (path, query),
        None => (path, ""),
    };

    let candidate = match host.as_str() {
        "youtu.be" => path.split('/').next().map(str::to_string),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            let mut segments = path.split('/');
            match segments.next() {
                Some("watch") => query
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("v="))
                    .map(str::to_string),
                Some("shorts") | Some("embed") | Some("v") => segments.next().map(str::to_string),
                _ => None,
            }
        }
        _ => None,
    }?;

    is_video_id(&candidate).then_some(candidate)
}

fn is_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain() -> ShareDomain {
        ShareDomain {
            domain: "frwd.top".into(),
            hosts: vec!["music.example.com".into()],
        }
    }

    #[test]
    fn carries_a_youtube_link_by_id() {
        assert_eq!(
            domain().rewrite("https://music.youtube.com/watch?v=aBcDeFgHiJk"),
            "https://frwd.top/listen?v=aBcDeFgHiJk"
        );
    }

    #[test]
    fn carries_an_allowed_host_by_url() {
        assert_eq!(
            domain().rewrite("https://music.example.com/share/xyz"),
            "https://frwd.top/listen?u=https%3A%2F%2Fmusic.example.com%2Fshare%2Fxyz"
        );
    }

    #[test]
    fn leaves_everything_else_alone() {
        let untouched = "https://elsewhere.example/track/1";
        assert_eq!(domain().rewrite(untouched), untouched);

        // Not https, so not carried.
        let plain = "http://music.example.com/share/xyz";
        assert_eq!(domain().rewrite(plain), plain);

        // The authority here is `evil.example`, however much it reads as youtube.com.
        let disguised = "https://music.youtube.com@evil.example/x";
        assert_eq!(domain().rewrite(disguised), disguised);
    }

    #[test]
    fn no_domain_means_no_rewriting() {
        let url = "https://music.youtube.com/watch?v=aBcDeFgHiJk";
        assert_eq!(ShareDomain::default().rewrite(url), url);
    }

    #[test]
    fn a_link_with_no_single_track_is_not_wrapped_as_one() {
        let playlist = "https://music.youtube.com/playlist?list=OLAK5uy_abc";
        assert_eq!(
            domain().rewrite(playlist),
            format!(
                "https://frwd.top/listen?u={}",
                urlencoding::encode(playlist)
            )
        );
    }
}
