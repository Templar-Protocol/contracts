pub mod read;
pub mod rpc;
pub mod write;

pub(crate) fn map_mailbox_error(
    error: actix::MailboxError,
    actor_name: &'static str,
) -> crate::GatewayError {
    crate::GatewayError::ActorError {
        actor: actor_name,
        source: error,
    }
}
