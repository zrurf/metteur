use metteur_daemon::{prompt_resource::{PromptService,PromptDraft},prompt_snapshots::SelectionStore,storage::persistence::Db};
use metteur_shared::resources::{PromptPurpose,Origin};
use uuid::Uuid;
#[test]
fn selected_snapshots_and_run_manifests_survive_source_edit_delete_and_restart() {
    let root=std::env::temp_dir().join(format!("nx-snapshots-{}",Uuid::new_v4()));std::fs::create_dir_all(root.join(".metteur")).unwrap();
    let db=Db::open(&root.join(".metteur/db")).unwrap();let prompts=PromptService::project(db.clone(),&root).unwrap();let store=SelectionStore::new(db.clone(),&root).unwrap();
    let id=Uuid::new_v4();let entry=prompts.save(PromptDraft{id,name:"Selected".into(),tags:vec![],body:"Frozen body".into()},0,Uuid::new_v4(),None).unwrap();
    let node=Uuid::new_v4();let snapshot=store.select(entry,"alice",PromptPurpose::Blueprint{blueprint_id:Uuid::new_v4(),node_id:node},Uuid::new_v4()).unwrap();
    assert!(store.read(snapshot.id,"bob").is_err());let run=Uuid::new_v4();
    store.freeze_run(run,&[(node,snapshot.clone())].into()).unwrap();
    prompts.save(PromptDraft{id,name:"Selected".into(),tags:vec![],body:"Changed body".into()},1,Uuid::new_v4(),None).unwrap();prompts.delete(id,2,Uuid::new_v4()).unwrap();
    assert_eq!(store.run_snapshot(run,node).unwrap().body,"Frozen body");
    assert_eq!(store.record_origin(run,node,"Frozen body",vec![snapshot.reference.clone()],true,true).unwrap(),Origin::UserSelectedPrompt);
    assert_eq!(store.record_origin(run,node,"Transformed body",vec![snapshot.reference.clone()],false,false).unwrap(),Origin::Derived);
    drop(store);drop(prompts);drop(db);let db=Db::open(&root.join(".metteur/db")).unwrap();
    assert_eq!(SelectionStore::new(db,&root).unwrap().read(snapshot.id,"alice").unwrap().body,"Frozen body");
}

#[tokio::test]
async fn real_resource_and_context_executors_use_frozen_text_at_user_role() {
    use metteur_daemon::{Registry,execution::context::ExecutionContext,llm::LlmClientFactory};
    use metteur_shared::{dsl::compile_with_catalog,Value,PinType,llm::{ContextManager,Role}};
    use std::{sync::Arc,collections::HashMap};
    let root=std::env::temp_dir().join(format!("nx-prompt-node-{}",Uuid::new_v4()));std::fs::create_dir_all(root.join(".metteur")).unwrap();
    let db=Db::open(&root.join(".metteur/db")).unwrap();let store=SelectionStore::new(db.clone(),&root).unwrap();
    let registry=Arc::new(Registry::with_builtins());
    let graph=compile_with_catalog("p: PromptResource\nentry merge: ContextMerge(Text <- p.Text, Role = \"system\")",&registry.node_signatures()).unwrap();
    let prompt=graph.nodes.iter().find(|node|node.kind=="PromptResource").unwrap().clone();let merge=graph.nodes.iter().find(|node|node.kind=="ContextMerge").unwrap().clone();
    let mut ctx=ExecutionContext::new(registry.clone(),LlmClientFactory::new(),root.clone());ctx.workspace_db=Some(db.clone());ctx.run_id=Uuid::new_v4();ctx.blueprint=Some(Arc::new(parking_lot::RwLock::new(graph.clone())));
    for personal in [false,true] {
        ctx.run_id=Uuid::new_v4();
        let service=if personal {PromptService::personal(db.clone(),"alice")}else{PromptService::project(db.clone(),&root)}.unwrap();
        let id=Uuid::new_v4();let entry=service.save(PromptDraft{id,name:"Same name".into(),tags:vec![],body:"Exactly selected".into()},0,Uuid::new_v4(),None).unwrap();
        let snapshot=store.select(entry,"alice",PromptPurpose::Blueprint{blueprint_id:graph.id,node_id:prompt.id},Uuid::new_v4()).unwrap();store.freeze_run(ctx.run_id,&[(prompt.id,snapshot)].into()).unwrap();
        service.save(PromptDraft{id,name:"Same name".into(),tags:vec![],body:"New file content".into()},1,Uuid::new_v4(),None).unwrap();
        let output=registry.node_executor("PromptResource").unwrap().execute(&prompt,&HashMap::new(),&mut ctx).await.unwrap();let text=output.values().next().unwrap().clone();assert_eq!(text,Value::String("Exactly selected".into()));
        let inputs=merge.pins.iter().filter(|pin|pin.pin_type==PinType::DataInput).map(|pin|(pin.id,match pin.name.as_str(){"Context"=>Value::Context(ContextManager::default()),"Text"=>text.clone(),"Role"=>Value::String("system".into()),_=>unreachable!()})).collect();
        let result=registry.node_executor("ContextMerge").unwrap().execute(&merge,&inputs,&mut ctx).await.unwrap();let context=result.values().next().unwrap().as_context().unwrap();assert_eq!(context.messages.last().unwrap().role,Role::User);
        assert!(serde_json::to_string(context).unwrap().contains("Exactly selected"));
    }
}
