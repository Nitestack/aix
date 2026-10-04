use axum::body::Body;
use axum::http::header::CONNECTION;
use axum::http::{HeaderName, Response};
use futures_util::StreamExt;
use std::collections::HashSet;

pub(crate) enum ResponseStreamEnd {
    Complete,
    Error,
}

pub(crate) trait ResponseBodyObserver: Send + 'static {
    fn observe(&mut self, bytes: &[u8]);
    fn finish(&mut self, stream_end: ResponseStreamEnd);
}

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

pub(crate) fn forward_response_observed(
    upstream: reqwest::Response,
    forwarded_headers: &'static [&'static str],
    observer: Box<dyn ResponseBodyObserver>,
) -> Result<Response<Body>, axum::http::Error> {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let upstream = Box::pin(upstream.bytes_stream());
    let stream = futures_util::stream::unfold(
        (Some(upstream), Some(observer)),
        |(mut upstream, mut observer)| async move {
            let upstream_stream = upstream.as_mut()?;
            match upstream_stream.next().await {
                Some(Ok(chunk)) => {
                    if let Some(observer) = observer.as_mut() {
                        observer.observe(&chunk);
                    }
                    Some((Ok::<_, std::io::Error>(chunk), (upstream, observer)))
                }
                Some(Err(error)) => {
                    if let Some(mut observer) = observer.take() {
                        observer.finish(ResponseStreamEnd::Error);
                    }
                    Some((Err(std::io::Error::other(error)), (None, None)))
                }
                None => {
                    if let Some(mut observer) = observer.take() {
                        observer.finish(ResponseStreamEnd::Complete);
                    }
                    None
                }
            }
        },
    );
    let mut response = Response::builder().status(status);
    for header in forwarded_headers {
        let name = HeaderName::from_static(header);
        if let Some(value) = headers.get(&name) {
            response = response.header(name, value);
        }
    }
    response.body(Body::from_stream(stream))
}

pub(crate) fn forward_response_passthrough_observed(
    upstream: reqwest::Response,
    observer: Box<dyn ResponseBodyObserver>,
) -> Result<Response<Body>, axum::http::Error> {
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let connection_headers = connection_header_names(&headers);
    let upstream = Box::pin(upstream.bytes_stream());
    let stream = futures_util::stream::unfold(
        (Some(upstream), Some(observer)),
        |(mut upstream, mut observer)| async move {
            let upstream_stream = upstream.as_mut()?;
            match upstream_stream.next().await {
                Some(Ok(chunk)) => {
                    if let Some(observer) = observer.as_mut() {
                        observer.observe(&chunk);
                    }
                    Some((Ok::<_, std::io::Error>(chunk), (upstream, observer)))
                }
                Some(Err(error)) => {
                    if let Some(mut observer) = observer.take() {
                        observer.finish(ResponseStreamEnd::Error);
                    }
                    Some((Err(std::io::Error::other(error)), (None, None)))
                }
                None => {
                    if let Some(mut observer) = observer.take() {
                        observer.finish(ResponseStreamEnd::Complete);
                    }
                    None
                }
            }
        },
    );
    let mut response = Response::builder().status(status);
    for (name, value) in &headers {
        if should_forward_header(name.as_str(), &connection_headers) {
            response = response.header(name, value);
        }
    }
    response.body(Body::from_stream(stream))
}

fn connection_header_names(headers: &reqwest::header::HeaderMap) -> HashSet<String> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}

fn should_forward_header(name: &str, connection_headers: &HashSet<String>) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "content-length"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    ) && !connection_headers.contains(&name.to_ascii_lowercase())
}
