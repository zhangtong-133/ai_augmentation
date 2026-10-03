use super::{AUTH, ChatGptClient, Error, RESOURCE, Result, body, now};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
fn random() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn issued(id: &str) -> bool {
    id.starts_with("oaiapp_")
        && id.len() <= 256
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Persistent registration. Deliberately does not implement Debug.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub client_id: String,
    pub subject: Option<String>,
    pub email: Option<String>,
    credentials: Option<Credentials>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: u64,
    scopes: Vec<String>,
}
impl Registration {
    fn pending(client_id: String) -> Self {
        Self {
            client_id,
            subject: None,
            email: None,
            credentials: None,
        }
    }
    #[must_use]
    pub fn signed_in(&self) -> bool {
        self.credentials.is_some()
    }
    #[must_use]
    pub fn plan_enabled(&self) -> bool {
        self.credentials.as_ref().is_some_and(|c| {
            ["resource.invoke", "chatgpt.tokens.use.direct"]
                .iter()
                .all(|s| c.scopes.iter().any(|v| v == s))
        })
    }
    /// # Errors
    /// Fails if not signed in or the saved access token has expired.
    pub fn needs_refresh(&self) -> Result<bool> {
        let c = self.credentials.as_ref().ok_or(Error("sign in first"))?;
        Ok(c.expires_at <= now()?.saturating_add(60))
    }
    /// Expiry of the currently authorized access credential, without exposing its value.
    /// # Errors
    /// Requires current plan/resource scopes and an unexpired access token.
    pub fn access_expires_at(&self) -> Result<u64> {
        self.access_token()?;
        self.credentials
            .as_ref()
            .map(|c| c.expires_at)
            .ok_or(Error("sign in first"))
    }
    pub(super) fn access_token(&self) -> Result<&str> {
        if !self.plan_enabled() {
            return Err(Error(
                "ChatGPT plan permission not granted; sign in and authorize usage",
            ));
        }
        let c = self.credentials.as_ref().ok_or(Error("sign in first"))?;
        if c.expires_at <= now()? {
            return Err(Error("session expired; refresh or sign in again"));
        }
        c.access_token
            .as_deref()
            .ok_or(Error("missing access token"))
    }
}

/// Single pending PKCE transaction. Consuming it prevents code replay.
#[cfg_attr(test, derive(Clone))]
pub struct PendingLogin {
    state: String,
    nonce: String,
    verifier: String,
    redirect: String,
    client_id: Option<String>,
    started_at: u64,
}
impl PendingLogin {
    /// The listener must already be bound to the supplied IPv4 loopback port.
    /// The URL intentionally omits optional ID-token hints so it is safe to display.
    /// # Errors
    /// Rejects malformed host/client identifiers and port zero.
    pub fn new(
        host: &str,
        port: u16,
        registration: Option<&Registration>,
    ) -> Result<(Self, String)> {
        let host_uuid = host
            .strip_prefix("urn:uuid:")
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or(Error("invalid host ID"))?;
        if host_uuid.get_version_num() != 4
            || host != host_uuid.urn().to_string()
            || port == 0
            || registration.is_some_and(|r| !issued(&r.client_id))
        {
            return Err(Error("invalid OAuth configuration"));
        }
        let pending = Self {
            state: random(),
            nonce: random(),
            verifier: random(),
            redirect: format!("http://127.0.0.1:{port}/auth/callback"),
            client_id: registration.map(|r| r.client_id.clone()),
            started_at: now()?,
        };
        let mut url = Url::parse(&format!("{AUTH}/api/accounts/authorize"))
            .map_err(|_| Error("invalid authorization endpoint"))?;
        url.query_pairs_mut().extend_pairs([
            (
                "client_id",
                pending
                    .client_id
                    .as_deref()
                    .unwrap_or("dynamic_agent_client"),
            ),
            ("ext_agent_host_id", host),
            ("response_type", "code"),
            ("redirect_uri", &pending.redirect),
            ("scope", SCOPES),
            ("resource", RESOURCE),
            ("state", &pending.state),
            ("nonce", &pending.nonce),
            ("code_challenge_method", "S256"),
            (
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(pending.verifier.as_bytes())),
            ),
        ]);
        if pending.client_id.is_none() {
            url.query_pairs_mut()
                .append_pair("agent_name_hint", "Personal AI Augmentation");
        }
        Ok((pending, url.into()))
    }

    /// Checks callback state without consuming the transaction (unrelated requests are ignored).
    #[must_use]
    pub fn matches_state(&self, query: &str) -> bool {
        callback_pairs(query).is_ok_and(|p| {
            p.get("state")
                .is_some_and(|v| bool::from(v.as_bytes().ct_eq(self.state.as_bytes())))
        })
    }

    /// Validate callback and retain the issued registration before exchanging its code.
    /// # Errors
    /// Rejects denial, expired/mismatched state, duplicated fields and client substitution.
    pub fn callback(self, query: &str) -> Result<CodeExchange> {
        if now()?.saturating_sub(self.started_at) > 300 || !self.matches_state(query) {
            return Err(Error("invalid or expired OAuth callback"));
        }
        let p = callback_pairs(query)?;
        if p.contains_key("error") {
            return Err(Error("ChatGPT authorization declined"));
        }
        let id = p
            .get("client_id")
            .or(self.client_id.as_ref())
            .filter(|id| issued(id))
            .ok_or(Error("missing issued client ID"))?;
        if self.client_id.as_ref().is_some_and(|old| old != id) {
            return Err(Error("OAuth client changed"));
        }
        let code = p
            .get("code")
            .filter(|s| !s.is_empty() && s.len() <= 8192)
            .ok_or(Error("missing authorization code"))?;
        Ok(CodeExchange {
            registration: Registration::pending(id.clone()),
            code: code.clone(),
            pending: self,
        })
    }
}
fn callback_pairs(query: &str) -> Result<std::collections::BTreeMap<String, String>> {
    if query.len() > 16384 {
        return Err(Error("callback too large"));
    }
    let url =
        Url::parse(&format!("http://127.0.0.1/?{query}")).map_err(|_| Error("invalid callback"))?;
    let mut values = std::collections::BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if values
            .insert(key.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(Error("duplicate callback field"));
        }
    }
    Ok(values)
}

pub struct CodeExchange {
    pub registration: Registration,
    code: String,
    pending: PendingLogin,
}
#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}
#[derive(Deserialize, Clone)]
struct Claims {
    sub: String,
    nonce: Option<String>,
    email: Option<String>,
    azp: Option<String>,
    aud: serde_json::Value,
}
#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
    revocation_endpoint: String,
}

fn credentials(
    token: TokenResponse,
    old: Option<&Credentials>,
    received_at: u64,
) -> Result<Credentials> {
    if token
        .access_token
        .as_ref()
        .is_some_and(|t| super::bearer(t).is_err())
        || token.access_token.is_some()
            && !token
                .token_type
                .as_deref()
                .is_some_and(|t| t.eq_ignore_ascii_case("Bearer"))
    {
        return Err(Error("invalid OAuth token response"));
    }
    let scopes = token.scope.map_or_else(
        || old.map_or_else(Vec::new, |c| c.scopes.clone()),
        |s| s.split_whitespace().map(str::to_owned).collect(),
    );
    let expires_at = if token.access_token.is_some() {
        received_at
            .checked_add(
                token
                    .expires_in
                    .filter(|v| *v > 0 && *v <= 604_800)
                    .ok_or(Error("invalid token expiry"))?,
            )
            .ok_or(Error("invalid token expiry"))?
    } else {
        received_at
    };
    Ok(Credentials {
        access_token: token.access_token,
        refresh_token: token
            .refresh_token
            .or_else(|| old.and_then(|c| c.refresh_token.clone())),
        expires_at,
        scopes,
    })
}
impl ChatGptClient {
    async fn discovery(&self) -> Result<Discovery> {
        let response = self
            .client
            .get(format!("{}/.well-known/openid-configuration", self.auth))
            .send()
            .await
            .map_err(|_| Error("OpenID discovery unavailable"))?;
        let d: Discovery = serde_json::from_slice(&body(response).await?)
            .map_err(|_| Error("invalid OpenID discovery"))?;
        if d.issuer != AUTH || !safe_auth_url(&d.jwks_uri) || !safe_auth_url(&d.revocation_endpoint)
        {
            return Err(Error("untrusted OpenID endpoint"));
        }
        Ok(d)
    }
    async fn verify(&self, jwt: &str, client_id: &str, nonce: Option<&str>) -> Result<Claims> {
        let discovery = self.discovery().await?;
        let response = self
            .client
            .get(discovery.jwks_uri)
            .send()
            .await
            .map_err(|_| Error("signing keys unavailable"))?;
        let keys: JwkSet = serde_json::from_slice(&body(response).await?)
            .map_err(|_| Error("invalid signing keys"))?;
        verify_claims(jwt, client_id, nonce, &keys)
    }
    /// Consumes a callback once. The caller must save the issued ID before calling.
    /// # Errors
    /// Rejects unverified identity, nonce/audience mismatch and account substitution.
    pub async fn exchange(
        &self,
        exchange: CodeExchange,
        previous: Option<&Registration>,
    ) -> Result<Registration> {
        if now()?.saturating_sub(exchange.pending.started_at) > 300 {
            return Err(Error("OAuth transaction expired; sign in again"));
        }
        let client_id = exchange.registration.client_id;
        if previous.is_some_and(|r| r.client_id != client_id) {
            return Err(Error("registration mismatch"));
        }
        let response = self
            .client
            .post(format!("{}/api/accounts/oauth/token", self.auth))
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", &client_id),
                ("code", &exchange.code),
                ("code_verifier", &exchange.pending.verifier),
                ("redirect_uri", &exchange.pending.redirect),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|_| Error("code exchange failed; start sign-in again"))?;
        let received_at = now()?;
        let token: TokenResponse = serde_json::from_slice(&body(response).await?)
            .map_err(|_| Error("invalid token response"))?;
        let claims = self
            .verify(
                token
                    .id_token
                    .as_deref()
                    .ok_or(Error("missing identity token"))?,
                &client_id,
                Some(&exchange.pending.nonce),
            )
            .await?;
        if previous
            .and_then(|r| r.subject.as_ref())
            .is_some_and(|s| s != &claims.sub)
        {
            return Err(Error("ChatGPT account changed"));
        }
        Ok(Registration {
            client_id,
            subject: Some(claims.sub),
            email: claims.email,
            credentials: Some(credentials(token, None, received_at)?),
        })
    }
    /// Serialize refresh and persistence externally; never retry a rotating token automatically.
    /// # Errors
    /// Expired/revoked grants require a new sign-in; existing credentials remain on failure.
    pub async fn refresh(&self, registration: &mut Registration) -> Result<()> {
        let old = registration
            .credentials
            .as_ref()
            .ok_or(Error("sign in first"))?;
        let refresh = old
            .refresh_token
            .as_deref()
            .ok_or(Error("sign in again; no refresh token"))?;
        let response = self
            .client
            .post(format!("{}/api/accounts/oauth/token", self.auth))
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &registration.client_id),
                ("refresh_token", refresh),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|_| Error("refresh outcome unknown; sign in again"))?;
        let received_at = now()?;
        let token: TokenResponse = serde_json::from_slice(&body(response).await?)
            .map_err(|_| Error("invalid refresh response"))?;
        if token.access_token.is_none() {
            return Err(Error("missing refreshed access token"));
        }
        if let Some(jwt) = &token.id_token {
            let claims = self.verify(jwt, &registration.client_id, None).await?;
            if registration.subject.as_ref() != Some(&claims.sub) {
                return Err(Error("refreshed account changed"));
            }
        }
        registration.credentials = Some(credentials(token, Some(old), received_at)?);
        Ok(())
    }
    /// Clears local credentials even when remote revocation fails; retains registration identity.
    /// # Errors
    /// Reports unconfirmed revocation, so users can disconnect in `ChatGPT` Settings.
    pub async fn sign_out(&self, registration: &mut Registration) -> Result<()> {
        let credentials = registration.credentials.take();
        let Some(refresh) = credentials.and_then(|c| c.refresh_token) else {
            return Ok(());
        };
        let d = self.discovery().await?;
        let response = self
            .client
            .post(d.revocation_endpoint)
            .form(&[
                ("token", refresh.as_str()),
                ("token_type_hint", "refresh_token"),
                ("client_id", registration.client_id.as_str()),
            ])
            .send()
            .await
            .map_err(|_| Error("remote revocation unconfirmed; disconnect in ChatGPT Settings"))?;
        if response.status().as_u16() != 200 {
            return Err(Error(
                "remote revocation unconfirmed; disconnect in ChatGPT Settings",
            ));
        }
        Ok(())
    }
}
fn safe_auth_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("auth.openai.com")
            && u.port_or_known_default() == Some(443)
            && u.username().is_empty()
            && u.password().is_none()
            && u.fragment().is_none()
    })
}
fn verify_claims(jwt: &str, client_id: &str, nonce: Option<&str>, keys: &JwkSet) -> Result<Claims> {
    let invalid = || Error("identity token verification failed");
    let header = decode_header(jwt).map_err(|_| invalid())?;
    if header.alg != Algorithm::RS256 {
        return Err(invalid());
    }
    let key = keys
        .find(header.kid.as_deref().ok_or_else(invalid)?)
        .ok_or_else(invalid)?;
    let key = DecodingKey::from_jwk(key).map_err(|_| invalid())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[AUTH]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.leeway = 0;
    validation.validate_nbf = true;
    let claims = decode::<Claims>(jwt, &key, &validation)
        .map_err(|_| invalid())?
        .claims;
    if claims.aud.as_array().is_some_and(|aud| aud.len() > 1)
        && claims.azp.as_deref() != Some(client_id)
    {
        return Err(invalid());
    }
    if claims.sub.is_empty()
        || claims.azp.as_ref().is_some_and(|a| a != client_id)
        || nonce.is_some_and(|n| {
            claims
                .nonce
                .as_deref()
                .is_none_or(|v| !bool::from(v.as_bytes().ct_eq(n.as_bytes())))
        })
    {
        return Err(invalid());
    }
    Ok(claims)
}

#[cfg(test)]
mod tests;
