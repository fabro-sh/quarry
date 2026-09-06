use crate::{Command, Document, DocumentError, Result};
use automerge::{ActorId, ChangeHash};
use serde::{Deserialize, Serialize};

/// The browser and authority execute this same request against the same base.
/// Its actor and generated segment IDs are deterministic. Commands following
/// an unacknowledged edit therefore retain valid native character references.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CommandRequest {
    pub request_id: String,
    pub base: Vec<String>,
    pub commands: Vec<Command>,
    #[serde(default)]
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct DocumentActor {
    pub kind: String,
    pub id: Option<String>,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct CommandBatch {
    pub request_id: String,
    pub actor: DocumentActor,
    pub requests: Vec<CommandRequest>,
}

impl Document {
    pub fn apply_request(&mut self, request: &CommandRequest) -> Result<()> {
        if request.commands.is_empty() {
            return Err(DocumentError::Invalid("Empty command request".into()));
        }
        let mut base: Vec<ChangeHash> = request
            .base
            .iter()
            .map(|h| h.parse())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| DocumentError::Invalid("Invalid document version".into()))?;
        base.sort();
        if base.is_empty() || base.windows(2).any(|p| p[0] == p[1]) {
            return Err(DocumentError::Invalid(
                "Missing or repeated document version".into(),
            ));
        }
        let mut current = self.heads();
        current.sort();
        let actor = self.request_actor(&request.request_id)?;
        let mut branch = if base == current {
            self.fork()
        } else {
            self.fork_at(&base)?
        };
        if base != current {
            self.validate_command_base(&branch, &request.commands)?;
        }
        branch.crdt.set_actor(actor);
        branch.command_ids = Some((request.request_id.clone(), 0));
        branch.command_time = Some(request.at.clone());
        branch.apply(&request.commands)?;
        for command in &request.commands {
            if let Command::AddComment { id, .. } = command {
                self.require_new(crate::COMMENTS, id)?;
            }
        }
        branch.command_ids = None;
        branch.command_time = None;
        if base == current {
            // The request already contains every current change.
            *self = branch;
            Ok(())
        } else {
            self.merge(&branch)
        }
    }
    pub(crate) fn request_actor(&self, request_id: &str) -> Result<ActorId> {
        if request_id.is_empty()
            || request_id.len() > 128
            || !request_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        {
            return Err(DocumentError::Invalid(
                "Request ID must contain 1–128 ASCII letters, digits, hyphens, or underscores"
                    .into(),
            ));
        }
        let actor =
            ActorId::from(format!("quarry-command:{}:{}", self.id()?, request_id).as_bytes());
        // Replays belong to the durable receipt layer. Reject actor reuse here
        // so a reused ID can never create two different changes at one sequence.
        let mut history = self.crdt.clone();
        history.set_actor(actor.clone());
        if history.get_last_local_change().is_some() {
            return Err(DocumentError::Conflict("Request ID already used".into()));
        }
        Ok(actor)
    }
}
