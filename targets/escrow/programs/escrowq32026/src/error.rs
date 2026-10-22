use anchor_lang::prelude::*;

#[error_code]
pub enum ErrorCode {
    #[msg("The escrow has expired")]
    EscrowExpired,
    #[msg("Receive amount must be greater than zero")]
    ReceiveIsZero,
    #[msg("Receive amount can only be lowered, never raised")]
    ReceiveRaised,
    #[msg("Expiration can only be pushed further out, never pulled in")]
    ExpirationPulledIn,
    #[msg("Escrow is asking for more than the taker agreed to pay")]
    ReceiveAboveMax,
}
