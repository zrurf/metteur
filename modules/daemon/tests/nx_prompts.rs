use metteur_daemon::{prompt_resource::{PromptService, PromptDraft, parse}, storage::persistence::Db};
use uuid::Uuid;
fn setup() -> (std::path::PathBuf, Db) {
    let root=std::env::temp_dir().join(format!("nx-prompts-{}",Uuid::new_v4()));
    std::fs::create_dir_all(root.join(".metteur")).unwrap();
    let db=Db::open(&root.join(".metteur/db")).unwrap(); (root,db)
}
#[test]
fn same_names_scopes_file_authority_and_revision_conflicts() {
    let (root,db)=setup(); let project=PromptService::project(db.clone(),&root).unwrap();
    let personal=PromptService::personal(db.clone(),"alice").unwrap();
    let bob=PromptService::personal(db.clone(),"bob").unwrap();
    let id=Uuid::new_v4(); let request=Uuid::new_v4();
    let draft=PromptDraft{id,name:"Review".into(),tags:vec![],body:"Line one\r\nLine two\n".into()};
    let first=project.save(draft.clone(),0,request,None).unwrap();
    assert_eq!(project.save(draft.clone(),0,request,None).unwrap().reference,first.reference);
    let second=personal.save(draft.clone(),0,Uuid::new_v4(),None).unwrap();
    assert_ne!(first.reference.scope,second.reference.scope); assert!(bob.list(None,20).unwrap().is_empty());
    assert_eq!(first.content.body,draft.body);
    let path=root.join(format!(".metteur/prompts/{id}.prompt.md"));
    let raw=std::fs::read_to_string(&path).unwrap(); std::fs::write(&path,raw.replace("Line two","Edited externally")).unwrap();
    let changed=project.read(id).unwrap(); assert_eq!(changed.reference.revision,2);
    assert_eq!(project.save(draft,1,Uuid::new_v4(),None).unwrap_err().code,"revision_conflict");
    let deletion=Uuid::new_v4();
    let removed=project.delete(id,2,deletion).unwrap();
    assert_eq!(project.delete(id,2,deletion).unwrap().reference,removed.reference); assert!(project.read(id).is_err());
    assert!(project.list(None,20).unwrap().is_empty());
    assert!(personal.read(id).is_ok());
    assert!(project.save(PromptDraft{id:Uuid::new_v4(),name:"../escape".into(),tags:vec![],body:"Text".into()},0,Uuid::new_v4(),None).is_ok());
    assert!(!root.join("escape.prompt.md").exists());
    let copied=Uuid::new_v4();
    project.save(PromptDraft{id:copied,name:"Copy".into(),tags:vec![],body:"Copied".into()},0,Uuid::new_v4(),Some(second.reference.clone())).unwrap();
    let edited=project.save(PromptDraft{id:copied,name:"Copy".into(),tags:vec![],body:"Independent edit".into()},1,Uuid::new_v4(),None).unwrap();
    assert_eq!(edited.content.copied_from,Some(second.reference));
}
#[test]
fn parser_rejects_authority_and_unsafe_metadata_without_normalizing_bytes() {
    let id=Uuid::new_v4();
    let raw=format!("+++\r\nschema_version = 1\r\nid = \"{id}\"\r\nname = \"Review\"\r\n+++\r\nBody\r\n");
    let parsed=parse(raw.as_bytes()).unwrap(); assert_eq!(parsed.body,"Body\r\n");
    assert_ne!(parsed.raw_digest,parse(raw.replace("\r\n","\n").as_bytes()).unwrap().raw_digest);
    for field in ["owner","scope","include","script"] { assert!(parse(raw.replace("name =",&format!("{field} = \"forged\"\r\nname =")).as_bytes()).is_err()); }
    assert!(parse(b"plain text").is_err());
}
#[test]
fn pending_file_intent_is_diagnostic_and_recoverable() {
    let (root,db)=setup(); let service=PromptService::project(db,&root).unwrap();
    let id=Uuid::new_v4(); let request=Uuid::new_v4();
    let directory=root.join(".metteur/prompts"); std::fs::create_dir_all(&directory).unwrap();
    let staging=directory.join(format!(".{request}.tmp")); std::fs::write(&staging,"unexpected staging content").unwrap();
    assert_eq!(service.save(PromptDraft{id,name:"Recovery".into(),tags:vec![],body:"Kept by durable intent".into()},0,request,None).unwrap_err().code,"file_conflict");
    assert!(!directory.join(format!("{id}.prompt.md")).exists());
    assert!(service.read(id).is_err());
    std::fs::remove_file(staging).unwrap();
    assert_eq!(service.read(id).unwrap().content.body,"Kept by durable intent");
    assert_eq!(service.read(id).unwrap().reference.revision,1);
}
#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_prompt_subtrees() {
    let (root,db)=setup(); let outside=root.join("outside"); std::fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside,root.join(".metteur/prompts")).unwrap();
    let service=PromptService::project(db,&root).unwrap();
    assert!(service.save(PromptDraft{id:Uuid::new_v4(),name:"x".into(),tags:vec![],body:"x".into()},0,Uuid::new_v4(),None).is_err());
    assert_eq!(std::fs::read_dir(outside).unwrap().count(),0);
}
