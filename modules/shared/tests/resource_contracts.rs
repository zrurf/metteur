use metteur_shared::resources::*;
use uuid::Uuid;

#[test]
fn scoped_identity_and_role_boundaries() {
    let id = ResourceId(Uuid::new_v4());
    let project = ResourceRef { scope: ResourceScope::Project("workspace-a".into()), id, revision: 1, content_digest: "abc".into() };
    let mut personal = project.clone();
    personal.scope = ResourceScope::Personal("local".into());
    assert_ne!(project, personal);
    for role in [ResourceRole::MainAgent, ResourceRole::Supervisor, ResourceRole::Concierge, ResourceRole::ReadOnlyAgent] {
        assert!(!role.can_promote());
    }
    assert!(!ResourceRole::ReadOnlyAgent.can_write_note());
    assert!(ResourceRole::MainAgent.can_write_note());
    let mut forged = serde_json::to_value(project).unwrap();
    forged["validation_passed"] = true.into();
    assert!(serde_json::from_value::<ResourceRef>(forged).is_err());
}
