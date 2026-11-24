use forall_api::AuthProvider;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;

/// Auth provider that sends the provider API key verbatim in a named header
/// instead of `Authorization: Bearer <key>`.
///
/// Used for providers whose `auth_header` is set, e.g. Azure OpenAI's
/// `api-key` header.
#[derive(Clone)]
pub struct HeaderAuthProvider {
    pub header_name: HeaderName,
    pub api_key: String,
}

impl HeaderAuthProvider {
    pub fn new(header_name: HeaderName, api_key: String) -> Self {
        Self {
            header_name,
            api_key,
        }
    }
}

impl AuthProvider for HeaderAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        if let Ok(mut value) = HeaderValue::from_str(&self.api_key) {
            value.set_sensitive(true);
            let _ = headers.insert(self.header_name.clone(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn header_auth_provider_sets_exactly_the_named_header() {
        let auth =
            HeaderAuthProvider::new(HeaderName::from_static("api-key"), "azure-key".to_string());
        let mut headers = HeaderMap::new();

        auth.add_auth_headers(&mut headers);

        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers.get("api-key").and_then(|value| value.to_str().ok()),
            Some("azure-key")
        );
        assert!(headers.get(http::header::AUTHORIZATION).is_none());
    }

    #[test]
    fn header_auth_provider_marks_key_sensitive() {
        let auth =
            HeaderAuthProvider::new(HeaderName::from_static("api-key"), "azure-key".to_string());
        let headers = auth.to_auth_headers();

        let value = headers.get("api-key").expect("api-key header present");
        assert!(value.is_sensitive());
    }
}
