//! Build a single native request while reading the effects of preceding commands.
//! An unfinished or failed builder cannot change its source document.
use crate::{Command, CommandRequest, Document, DocumentError, Result};

pub struct CommandBuilder {
    document: Document,
    request: CommandRequest,
    failed: bool,
}

impl Document {
    pub fn command_builder(&self, request_id: &str, at: &str) -> Result<CommandBuilder> {
        let actor = self.request_actor(request_id)?;
        let mut document = self.fork();
        document.crdt.set_actor(actor);
        document.command_ids = Some((request_id.into(), 0));
        document.command_time = Some(at.into());
        document.batch_active = true;
        Ok(CommandBuilder {
            document,
            request: CommandRequest {
                request_id: request_id.into(),
                base: self.heads().iter().map(ToString::to_string).collect(),
                commands: Vec::new(),
                at: at.into(),
            },
            failed: false,
        })
    }
}

impl std::ops::Deref for CommandBuilder {
    type Target = Document;
    fn deref(&self) -> &Document {
        &self.document
    }
}

impl CommandBuilder {
    pub fn push(&mut self, commands: Vec<Command>) -> Result<()> {
        if self.failed {
            return Err(DocumentError::Invalid(
                "Command builder already failed".into(),
            ));
        }
        for command in commands {
            if let Err(error) = self.document.apply_one(&command) {
                self.failed = true;
                return Err(error);
            }
            self.request.commands.push(command);
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(Document, CommandRequest)> {
        if self.failed {
            return Err(DocumentError::Invalid(
                "Cannot publish a failed command builder".into(),
            ));
        }
        self.document.batch_active = false;
        self.document.crdt.commit();
        self.document.validate()?;
        self.document.command_ids = None;
        self.document.command_time = None;
        Ok((self.document, self.request))
    }
}
