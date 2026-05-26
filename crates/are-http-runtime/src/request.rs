use std::collections::HashMap;
use tiny_http::Method;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeRequest {
    pub(crate) method: Method,
    pub(crate) url: String,
    pub(crate) body: String,
    pub(crate) headers: HashMap<String, String>,
}

impl RuntimeRequest {
    pub(crate) fn new(method: Method, url: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            body: body.into(),
            headers: HashMap::new(),
        }
    }

    pub(crate) fn with_headers(mut self, headers: HashMap<String, String>) -> Self {
        self.headers = headers
            .into_iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value))
            .collect();
        self
    }

    pub(crate) fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers
            .insert(name.into().to_ascii_lowercase(), value.into());
        self
    }

    pub(crate) fn path(&self) -> &str {
        self.url
            .split_once('?')
            .map_or(self.url.as_str(), |(path, _query)| path)
    }

    pub(crate) fn query(&self) -> &str {
        self.url.split_once('?').map_or("", |(_path, query)| query)
    }
}
