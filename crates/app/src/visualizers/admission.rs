//! Borrow retained values for native matching; allocate projected bytes only on activation.
use super::evidence::Boundaries;
use transmog_core::HeaderField;
use transmog_extensions::{
    Direction, ExchangeMetadata, HeaderMetadata, MessageMetadata, Surface, Visualizer, matches,
};
use transmog_session::SessionSnapshot;

#[derive(Clone, Copy, Debug)]
pub(super) struct RetainedHeader<'a>(pub(super) &'a HeaderField);
impl HeaderMetadata for RetainedHeader<'_> {
    fn name(&self) -> &[u8] {
        self.0.name()
    }
    fn value(&self) -> Option<&[u8]> {
        (!self.0.is_redacted()).then(|| self.0.value())
    }
    fn redacted(&self) -> bool {
        self.0.is_redacted()
    }
}
pub(super) struct Admission<'a> {
    direction: Direction,
    request: Option<(&'a str, &'a str)>,
    response: Option<u16>,
    request_headers: Vec<RetainedHeader<'a>>,
    response_headers: Vec<RetainedHeader<'a>>,
}
impl<'a> Admission<'a> {
    pub(super) fn new(snapshot: &'a SessionSnapshot, selected: Boundaries) -> Self {
        let request = snapshot
            .request_heads
            .iter()
            .find(|head| head.boundary == selected.request)
            .map(|head| &head.head);
        let response = snapshot
            .response_heads
            .iter()
            .rev()
            .find(|head| {
                head.boundary == selected.response
                    && (head.head.status >= 200 || head.head.status == 101)
            })
            .map(|head| &head.head);
        Self {
            direction: selected.direction,
            request: request.map(|head| (head.method.as_str(), head.target.path.as_str())),
            response: response.map(|head| head.status),
            request_headers: request
                .map(|head| head.headers.iter().map(RetainedHeader).collect())
                .unwrap_or_default(),
            response_headers: response
                .map(|head| head.headers.iter().map(RetainedHeader).collect())
                .unwrap_or_default(),
        }
    }
    pub(super) fn metadata(&self) -> ExchangeMetadata<'_, RetainedHeader<'a>> {
        ExchangeMetadata {
            request: self.request.map(|(method, path)| MessageMetadata {
                direction: Direction::Request,
                headers: &self.request_headers,
                method: Some(method),
                path: Some(path),
                status: None,
            }),
            response: self.response.map(|status| MessageMetadata {
                direction: Direction::Response,
                headers: &self.response_headers,
                method: None,
                path: None,
                status: Some(status),
            }),
        }
    }
    pub(super) fn applicable(&self, declaration: &Visualizer, surface: Surface) -> bool {
        declaration.surfaces.contains(&surface)
            && declaration
                .matches
                .iter()
                .any(|rule| matches(rule, self.metadata(), self.direction))
    }
}
