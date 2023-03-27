use super::client::Config;
use async_trait::async_trait;
use base64::Engine;

use super::{parse_query_result, QueryResult, Statement};

/// Database client. This is the main structure used to
/// communicate with the database.
#[derive(Clone, Debug)]
pub struct Client {
    base_url: String,
    url_for_queries: String,
    auth: String,
}

impl Client {
    /// Creates a database client with JWT authentication.
    ///
    /// # Arguments
    /// * `url` - URL of the database endpoint
    /// * `token` - auth token
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        let token = token.into();
        let url = url.into();
        // Auto-update the URL to start with https:// if no protocol was specified
        let base_url = if !url.contains("://") {
            "https://".to_owned() + &url
        } else {
            url
        };
        let url_for_queries = if cfg!(feature = "separate_url_for_queries") {
            format!("{base_url}/queries")
        } else {
            base_url.clone()
        };
        Self {
            base_url,
            url_for_queries,
            auth: format!("Bearer {token}"),
        }
    }

    /// Creates a database client with Basic HTTP authentication.
    ///
    /// # Arguments
    /// * `url` - URL of the database endpoint
    /// * `username` - database username
    /// * `pass` - user's password
    pub fn from_credentials(
        url: impl Into<String>,
        username: impl Into<String>,
        pass: impl Into<String>,
    ) -> Self {
        let username = username.into();
        let pass = pass.into();
        let url = url.into();
        // Auto-update the URL to start with https:// if no protocol was specified
        let base_url = if !url.contains("://") {
            "https://".to_owned() + &url
        } else {
            url
        };
        let url_for_queries = if cfg!(feature = "separate_url_for_queries") {
            format!("{base_url}/queries")
        } else {
            base_url.clone()
        };
        Self {
            base_url,
            url_for_queries,
            auth: format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{pass}"))
            ),
        }
    }

    /// Creates a database client from a `Config` object.
    pub fn from_config(config: Config) -> Self {
        Self::new(config.url, config.auth_token.unwrap_or_default())
    }

    /// Creates a database client, given a `Url`
    ///
    /// # Arguments
    /// * `url` - `Url` object of the database endpoint. This cannot be a relative URL;
    ///
    /// # Examples
    ///
    /// ```
    /// # use libsql_client::reqwest::Client;
    /// use url::Url;
    ///
    /// let url  = Url::parse("https://foo:bar@localhost:8080").unwrap();
    /// let db = Client::from_url(&url).unwrap();
    /// ```
    pub fn from_url(url: &url::Url) -> anyhow::Result<Client> {
        let mut params = url.query_pairs();
        // Try a token=XXX parameter first, continue if not found
        if let Some((_, token)) = params.find(|(param_key, _)| param_key == "token") {
            return Ok(Client::new(url.as_str(), token.into_owned()));
        }

        let username = url.username();
        let password = url.password().unwrap_or_default();
        let mut url = url.clone();
        url.set_username("")
            .map_err(|_| anyhow::anyhow!("Could not extract username from URL. Invalid URL?"))?;
        url.set_password(None)
            .map_err(|_| anyhow::anyhow!("Could not extract password from URL. Invalid URL?"))?;
        Ok(Client::from_credentials(url.as_str(), username, password))
    }

    fn batch(
        &self,
        stmts: impl IntoIterator<Item = impl Into<Statement>>,
    ) -> anyhow::Result<Vec<QueryResult>> {
        // FIXME: serialize and deserialize with existing routines from sqld
        let mut body = "{\"statements\": [".to_string();
        let mut stmts_count = 0;
        for stmt in stmts {
            body += &format!("{},", stmt.into());
            stmts_count += 1;
        }
        if stmts_count > 0 {
            body.pop();
        }
        body += "]}";

        let req = http::Request::builder()
            .uri(&self.url_for_queries)
            .header("Authorization", &self.auth)
            .method("POST")
            .body(Some(bytes::Bytes::copy_from_slice(body.as_bytes())))?;

        // NOTICE: legacy base_url parameter is not used in Spin backend
        let _ = &self.base_url;

        let response = spin_sdk::outbound_http::send_request(req);
        let resp: String =
            std::str::from_utf8(&response?.into_body().unwrap_or_default())?.to_string();
        let response_json: serde_json::Value = serde_json::from_str(&resp)?;
        match response_json {
            serde_json::Value::Array(results) => {
                if results.len() != stmts_count {
                    Err(anyhow::anyhow!(
                        "Response array did not contain expected {stmts_count} results"
                    ))
                } else {
                    let mut query_results: Vec<QueryResult> = Vec::with_capacity(stmts_count);
                    for (idx, result) in results.into_iter().enumerate() {
                        query_results.push(parse_query_result(result, idx)?);
                    }

                    Ok(query_results)
                }
            }
            e => Err(anyhow::anyhow!("Error: {} ({:?})", e, body)),
        }
    }
}

#[async_trait(?Send)]
impl super::DatabaseClient for Client {
    async fn batch(
        &self,
        stmts: impl IntoIterator<Item = impl Into<Statement>>,
    ) -> anyhow::Result<Vec<QueryResult>> {
        self.batch(stmts).map_err(|e| anyhow::anyhow!("{e}"))
    }
}
