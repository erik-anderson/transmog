use super::*;
use abi::{InputBudget, Optional, Project, Slice, Text};
use transmog_app::{ProductState, ProxyStartRequest, SessionQueryInput};

fn roundtrip<T: FromAbi + serde::Serialize>(value: T) {
    let expected = serde_json::to_value(&value).unwrap();
    let owner = Projected::new(value).unwrap();
    let actual = unsafe { T::read(&owner.view, &mut InputBudget::default()) }.unwrap();
    assert_eq!(serde_json::to_value(actual).unwrap(), expected);
}

unsafe fn view<'a, T: Model>(handle: *const OperationResult) -> &'a T::View {
    let pointer = unsafe { result_model(handle, T::KIND) }.cast::<T::View>();
    assert!(!pointer.is_null());
    unsafe { &*pointer }
}

fn text(value: Text) -> String {
    unsafe { String::read(&value, &mut InputBudget::default()) }.unwrap()
}

#[test]
fn generated_inputs_preserve_nested_models_and_exact_values() {
    use transmog_core::client_authentication::*;
    let mut product = ProductState::default();
    product.preferences.proxy_port = Some(65535);
    product.privacy.buffer_limit = transmog_app::BufferLimit::Custom { bytes: u64::MAX };
    product.preferences.client_certificate_policy.rules = vec![ClientCertificateRule {
        id: "unicode-雪\0id".into(),
        enabled: true,
        host: "example.test".into(),
        port: 443,
        mode: ClientCertificateAction::Automatic {
            selector: ClientCertificateSelector {
                identity: Some(String::new()),
                subject: [("CN".into(), "雪".into())].into(),
                issuer: [("O".into(), "".into())].into(),
            },
        },
    }];
    roundtrip(product.clone());
    for mode in [
        ClientCertificateAction::NoCertificate,
        ClientCertificateAction::Prompt,
    ] {
        product.preferences.client_certificate_policy.rules[0].mode = mode;
        roundtrip(product.clone());
    }
    for limit in [
        transmog_app::BufferLimit::Automatic,
        transmog_app::BufferLimit::Unlimited,
    ] {
        product.privacy.buffer_limit = limit;
        roundtrip(product.clone());
    }
    roundtrip(ProxyStartRequest {
        listen: "[::1]:12345".parse().unwrap(),
        ..Default::default()
    });
    use transmog_app::{TrafficColumn, TrafficDimension, TrafficFilterGroup, TrafficFilterValue};
    roundtrip(SessionQueryInput {
        search: Some(String::new()),
        terminal: Some(false),
        limit: Some(512),
        offset: Some(0),
        filter_groups: vec![TrafficFilterGroup {
            dimension: TrafficDimension::Column(TrafficColumn::Status),
            values: vec![
                TrafficFilterValue::Text("雪\0".into()),
                TrafficFilterValue::Number(u64::MAX.to_string()),
                TrafficFilterValue::StatusFamily(5),
                TrafficFilterValue::Unavailable,
                TrafficFilterValue::Other,
            ],
        }],
        ..Default::default()
    });
}

#[test]
fn invalid_abi_values_fail_before_calling_the_application() {
    let mut request = Projected::new(SessionQueryInput::default()).unwrap().view;
    let read = |value: &models::ViewSessionQueryInput| unsafe {
        model_input::<SessionQueryInput>((value as *const models::ViewSessionQueryInput).cast())
    };
    request.latest = 2;
    assert_eq!(
        read(&request).unwrap_err().category,
        ErrorCategory::InvalidInput
    );
    request.latest = 0;
    request.limit.has_value = 2;
    assert_eq!(
        read(&request).unwrap_err().category,
        ErrorCategory::InvalidInput
    );
    request.limit = Optional::default();
    request.search = Optional {
        has_value: 1,
        value: Text {
            data: b"\xff".as_ptr(),
            size: 1,
        },
    };
    assert_eq!(
        read(&request).unwrap_err().category,
        ErrorCategory::InvalidInput
    );
    request.search.value = Text {
        data: std::ptr::null(),
        size: MAX_INPUT_BYTES + 1,
    };
    assert_eq!(read(&request).unwrap_err().category, ErrorCategory::Limit);
    request.search.value.size = 1;
    assert_eq!(
        read(&request).unwrap_err().category,
        ErrorCategory::InvalidInput
    );
    request.search = Optional::default();
    request.filters = Slice {
        data: std::ptr::null(),
        size: 1,
    };
    assert_eq!(
        read(&request).unwrap_err().category,
        ErrorCategory::InvalidInput
    );
    request.filters.size = usize::MAX;
    assert_eq!(read(&request).unwrap_err().category, ErrorCategory::Limit);
    unsafe {
        assert_eq!(
            model_input::<SessionQueryInput>(std::ptr::null())
                .unwrap_err()
                .category,
            ErrorCategory::InvalidInput
        );
        let bytes = [0u64; 128];
        let misaligned = bytes.as_ptr().cast::<u8>().add(1);
        assert_eq!(
            model_input::<SessionQueryInput>(misaligned)
                .unwrap_err()
                .category,
            ErrorCategory::InvalidInput
        );
    }
    let mut start = Projected::new(ProxyStartRequest::default()).unwrap();
    start.view.route = 999;
    assert_eq!(
        unsafe { ProxyStartRequest::read(&start.view, &mut InputBudget::default()) }
            .unwrap_err()
            .category,
        ErrorCategory::InvalidInput
    );
    let mut product = Projected::new(ProductState::default()).unwrap();
    product.view.privacy.buffer_limit.kind = 999;
    assert_eq!(
        unsafe { ProductState::read(&product.view, &mut InputBudget::default()) }
            .unwrap_err()
            .category,
        ErrorCategory::InvalidInput
    );
}

#[test]
fn aggregate_input_budget_and_duplicate_map_keys_are_enforced() {
    let value = SessionQueryInput {
        method: Some("a".repeat(140_000)),
        host: Some("b".repeat(140_000)),
        ..Default::default()
    };
    let owner = Projected::new(value).unwrap();
    assert_eq!(
        unsafe { SessionQueryInput::read(&owner.view, &mut InputBudget::default()) }
            .unwrap_err()
            .category,
        ErrorCategory::Limit
    );
    let mut arena = abi::Arena::default();
    let entries = [abi::TextEntry {
        key: "CN".project(&mut arena).unwrap(),
        value: "v".project(&mut arena).unwrap(),
    }; 2];
    let duplicate = Slice {
        data: entries.as_ptr(),
        size: entries.len(),
    };
    assert_eq!(
        unsafe {
            std::collections::BTreeMap::<String, String>::read(
                &duplicate,
                &mut InputBudget::default(),
            )
        }
        .unwrap_err()
        .category,
        ErrorCategory::InvalidInput
    );
}

#[test]
fn defaults_results_and_errors_use_the_shared_application() {
    unsafe {
        let proxy = default_proxy_start_request();
        let actual = ProxyStartRequest::read(
            view::<ProxyStartRequest>(proxy),
            &mut InputBudget::default(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(ProxyStartRequest::default()).unwrap()
        );
        release(proxy);
        let defaults = default_session_query_input();
        let actual = SessionQueryInput::read(
            view::<SessionQueryInput>(defaults),
            &mut InputBudget::default(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(SessionQueryInput::default()).unwrap()
        );
        release(defaults);
        let app = application_create();
        let request = SessionQueryInput {
            limit: Some(0),
            ..Default::default()
        };
        let expected = state(app)
            .unwrap()
            .application
            .query_sessions(request.clone())
            .unwrap_err();
        let request = Projected::new(request).unwrap();
        let actual = query_sessions(app, (&raw const request.view).cast());
        let error = view::<AppError>(actual);
        assert_eq!(text(error.message), expected.message);
        assert_eq!(
            error.category,
            expected
                .category
                .project(&mut abi::Arena::default())
                .unwrap()
        );
        assert_eq!(error.retryable, u32::from(expected.retryable));
        assert!(result_model(actual, transmog_app::SessionPage::KIND).is_null());
        release(actual);
        let retained = product_state(app);
        application_release(app);
        assert_eq!(succeeded(retained), 1);
        let copied =
            ProductState::read(view::<ProductState>(retained), &mut InputBudget::default())
                .unwrap();
        assert_eq!(
            serde_json::to_value(copied).unwrap(),
            serde_json::to_value(ProductState::default()).unwrap()
        );
        assert!(result_model(retained, transmog_app::AppStatus::KIND).is_null());
        release(retained);
    }
}

#[test]
fn traffic_view_uses_shared_validation_and_release() {
    unsafe {
        let app = application_create();
        let request = Projected::new(SessionQueryInput::default()).unwrap();
        let result = query_traffic_view(app, (&raw const request.view).cast());
        let id = text(view::<transmog_app::TrafficView>(result).id);
        let rows = traffic_view_rows(app, id.as_ptr(), id.len(), 0, 1);
        assert_eq!(view::<Vec<transmog_app::SessionSummary>>(rows).size, 0);
        release(rows);
        let expected = state(app)
            .unwrap()
            .application
            .traffic_view_rows(&id, 0, 0)
            .unwrap_err();
        let invalid = traffic_view_rows(app, id.as_ptr(), id.len(), 0, 0);
        assert_eq!(text(view::<AppError>(invalid).message), expected.message);
        release(invalid);
        let released = release_traffic_view(app, id.as_ptr(), id.len());
        assert_eq!(succeeded(released), 1);
        release(released);
        let expired = traffic_view_rows(app, id.as_ptr(), id.len(), 0, 1);
        assert_eq!(view::<AppError>(expired).category, 8);
        release(expired);
        release(result);
        application_release(app);
    }
}

#[test]
fn typed_callbacks_can_outlive_the_application_owner() {
    unsafe extern "C" fn receive(context: usize, bytes: *const u8, length: usize) {
        let sender = unsafe { &*(context as *const std::sync::mpsc::Sender<(u64, u32)>) };
        if length == std::mem::size_of::<models::ViewSessionHint>() {
            let hint = unsafe { &*bytes.cast::<models::ViewSessionHint>() };
            let _ = sender.send((hint.sequence, hint.lagged));
        }
    }
    let (sender, receiver) = std::sync::mpsc::channel::<(u64, u32)>();
    unsafe {
        let app = application_create();
        let updates = subscribe_session_updates(
            app,
            receive as *const () as usize,
            (&raw const sender) as usize,
        );
        assert_eq!(session_updates_valid(updates), 1);
        let state = state(app).unwrap();
        state
            .application
            .save_product_state(state.application.product_state())
            .unwrap();
        let (_, lagged) = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(lagged <= 1);
        application_release(app);
        session_updates_release(updates);
    }
}

#[test]
fn contains_recoverable_panics_and_preserves_body_opt_in() {
    assert_eq!(
        contain::<()>(|| panic!("injected panic"))
            .unwrap_err()
            .category,
        ErrorCategory::Internal
    );
    unsafe {
        let bad = application_create_with_body_store(b"\xff".as_ptr(), 1);
        assert_eq!(application_valid(bad), 0);
        let failure = status(bad);
        assert_eq!(view::<AppError>(failure).category, 3);
        release(failure);
        application_release(bad);
        let app = application_create();
        assert!(state(app).unwrap().application.body_store().is_none());
        let page = read_body_range(
            app,
            b"00000000000000000000000000000000".as_ptr(),
            32,
            b"client-response".as_ptr(),
            15,
            0,
            1,
        );
        assert_eq!(body_page_number(page, 10), 8);
        body_page_release(page);
        application_release(app);
    }
}
