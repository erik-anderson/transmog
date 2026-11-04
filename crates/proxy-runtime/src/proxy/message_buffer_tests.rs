use super::*;
use transmog_core::intercept::ExchangeInterceptor;
use transmog_core::intercept::{BodyHookError, BoxBodyFuture, BufferedBodyHandler, BufferedHead};
use transmog_tls::SystemTrustSource;

#[derive(Clone, Copy)]
struct MessageHook {
    status: u16,
    fail: bool,
    pass: bool,
}

impl InterceptorFactory for MessageHook {
    fn create(
        &self,
        _: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, transmog_core::intercept::HookInitError> {
        Ok(Arc::new(*self))
    }
}
impl ExchangeInterceptor for MessageHook {
    fn on_response_body(
        &self,
        _: transmog_core::intercept::ResponseBodyEvent,
    ) -> transmog_core::intercept::BoxHookFuture<'_, transmog_core::intercept::ResponseBodyAction>
    {
        let handler = *self;
        Box::pin(async move {
            transmog_core::intercept::ResponseBodyAction::decoded(
                transmog_core::intercept::BodyPlan::Buffer {
                    limit: NonZeroUsize::new(64).unwrap(),
                    handler: Box::new(handler),
                },
            )
        })
    }
}
impl BufferedBodyHandler for MessageHook {
    fn edits_head(&self) -> bool {
        true
    }
    fn on_body(
        &mut self,
        body: transmog_core::intercept::BufferedBody,
    ) -> BoxBodyFuture<'_, Result<transmog_core::intercept::BufferedBody, BodyHookError>> {
        Box::pin(async move {
            assert_eq!(body.data(), b"complete".as_slice());
            if self.fail {
                return Err(BodyHookError::Failed("test failure".into()));
            }
            if self.pass {
                return Ok(body);
            }
            let Some(BufferedHead::Response(mut head)) = body.head().cloned() else {
                panic!("missing head")
            };
            head.status = self.status;
            head.headers
                .push(HeaderField::try_new("x-body-checked", "yes").unwrap());
            Ok(transmog_core::intercept::BufferedBody::try_new(
                64,
                Bytes::from_static(b"edited"),
                None,
            )
            .unwrap()
            .with_head(BufferedHead::Response(head)))
        })
    }
}

#[tokio::test]
async fn complete_message_hooks_finish_before_headers_are_committed() {
    for (status, fail, pass) in [
        (201, false, false),
        (204, false, false),
        (205, false, false),
        (201, true, false),
        (200, false, true),
    ] {
        let proxy = ProxyServer::bind(
            ProxyConfig::default(),
            ProxyCa::generate("Message test", 1).unwrap(),
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap()),
            Arc::new(MessageHook { status, fail, pass }),
        )
        .await
        .unwrap();
        let request = fixture_request();
        let session = fixture_session(&proxy);
        let mut chain = proxy
            .state
            .hooks
            .create_exchange(ExchangeMetadata::from_session(
                &session,
                request.target.clone(),
            ))
            .unwrap();
        chain.request_head(request.clone()).await.unwrap();
        let mut head = ResponseHead {
            status: 200,
            headers: HeaderBlock::new(),
            source_version: HttpLegVersion::Http1,
        };
        let (sender, mut source) = BodyStream::channel(NonZeroUsize::new(2).unwrap());
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"com"))))
            .await
            .unwrap();
        let mut preparation = Box::pin(proxy.state.prepare_streaming_response_pipeline(
            &chain,
            &request,
            &mut head,
            false,
            HttpLegVersion::Http1,
            &mut source,
        ));
        assert!(
            timeout(Duration::from_millis(20), &mut preparation)
                .await
                .is_err(),
            "headers must wait for the complete source"
        );
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"plete"))))
            .await
            .unwrap();
        drop(sender);
        let result = preparation.await;
        if fail {
            assert!(result.is_err());
            assert_eq!(head.status, 200);
            assert_eq!(head.headers.values("x-body-checked").count(), 0);
        } else {
            let mut pipeline = result.unwrap();
            assert_eq!(head.status, status);
            if !pass {
                assert_eq!(
                    head.headers.values("x-body-checked").next().unwrap(),
                    b"yes"
                );
            }
            assert!(pipeline.has_buffered_output());
            let frames = pipeline.finish().await.unwrap();
            if matches!(status, 204 | 205) {
                assert_eq!(frames, [] as [BodyFrame; 0]);
                assert_eq!(
                    head.headers.values("content-length").next(),
                    (status == 205).then_some(b"0".as_slice())
                );
            } else if pass {
                assert_eq!(
                    frames,
                    vec![BodyFrame::Data(Bytes::from_static(b"complete"))]
                );
                assert_eq!(head.headers.values("content-length").next().unwrap(), b"8");
            } else {
                assert_eq!(frames, vec![BodyFrame::Data(Bytes::from_static(b"edited"))]);
                assert_eq!(head.headers.values("content-length").next().unwrap(), b"6");
            }
        }
    }
}

#[test]
fn known_encoded_length_respects_status_method_and_trailers() {
    for destination in [
        HttpLegVersion::Http1,
        HttpLegVersion::Http2,
        HttpLegVersion::Http3,
    ] {
        for (method, status, trailers, expected) in [
            ("GET", 200, false, Some("7")),
            (
                "GET",
                200,
                true,
                if destination == HttpLegVersion::Http1 {
                    None
                } else {
                    Some("7")
                },
            ),
            ("HEAD", 200, false, Some("123")),
            ("head", 200, false, Some("123")),
            ("GET", 304, false, Some("123")),
            ("GET", 204, false, None),
            ("GET", 205, false, Some("0")),
            ("HEAD", 205, false, Some("0")),
            ("GET", 103, false, None),
            ("CONNECT", 200, false, None),
            ("connect", 200, false, None),
        ] {
            let mut head = ResponseHead {
                status,
                headers: HeaderBlock::new(),
                source_version: destination,
            };
            head.headers
                .push(HeaderField::try_new("content-length", "123").unwrap());
            let mut frames = vec![BodyFrame::Data(Bytes::from_static(b"encoded"))];
            if trailers {
                frames.push(BodyFrame::Trailers(HeaderBlock::new()));
            }
            finalize_response_length(&mut head, &frames, method, destination);
            assert_eq!(
                head.headers.values("content-length").next(),
                expected.map(str::as_bytes)
            );
        }
        let mut head = ResponseHead {
            status: 200,
            headers: HeaderBlock::new(),
            source_version: destination,
        };
        finalize_response_length(&mut head, &[], "GET", destination);
        assert_eq!(head.headers.values("content-length").next().unwrap(), b"0");
    }
}

#[derive(Clone, Copy)]
struct PrefixHook {
    deferred: bool,
    status: u16,
}
impl InterceptorFactory for PrefixHook {
    fn create(
        &self,
        _: &ExchangeMetadata,
    ) -> Result<Arc<dyn ExchangeInterceptor>, transmog_core::intercept::HookInitError> {
        Ok(Arc::new(*self))
    }
}
impl ExchangeInterceptor for PrefixHook {
    fn on_response_body(
        &self,
        _: transmog_core::intercept::ResponseBodyEvent,
    ) -> transmog_core::intercept::BoxHookFuture<'_, transmog_core::intercept::ResponseBodyAction>
    {
        let filter = PrefixFilter {
            deferred: self.deferred,
            status: self.status,
            pending: Vec::new(),
            head: None,
            edit: None,
        };
        Box::pin(async move {
            transmog_core::intercept::ResponseBodyAction::decoded(
                transmog_core::intercept::BodyPlan::Transform(Box::new(filter)),
            )
        })
    }
}
struct PrefixFilter {
    deferred: bool,
    status: u16,
    pending: Vec<u8>,
    head: Option<BufferedHead>,
    edit: Option<BufferedHead>,
}
impl transmog_core::intercept::BodyFilter for PrefixFilter {
    fn head_pending(&self) -> bool {
        self.deferred
    }
    fn set_head(&mut self, head: Option<BufferedHead>) {
        self.head = head;
    }
    fn take_head_edit(&mut self) -> Option<BufferedHead> {
        self.edit.take()
    }
    fn on_frame(
        &mut self,
        frame: BodyFrame,
    ) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        Box::pin(async move {
            let BodyFrame::Data(bytes) = frame else {
                return Ok(vec![frame]);
            };
            self.pending.extend_from_slice(&bytes);
            if self.deferred {
                if self.pending.len() < 4 {
                    return Ok(Vec::new());
                }
                self.deferred = false;
                let Some(BufferedHead::Response(mut head)) = self.head.clone() else {
                    panic!("missing head")
                };
                head.status = self.status;
                head.headers
                    .push(HeaderField::try_new("x-prefix", "checked").unwrap());
                self.edit = Some(BufferedHead::Response(head));
            }
            Ok(vec![BodyFrame::Data(Bytes::from(std::mem::take(
                &mut self.pending,
            )))])
        })
    }
    fn finish(&mut self) -> BoxBodyFuture<'_, Result<Vec<BodyFrame>, BodyHookError>> {
        let forbidden = !self.deferred && matches!(self.status, 204 | 205 | 304);
        Box::pin(async move {
            if forbidden {
                return Err(BodyHookError::Failed(
                    "discarded body reached EOF callback".into(),
                ));
            }
            Ok(Vec::new())
        })
    }
}

async fn prefix_fixture(
    deferred: bool,
    status: u16,
    wait: Duration,
) -> (ProxyServer, ExchangeChain, RequestHead, ResponseHead) {
    let mut config = ProxyConfig::default();
    config.limits.body_idle_timeout = wait;
    let proxy = ProxyServer::bind(
        config,
        ProxyCa::generate("Prefix test", 1).unwrap(),
        Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap()),
        Arc::new(PrefixHook { deferred, status }),
    )
    .await
    .unwrap();
    let request = fixture_request();
    let session = fixture_session(&proxy);
    let mut chain = proxy
        .state
        .hooks
        .create_exchange(ExchangeMetadata::from_session(
            &session,
            request.target.clone(),
        ))
        .unwrap();
    chain.request_head(request.clone()).await.unwrap();
    let mut head = ResponseHead {
        status: 200,
        headers: HeaderBlock::new(),
        source_version: HttpLegVersion::Http1,
    };
    head.headers
        .push(HeaderField::try_new("content-length", "100").unwrap());
    (proxy, chain, request, head)
}

#[tokio::test]
async fn streaming_headers_are_prompt_and_deferred_inspection_releases_before_eof() {
    for deferred in [false, true] {
        let (proxy, chain, request, mut head) =
            prefix_fixture(deferred, 200, Duration::from_secs(2)).await;
        let (sender, mut source) = BodyStream::channel(NonZeroUsize::new(2).unwrap());
        let mut preparation = Box::pin(proxy.state.prepare_streaming_response_pipeline(
            &chain,
            &request,
            &mut head,
            false,
            HttpLegVersion::Http1,
            &mut source,
        ));
        if deferred {
            sender
                .send(Ok(BodyFrame::Data(Bytes::from_static(b"ab"))))
                .await
                .unwrap();
            assert!(
                timeout(Duration::from_millis(20), &mut preparation)
                    .await
                    .is_err()
            );
            sender
                .send(Ok(BodyFrame::Data(Bytes::from_static(b"cd"))))
                .await
                .unwrap();
        }
        let mut pipeline = timeout(Duration::from_secs(1), preparation)
            .await
            .expect("headers must not wait for origin EOF")
            .unwrap();
        assert!(head.headers.values("content-length").next().is_none());
        if deferred {
            assert_eq!(head.headers.values("x-prefix").next().unwrap(), b"checked");
            let (frames, input, trailers) = pipeline.take_initial_output();
            assert_eq!(frames, vec![BodyFrame::Data(Bytes::from_static(b"abcd"))]);
            assert_eq!(input, 4);
            assert!(!trailers);
        }
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"later"))))
            .await
            .unwrap();
        assert_eq!(
            pipeline
                .process(source.recv().await.unwrap().unwrap())
                .await
                .unwrap(),
            vec![BodyFrame::Data(Bytes::from_static(b"later"))]
        );
    }
}

#[tokio::test]
async fn deferred_status_can_discard_the_unfinished_source_and_waits_are_bounded() {
    for (status, length) in [(204, None), (205, Some(b"0".as_slice()))] {
        let (proxy, chain, request, mut head) =
            prefix_fixture(true, status, Duration::from_secs(1)).await;
        let (sender, mut source) = BodyStream::channel(NonZeroUsize::MIN);
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"abcd"))))
            .await
            .unwrap();
        let mut pipeline = proxy
            .state
            .prepare_streaming_response_pipeline(
                &chain,
                &request,
                &mut head,
                false,
                HttpLegVersion::Http1,
                &mut source,
            )
            .await
            .unwrap();
        assert_eq!(head.status, status);
        assert_eq!(head.headers.values("content-length").next(), length);
        assert!(
            sender
                .send(Ok(BodyFrame::Data(Bytes::from_static(b"unused"))))
                .await
                .is_err()
        );
        assert_eq!(pipeline.finish().await.unwrap(), [] as [BodyFrame; 0]);
    }
    let (proxy, chain, request, mut head) =
        prefix_fixture(true, 200, Duration::from_millis(20)).await;
    let (_sender, mut source) = BodyStream::channel(NonZeroUsize::MIN);
    assert!(matches!(
        proxy
            .state
            .prepare_streaming_response_pipeline(
                &chain,
                &request,
                &mut head,
                false,
                HttpLegVersion::Http1,
                &mut source
            )
            .await,
        Err(ProxyRuntimeError::BodyIdleTimeout)
    ));
    assert!(head.headers.values("x-prefix").next().is_none());
}

#[tokio::test]
async fn deferred_bodyless_status_skips_eof_callbacks_when_the_source_is_already_closed() {
    for status in [204, 205, 304] {
        let (proxy, chain, request, mut head) =
            prefix_fixture(true, status, Duration::from_secs(1)).await;
        let (sender, mut source) = BodyStream::channel(NonZeroUsize::MIN);
        sender
            .send(Ok(BodyFrame::Data(Bytes::from_static(b"abcd"))))
            .await
            .unwrap();
        drop(sender);
        let mut pipeline = proxy
            .state
            .prepare_streaming_response_pipeline(
                &chain,
                &request,
                &mut head,
                false,
                HttpLegVersion::Http1,
                &mut source,
            )
            .await
            .expect("bodyless status must stop before another body callback");
        assert_eq!(head.status, status);
        assert_eq!(pipeline.finish().await.unwrap(), [] as [BodyFrame; 0]);
    }
}

#[tokio::test]
async fn cancellation_interrupts_deferred_header_inspection() {
    let (proxy, chain, request, mut head) =
        prefix_fixture(true, 200, Duration::from_secs(10)).await;
    let (_sender, mut source) = BodyStream::channel(NonZeroUsize::MIN);
    let mut preparation = Box::pin(proxy.state.prepare_streaming_response_pipeline(
        &chain,
        &request,
        &mut head,
        false,
        HttpLegVersion::Http1,
        &mut source,
    ));
    assert!(
        timeout(Duration::from_millis(20), &mut preparation)
            .await
            .is_err()
    );
    chain.context().cancellation().cancel();
    assert!(matches!(
        timeout(Duration::from_secs(1), preparation).await.unwrap(),
        Err(ProxyRuntimeError::BodyPipeline(
            BodyPipelineError::Execution(HookExecutionError::Cancelled)
        ))
    ));
    assert!(head.headers.values("x-prefix").next().is_none());
}

fn fixture_request() -> RequestHead {
    RequestHead {
        method: "GET".into(),
        target: Target {
            scheme: "http".into(),
            host: "example.test".into(),
            authority: "example.test".into(),
            port: 80,
            path: "/".into(),
            query: None,
        },
        headers: HeaderBlock::new(),
        source_version: HttpLegVersion::Http1,
    }
}
fn fixture_session(proxy: &ProxyServer) -> SessionMetadata {
    SessionMetadata {
        session_id: SessionId(1),
        downstream_connection_id: ConnectionId(1),
        stream_id: StreamId(1),
        client_addr: "127.0.0.1:1".parse().unwrap(),
        proxy_addr: proxy.local_addr().unwrap(),
        ingress_version: HttpLegVersion::Http1,
        egress_version: None,
        client_identity: transmog_core::ClientIdentity::default(),
    }
}
