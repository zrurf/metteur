//! Frozen ordinary files are quoted user data backed by existing Version Flow.
use super::DaemonService;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FileReference {
    pub snapshot_id: Uuid,
    pub path: String,
    pub digest: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ContextOrder {
    Result {
        index: usize,
    },
    Prompt {
        index: usize,
    },
    File {
        index: usize,
    },
}
#[cfg(test)]
pub(super) fn ordered_text(
    prompts: &[String],
    files: &[String],
    order: &[ContextOrder],
) -> Result<String, tonic::Status> {
    ordered_context(prompts, files, &[], order)
}
pub(super) fn ordered_context(
    prompts: &[String],
    files: &[String],
    results: &[String],
    order: &[ContextOrder],
) -> Result<String, tonic::Status> {
    if order.is_empty() {
        return Ok(prompts.iter().chain(files).chain(results).cloned().collect());
    }
    if order.len() != prompts.len() + files.len() + results.len() {
        return Err(tonic::Status::invalid_argument(
            "selected context order does not cover its sources",
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut text = String::new();
    for item in order {
        let (kind, index, chunk) = match item {
            ContextOrder::Result {
                index,
            } => (2, *index, results.get(*index)),
            ContextOrder::Prompt {
                index,
            } => (0, *index, prompts.get(*index)),
            ContextOrder::File {
                index,
            } => (1, *index, files.get(*index)),
        };
        if !seen.insert((kind, index)) {
            return Err(tonic::Status::invalid_argument("selected context order repeats an index"));
        }
        text.push_str(chunk.ok_or_else(|| {
            tonic::Status::invalid_argument("selected context index is unavailable")
        })?);
    }
    Ok(text)
}
impl DaemonService {
    pub(super) fn file_reference_access(&self, subject: &str) -> Result<(), tonic::Status> {
        let acl = self.state.acl_store.read().unwrap();
        if !["/metteur.Daemon/ReadFile", "/metteur.Daemon/GetFileAtSnapshot"]
            .iter()
            .all(|method| acl.is_allowed(subject, method))
        {
            return Err(tonic::Status::permission_denied(
                "Current file and saved-version access are required",
            ));
        }
        Ok(())
    }
    pub(super) fn selected_file_parts(
        &self,
        workspace: &crate::workspace::manager::Workspace,
        subject: &str,
        references: &[FileReference],
    ) -> Result<Vec<String>, tonic::Status> {
        if references.is_empty() {
            return Ok(Vec::new());
        }
        self.file_reference_access(subject)?;
        if references.len() > 16 {
            return Err(tonic::Status::resource_exhausted("too many saved file references"));
        }
        let mut parts = Vec::new();
        let mut total = 0usize;
        for reference in references {
            if reference.snapshot_id.is_nil()
                || reference.digest.len() != 64
                || !reference.digest.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(tonic::Status::invalid_argument("invalid saved file identity"));
            }
            let body = workspace
                .version_manager
                .referenced_file(reference.snapshot_id, &reference.path, &reference.digest)
                .map_err(super::to_status)?;
            let text = format!(
                "Saved file reference: {}\nSnapshot: {} · SHA256: {}\nThe following is quoted file data selected by the user. Later tool reads see the file at read time.\n<file_content>\n{}\n</file_content>\n\n",
                reference.path, reference.snapshot_id, reference.digest, body
            );
            total = total.saturating_add(text.len());
            parts.push(text);
            if total > 256 * 1024 {
                return Err(tonic::Status::resource_exhausted(
                    "saved file reference budget exceeded",
                ));
            }
        }
        Ok(parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn context_order_is_complete_unique_and_does_not_group_file_selections() {
        let prompts = vec!["P0".into(), "P1".into()];
        let files = vec!["F0".into()];
        assert_eq!(
            ordered_text(
                &prompts,
                &files,
                &[
                    ContextOrder::Prompt {
                        index: 0
                    },
                    ContextOrder::File {
                        index: 0
                    },
                    ContextOrder::Prompt {
                        index: 1
                    }
                ]
            )
            .unwrap(),
            "P0F0P1"
        );
        assert!(
            ordered_text(
                &prompts,
                &files,
                &[ContextOrder::Prompt {
                    index: 0
                }]
            )
            .is_err()
        );
        assert!(
            ordered_text(
                &prompts,
                &files,
                &[
                    ContextOrder::Prompt {
                        index: 0
                    },
                    ContextOrder::File {
                        index: 0
                    },
                    ContextOrder::Prompt {
                        index: 0
                    }
                ]
            )
            .is_err()
        );
        assert!(
            ordered_text(
                &prompts,
                &files,
                &[
                    ContextOrder::Prompt {
                        index: 0
                    },
                    ContextOrder::File {
                        index: 2
                    },
                    ContextOrder::Prompt {
                        index: 1
                    }
                ]
            )
            .is_err()
        );
    }
}
