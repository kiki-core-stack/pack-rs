use std::{
    error::Error,
    fmt,
    fmt::{
        Display,
        Formatter,
    },
    mem,
    time::Duration,
};

use anyhow::{
    Context,
    Result,
    ensure,
};
use reqwest::{
    Client,
    Method,
    RequestBuilder,
    Response,
    StatusCode,
    Url,
    header::HeaderMap,
    redirect::Policy,
};
use serde::{
    Deserialize,
    Serialize,
    de::DeserializeOwned,
};

// Constants/Statics
const CONNECTION_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

// Structs
#[derive(Debug)]
pub struct ApiClient {
    base_url: Url,
    client: Client,
}

impl ApiClient {
    /// Creates a client for an API origin (e.g. `http://127.0.0.1:8080`); request paths are absolute, such as
    /// `/api/services/...`.
    pub fn new(base_url: &str) -> Result<Self> {
        Self::with_default_headers(base_url, HeaderMap::new())
    }

    // Private methods
    async fn request_json<D: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        data: Option<&D>,
    ) -> Result<T> {
        let mut request = self.build_request(method, path)?;
        if let Some(data) = data {
            request = request.json(data);
        }

        self.request(request).await
    }

    // Public methods
    pub fn build_request(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let url = self.base_url.join(path)?;

        Ok(self.client.request(method, url))
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(self.build_request(Method::GET, path)?).await
    }

    pub async fn patch<D: Serialize, T: DeserializeOwned>(&self, path: &str, data: Option<&D>) -> Result<T> {
        self.request_json(Method::PATCH, path, data).await
    }

    pub async fn post<D: Serialize, T: DeserializeOwned>(&self, path: &str, data: Option<&D>) -> Result<T> {
        self.request_json(Method::POST, path, data).await
    }

    /// Sends the request and returns the response envelope `data`.
    ///
    /// A failed envelope, or a non-success HTTP status whose body is not an envelope, returns an
    /// [`ApiResponseError`] that callers can reach with `anyhow::Error::downcast_ref`.
    pub async fn request<T: DeserializeOwned>(&self, request_builder: RequestBuilder) -> Result<T> {
        self.request_with_max_response_body_bytes(request_builder, DEFAULT_MAX_RESPONSE_BODY_BYTES)
            .await
    }

    pub async fn request_raw(&self, request_builder: RequestBuilder) -> Result<RawApiResponse> {
        self.request_raw_with_max_response_body_bytes(request_builder, DEFAULT_MAX_RESPONSE_BODY_BYTES)
            .await
    }

    pub async fn request_raw_with_max_response_body_bytes(
        &self,
        request_builder: RequestBuilder,
        max_response_body_bytes: usize,
    ) -> Result<RawApiResponse> {
        let request = request_builder.build().context("failed to build API request")?;
        let method = request.method().clone();
        let url = request.url().clone();
        let mut response = self
            .client
            .execute(request)
            .await
            .with_context(|| format!("failed to send API request {method} {url}"))?;

        let status = response.status();
        let headers = mem::take(response.headers_mut());
        let body = read_response_body(response, &method, &url, max_response_body_bytes).await?;

        Ok(RawApiResponse { body, headers, status })
    }

    pub async fn request_with_max_response_body_bytes<T: DeserializeOwned>(
        &self,
        request_builder: RequestBuilder,
        max_response_body_bytes: usize,
    ) -> Result<T> {
        let response = self
            .request_raw_with_max_response_body_bytes(request_builder, max_response_body_bytes)
            .await?;

        // A non-envelope body with an error status still reports that status as a failed response
        let envelope: ApiResponseEnvelope = match serde_json::from_slice(&response.body) {
            Ok(envelope) => envelope,
            Err(_) if !response.status.is_success() => ApiResponseEnvelope::default(),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to decode API response JSON with HTTP {}", response.status));
            },
        };

        if !envelope.success {
            return Err(ApiResponseError::from_envelope(envelope, response.status).into());
        }

        // Missing data decodes as null so `()` and `Option<_>` accept data-less success responses
        serde_json::from_value(envelope.data.unwrap_or(serde_json::Value::Null))
            .context("failed to decode API response data")
    }

    pub fn with_default_headers(base_url: &str, default_headers: HeaderMap) -> Result<Self> {
        let base_url = Url::parse(base_url).context("invalid API base URL")?;
        ensure!(
            matches!(base_url.scheme(), "http" | "https"),
            "API base URL must use HTTP or HTTPS"
        );

        let client = Client::builder()
            .default_headers(default_headers)
            .pool_idle_timeout(CONNECTION_POOL_IDLE_TIMEOUT)
            .redirect(Policy::none())
            .timeout(REQUEST_TIMEOUT)
            .build()?;

        Ok(Self { base_url, client })
    }
}

/// The API envelope: `{ success: true, data? }` or
/// `{ success: false, errorCode, errorData?, message, requestId? }`.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiResponseEnvelope {
    data: Option<serde_json::Value>,
    error_code: Option<String>,
    error_data: Option<serde_json::Value>,
    message: Option<String>,
    request_id: Option<String>,
    success: bool,
}

/// A failed API response, either a `success: false` envelope or a non-success HTTP status whose body is not an
/// envelope (e.g. a proxy error page), in which case every field except `status` is `None`.
#[derive(Debug)]
pub struct ApiResponseError {
    /// Error code to branch on, e.g. `notFound`.
    pub error_code: Option<String>,

    /// Structured details of the error, e.g. the issues of `validationFailed`.
    pub error_data: Option<serde_json::Value>,

    /// Message localized by the server from the request's `accept-language`; for logs and display only.
    pub message: Option<String>,

    /// Server-side request ID for correlating logs.
    pub request_id: Option<String>,
    pub status: StatusCode,
}

impl Display for ApiResponseError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "API request failed with HTTP {}", self.status)?;
        if let Some(error_code) = &self.error_code {
            write!(formatter, " ({error_code})")?;
        }

        if let Some(message) = &self.message {
            write!(formatter, ": {message}")?;
        }

        if let Some(request_id) = &self.request_id {
            write!(formatter, " [request ID: {request_id}]")?;
        }

        Ok(())
    }
}

impl Error for ApiResponseError {}

impl ApiResponseError {
    // Private methods
    fn from_envelope(envelope: ApiResponseEnvelope, status: StatusCode) -> Self {
        Self {
            error_code: envelope.error_code,
            error_data: envelope.error_data,
            message: envelope.message,
            request_id: envelope.request_id,
            status,
        }
    }

    // Public methods

    /// Returns whether `error` is a failed API response with the given error code.
    #[must_use]
    pub fn has_error_code(error: &anyhow::Error, error_code: &str) -> bool {
        error
            .downcast_ref::<Self>()
            .is_some_and(|error| error.error_code.as_deref() == Some(error_code))
    }

    /// Returns whether `error` is a failed API response with a 4xx status, meaning the server rejected the
    /// request rather than failing transiently.
    #[must_use]
    pub fn is_client_error(error: &anyhow::Error) -> bool {
        error
            .downcast_ref::<Self>()
            .is_some_and(|error| error.status.is_client_error())
    }
}

#[derive(Debug)]
pub struct RawApiResponse {
    pub body: Vec<u8>,
    pub headers: HeaderMap,
    pub status: StatusCode,
}

impl RawApiResponse {
    /// Returns the response when its HTTP status is a success; otherwise an [`ApiResponseError`] read from the
    /// body's envelope (only `status` is set when the body is not one).
    pub fn error_for_status(self) -> Result<Self> {
        if self.status.is_success() {
            return Ok(self);
        }

        let envelope = serde_json::from_slice(&self.body).unwrap_or_default();
        Err(ApiResponseError::from_envelope(envelope, self.status).into())
    }
}

// Functions
async fn read_response_body(
    mut response: Response,
    method: &Method,
    url: &Url,
    max_response_body_bytes: usize,
) -> Result<Vec<u8>> {
    let initial_capacity = match response.content_length() {
        Some(content_length) => {
            ensure!(
                content_length <= max_response_body_bytes as u64,
                "API response Content-Length {content_length} exceeds maximum of {max_response_body_bytes} bytes for {method} {url}"
            );

            usize::try_from(content_length).context("API response Content-Length does not fit in usize")?
        },
        None => 0,
    };

    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("failed to read API response body for {method} {url}"))?
    {
        let remaining_capacity = max_response_body_bytes - body.len();
        ensure!(
            chunk.len() <= remaining_capacity,
            "API response body exceeds maximum of {max_response_body_bytes} bytes for {method} {url}"
        );

        body.extend_from_slice(&chunk);
    }

    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{
            Read,
            Write,
        },
        net::TcpListener,
        thread,
    };

    use super::*;

    fn api_response_error(error: &anyhow::Error) -> &ApiResponseError {
        error.downcast_ref::<ApiResponseError>().unwrap()
    }

    fn http_response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len()).into_bytes();
        for (name, value) in headers {
            write!(response, "{name}: {value}\r\n").unwrap();
        }

        response.extend_from_slice(b"Connection: close\r\n\r\n");
        response.extend_from_slice(body);
        response
    }

    fn serve_once(response: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let bytes_read = stream.read(&mut request).unwrap();
            assert!(bytes_read > 0);
            stream.write_all(&response).unwrap();
        });

        (format!("http://{address}/"), handle)
    }

    #[test]
    fn accepts_http_and_https_base_urls() {
        ApiClient::new("http://example.test").unwrap();
        ApiClient::new("https://example.test").unwrap();
    }

    #[test]
    fn rejects_non_http_base_url() {
        assert!(ApiClient::new("ftp://example.test").is_err());
    }

    #[tokio::test]
    async fn does_not_follow_redirects() {
        let body = br#"{"success":false}"#;
        let response = http_response("302 Found", &[("Location", "http://127.0.0.1:9/redirected")], body);

        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();

        let error = client.get::<serde_json::Value>("").await.unwrap_err();

        assert_eq!(api_response_error(&error).status, StatusCode::FOUND);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn returns_successful_api_envelope_data_regardless_of_http_status() {
        let body = br#"{"data":42,"success":true}"#;
        let response = http_response("400 Bad Request", &[], body);

        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();

        assert_eq!(client.get::<u8>("").await.unwrap(), 42);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn accepts_data_less_success_only_for_unit_or_option() {
        let success_response = || http_response("200 OK", &[], br#"{"success":true}"#);

        let (base_url, server) = serve_once(success_response());
        ApiClient::new(&base_url).unwrap().get::<()>("").await.unwrap();
        server.join().unwrap();

        let (base_url, server) = serve_once(success_response());
        assert_eq!(
            ApiClient::new(&base_url).unwrap().get::<Option<u8>>("").await.unwrap(),
            None
        );

        server.join().unwrap();

        let (base_url, server) = serve_once(success_response());
        let error = ApiClient::new(&base_url).unwrap().get::<u8>("").await.unwrap_err();
        assert!(error.downcast_ref::<ApiResponseError>().is_none());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn returns_failed_api_envelope_as_api_response_error() {
        let body = br#"{"errorCode":"notFound","errorData":{"field":"name"},"message":"missing","requestId":"abc","success":false}"#;
        let response = http_response("404 Not Found", &[], body);

        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();

        let error = client.get::<u8>("").await.unwrap_err();
        let api_response_error = api_response_error(&error);

        assert_eq!(api_response_error.status, StatusCode::NOT_FOUND);
        assert_eq!(api_response_error.error_code.as_deref(), Some("notFound"));
        assert_eq!(
            api_response_error.error_data,
            Some(serde_json::json!({ "field": "name" }))
        );

        assert_eq!(api_response_error.message.as_deref(), Some("missing"));
        assert_eq!(api_response_error.request_id.as_deref(), Some("abc"));
        assert!(ApiResponseError::has_error_code(&error, "notFound"));
        assert!(ApiResponseError::is_client_error(&error));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn returns_non_envelope_error_status_as_api_response_error() {
        let response = http_response("502 Bad Gateway", &[], b"<html>bad gateway</html>");

        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();

        let error = client.get::<u8>("").await.unwrap_err();
        let api_response_error = api_response_error(&error);

        assert_eq!(api_response_error.status, StatusCode::BAD_GATEWAY);
        assert!(api_response_error.error_code.is_none());
        assert!(!ApiResponseError::is_client_error(&error));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn returns_decode_error_for_non_envelope_success_status() {
        let response = http_response("200 OK", &[], b"not json");

        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();

        let error = client.get::<u8>("").await.unwrap_err();

        assert!(error.downcast_ref::<ApiResponseError>().is_none());
        assert!(!ApiResponseError::is_client_error(&error));
        assert!(error.to_string().contains("failed to decode API response JSON"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn returns_bounded_raw_response_with_metadata() {
        let body = b"artifact";
        let response = http_response("206 Partial Content", &[("X-Artifact-Version", "7")], body);

        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();
        let response = client
            .request_raw(client.build_request(Method::GET, "").unwrap())
            .await
            .unwrap();

        assert_eq!(response.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers.get("x-artifact-version").unwrap(), "7");
        assert_eq!(response.body, body);
        server.join().unwrap();
    }

    #[tokio::test]
    async fn raw_response_error_for_status_reads_failed_envelope() {
        let body = br#"{"errorCode":"notFound","message":"missing","success":false}"#;
        let response = http_response("404 Not Found", &[], body);

        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();
        let error = client
            .request_raw(client.build_request(Method::GET, "").unwrap())
            .await
            .unwrap()
            .error_for_status()
            .unwrap_err();

        assert_eq!(api_response_error(&error).status, StatusCode::NOT_FOUND);
        assert!(ApiResponseError::has_error_code(&error, "notFound"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_content_length() {
        const MAX_RESPONSE_BODY_BYTES: usize = 16;

        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n".to_vec();
        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();
        let error = client
            .request_raw_with_max_response_body_bytes(
                client.build_request(Method::GET, "").unwrap(),
                MAX_RESPONSE_BODY_BYTES,
            )
            .await
            .expect_err("oversized Content-Length was accepted");

        assert!(error.to_string().contains("Content-Length"));
        server.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_chunked_response() {
        const MAX_RESPONSE_BODY_BYTES: usize = 16;

        let oversized_body = vec![b'a'; MAX_RESPONSE_BODY_BYTES + 1];
        let mut response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        write!(response, "{:X}\r\n", oversized_body.len()).unwrap();
        response.extend_from_slice(&oversized_body);
        response.extend_from_slice(b"\r\n0\r\n\r\n");
        let (base_url, server) = serve_once(response);
        let client = ApiClient::new(&base_url).unwrap();
        let error = client
            .request_raw_with_max_response_body_bytes(
                client.build_request(Method::GET, "").unwrap(),
                MAX_RESPONSE_BODY_BYTES,
            )
            .await
            .expect_err("oversized response was accepted");

        assert!(error.to_string().contains("exceeds maximum"));
        server.join().unwrap();
    }
}
