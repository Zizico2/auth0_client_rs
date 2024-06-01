//! Types, traits and functions relative to authentication process.

use async_trait::async_trait;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, TokenData, Validation};
use serde::Deserialize;
use std::collections::HashMap;

use crate::error::{Auth0Result, Error};
use crate::utils::URL_REGEX;
use crate::Auth0Client;

/// Trait for authenticating an Auth0 client.
#[async_trait]
pub trait Authenticatable {
    /// Authenticates the client from its configuration.
    ///
    /// # Example
    ///
    /// ```
    /// # async fn new_client() -> auth0_client::error::Auth0Result<()> {
    /// # use auth0_client::authorization::Authenticatable;
    /// let mut client =
    ///     auth0_client::Auth0Client::new("client_id", "client_secret", "domain", "audience");
    ///
    /// client.authenticate().await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn authenticate(&mut self) -> Auth0Result<String>;

    /// Authenticates the a user from its password.
    ///
    /// # Example
    ///
    /// ```
    /// # async fn new_client() -> auth0_client::error::Auth0Result<()> {
    /// # use auth0_client::authorization::Authenticatable;
    /// let mut client =
    ///     auth0_client::Auth0Client::new("client_id", "client_secret", "domain", "audience");
    ///
    /// client.authenticate_user("user@email.com".to_string(), "password".to_string()).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn authenticate_user(&mut self, username: String, password: String) -> Auth0Result<()>;

    /// Calls an authentication request with body
    async fn authenticate_with_body(
        &mut self,
        body: HashMap<&str, String>,
    ) -> Auth0Result<AccessTokenResponse>;

    /// Returns the access token if autenticated or `None` if it is not.
    fn access_token(&self) -> Option<String>;
}

/// The token type we use to authenticate.
#[derive(Deserialize)]
enum TokenType {
    Bearer,
}

/// The response we get when we authenticate.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AccessTokenResponse {
    pub access_token: String,
}

#[async_trait]
impl Authenticatable for Auth0Client {
    async fn authenticate(&mut self) -> Auth0Result<String> {
        let url = URL_REGEX
            .replace_all(&format!("{}/oauth/token", self.domain), "$1")
            .to_string();

        tracing::debug!("Starting authentication at {url}...");

        let body = {
            let mut body = std::collections::HashMap::new();

            body.insert("grant_type", self.grant_type.to_string());
            body.insert("client_id", self.client_id.clone());
            body.insert("client_secret", self.client_secret.clone());
            body.insert("audience", self.audience.clone());
            body
        };

        let response = self.authenticate_with_body(body).await?;

        self.access_token = Some(response.access_token.clone());
        Ok(response.access_token)
    }

    async fn authenticate_user(&mut self, username: String, password: String) -> Auth0Result<()> {
        let url = URL_REGEX
            .replace_all(&format!("{}/oauth/token", self.domain), "$1")
            .to_string();

        tracing::debug!("Starting authentication at {url}...");

        let body = {
            let mut body = HashMap::new();

            body.insert("grant_type", self.grant_type.to_string());
            body.insert("client_id", self.client_id.clone());
            body.insert("client_secret", self.client_secret.clone());
            body.insert("audience", self.audience.clone());
            body.insert("username", username);
            body.insert("password", password);
            body
        };

        self.authenticate_with_body(body).await?;

        Ok(())
    }

    async fn authenticate_with_body(
        &mut self,
        body: HashMap<&str, String>,
    ) -> Auth0Result<AccessTokenResponse> {
        let url = URL_REGEX
            .replace_all(&format!("{}/oauth/token", self.domain), "$1")
            .to_string();

        tracing::debug!("Starting authentication at {url}...");

        let response = self.http_client.post(&url).json(&body).send().await?;
        let status = response.status();
        let resp_body = response.text().await?;

        tracing::debug!("Response from Auth0 ({}): {resp_body}", status.as_u16());

        Ok(serde_json::from_str::<AccessTokenResponse>(&resp_body)?)
    }

    fn access_token(&self) -> Option<String> {
        self.access_token.clone()
    }
}

/// Fetches the jwks from the given URI.
async fn fetch_jwks(url: &str) -> Auth0Result<JwkSet> {
    let url = URL_REGEX.replace_all(url, "$1").to_string();
    let res = reqwest::get(url).await?;
    let val = res.json::<JwkSet>().await?;

    Ok(val)
}

/// Fetches the jwks from the given URI if needed.
async fn fetch_jwks_if_needed(jwks: Option<&JwkSet>, authority: &str) -> Auth0Result<JwkSet> {
    match jwks {
        Some(jwks) => Ok(jwks.clone()),
        None => fetch_jwks(&format!("{authority}/.well-known/jwks.json")).await,
    }
}

/// Attempts to find the key in the jwks.
/// If it fails, it fetches the jwks again and tries again.
async fn get_jwk(kid: &str, jwks: JwkSet, authority: &str) -> Auth0Result<(Jwk, JwkSet)> {
    match jwks.find(kid) {
        Some(jwk) => Ok((jwk.clone(), jwks)),
        None => {
            let jwks = fetch_jwks(authority).await?;

            Ok((jwks.find(kid).ok_or(Error::JwtMissingKid)?.clone(), jwks))
        }
    }
}

/// Validates a JWT token and returns its decoded payload.
///
/// # Arguments
///
/// * `token` - The JWT token to validate.
/// * `authority` - The authority to retreive the jwks from.
/// * `validations` - The validations to perform on the token.
///
/// # Example
/// ```
/// # async fn validate_jwt() -> auth0_client::error::Auth0Result<()> {
/// # use alcoholic_jwt::Validation;
/// # use auth0_client::authorization::valid_jwt;
/// valid_jwt(
///     "...jwt_token...",
///     "authority_to_retreive_jwks_from",
///     vec![Validation::SubjectPresent, Validation::NotExpired],
///     None,
/// ).await?;
/// # Ok(())
/// # }
pub async fn valid_jwt(
    token: &str,
    authority: &str,
    validation: Validation,
    jwks: Option<&JwkSet>,
) -> Auth0Result<(TokenData<Claims>, JwkSet)> {
    let header = decode_header(token)?;
    let kid: String = header.kid.ok_or(Error::JwtMissingKid)?;
    let jwks = fetch_jwks_if_needed(jwks, authority).await?;
    let jwk = get_jwk(&kid, jwks, authority).await?;
    // let jwt = validate(token, &jwk.0, validations)?;

    let jwt = match jwk.0.algorithm {
        AlgorithmParameters::RSA(ref rsa) => {
            let key =
                DecodingKey::from_rsa_components(&rsa.n, &rsa.e).map_err(|_| Error::InvalidJwk)?;
            decode::<Claims>(token, &key, &validation)?
        }
        _ => return Err(Error::InvalidJwk),
    };

    Ok((jwt, jwk.1))
}

#[derive(Debug, Deserialize)]
pub struct Claims {}

#[cfg(test)]
mod tests {
    use mockito::{mock, Mock};
    use serde_json::json;

    use super::*;

    fn new_client() -> Auth0Client {
        Auth0Client::new(
            "client_id",
            "client_secret",
            &mockito::server_url(),
            "https://audience.com",
        )
    }

    fn auth_mock() -> Mock {
        mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(
                json!({ "access_token": "access_token", "token_type": "Bearer" }).to_string(),
            )
            .create()
    }

    mod authenticate {
        use super::*;

        #[tokio::test]
        async fn save_the_access_token_to_the_client() {
            let _m = auth_mock();
            let mut client = new_client();

            client.authenticate().await.unwrap();
            assert_eq!(client.access_token, Some("access_token".to_owned()));
        }
    }

    mod access_token {
        use super::*;

        #[test]
        fn return_none_when_not_authenticated() {
            let _m = auth_mock();
            let client = new_client();

            assert_eq!(client.access_token(), None);
        }

        #[tokio::test]
        async fn return_access_token_when_authenticated() {
            let _m = auth_mock();
            let mut client = new_client();

            client.authenticate().await.unwrap();
            assert_eq!(client.access_token(), Some("access_token".to_owned()));
        }
    }

    mod jwt_validation {
        use super::*;

        fn jwks_mock() -> Mock {
            let jwks_response = std::fs::read_to_string("tests/data/jwks.json").unwrap();

            mock("GET", "/.well-known/jwks.json")
                .with_status(200)
                .with_body(jwks_response)
                .create()
        }

        mod fetch_jwks {
            use super::*;

            #[tokio::test]
            async fn works_with_sample_response() {
                let _m = jwks_mock();

                fetch_jwks(&format!("{}/.well-known/jwks.json", mockito::server_url()))
                    .await
                    .unwrap();
            }
        }

        mod valid_jwt {

            use std::collections::HashSet;

            use jsonwebtoken::errors::ErrorKind;

            use super::*;

            #[tokio::test]
            async fn validate_valid_jwt() {
                let _m = jwks_mock();
                let valid_token = std::fs::read_to_string("tests/data/valid_jwt.txt").unwrap();
                let mut validation = Validation::new(Algorithm::RS256);
                validation.validate_exp = false;
                validation.validate_aud = false;
                validation.required_spec_claims =
                    HashSet::from_iter([String::from("sub")].into_iter());
                valid_jwt(&valid_token, &mockito::server_url(), validation, None)
                    .await
                    .unwrap();
            }

            #[tokio::test]
            async fn errored_with_missing_kid() {
                let jwks_response = std::fs::read_to_string("tests/data/jwks_no_key.json").unwrap();
                let _m = mock("GET", "/.well-known/jwks.json")
                    .with_status(200)
                    .with_body(jwks_response)
                    .create();
                let valid_token = std::fs::read_to_string("tests/data/valid_jwt.txt").unwrap();
                let mut validation = Validation::new(Algorithm::RS256);
                validation.validate_exp = false;
                validation.validate_aud = false;
                validation.required_spec_claims =
                    HashSet::from_iter([String::from("sub")].into_iter());
                let res = valid_jwt(&valid_token, &mockito::server_url(), validation, None).await;

                match res {
                    Err(Error::JwtMissingKid) => (),
                    Err(err) => panic!("Expected JWTError(InvalidSignature) but got {err:?}"),
                    _ => panic!("Expected JWTError but got a valid JWT"),
                }
            }

            
            #[tokio::test]
            async fn errored_with_invalid_jwt() {
                let _m = jwks_mock();
                let invalid_token = std::fs::read_to_string("tests/data/invalid_jwt.txt").unwrap();
                let mut validation = Validation::new(Algorithm::RS256);
                validation.required_spec_claims =
                    HashSet::from_iter([String::from("sub")].into_iter());
                let res = valid_jwt(
                    &invalid_token,
                    &mockito::server_url(),
                    validation,
                    None,
                )
                .await;

                match res {
                    Err(Error::InvalidJwt(err)) => {
                        if *err.kind() != ErrorKind::InvalidSignature {
                            panic!("Expected ErrorKind::InvalidSignature but got {err:?}")
                        }
                    },
                    Err(err) => panic!("Expected JWTError(InvalidSignature) but got {err:?}"),
                    _ => panic!("Expected JWTError but got a valid JWT"),
                }
            }
        }
    }
}
