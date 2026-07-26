//! On-demand lyric translation against a user-supplied endpoint.
//!
//! Nothing here runs unless the user both configures `[lyrics] translate_url`
//! and presses the key: translating means sending the words of whatever they are
//! listening to to a third-party service, which is not a thing to do quietly.
//!
//! The wire format is LibreTranslate's, which self-hosts and which several other
//! services imitate.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::lyrics::{Line, Lyrics};
use crate::config::LyricsConfig;

/// Sent as one request for the whole track: line-by-line requests would be
/// dozens of round trips and would lose the context that makes them readable.
#[derive(Serialize)]
struct Request<'a> {
    q: Vec<&'a str>,
    source: &'a str,
    target: &'a str,
    format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<&'a str>,
}

#[derive(Deserialize)]
struct Response {
    /// A batch request answers with a list, a single one with a string.
    #[serde(rename = "translatedText")]
    translated: Translated,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Translated {
    Many(Vec<String>),
    One(String),
}

/// Translate one variant, preserving its timings so the result still scrolls.
pub async fn translate(
    http: &reqwest::Client,
    config: &LyricsConfig,
    lyrics: &Lyrics,
) -> Result<Lyrics> {
    if !config.translation_enabled() {
        bail!("no translation endpoint configured");
    }
    if lyrics.is_empty() {
        bail!("nothing to translate");
    }

    // Blank lines are instrumental gaps: sending them wastes a slot and some
    // endpoints echo something unhelpful back.
    let indices: Vec<usize> = lyrics
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.text.trim().is_empty())
        .map(|(index, _)| index)
        .collect();
    let batch: Vec<&str> = indices
        .iter()
        .map(|index| lyrics.lines[*index].text.as_str())
        .collect();
    if batch.is_empty() {
        bail!("nothing to translate");
    }

    let api_key = config.translate_api_key.trim();
    let request = Request {
        q: batch,
        // "auto" asks the endpoint to detect the language, which is the point:
        // the user should not have to tell us what they are listening to.
        source: "auto",
        target: config.translate_to.trim(),
        format: "text",
        api_key: (!api_key.is_empty()).then_some(api_key),
    };

    let response = http
        .post(config.translate_url.trim())
        .json(&request)
        .send()
        .await
        .context("contacting the translation endpoint")?;
    if !response.status().is_success() {
        bail!("translation endpoint returned {}", response.status());
    }
    let response: Response = response
        .json()
        .await
        .context("reading the translation endpoint's reply")?;

    let translated = match response.translated {
        Translated::Many(many) => many,
        Translated::One(one) => vec![one],
    };
    if translated.len() != indices.len() {
        bail!(
            "translation endpoint returned {} lines for {}",
            translated.len(),
            indices.len()
        );
    }

    // Rebuild the variant in place: same timings, same gaps, new words.
    let mut lines = lyrics.lines.clone();
    for (index, text) in indices.into_iter().zip(translated) {
        lines[index] = Line {
            at: lines[index].at,
            text,
        };
    }

    Ok(Lyrics {
        synced: lyrics.synced,
        lines,
        lang: Some(format!("{} (machine)", config.translate_to.trim())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn config() -> LyricsConfig {
        LyricsConfig {
            translate_url: "http://localhost:5000/translate".into(),
            translate_api_key: String::new(),
            translate_to: "en".into(),
        }
    }

    fn lyrics() -> Lyrics {
        Lyrics {
            synced: true,
            lines: vec![
                Line {
                    at: Duration::ZERO,
                    text: "uno".into(),
                },
                Line {
                    at: Duration::from_secs(2),
                    text: String::new(),
                },
                Line {
                    at: Duration::from_secs(4),
                    text: "dos".into(),
                },
            ],
            lang: Some("es".into()),
        }
    }

    #[tokio::test]
    async fn an_unconfigured_endpoint_is_refused_before_any_request() {
        let mut config = config();
        config.translate_url = "  ".into();
        let error = translate(&reqwest::Client::new(), &config, &lyrics())
            .await
            .expect_err("should refuse");
        assert!(error.to_string().contains("no translation endpoint"));
    }

    #[tokio::test]
    async fn empty_lyrics_are_not_sent_anywhere() {
        assert!(
            translate(&reqwest::Client::new(), &config(), &Lyrics::default())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn lyrics_that_are_all_gaps_are_not_sent_anywhere() {
        let mut all_gaps = lyrics();
        for line in &mut all_gaps.lines {
            line.text = "  ".into();
        }
        let error = translate(&reqwest::Client::new(), &config(), &all_gaps)
            .await
            .expect_err("should refuse");
        assert!(error.to_string().contains("nothing to translate"));
    }

    #[test]
    fn a_batch_reply_and_a_single_reply_both_parse() {
        let many: Response =
            serde_json::from_str(r#"{"translatedText":["one","two"]}"#).expect("batch");
        assert!(matches!(many.translated, Translated::Many(v) if v.len() == 2));
        let one: Response = serde_json::from_str(r#"{"translatedText":"one"}"#).expect("single");
        assert!(matches!(one.translated, Translated::One(_)));
    }

    #[test]
    fn the_request_omits_an_unset_api_key() {
        let request = Request {
            q: vec!["hola"],
            source: "auto",
            target: "en",
            format: "text",
            api_key: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(!json.contains("api_key"), "{json}");
    }
}
