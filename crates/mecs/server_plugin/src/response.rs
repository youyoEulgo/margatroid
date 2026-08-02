use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use futures_util::Stream;
use tokio::sync::{mpsc, oneshot};

use crate::{HttpResponseError, HttpStreamError};

pub type HttpResponse = Response<Bytes>;

#[derive(Clone, Debug)]
pub struct HttpResponseHead {
    status: StatusCode,
    headers: HeaderMap,
}

impl HttpResponseHead {
    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    pub fn with_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }
}

impl Default for HttpResponseHead {
    fn default() -> Self {
        Self {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
        }
    }
}

enum HttpResponseState {
    Waiting(Option<oneshot::Sender<Response<Body>>>),
    Streaming {
        sender: mpsc::Sender<Result<Bytes, HttpStreamError>>,
        finished: Arc<AtomicBool>,
    },
    ClosedNormally,
    ClosedWithError,
}

#[derive(Clone)]
pub struct HttpResponseSession {
    state: Arc<Mutex<HttpResponseState>>,
    stream_buffer_capacity: usize,
}

impl HttpResponseSession {
    pub(crate) fn new(
        response_sender: oneshot::Sender<Response<Body>>,
        stream_buffer_capacity: usize,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(HttpResponseState::Waiting(Some(
                response_sender,
            )))),
            stream_buffer_capacity,
        }
    }

    pub fn respond(&self, response: HttpResponse) -> Result<(), HttpResponseError> {
        let sender = {
            let mut state = self
                .state
                .lock()
                .expect("HTTP response state lock poisoned");
            let HttpResponseState::Waiting(sender) = &mut *state else {
                return Err(HttpResponseError::ResponseAlreadyStarted);
            };
            let sender = sender
                .take()
                .ok_or(HttpResponseError::ResponseAlreadyStarted)?;
            *state = HttpResponseState::ClosedNormally;
            sender
        };

        let (parts, body) = response.into_parts();
        if sender
            .send(Response::from_parts(parts, Body::from(body)))
            .is_err()
        {
            *self
                .state
                .lock()
                .expect("HTTP response state lock poisoned") = HttpResponseState::ClosedWithError;
            return Err(HttpResponseError::RequestClosed);
        }
        Ok(())
    }

    pub fn start_stream(&self, head: HttpResponseHead) -> Result<(), HttpResponseError> {
        let (chunk_sender, chunk_receiver) = mpsc::channel(self.stream_buffer_capacity);
        let finished = Arc::new(AtomicBool::new(false));
        let stream = HttpResponseBodyStream {
            receiver: chunk_receiver,
            finished: Arc::clone(&finished),
            terminal_reported: false,
        };
        let mut response = Response::new(Body::from_stream(stream));
        *response.status_mut() = head.status;
        *response.headers_mut() = head.headers;

        let mut state = self
            .state
            .lock()
            .expect("HTTP response state lock poisoned");
        let HttpResponseState::Waiting(sender) = &mut *state else {
            return Err(HttpResponseError::ResponseAlreadyStarted);
        };
        let sender = sender
            .take()
            .ok_or(HttpResponseError::ResponseAlreadyStarted)?;
        if sender.send(response).is_err() {
            *state = HttpResponseState::ClosedWithError;
            return Err(HttpResponseError::RequestClosed);
        }
        *state = HttpResponseState::Streaming {
            sender: chunk_sender,
            finished,
        };
        Ok(())
    }

    pub async fn send_chunk(&self, chunk: Bytes) -> Result<(), HttpResponseError> {
        let sender = {
            let state = self
                .state
                .lock()
                .expect("HTTP response state lock poisoned");
            match &*state {
                HttpResponseState::Waiting(_) => return Err(HttpResponseError::StreamNotStarted),
                HttpResponseState::Streaming { sender, .. } => sender.clone(),
                HttpResponseState::ClosedNormally | HttpResponseState::ClosedWithError => {
                    return Err(HttpResponseError::ResponseClosed);
                }
            }
        };

        if sender.send(Ok(chunk)).await.is_err() {
            let mut state = self
                .state
                .lock()
                .expect("HTTP response state lock poisoned");
            if matches!(*state, HttpResponseState::Streaming { .. }) {
                *state = HttpResponseState::ClosedWithError;
            }
            return Err(HttpResponseError::RequestClosed);
        }
        Ok(())
    }

    pub fn finish(&self) -> Result<(), HttpResponseError> {
        let mut state = self
            .state
            .lock()
            .expect("HTTP response state lock poisoned");
        match &*state {
            HttpResponseState::Waiting(_) => Err(HttpResponseError::StreamNotStarted),
            HttpResponseState::Streaming { sender, .. } if sender.is_closed() => {
                *state = HttpResponseState::ClosedWithError;
                Err(HttpResponseError::RequestClosed)
            }
            HttpResponseState::Streaming { finished, .. } => {
                finished.store(true, Ordering::Release);
                *state = HttpResponseState::ClosedNormally;
                Ok(())
            }
            HttpResponseState::ClosedNormally | HttpResponseState::ClosedWithError => {
                Err(HttpResponseError::ResponseClosed)
            }
        }
    }

    pub async fn abort(&self, error: HttpStreamError) -> Result<(), HttpResponseError> {
        let sender = {
            let mut state = self
                .state
                .lock()
                .expect("HTTP response state lock poisoned");
            let sender = match &*state {
                HttpResponseState::Waiting(_) => return Err(HttpResponseError::StreamNotStarted),
                HttpResponseState::Streaming { sender, .. } => sender.clone(),
                HttpResponseState::ClosedNormally | HttpResponseState::ClosedWithError => {
                    return Err(HttpResponseError::ResponseClosed);
                }
            };
            *state = HttpResponseState::ClosedWithError;
            sender
        };
        sender
            .send(Err(error))
            .await
            .map_err(|_| HttpResponseError::RequestClosed)
    }

    pub fn is_started(&self) -> bool {
        !matches!(
            *self
                .state
                .lock()
                .expect("HTTP response state lock poisoned"),
            HttpResponseState::Waiting(_)
        )
    }

    pub fn is_closed(&self) -> bool {
        match &*self
            .state
            .lock()
            .expect("HTTP response state lock poisoned")
        {
            HttpResponseState::Waiting(sender) => {
                sender.as_ref().is_none_or(|sender| sender.is_closed())
            }
            HttpResponseState::Streaming { sender, .. } => sender.is_closed(),
            HttpResponseState::ClosedNormally | HttpResponseState::ClosedWithError => true,
        }
    }
}

struct HttpResponseBodyStream {
    receiver: mpsc::Receiver<Result<Bytes, HttpStreamError>>,
    finished: Arc<AtomicBool>,
    terminal_reported: bool,
}

impl Stream for HttpResponseBodyStream {
    type Item = Result<Bytes, HttpStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.receiver.poll_recv(context) {
            Poll::Ready(Some(Err(error))) => {
                self.terminal_reported = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) if self.terminal_reported => Poll::Ready(None),
            Poll::Ready(None) => {
                if self.finished.load(Ordering::Acquire) {
                    Poll::Ready(None)
                } else {
                    self.terminal_reported = true;
                    Poll::Ready(Some(Err(HttpStreamError::abandoned())))
                }
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn complete_response_is_delivered_once() {
        let (sender, receiver) = oneshot::channel();
        let session = HttpResponseSession::new(sender, 2);

        session
            .respond(Response::new(Bytes::from_static(b"complete")))
            .unwrap();

        let response = receiver.await.unwrap();
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "complete"
        );
        assert_eq!(
            session.respond(Response::new(Bytes::new())),
            Err(HttpResponseError::ResponseAlreadyStarted)
        );
    }

    #[tokio::test]
    async fn stream_applies_chunks_and_finishes_explicitly() {
        let (sender, receiver) = oneshot::channel();
        let session = HttpResponseSession::new(sender, 2);
        session.start_stream(HttpResponseHead::default()).unwrap();
        let response = receiver.await.unwrap();

        session
            .send_chunk(Bytes::from_static(b"hello "))
            .await
            .unwrap();
        session
            .send_chunk(Bytes::from_static(b"world"))
            .await
            .unwrap();
        session.finish().unwrap();

        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "hello world"
        );
    }

    #[tokio::test]
    async fn finished_stream_remains_normal_after_the_last_session_is_dropped() {
        let (sender, receiver) = oneshot::channel();
        let session = HttpResponseSession::new(sender, 1);
        session.start_stream(HttpResponseHead::default()).unwrap();
        let response = receiver.await.unwrap();

        session
            .send_chunk(Bytes::from_static(b"complete"))
            .await
            .unwrap();
        session.finish().unwrap();
        drop(session);

        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "complete"
        );
    }

    #[tokio::test]
    async fn abandoned_stream_ends_with_a_body_error() {
        let (sender, receiver) = oneshot::channel();
        let session = HttpResponseSession::new(sender, 1);
        session.start_stream(HttpResponseHead::default()).unwrap();
        let response = receiver.await.unwrap();
        drop(session);

        assert!(to_bytes(response.into_body(), usize::MAX).await.is_err());
    }

    #[tokio::test]
    async fn aborted_stream_reports_a_body_error() {
        let (sender, receiver) = oneshot::channel();
        let session = HttpResponseSession::new(sender, 1);
        session.start_stream(HttpResponseHead::default()).unwrap();
        let response = receiver.await.unwrap();

        session
            .abort(HttpStreamError::new("upstream failed"))
            .await
            .unwrap();

        assert!(to_bytes(response.into_body(), usize::MAX).await.is_err());
    }
}
