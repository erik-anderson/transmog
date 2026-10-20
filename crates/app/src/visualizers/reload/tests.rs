use super::*;
use crate::visualizers::{VisualizerActivation, VisualizerSelection, tests::declaration};
use transmog_extension_registry::{Instance, Registry};
use transmog_extensions::Surface;

fn instance(id: &str) -> Instance {
    Instance {
        id: id.into(),
        extension_id: "example.evidence".into(),
        visualizer_id: "view".into(),
        digest: "a".repeat(64),
        development: true,
        declaration: declaration(),
    }
}
fn setup() -> (Service, VisualizerHandle, Arc<Ticket>) {
    let service = Service::new(Registry::default(), "owner".into());
    let ticket = service.begin("pane", "1", "1").unwrap();
    let current = instance("original");
    ticket.bind(current.clone()).unwrap();
    let activation = VisualizerActivation {
        slot: "pane".into(),
        generation: "1".into(),
        page_id: "1".into(),
        selection: VisualizerSelection {
            session_id: "1".into(),
            boundary: "client-request".into(),
            surface: Surface::Headers,
        },
        extension_id: current.extension_id.clone(),
        visualizer_id: current.visualizer_id.clone(),
        live: false,
    };
    let handle = activation.handle(&current);
    *ticket.activation.lock().unwrap() = Some(activation);
    (service, handle, ticket)
}

#[test]
fn failed_candidate_retains_the_original_binding_and_its_generation() {
    let (service, handle, current) = setup();
    let candidate = service.begin_reload(&handle, "2").unwrap();
    candidate.bind(instance("candidate")).unwrap();
    assert!(service.ticket_binding(&handle).is_ok());
    assert!(
        service
            .commit_reload_for_test(&handle, &candidate, || Err(unavailable()))
            .is_err()
    );
    service.discard_reload("pane", &candidate);
    assert!(candidate.canceled.load(Ordering::Acquire));
    assert!(!current.canceled.load(Ordering::Acquire));
    assert!(service.ticket_binding(&handle).is_ok());
    assert!(service.begin_reload(&handle, "2").is_err());
    let retry = service.begin_reload(&handle, "3").unwrap();
    service.cancel("pane", "3", "1");
    assert!(current.canceled.load(Ordering::Acquire) && retry.canceled.load(Ordering::Acquire));
}

#[tokio::test]
async fn hiding_a_candidate_waiting_for_reload_capacity_cancels_promptly() {
    let (service, handle, _) = setup();
    let service = Arc::new(service);
    let held = service.registry.reload_gate().lock_owned().await;
    let candidate = service.begin_reload(&handle, "2").unwrap();
    let waiting = Arc::clone(&service);
    let task = tokio::spawn(async move { waiting.reload_permit(&candidate).await });
    service.cancel("pane", "2", "1");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    drop(held);
}

#[test]
fn successful_swap_revokes_original_handles_only_after_publication() {
    let (service, handle, current) = setup();
    let candidate = service.begin_reload(&handle, "2").unwrap();
    candidate.bind(instance("candidate")).unwrap();
    assert!(
        service
            .commit_reload_for_test(&handle, &candidate, || {
                assert!(!current.canceled.load(Ordering::Acquire));
                Ok(true)
            })
            .unwrap()
    );
    assert!(current.canceled.load(Ordering::Acquire));
    assert!(service.ticket_binding(&handle).is_err());
    let replacement = VisualizerHandle {
        generation: "2".into(),
        instance_id: "candidate".into(),
        ..handle
    };
    assert!(service.ticket_binding(&replacement).is_ok());
    service.cancel("pane", "1", "1");
    assert!(!candidate.canceled.load(Ordering::Acquire));
    service.visible(false);
    assert!(candidate.canceled.load(Ordering::Acquire));
}

#[test]
fn supersession_navigation_and_hide_cancel_candidates_before_publication() {
    for cancel in 0..4 {
        let (service, handle, current) = setup();
        let candidate = service.begin_reload(&handle, "2").unwrap();
        candidate.bind(instance("candidate")).unwrap();
        match cancel {
            0 => {
                service.begin_reload(&handle, "3").unwrap();
            }
            1 => service.visible(false),
            2 => service.reset_document(),
            _ => {
                service.begin("pane", "3", "1").unwrap();
            }
        }
        assert!(candidate.canceled.load(Ordering::Acquire));
        let mut published = false;
        assert!(
            service
                .commit_reload_for_test(&handle, &candidate, || {
                    published = true;
                    Ok(true)
                })
                .is_err()
        );
        assert!(!published);
        if cancel == 0 {
            assert!(!current.canceled.load(Ordering::Acquire));
        } else {
            assert!(current.canceled.load(Ordering::Acquire));
        }
    }
}

#[test]
fn operation_failure_stops_the_current_ticket_after_a_reload_advanced_the_watermark() {
    let (service, handle, current) = setup();
    let failed = service.begin_reload(&handle, "2").unwrap();
    service.discard_reload("pane", &failed);
    let next = service.begin_reload(&handle, "3").unwrap();
    service.cancel_ticket(&handle, &current);
    assert!(current.canceled.load(Ordering::Acquire) && next.canceled.load(Ordering::Acquire));
    assert!(service.ticket_binding(&handle).is_err());
}
