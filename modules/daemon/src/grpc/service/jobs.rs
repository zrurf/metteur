//! Background-command RPCs: list the workspace's jobs and stream their output.

use std::path::PathBuf;

use tonic::{Request, Response, Status};

use crate::execution::jobs::{JobNoticeKind, JobSnapshot, JobState};

use super::super::proto::{
    JobEvent, JobInfo, JobList, KillJobRequest, KillJobResponse, ListJobsRequest, WatchJobsRequest,
};
use super::*;

/// Trailing output lines included in a `JobInfo` snapshot.
const LIST_TAIL_LINES: usize = 40;

impl DaemonService {
    pub(crate) async fn list_jobs(
        &self,
        request: Request<ListJobsRequest>,
    ) -> Result<Response<JobList>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let jobs = ws.jobs();
        let infos: Vec<JobInfo> = jobs
            .list(None)
            .into_iter()
            .map(|job| job_info(&job, &jobs.tail(&job.id, LIST_TAIL_LINES)))
            .collect();
        Ok(Response::new(JobList {
            jobs: infos,
        }))
    }

    pub(crate) async fn kill_job(
        &self,
        request: Request<KillJobRequest>,
    ) -> Result<Response<KillJobResponse>, Status> {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let jobs = ws.jobs();
        let snapshot = jobs
            .snapshot(&req.job_id)
            .ok_or_else(|| Status::not_found(format!("job {} not found", req.job_id)))?;
        let killed = jobs.kill(&req.job_id);
        Ok(Response::new(KillJobResponse {
            killed,
            state: snapshot.state.label().to_string(),
        }))
    }

    /// Streams job lifecycle and output notices for a workspace.
    ///
    /// The stream stays open until the client disconnects; a subscriber that
    /// falls behind receives `Lagged` and can rebuild its view with `ListJobs`.
    pub(crate) async fn watch_jobs(
        &self,
        request: Request<WatchJobsRequest>,
    ) -> Result<Response<tokio_stream::wrappers::ReceiverStream<Result<JobEvent, Status>>>, Status>
    {
        let req = request.into_inner();
        let ws = self
            .state
            .workspaces
            .get(&PathBuf::from(&req.workspace_path))
            .await
            .ok_or_else(|| Status::not_found("workspace not open"))?;
        let mut rx = ws.jobs().subscribe();
        let (tx, out_rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                let notice = tokio::select! {
                    _ = tx.closed() => break,
                    notice = rx.recv() => notice,
                };
                match notice {
                    Ok(notice) => {
                        let exit_code = exit_code_of(&notice.snapshot);
                        let event = JobEvent {
                            job_id: notice.job_id,
                            kind: notice.kind.label().to_string(),
                            chunk: notice.chunk,
                            state: notice.snapshot.state.label().to_string(),
                            exit_code,
                            summary: match notice.kind {
                                JobNoticeKind::Output => String::new(),
                                _ => notice.snapshot.summary(),
                            },
                        };
                        if tx.send(Ok(event)).await.is_err() {
                            break;
                        }
                    }
                    // Backlog overflow: the client resyncs with ListJobs.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = tx
                            .send(Err(Status::resource_exhausted(
                                "Job events overflowed; resubscribe and resync",
                            )))
                            .await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(out_rx)))
    }
}

/// Maps a job snapshot onto its transport form.
fn job_info(job: &JobSnapshot, tail: &str) -> JobInfo {
    JobInfo {
        id: job.id.clone(),
        command: job.command.clone(),
        cwd: job.cwd.clone(),
        state: job.state.label().to_string(),
        exit_code: exit_code_of(job),
        run_id: job.owner.to_string(),
        started_at: job.started_at,
        finished_at: job.finished_at.unwrap_or(0),
        output_bytes: job.output_bytes,
        tail: tail.to_string(),
    }
}

/// The exit code reported for a job, or `-1` when it has none.
fn exit_code_of(job: &JobSnapshot) -> i32 {
    match &job.state {
        JobState::Exited {
            code,
        } => *code,
        _ => -1,
    }
}
