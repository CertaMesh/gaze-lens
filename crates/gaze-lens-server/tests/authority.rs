use gaze_lens_protocol::{
    Error,
    wire::{Operation, Prepare, Privacy, Version},
};
use gaze_lens_server::auth::Authority;

fn state() -> String {
    format!(
        r#"{{"principals":[{{"id":"{}","generation":"{}","sha256":"ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"}}],"resources":[{{"alias":"fixture","id":"{}","generation":"{}","class":"database"}}],"grants":[{{"principal":"{}","resource":"{}","operation":"readiness","enabled":true,"expires-unix":4102444800}}]}}"#,
        "1".repeat(32),
        "2".repeat(32),
        "3".repeat(32),
        "4".repeat(32),
        "1".repeat(32),
        "3".repeat(32)
    )
}
fn prepare() -> Prepare {
    Prepare {
        version: Version::V2,
        privacy: Privacy::ClientGaze,
        id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".into(),
        credential: "a".repeat(64),
        resource: "fixture".into(),
        operation: Operation::Readiness,
    }
}
#[test]
fn exact_grant_and_pinned_binding_are_required() {
    let authority = Authority::parse(state().as_bytes()).unwrap();
    let p = prepare();
    let pin = authority.prepare(&p, 1).unwrap();
    assert_eq!(pin.binding().principal, "1".repeat(32));
    assert_eq!(pin.binding().resource, "3".repeat(32));
    assert!(authority.revalidate(&pin, 1).is_ok());
    let mut wrong = prepare();
    wrong.credential = "b".repeat(64);
    assert!(matches!(
        authority.prepare(&wrong, 1),
        Err(Error::Unauthorized)
    ));
    let mut wrong = prepare();
    wrong.operation = Operation::Query;
    assert!(matches!(
        authority.prepare(&wrong, 1),
        Err(Error::Unauthorized)
    ));
    let revoked = Authority::parse(state().replace("true", "false").as_bytes()).unwrap();
    assert_eq!(revoked.revalidate(&pin, 1), Err(Error::Unauthorized));
    let rebound =
        Authority::parse(state().replace(&"4".repeat(32), &"5".repeat(32)).as_bytes()).unwrap();
    assert_eq!(rebound.revalidate(&pin, 1), Err(Error::BindingChanged));
    assert_eq!(
        authority.revalidate(&pin, 4102444800),
        Err(Error::Unauthorized)
    );
}
#[test]
fn a_resource_may_not_reuse_a_principal_identity() {
    // Principals and resources share one nonoverlapping ID namespace, so
    // `identities` may treat every enrolled ID as distinct and history may
    // compare the flattened set for equality. A collision must deny at parse,
    // before any of that runs, rather than at the first History comparison.
    let collided = state().replace(&"3".repeat(32), &"1".repeat(32));
    assert!(matches!(
        Authority::parse(collided.as_bytes()),
        Err(Error::Unauthorized)
    ));
    // The same file with the namespaces kept apart is accepted, so the test
    // cannot be satisfied by an unrelated parse failure.
    assert!(Authority::parse(state().as_bytes()).is_ok());
}
