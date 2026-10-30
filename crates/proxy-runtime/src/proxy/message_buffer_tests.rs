use super::*;
use transmog_core::intercept::ExchangeInterceptor;
use transmog_core::intercept::{BodyHookError, BoxBodyFuture, BufferedBodyHandler, BufferedHead};
use transmog_tls::SystemTrustSource;

#[derive(Clone, Copy)]
struct MessageHook {
    status: u16,
    fail: bool,
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
    for (status, fail) in [(201, false), (204, false), (201, true)] {
        let proxy = ProxyServer::bind(
            ProxyConfig::default(),
            ProxyCa::generate("Message test", 1).unwrap(),
            Arc::new(TrustSnapshot::load(&SystemTrustSource, 1).unwrap()),
            Arc::new(MessageHook { status, fail }),
        )
        .await
        .unwrap();
        let request = RequestHead {
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
        };
        let session = SessionMetadata {
            session_id: SessionId(1),
            downstream_connection_id: transmog_core::ConnectionId(1),
            stream_id: transmog_core::StreamId(1),
            client_addr: "127.0.0.1:1".parse().unwrap(),
            proxy_addr: proxy.local_addr().unwrap(),
            ingress_version: HttpLegVersion::Http1,
            egress_version: None,
            client_identity: transmog_core::ClientIdentity::default(),
        };
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
            assert_eq!(
                head.headers.values("x-body-checked").next().unwrap(),
                b"yes"
            );
            assert!(pipeline.has_buffered_output());
            let frames = pipeline.finish().await.unwrap();
            if status == 204 {
                assert_eq!(frames, [] as [BodyFrame; 0]);
            } else {
                assert_eq!(frames, vec![BodyFrame::Data(Bytes::from_static(b"edited"))]);
            }
        }
    }
}
