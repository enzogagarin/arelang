use tiny_http::Method;

#[derive(Debug, Clone)]
pub(crate) struct RuntimeRequest {
    pub(crate) method: Method,
    pub(crate) url: String,
    pub(crate) body: String,
}

impl RuntimeRequest {
    pub(crate) fn new(method: Method, url: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            body: body.into(),
        }
    }

    pub(crate) fn path(&self) -> &str {
        self.url
            .split_once('?')
            .map_or(self.url.as_str(), |(path, _query)| path)
    }
}
