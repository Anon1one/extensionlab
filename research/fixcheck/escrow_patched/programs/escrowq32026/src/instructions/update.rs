use anchor_lang::prelude::*;

use crate::{error::ErrorCode, state::Escrow, ESCROW_SEED};

// Only `receive` and `expiration` are touchable here, and both only in the direction
// that is safe for a taker who is already mid-flight.
//
// seed / maker  — they are PDA seeds. Change them and the account no longer sits at
//                 its own address, so every `seeds = [..]` check breaks.
// bump          — derived, not a preference.
// mint_a        — the vault already holds real mint_a tokens. Point the escrow at a
//                 different mint and the deposit is stranded forever: `take` would
//                 look for a vault ATA of the new mint (empty), and `refund` fails
//                 its own `has_one = mint_a`.
// mint_b        — swapping it out under a pending taker is pure griefing.
//
// None of the five are arguments to this instruction at all, so they cannot be
// changed even by mistake.

#[derive(Accounts)]
pub struct Update<'info> {
    maker: Signer<'info>,
    #[account(
        mut,
        has_one = maker,
        seeds = [ESCROW_SEED, maker.key().as_ref(), escrow.seed.to_le_bytes().as_ref()],
        bump = escrow.bump,
    )]
    pub escrow: Account<'info, Escrow>,
}

impl<'info> Update<'info> {
    //Re-price the deal, but only ever in the taker's favour
    pub fn update_terms(&mut self, receive: Option<u64>, expiration: Option<i64>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;

        // a dead escrow is not a deal any more, so there is nothing to re-price
        require!(now < self.escrow.expiration, ErrorCode::EscrowExpired);

        if let Some(receive) = receive {
            require!(receive > 0, ErrorCode::ReceiveIsZero);

            // The whole point. `take` reads escrow.receive straight off the account and
            // moves that many token B out of the taker's ATA — there is no amount the
            // taker gets to argue with. If the maker could raise this, they could watch
            // for a take in the mempool, land an update in front of it, and the taker
            // would pay the new price on a transaction they already signed.
            //
            // Letting it only go down removes the incentive entirely: a front-run can
            // only ever make the deal cheaper for the taker.
            require!(receive <= self.escrow.receive, ErrorCode::ReceiveRaised);

            self.escrow.receive = receive;
        }

        if let Some(expiration) = expiration {
            // Same argument. Pulling the expiration in is a way to kill a take that is
            // already on its way, so the maker may only push it out.
            require!(
                expiration >= self.escrow.expiration,
                ErrorCode::ExpirationPulledIn
            );

            self.escrow.expiration = expiration;
        }

        Ok(())
    }
}
