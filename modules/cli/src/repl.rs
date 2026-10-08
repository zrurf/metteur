//! The interactive REPL loop.

use metteur_proto::proto::daemon_client::DaemonClient;
use tonic::transport::Channel;

use crate::commands::{self, Command, Outcome, SessionState};
use crate::print;

/// An execution stream currently producing events.
struct ActiveRun {
    stream: tonic::codec::Streaming<metteur_proto::proto::ExecutionEvent>,
    label: String,
}

/// A chat stream currently producing reply events.
struct ActiveChat {
    stream: tonic::codec::Streaming<metteur_proto::proto::ChatEvent>,
    label: String,
}

/// Runs the read-eval-print loop until the user exits.
pub async fn run(
    mut client: DaemonClient<Channel>,
    initial_workspace: Option<String>,
) -> anyhow::Result<()> {
    let mut state = SessionState::default();
    if let Some(path) = initial_workspace {
        handle(
            &mut client,
            &mut state,
            Command::Open {
                path,
            },
        )
        .await;
    }

    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if line_tx.send(trimmed).is_err() {
                        break;
                    }
                }
            }
        }
    });

    println!("Metteur REPL connected. Type 'help' for commands.");
    let mut active: Option<ActiveRun> = None;
    let mut chat: Option<ActiveChat> = None;
    let mut concierge: Option<tonic::codec::Streaming<metteur_proto::proto::ConciergeEvent>> = None;
    loop {
        tokio::select! {
            line = line_rx.recv() => {
                let Some(line) = line else { break };
                if line.is_empty() {
                    continue;
                }
                match commands::parse(&line) {
                    Err(err) => println!("error: {err}"),
                    Ok(cmd) => match commands::dispatch(&mut client, &mut state, cmd).await {
                        Err(err) => println!("error: {err:#}"),
                        Ok(Outcome::Printed(text)) => println!("{text}"),
                        Ok(Outcome::Exit) => break,
                        Ok(Outcome::Started(start)) => {
                            println!("-- started: {} --", start.label);
                            active = Some(ActiveRun {
                                stream: start.stream,
                                label: start.label,
                            });
                        }
                        Ok(Outcome::StartedConcierge(stream)) => { concierge=Some(*stream); }
                        Ok(Outcome::StartedChat(start)) => {
                            println!("-- started: {} --", start.label);
                            chat = Some(ActiveChat {
                                stream: start.stream,
                                label: start.label,
                            });
                        }
                    },
                }
            }
            event = async {
                match active.as_mut() {
                    Some(run) => Some(run.stream.message().await),
                    // Park forever while no run is active so `select!` waits
                    // on input instead of spinning on this branch.
                    None => {
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                }
            } => {
                match event {
                    Some(Ok(Some(event))) => react(&mut client, &mut state, &event).await,
                    Some(Err(status)) => {
                        active = None;
                        println!("-- run failed: {status} --");
                    }
                    _ => {
                        if let Some(run) = active.take() {
                            println!("-- run finished: {} --", run.label);
                        }
                    }
                }
            }
            event = async {
                match concierge.as_mut() {
                    Some(stream)=>stream.message().await,
                    None=>std::future::pending().await,
                }
            } => {
                match event {
                    Ok(Some(event))=>println!("{}",commands::concierge::line(&event)),
                    Ok(None)=>{concierge=None;},
                    Err(error)=>{println!("concierge failed: {error}");concierge=None;}
                }
            }
            chat_event = async {
                match chat.as_mut() {
                    Some(active) => Some(active.stream.message().await),
                    None => {
                        std::future::pending::<()>().await;
                        unreachable!()
                    }
                }
            } => {
                match chat_event {
                    Some(Ok(Some(event))) => react_chat(&event),
                    Some(Err(status)) => {
                        chat = None;
                        println!("\n-- chat failed: {status} --");
                    }
                    _ => {
                        if let Some(active) = chat.take() {
                            println!("\n-- chat finished: {} --", active.label);
                        }
                    }
                }
            }
        }
    }
    println!("bye");
    Ok(())
}

async fn handle(client: &mut DaemonClient<Channel>, state: &mut SessionState, cmd: Command) {
    match commands::dispatch(client, state, cmd).await {
        Ok(Outcome::Printed(text)) => println!("{text}"),
        Err(err) => println!("error: {err:#}"),
        _ => {}
    }
}

/// Prints one live chat event; deltas stream inline without newlines.
fn react_chat(event: &metteur_proto::proto::ChatEvent) {
    match print::chat_event(event) {
        print::ChatLine::Inline(text) => {
            use std::io::Write;
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
        print::ChatLine::Line(text) => println!("{text}"),
        print::ChatLine::Done => {}
    }
}

/// Prints one live event and answers approvals in auto-approve mode.
async fn react(
    client: &mut DaemonClient<Channel>,
    state: &mut SessionState,
    event: &metteur_proto::proto::ExecutionEvent,
) {
    println!("{}", print::event(event));
    if state.auto_approve && event.kind == "approval_request" && !event.message.is_empty() {
        let decision = "AllowRun".to_string();
        println!("auto-approving {} -> {}", event.message, decision);
        let request = metteur_proto::proto::ApprovalDecisionRequest {
            workspace_path: state.current_ws.clone().unwrap_or_default(),
            request_id: event.message.clone(),
            decision,
        };
        if let Err(err) = client.respond_approval(request).await {
            println!("auto-approve failed: {err}");
        }
    }
}
