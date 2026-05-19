//! PKCE OAuth flow against Anthropic for the `@anthropic-ai/claude-code` CLI's
//! public OAuth app (used by `claude /login`).
//!
//! Constants and flow shape: see research.md §2 and §3 in the feature spec.

use std::{collections::HashMap, sync::Mutex, time::Instant};

use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use uuid::Uuid;

use super::types::ClaudeOAuthCredentials;

const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const PRIMARY_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const FALLBACK_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const DEFAULT_SCOPES: &[&str] = &["user:inference", "user:profile"];
const STATE_TTL_SECONDS: u64 = 10 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeOAuthStartResponse {
    pub auth_url: String,
    pub state: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeOAuthStartRequest {
    #[serde(default)]
    pub account_id: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ClaudeOAuthCompleteRequest {
    pub state: String,
    pub code: String,
}

/// Account metadata captured from Anthropic's OAuth response (best-effort).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthAccountInfo {
    pub uuid: Option<String>,
    pub email: Option<String>,
    pub organization_uuid: Option<String>,
}

#[derive(Debug)]
pub struct PendingOAuthState {
    pub code_verifier: String,
    pub account_id: Option<Uuid>,
    pub created_at: Instant,
}

#[derive(Debug, thiserror::Error)]
pub enum ClaudeOAuthError {
    #[error("unknown or expired oauth state")]
    UnknownState,
    #[error("anthropic token endpoint rejected the code: {0}")]
    CodeExchangeFailed(String),
    #[error("anthropic token endpoint returned an unexpected response: {0}")]
    BadResponse(String),
    #[error("network error talking to anthropic: {0}")]
    Network(#[from] reqwest::Error),
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    account: Option<TokenAccountField>,
}

#[derive(Debug, Deserialize)]
struct TokenAccountField {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    email_address: Option<String>,
    #[serde(default)]
    organization_uuid: Option<String>,
}

pub struct ClaudeOAuthClient {
    pending: Mutex<HashMap<String, PendingOAuthState>>,
    http: Client,
}

impl Default for ClaudeOAuthClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeOAuthClient {
    pub fn new() -> Self {
        // Bound the OAuth network calls so a stalled Anthropic endpoint
        // produces a real error rather than hanging the enrollment flow
        // forever. Matches the conservative defaults used elsewhere
        // (analytics, releases) — 10s to connect, 30s total.
        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client builds with valid defaults");
        Self {
            pending: Mutex::new(HashMap::new()),
            http,
        }
    }

    /// Begin a PKCE flow. Returns the URL the user must visit and an opaque
    /// state token to pass back to `complete()`.
    pub fn start(&self, account_id: Option<Uuid>) -> ClaudeOAuthStartResponse {
        let (verifier, challenge) = generate_pkce_pair();
        let state = random_state_token();

        let auth_url = format!(
            "{authorize}?response_type=code&client_id={client_id}&redirect_uri={redirect}&scope={scope}&state={state}&code_challenge={challenge}&code_challenge_method=S256",
            authorize = AUTHORIZE_URL,
            client_id = CLIENT_ID,
            redirect = urlencoding::encode(REDIRECT_URI),
            scope = urlencoding::encode(&DEFAULT_SCOPES.join(" ")),
            state = state,
            challenge = challenge,
        );

        let now = Instant::now();
        let mut guard = self.pending.lock().expect("oauth state mutex");
        // GC expired entries opportunistically.
        guard.retain(|_, entry| now.duration_since(entry.created_at).as_secs() < STATE_TTL_SECONDS);
        guard.insert(
            state.clone(),
            PendingOAuthState {
                code_verifier: verifier,
                account_id,
                created_at: now,
            },
        );

        ClaudeOAuthStartResponse { auth_url, state }
    }

    /// Complete the flow by exchanging the code for tokens. Returns the new
    /// credential bundle and (when supplied by Anthropic) account metadata.
    pub async fn complete(
        &self,
        state: &str,
        code: &str,
    ) -> Result<
        (
            ClaudeOAuthCredentials,
            Option<OauthAccountInfo>,
            Option<Uuid>,
        ),
        ClaudeOAuthError,
    > {
        let pending = {
            let mut guard = self.pending.lock().expect("oauth state mutex");
            guard.remove(state).ok_or(ClaudeOAuthError::UnknownState)?
        };
        if pending.created_at.elapsed().as_secs() >= STATE_TTL_SECONDS {
            return Err(ClaudeOAuthError::UnknownState);
        }

        let body = serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": REDIRECT_URI,
            "client_id": CLIENT_ID,
            "code_verifier": pending.code_verifier,
        });

        let token = self.exchange_with_fallback(&body).await?;
        let (creds, info) = token_response_into_credentials(token);
        Ok((creds, info, pending.account_id))
    }

    /// Refresh an account's access token using its refresh token. Rotates the
    /// refresh token (Anthropic issues a new one on every refresh).
    pub async fn refresh(
        &self,
        creds: &mut ClaudeOAuthCredentials,
    ) -> Result<(), ClaudeOAuthError> {
        // Explicit clone — the `json!` macro serializes by reference under
        // the hood, but cloning here makes the intent obvious and protects
        // against a future maintainer accidentally moving out of `*creds`.
        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": creds.refresh_token.clone(),
            "client_id": CLIENT_ID,
        });
        let token = self.exchange_with_fallback(&body).await?;
        let (new_creds, _) = token_response_into_credentials(token);
        *creds = new_creds;
        Ok(())
    }

    async fn exchange_with_fallback(
        &self,
        body: &serde_json::Value,
    ) -> Result<TokenResponse, ClaudeOAuthError> {
        let primary = std::env::var("VIBE_KANBAN_CLAUDE_OAUTH_TOKEN_URL")
            .unwrap_or_else(|_| PRIMARY_TOKEN_URL.to_string());
        match self.exchange_at(&primary, body).await {
            Ok(t) => Ok(t),
            Err(ClaudeOAuthError::Network(e)) if e.is_connect() => {
                tracing::warn!(
                    "primary claude oauth host failed ({}), trying fallback",
                    primary
                );
                self.exchange_at(FALLBACK_TOKEN_URL, body).await
            }
            Err(other) => Err(other),
        }
    }

    async fn exchange_at(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<TokenResponse, ClaudeOAuthError> {
        let resp = self
            .http
            .post(url)
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(ClaudeOAuthError::CodeExchangeFailed(format!(
                "status={status} body={text}"
            )));
        }
        let token: TokenResponse = resp
            .json()
            .await
            .map_err(|e| ClaudeOAuthError::BadResponse(e.to_string()))?;
        Ok(token)
    }
}

fn token_response_into_credentials(
    token: TokenResponse,
) -> (ClaudeOAuthCredentials, Option<OauthAccountInfo>) {
    let expires_in = token.expires_in.unwrap_or(8 * 3600);
    let creds = ClaudeOAuthCredentials {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: Utc::now() + ChronoDuration::seconds(expires_in),
        scopes: token
            .scope
            .map(|s| s.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default(),
    };
    let info = token.account.map(|a| OauthAccountInfo {
        uuid: a.uuid,
        email: a.email_address,
        organization_uuid: a.organization_uuid,
    });
    (creds, info)
}

fn generate_pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
    (verifier, challenge)
}

fn random_state_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_returns_url_with_required_query_params() {
        let client = ClaudeOAuthClient::new();
        let resp = client.start(None);
        assert!(
            resp.auth_url
                .starts_with("https://claude.ai/oauth/authorize?")
        );
        assert!(resp.auth_url.contains("response_type=code"));
        assert!(resp.auth_url.contains(&format!("client_id={CLIENT_ID}")));
        assert!(resp.auth_url.contains("code_challenge_method=S256"));
        assert!(resp.auth_url.contains(&format!("state={}", resp.state)));
    }

    #[test]
    fn pkce_pair_is_deterministically_related() {
        let (verifier, challenge) = generate_pkce_pair();
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let recomputed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(challenge, recomputed);
    }

    #[test]
    fn unknown_state_is_rejected() {
        let client = ClaudeOAuthClient::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let err = runtime
            .block_on(client.complete("never-issued", "code"))
            .unwrap_err();
        assert!(matches!(err, ClaudeOAuthError::UnknownState));
    }
}
