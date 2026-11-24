//! Bridges Gitoxide's Git HTTP operations to the backend's quota-admitted transport.

use std::any::Any;
use std::cell::RefCell;
use std::io::{self, BufReader, Cursor, Read, Write};
use std::rc::Rc;

use gix::error::ErrorExt as _;
use gix_transport::client::blocking_io::http::{GetResponse, Http, PostBodyDataKind, PostResponse};
use hyper::body::Bytes;
use tokio::sync::{mpsc, oneshot};

const MAX_NEGOTIATION_BYTES: usize = 256 * 1024;

pub(super) struct Exchange {
    pub(super) endpoint: Endpoint,
    pub(super) protocol_v2: bool,
    pub(super) body: Bytes,
    pub(super) reply: oneshot::Sender<io::Result<Response>>,
}

#[derive(Clone, Copy)]
pub(super) enum Endpoint {
    Advertisement,
    UploadPack,
}

pub(super) struct Response {
    pub(super) headers: Bytes,
    pub(super) body: Bytes,
}

pub(super) struct Wire(pub(super) mpsc::Sender<Exchange>);

impl Http for Wire {
    type Headers = BufReader<Part>;
    type ResponseBody = BufReader<Part>;
    type PostBody = Upload;

    fn get(
        &mut self,
        url: &str,
        base: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> gix::Result<GetResponse<Self::Headers, Self::ResponseBody>> {
        verify_path(url, base, "/info/refs?service=git-upload-pack")?;
        let state = self.request(Endpoint::Advertisement, protocol_version(headers));
        Ok(GetResponse {
            headers: part(&state, true),
            body: part(&state, false),
        })
    }

    fn post(
        &mut self,
        url: &str,
        base: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        _: PostBodyDataKind,
    ) -> gix::Result<PostResponse<Self::Headers, Self::ResponseBody, Self::PostBody>> {
        verify_path(url, base, "/git-upload-pack")?;
        let state = self.request(Endpoint::UploadPack, protocol_version(headers));
        Ok(PostResponse {
            headers: part(&state, true),
            body: part(&state, false),
            post_body: Upload(state),
        })
    }

    fn configure(&mut self, _: &dyn Any) -> gix::Result<()> {
        Ok(())
    }
}

fn verify_path(url: &str, base: &str, suffix: &str) -> gix::Result<()> {
    if url.strip_prefix(base) == Some(suffix) {
        Ok(())
    } else {
        Err(io::Error::other("Unexpected Git route").raise())
    }
}

fn protocol_version(headers: impl IntoIterator<Item = impl AsRef<str>>) -> bool {
    headers.into_iter().any(|header| {
        header
            .as_ref()
            .split_once(':')
            .is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case("Git-Protocol") && value.trim() == "version=2"
            })
    })
}

type Shared = Rc<RefCell<Request>>;

struct Request {
    requests: mpsc::Sender<Exchange>,
    endpoint: Endpoint,
    protocol_v2: bool,
    outbound: Vec<u8>,
    response: Option<Response>,
}

impl Wire {
    fn request(&self, endpoint: Endpoint, protocol_v2: bool) -> Shared {
        Rc::new(RefCell::new(Request {
            requests: self.0.clone(),
            endpoint,
            protocol_v2,
            outbound: Vec::new(),
            response: None,
        }))
    }
}

impl Request {
    fn start(&mut self) -> io::Result<Response> {
        let (reply, result) = oneshot::channel();
        self.requests
            .blocking_send(Exchange {
                endpoint: self.endpoint,
                protocol_v2: self.protocol_v2,
                body: std::mem::take(&mut self.outbound).into(),
                reply,
            })
            .map_err(io::Error::other)?;
        result.blocking_recv().map_err(io::Error::other)?
    }
}

pub(super) struct Upload(Shared);

impl Write for Upload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut request = self.0.borrow_mut();
        if request.response.is_some()
            || request.outbound.len().saturating_add(bytes.len()) > MAX_NEGOTIATION_BYTES
        {
            return Err(io::Error::other("Git negotiation exceeds its limits"));
        }
        request.outbound.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) struct Part {
    state: Shared,
    header: bool,
    content: Option<Cursor<Bytes>>,
}

fn part(state: &Shared, header: bool) -> BufReader<Part> {
    BufReader::new(Part {
        state: state.clone(),
        header,
        content: None,
    })
}

impl Read for Part {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.content.is_none() {
            let mut request = self.state.borrow_mut();
            if request.response.is_none() {
                request.response = Some(request.start()?);
            }
            let response = request
                .response
                .as_mut()
                .ok_or_else(|| io::Error::other("Missing Git response"))?;
            self.content = Some(Cursor::new(if self.header {
                std::mem::take(&mut response.headers)
            } else {
                std::mem::take(&mut response.body)
            }));
        }
        self.content
            .as_mut()
            .ok_or_else(|| io::Error::other("Missing Git response"))?
            .read(out)
    }
}
