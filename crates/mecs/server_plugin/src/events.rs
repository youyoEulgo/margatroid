use std::net::SocketAddr;

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, Uri};
use core_plugin::Event;

use crate::{HttpResponse, HttpResponseError, HttpResponseHead, HttpResponseSession};

pub struct HttpRequestReceived {
    id: u64,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
    response: HttpResponseSession,
}

impl HttpRequestReceived {
    pub(crate) fn new(
        id: u64,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
        response: HttpResponseSession,
    ) -> Self {
        Self {
            id,
            method,
            uri,
            headers,
            body,
            response,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn method(&self) -> &Method {
        &self.method
    }

    pub fn uri(&self) -> &Uri {
        &self.uri
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn body(&self) -> &Bytes {
        &self.body
    }

    pub fn response_session(&self) -> HttpResponseSession {
        self.response.clone()
    }

    pub fn respond(&self, response: HttpResponse) -> Result<(), HttpResponseError> {
        self.response.respond(response)
    }

    pub fn start_stream(&self, head: HttpResponseHead) -> Result<(), HttpResponseError> {
        self.response.start_stream(head)
    }
}

impl Event for HttpRequestReceived {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerStarted {
    pub address: SocketAddr,
}

impl Event for ServerStarted {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFailed {
    pub message: String,
}

impl Event for ServerFailed {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerStopped;

impl Event for ServerStopped {}
