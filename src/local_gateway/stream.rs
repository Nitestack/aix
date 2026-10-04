use axum::body::Body;
use axum::http::{HeaderName, Response};
use futures_util::StreamExt;

pub(crate) fn forward_response(
    upstream: reqwest::Response,
    forwarded_headers: &'static [&'static str],
) -> Result<Response<Body>, axum::http::Error> {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let stream = upstream
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let mut response = Response::builder().status(status);
    for header in forwarded_headers {
        let name = HeaderName::from_static(header);
        if let Some(value) = headers.get(&name) {
            response = response.header(name, value);
        }
    }
    response.body(Body::from_stream(stream))
}
