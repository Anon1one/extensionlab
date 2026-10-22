pub mod constants;
pub mod error;
pub mod instructions;
pub mod state;

use anchor_lang::prelude::*;

pub use constants::*;
pub use instructions::*;
pub use state::*;

declare_id!("9RsyeAzAeTGHDARy5VRDfzhTkrVZChnvjsVzVmbEX54t");

// Two parties — a maker and a taker — can swap tokens without trusting each other or a third party.
// The maker deposits token A into a program-controlled vault and specifies how much of token B they want in return.
// Any taker who holds token B can complete the swap atomically. If no taker appears, the maker can reclaim their tokens at any time.

// Maker deposits token A  →  vault (PDA-owned)
//                                       ↓  taker sends token B to maker
//                                       ↓  vault releases token A to taker
//                                       ↓  escrow + vault accounts closed, rent returned

#[program]
pub mod escrowq32026 {
    use super::*;

    #[instruction(discriminator = 0)]
    pub fn make(
        ctx: Context<Make>,
        seed: u64,
        deposit: u64,
        receive: u64,
        expiration: i64,
    ) -> Result<()> {
        ctx.accounts
            .init_escrow(seed, receive, &ctx.bumps, expiration)?;
        ctx.accounts.deposit(deposit)
    }

    // max_receive is the taker's own ceiling. The escrow can be re-priced by the maker
    // between the moment this transaction is built and the moment it lands, so the taker
    // pins the worst price they are willing to accept instead of trusting whatever the
    // account happens to say on arrival.
    #[instruction(discriminator = 1)]
    pub fn take(ctx: Context<Take>, max_receive: u64) -> Result<()> {
        ctx.accounts.take_and_close_vault(max_receive)
    }

    #[instruction(discriminator = 2)]
    pub fn refund(ctx: Context<Refund>) -> Result<()> {
        ctx.accounts.refund_and_close_vault()
    }

    // Pass None for anything you are not changing.
    #[instruction(discriminator = 3)]
    pub fn update(
        ctx: Context<Update>,
        receive: Option<u64>,
        expiration: Option<i64>,
    ) -> Result<()> {
        ctx.accounts.update_terms(receive, expiration)
    }
}
