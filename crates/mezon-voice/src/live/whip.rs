use std::time::Duration;

use reqwest::Url;

use super::{LiveEndpoint, LiveFailure};

const WHIP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const WHIP_DELETE_TIMEOUT: Duration = Duration::from_secs(3);
const WHIP_DELETE_RETRY_DELAY: Duration = Duration::from_millis(150);

pub(super) struct WhipSession {
    pub resource_url: Url,
    pub answer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WhipError {
    Terminal {
        failure: LiveFailure,
        status: u16,
    },
    Retry {
        after: Option<Duration>,
        status: Option<u16>,
    },
}

pub(super) fn build_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(WHIP_REQUEST_TIMEOUT)
        .timeout(WHIP_REQUEST_TIMEOUT)
        .build()
}

pub(super) async fn publish(
    client: &reqwest::Client,
    endpoint: &LiveEndpoint,
    offer: &str,
) -> Result<WhipSession, WhipError> {
    let whip_url = Url::parse(&endpoint.whip_url)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
        .ok_or(WhipError::Terminal {
            failure: LiveFailure::Unsupported,
            status: 400,
        })?;

    let response = match tokio::time::timeout(
        WHIP_REQUEST_TIMEOUT,
        client
            .post(whip_url.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/sdp")
            .bearer_auth(&endpoint.token)
            .body(offer.to_owned())
            .send(),
    )
    .await
    {
        Ok(Ok(resp)) => resp,
        Ok(Err(err)) => {
            tracing::warn!("whip publish transport error: {}", err.without_url());
            return Err(WhipError::Retry {
                after: None,
                status: None,
            });
        }
        Err(_) => {
            tracing::warn!("whip publish timed out");
            return Err(WhipError::Retry {
                after: None,
                status: None,
            });
        }
    };

    let status = response.status().as_u16();
    if status != 201 {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok());
        let classified = classify(status, retry_after);
        tracing::warn!(status, "whip publish rejected");
        return Err(classified);
    }

    let Some(resource_url) = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|loc| resolve_location(&whip_url, loc))
    else {
        tracing::warn!(status, "whip 201 missing valid Location header");
        return Err(WhipError::Retry {
            after: None,
            status: Some(status),
        });
    };

    let answer = match tokio::time::timeout(WHIP_REQUEST_TIMEOUT, response.text()).await {
        Ok(Ok(body)) if !body.trim().is_empty() => body,
        Ok(Ok(_)) => {
            tracing::warn!(status, "whip 201 returned empty answer");
            return Err(WhipError::Retry {
                after: None,
                status: Some(status),
            });
        }
        Ok(Err(err)) => {
            tracing::warn!("whip answer body read failed: {}", err.without_url());
            return Err(WhipError::Retry {
                after: None,
                status: None,
            });
        }
        Err(_) => {
            tracing::warn!("whip answer body read timed out");
            return Err(WhipError::Retry {
                after: None,
                status: None,
            });
        }
    };

    tracing::info!(
        status,
        offer_bytes = offer.len(),
        answer_bytes = answer.len(),
        "whip publish accepted"
    );

    Ok(WhipSession {
        resource_url,
        answer,
    })
}

pub(super) async fn delete(client: &reqwest::Client, resource_url: &Url, token: &str) {
    let _ = tokio::time::timeout(WHIP_DELETE_TIMEOUT, async {
        for attempt in 0..2 {
            match client
                .delete(resource_url.clone())
                .bearer_auth(token)
                .send()
                .await
            {
                Ok(resp) if resp.status().as_u16() == 503 && attempt == 0 => {
                    tokio::time::sleep(WHIP_DELETE_RETRY_DELAY).await;
                }
                Ok(resp) => {
                    tracing::info!(status = resp.status().as_u16(), "whip resource deleted");
                    break;
                }
                Err(err) => {
                    tracing::debug!("whip delete transport error: {}", err.without_url());
                    break;
                }
            }
        }
    })
    .await;
}

pub(super) fn classify(status: u16, retry_after: Option<&str>) -> WhipError {
    match status {
        409 => WhipError::Terminal {
            failure: LiveFailure::ChannelBusy,
            status,
        },
        401 | 403 => WhipError::Terminal {
            failure: LiveFailure::Forbidden,
            status,
        },
        400 | 406 => WhipError::Terminal {
            failure: LiveFailure::Unsupported,
            status,
        },
        503 => WhipError::Retry {
            after: retry_after
                .and_then(|value| value.trim().parse::<u64>().ok())
                .map(Duration::from_secs),
            status: Some(status),
        },
        _ => WhipError::Retry {
            after: None,
            status: Some(status),
        },
    }
}

pub(super) fn resolve_location(whip_url: &Url, location: &str) -> Option<Url> {
    let trimmed = location.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(char::is_whitespace)
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    let resolved = whip_url.join(trimmed).ok()?;
    if !matches!(resolved.scheme(), "http" | "https") || !resolved.has_host() {
        return None;
    }
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_terminal_and_retry_statuses() {
        assert_eq!(
            classify(409, None),
            WhipError::Terminal {
                failure: LiveFailure::ChannelBusy,
                status: 409
            }
        );
        assert_eq!(
            classify(401, None),
            WhipError::Terminal {
                failure: LiveFailure::Forbidden,
                status: 401
            }
        );
        assert_eq!(
            classify(403, None),
            WhipError::Terminal {
                failure: LiveFailure::Forbidden,
                status: 403
            }
        );
        assert_eq!(
            classify(400, None),
            WhipError::Terminal {
                failure: LiveFailure::Unsupported,
                status: 400
            }
        );
        assert_eq!(
            classify(406, None),
            WhipError::Terminal {
                failure: LiveFailure::Unsupported,
                status: 406
            }
        );
        assert_eq!(
            classify(503, Some("7")),
            WhipError::Retry {
                after: Some(Duration::from_secs(7)),
                status: Some(503)
            }
        );
        assert_eq!(
            classify(503, Some(" invalid ")),
            WhipError::Retry {
                after: None,
                status: Some(503)
            }
        );
        assert_eq!(
            classify(503, None),
            WhipError::Retry {
                after: None,
                status: Some(503)
            }
        );
        assert_eq!(
            classify(500, Some("10")),
            WhipError::Retry {
                after: None,
                status: Some(500)
            }
        );
        assert_eq!(
            classify(404, None),
            WhipError::Retry {
                after: None,
                status: Some(404)
            }
        );
    }

    #[test]
    fn resolve_location_handles_relative_absolute_and_garbage() {
        let base = Url::parse("https://stream.example.com/whip/987654321").unwrap();

        let rel = resolve_location(&base, "/resource/0.1.0123456789abcdef").unwrap();
        assert_eq!(
            rel.as_str(),
            "https://stream.example.com/resource/0.1.0123456789abcdef"
        );

        let abs =
            resolve_location(&base, "https://node2.example.com:8443/resource/2.5.fedcba").unwrap();
        assert_eq!(
            abs.as_str(),
            "https://node2.example.com:8443/resource/2.5.fedcba"
        );

        assert!(resolve_location(&base, "").is_none());
        assert!(resolve_location(&base, "   ").is_none());
        assert!(resolve_location(&base, "not a valid url").is_none());
        assert!(resolve_location(&base, "http://[::1").is_none());
        assert!(resolve_location(&base, "ftp://stream.example.com/resource/1").is_none());
        assert!(resolve_location(&base, "javascript:alert(1)").is_none());
    }
}
